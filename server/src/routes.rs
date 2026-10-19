use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
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
use crate::service::{CommandError, TrainService};
use crate::track::TrackConfig;
use crate::types::{
    CommandKind, ManualAction, RideRequestBody, StepsRequestBody, TrainServiceEvent, now_ms,
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
    pub track: RwLock<Arc<TrackConfig>>,
    pub track_path: PathBuf,
}

impl AppState {
    fn track(&self) -> Arc<TrackConfig> {
        self.track.read().expect("track lock poisoned").clone()
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(assets::index_html))
        .route("/style.css", get(assets::style_css))
        .route("/app.js", get(assets::app_js))
        .route("/api/train/state", get(get_state))
        .route("/api/train/command", post(post_command))
        .route("/api/train/steps", post(post_steps))
        .route("/api/train/manual", post(post_manual))
        .route("/api/train/stream", get(get_stream))
        .route("/api/track", get(get_track))
        .route("/api/admin/track/reload", post(post_track_reload))
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
        "ring": track.nodes.iter().map(|n| n.tag.as_str()).collect::<Vec<_>>(),
    })
}

async fn get_track(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(track_summary(&state.track()))
}

async fn post_track_reload(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match TrackConfig::load(&state.track_path) {
        Ok(track) => {
            let summary = track_summary(&track);
            *state.track.write().expect("track lock poisoned") = Arc::new(track);
            tracing::info!(path = %state.track_path.display(), "track config reloaded");
            (
                StatusCode::OK,
                Json(json!({ "ok": true, "track": summary })),
            )
        }
        Err(err) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "Track config rejected", "details": err })),
        ),
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

    let track = state.track();

    // pin the start node from the last two consecutive progress tags; with
    // no fix the compiled schedule covers the full ring and the train
    // resyncs itself on its first read
    let from_node = match state.service.last_tag_pair() {
        (Some(prev), Some(cur)) => track.node_for_pair(&prev, &cur),
        _ => None,
    };

    let legs = track
        .compile_route(from_node, &request.target, request.speed)
        .map_err(|err| bad_request("Unroutable", &err))?;

    let command = CommandKind::Drive {
        legs,
        accel: track.train.accel,
        max_run_ms_without_tag: track.train.max_run_ms_without_tag,
        ttl_ms: request.ttl_ms.unwrap_or(track.train.default_ttl_ms),
    };
    let cmd_id = state
        .service
        .send_command(command)
        .map_err(command_error_response)?;
    Ok((
        StatusCode::OK,
        Json(json!({ "ok": true, "cmd_id": cmd_id })),
    ))
}

async fn post_steps(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<ApiResponse, ApiResponse> {
    const ERR: &str = "Invalid steps request";
    let request: StepsRequestBody = parse_body(&body, ERR)?;
    check_speed(request.speed, ERR)?;

    let track = state.track();
    let command = CommandKind::Steps {
        steps: request.steps.unwrap_or(track.refill.steps),
        direction: request.direction,
        speed: request.speed.unwrap_or(track.refill.speed),
        ttl_ms: request.ttl_ms.unwrap_or(track.train.default_ttl_ms),
    };
    let cmd_id = state
        .service
        .send_command(command)
        .map_err(command_error_response)?;
    Ok((
        StatusCode::OK,
        Json(json!({ "ok": true, "cmd_id": cmd_id })),
    ))
}

fn command_error_response(err: CommandError) -> ApiResponse {
    let status = match err {
        CommandError::Busy => StatusCode::CONFLICT,
        CommandError::NotConnected | CommandError::Publish(_) => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, Json(json!({ "error": err.message() })))
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
    state
        .service
        .send_manual_command(request.action)
        .map_err(|err| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": err.message() })),
            )
        })?;
    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
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
