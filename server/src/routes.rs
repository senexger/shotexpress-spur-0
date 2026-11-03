use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use futures::stream::{self, Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::{BroadcastStream, IntervalStream};

use crate::assets;
use crate::orders::OrderQueue;
use crate::track::StationRole;
use crate::service::{
    CommandError, FUEL_TTL_MS, MAX_ACCEL, MAX_FILL_ML, MAX_MOVE_STEPS, MAX_SPEED_HZ,
    TrainService,
};
use crate::track::TrackConfig;
use crate::trip::Trip;
use crate::types::{
    FuelCommandKind, ManualAction, RideRequestBody, TrainServiceEvent, now_ms,
};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);

/// Every API endpoint answers with this shape; handlers return
/// `Result<ApiResponse, ApiResponse>` so `?` can short-circuit on errors.
type ApiResponse = (StatusCode, Json<serde_json::Value>);

fn bad_request(error: &str, details: &str) -> ApiResponse {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": error, "details": details })),
    )
}

fn parse_body<T: DeserializeOwned>(body: &Bytes, error: &str) -> Result<T, ApiResponse> {
    serde_json::from_slice(body).map_err(|err| bad_request(error, &err.to_string()))
}

fn check_speed(speed: Option<f64>, error: &str) -> Result<(), ApiResponse> {
    match speed {
        Some(speed) if !(0.0..=1.0).contains(&speed) => {
            Err(bad_request(error, "speed must be between 0 and 1"))
        }
        _ => Ok(()),
    }
}

pub struct AppState {
    pub service: Arc<TrainService>,
    pub track: Arc<TrackConfig>,
    pub orders: Arc<OrderQueue>,
    pub trip: Arc<Trip>,
}


pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(assets::order_html))
        .route("/order.css", get(assets::order_css))
        .route("/order.js", get(assets::order_js))
        .route("/logo.svg", get(assets::logo_svg))
        .route("/banner.jpg", get(assets::banner_jpg))
        .route("/admin", get(assets::admin_html))
        .route("/api/order", post(post_order))
        .route("/api/orders", get(get_orders))
        .route("/api/orders/{id}", get(get_order))
        .route("/style.css", get(assets::style_css))
        .route("/app.js", get(assets::app_js))
        .route("/api/train/state", get(get_state))
        .route("/api/train/command", post(post_command))
        .route("/api/trip", get(get_trip))
        .route("/api/train/manual", post(post_manual))
        .route("/api/train/speed", post(post_speed))
        .route("/api/train/move", post(post_move))
        .route("/api/train/accel", post(post_accel))
        .route("/api/fuel/dispense", post(post_fuel_dispense))
        .route("/api/fuel/stop", post(post_fuel_stop))
        .route("/api/train/stream", get(get_stream))
        .route("/api/track", get(get_track))
        .with_state(state)
}

async fn get_state(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.service.snapshot())
}

fn track_summary(track: &TrackConfig) -> serde_json::Value {
    json!({
        "train": track.train,
        "refill": track.refill,
        "stations": track.stations,
        "speeds": track.speeds,
        "version": track.table().version,
    })
}

async fn get_track(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(track_summary(&state.track))
}

async fn post_order(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<ApiResponse, ApiResponse> {
    #[derive(serde::Deserialize)]
    struct OrderBody {
        target: String,
        shots: u32,
    }

    const ERR: &str = "Ungültige Bestellung";
    let request: OrderBody = parse_body(&body, ERR)?;
    let track = &state.track;
    match track.station(&request.target) {
        Some(station) if station.role == StationRole::Unload => {}
        _ => return Err(bad_request(ERR, &format!("Unbekannter Bahnhof {}", request.target))),
    }
    if request.shots == 0 || request.shots > track.refill.max_shots {
        return Err(bad_request(
            ERR,
            &format!("Bitte 1 bis {} Shots bestellen", track.refill.max_shots),
        ));
    }
    let order = state
        .orders
        .place(&track.refill.station, &request.target, request.shots);
    Ok((StatusCode::OK, Json(json!({ "ok": true, "order": order }))))
}

async fn get_orders(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.orders.list())
}

async fn get_order(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<u32>,
) -> Result<ApiResponse, ApiResponse> {
    match state.orders.get(id) {
        Some(order) => Ok((StatusCode::OK, Json(json!(order)))),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Bestellung nicht gefunden" })),
        )),
    }
}

async fn post_command(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<ApiResponse, ApiResponse> {
    const ERR: &str = "Invalid ride request";
    let request: RideRequestBody = parse_body(&body, ERR)?;
    check_speed(request.speed, ERR)?;
    if request.ttl_ms == Some(0) {
        return Err(bad_request(ERR, "ttl_ms must be positive"));
    }

    let track = &state.track;
    if track.station(&request.target).is_none() {
        return Err(bad_request(
            "Unroutable",
            &format!("unknown station {}", request.target),
        ));
    }

    // a trip: drive there, then follow the station's `then` / `wait_s`.
    // Overrides whatever the train is doing; progress is on GET /api/trip
    state
        .trip
        .start(state.service.clone(), track.clone(), request.target.clone());
    Ok((StatusCode::OK, Json(json!({ "ok": true, "trip": state.trip.status() }))))
}

/// Every manual control overrides the trip: stop it and free the busy
/// slot, so the manual command goes through even mid-ride.
fn manual_override(state: &AppState) {
    state.trip.cancel("manual control");
    state.service.release_current();
}

async fn get_trip(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.trip.status())
}

