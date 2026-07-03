use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use futures::stream::{self, Stream, StreamExt};
use serde_json::json;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::{BroadcastStream, IntervalStream};

use crate::assets;
use crate::service::{CommandError, TrainService};
use crate::types::{
    CommandRequestBody, DEFAULT_TTL_MS, ManualAction, TrainServiceEvent, default_move_parameters,
};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);

pub fn router(service: Arc<TrainService>) -> Router {
    Router::new()
        .route("/", get(assets::index_html))
        .route("/style.css", get(assets::style_css))
        .route("/app.js", get(assets::app_js))
        .route("/api/train/state", get(get_state))
        .route("/api/train/command", post(post_command))
        .route("/api/train/manual", post(post_manual))
        .route("/api/train/stream", get(get_stream))
        .with_state(service)
}

async fn get_state(State(service): State<Arc<TrainService>>) -> impl IntoResponse {
    Json(service.snapshot())
}

async fn post_command(State(service): State<Arc<TrainService>>, body: Bytes) -> impl IntoResponse {
    // Mirror the Next.js route: an unreadable body (empty / invalid JSON syntax)
    // falls back to defaults, while readable JSON must match the schema.
    let parsed: Option<CommandRequestBody> = if body.is_empty() {
        None
    } else {
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Err(_) => None,
            Ok(value) => match serde_json::from_value::<CommandRequestBody>(value) {
                Ok(request) => Some(request),
                Err(err) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(
                            json!({ "error": "Invalid command payload", "details": err.to_string() }),
                        ),
                    );
                }
            },
        }
    };
    let request = parsed.unwrap_or_default();

    if let Some(ttl_ms) = request.ttl_ms
        && ttl_ms == 0
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "error": "Invalid command payload", "details": "ttl_ms must be positive" }),
            ),
        );
    }
    if let Some(speed) = request.parameters.as_ref().and_then(|p| p.speed)
        && !(0.0..=1.0).contains(&speed)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "error": "Invalid command payload", "details": "speed must be between 0 and 1" }),
            ),
        );
    }

    let parameters = request
        .parameters
        .unwrap_or_default()
        .merge_over(default_move_parameters());
    let ttl_ms = request.ttl_ms.unwrap_or(DEFAULT_TTL_MS);

    match service.send_move_command(parameters, ttl_ms) {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))),
        Err(err) => {
            let status = match err {
                CommandError::Busy => StatusCode::CONFLICT,
                CommandError::NotConnected | CommandError::Publish(_) => {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            };
            (status, Json(json!({ "error": err.message() })))
        }
    }
}

async fn post_manual(State(service): State<Arc<TrainService>>, body: Bytes) -> impl IntoResponse {
    #[derive(serde::Deserialize)]
    struct ManualBody {
        action: ManualAction,
    }

    let action = match serde_json::from_slice::<ManualBody>(&body) {
        Ok(body) => body.action,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid manual command", "details": err.to_string() })),
            );
        }
    };

    match service.send_manual_command(action) {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))),
        Err(err) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": err.message() })),
        ),
    }
}

async fn get_stream(
    State(service): State<Arc<TrainService>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    // Subscribe before snapshotting so no event can fall between the two;
    // a duplicate event after the snapshot is harmless (payloads are full-state).
    let receiver = service.subscribe();
    let snapshot = TrainServiceEvent::Snapshot(service.snapshot());
    let lag_service = service.clone();

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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as u64
}
