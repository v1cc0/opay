use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/healthz", get(healthz))
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    database: DatabaseStatus,
}

#[derive(Debug, Serialize)]
struct DatabaseStatus {
    kind: &'static str,
    path: String,
    ok: bool,
    applied_migrations: i64,
}

async fn healthz(State(state): State<AppState>) -> Json<HealthResponse> {
    let db_ok = state.db.ping().await.is_ok();
    let applied_migrations = state.db.applied_migration_count().await.unwrap_or(-1);

    Json(HealthResponse {
        ok: db_ok,
        service: "opay-rs",
        database: DatabaseStatus {
            kind: "turso-local",
            path: state.config.db_path.display().to_string(),
            ok: db_ok,
            applied_migrations,
        },
    })
}