async fn post_manual(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<ApiResponse, ApiResponse> {
    #[derive(serde::Deserialize)]
    struct ManualBody {
        action: ManualAction,
    }

    let request: ManualBody = parse_body(&body, "Invalid manual command")?;
    manual_override(&state);
    if request.action == ManualAction::Stop {
        // also when the train is offline: nothing queued may start later
        let cancelled = state.orders.cancel_all("Manuell gestoppt.");
        if cancelled > 0 {
            tracing::warn!(cancelled, "manual stop cancelled all orders");
        }
    }
    state
        .service
        .send_manual_command(request.action)
        .map_err(raw_error_response)?;
    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
}

async fn post_speed(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<ApiResponse, ApiResponse> {
    #[derive(serde::Deserialize)]
    struct SpeedBody {
        hz: u32,
        #[serde(default)]
        accel: Option<u32>,
    }

    const ERR: &str = "Invalid speed";
    let request: SpeedBody = parse_body(&body, ERR)?;
    manual_override(&state);
    check_hz(request.hz, ERR)?;
    if let Some(accel) = request.accel {
        check_accel(accel, ERR)?;
    }
    state
        .service
        .send_speed(request.hz, request.accel)
        .map_err(raw_error_response)?;
    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
}

/// POST /api/train/move {"steps": n, "hz"?: n, "accel"?: n} — raw relative move.
async fn post_move(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<ApiResponse, ApiResponse> {
    #[derive(serde::Deserialize)]
    struct MoveBody {
        steps: i32,
        #[serde(default)]
        hz: Option<u32>,
        #[serde(default)]
        accel: Option<u32>,
    }

    const ERR: &str = "Invalid move";
    let request: MoveBody = parse_body(&body, ERR)?;
    manual_override(&state);
    if request.steps == 0 || request.steps.abs() > MAX_MOVE_STEPS {
        return Err(bad_request(
            ERR,
            &format!("steps must be non-zero and within ±{MAX_MOVE_STEPS}"),
        ));
    }
    if let Some(hz) = request.hz {
        check_hz(hz, ERR)?;
    }
    if let Some(accel) = request.accel {
        check_accel(accel, ERR)?;
    }
    state
        .service
        .send_move(request.steps, request.hz, request.accel)
        .map_err(raw_error_response)?;
    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
}

/// POST /api/train/accel {"accel": n} — set the start ramp, no motion.
async fn post_accel(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<ApiResponse, ApiResponse> {
    #[derive(serde::Deserialize)]
    struct AccelBody {
        accel: u32,
    }

    const ERR: &str = "Invalid acceleration";
    let request: AccelBody = parse_body(&body, ERR)?;
    check_accel(request.accel, ERR)?;
    state
        .service
        .send_accel(request.accel)
        .map_err(raw_error_response)?;
    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
}

fn check_accel(accel: u32, error: &str) -> Result<(), ApiResponse> {
    if accel == 0 || accel > MAX_ACCEL {
        return Err(bad_request(
            error,
            &format!("accel must be between 1 and {MAX_ACCEL}"),
        ));
    }
    Ok(())
}

/// POST /api/fuel/dispense {"volume_ml": n}
async fn post_fuel_dispense(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<ApiResponse, ApiResponse> {
    #[derive(serde::Deserialize)]
    struct DispenseBody {
        volume_ml: u32,
    }

    const ERR: &str = "Invalid fill amount";
    let request: DispenseBody = parse_body(&body, ERR)?;
    if request.volume_ml == 0 || request.volume_ml > MAX_FILL_ML {
        return Err(bad_request(
            ERR,
            &format!("volume_ml must be between 1 and {MAX_FILL_ML}"),
        ));
    }
    let cmd_id = state
        .service
        .send_fuel_command(FuelCommandKind::Dispense {
            volume_ml: request.volume_ml,
            ttl_ms: FUEL_TTL_MS,
        })
        .map_err(raw_error_response)?;
    Ok((
        StatusCode::OK,
        Json(json!({ "ok": true, "cmd_id": cmd_id })),
    ))
}

/// POST /api/fuel/stop — close the valve now.
async fn post_fuel_stop(State(state): State<Arc<AppState>>) -> Result<ApiResponse, ApiResponse> {
    state
        .service
        .send_fuel_command(FuelCommandKind::Stop)
        .map_err(raw_error_response)?;
    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
}

fn check_hz(hz: u32, error: &str) -> Result<(), ApiResponse> {
    if hz == 0 || hz > MAX_SPEED_HZ {
        return Err(bad_request(
            error,
            &format!("hz must be between 1 and {MAX_SPEED_HZ}"),
        ));
    }
    Ok(())
}

fn raw_error_response(err: CommandError) -> ApiResponse {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": err.message() })),
    )
}

async fn get_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    // Subscribe before snapshotting so no event can fall between the two;
    // a duplicate event after the snapshot is harmless (payloads are full-state).
    let receiver = state.service.subscribe();
    let snapshot = TrainServiceEvent::Snapshot(state.service.snapshot());
    let lag_service = state.service.clone();

    let data = stream::once(std::future::ready(snapshot))
        .chain(
            BroadcastStream::new(receiver).map(move |result| match result {
                Ok(event) => event,
                Err(BroadcastStreamRecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "SSE consumer lagged, resyncing with snapshot");
                    TrainServiceEvent::Snapshot(lag_service.snapshot())
                }
            }),
        )
        .map(|event| {
            Ok(SseEvent::default().data(
                serde_json::to_string(&event).expect("TrainServiceEvent serialization cannot fail"),
            ))
        });

    let heartbeat = IntervalStream::new(tokio::time::interval_at(
        tokio::time::Instant::now() + HEARTBEAT_INTERVAL,
        HEARTBEAT_INTERVAL,
    ))
    .map(|_| {
        Ok(SseEvent::default()
            .event("heartbeat")
            .data(now_ms().to_string()))
    });

    Sse::new(stream::select(data, heartbeat))
}
