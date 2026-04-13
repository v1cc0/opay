use axum::{
    Json, Router,
    extract::{Query, State},
    http::HeaderMap,
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    admin_auth::verify_admin_values,
    error::{AppError, AppResult},
};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/admin/user-balance", get(get_user_balance))
}

#[derive(Debug, Deserialize)]
struct UserBalanceQuery {
    token: Option<String>,
    lang: Option<String>,
    #[serde(rename = "userId")]
    user_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct UserBalanceResponse {
    balance: f64,
}

async fn get_user_balance(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserBalanceQuery>,
) -> AppResult<Json<UserBalanceResponse>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let user_id = query
        .user_id
        .as_deref()
        .map(str::trim)
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::bad_request("Invalid userId"))?;

    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| AppError::public_internal("Failed to fetch user balance"))?;
    let admin_api_key = platform_admin_api_key(&state).await?;
    let user = platform
        .get_user(user_id, &admin_api_key)
        .await
        .map_err(|_| AppError::public_internal("Failed to fetch user balance"))?;

    Ok(Json(UserBalanceResponse {
        balance: user.balance.unwrap_or(0.0),
    }))
}

async fn platform_admin_api_key(state: &AppState) -> AppResult<String> {
    let value = state
        .system_config
        .get("PLATFORM_ADMIN_API_KEY")
        .await
        .map_err(AppError::internal)?
        .unwrap_or_default();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::public_internal("Failed to fetch user balance"));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::{Json, Router, extract::Path, routing::get};
    use serde_json::json;
    use tokio::{net::TcpListener, task::JoinHandle};
    use uuid::Uuid;

    use super::*;
    use crate::{
        config::AppConfig,
        db::DatabaseHandle,
        order::{audit::AuditLogRepository, repository::OrderRepository, service::OrderService},
        platform::PlatformClient,
        subscription_plan::SubscriptionPlanRepository,
        system_config::{SystemConfigService, UpsertSystemConfig},
    };

    async fn test_state(platform_base_url: Option<String>) -> AppState {
        let db_path = std::env::temp_dir().join(format!(
            "opay-admin-user-balance-{}.db",
            Uuid::new_v4()
        ));
        let db = DatabaseHandle::open_local(&db_path).await.unwrap();
        db.run_migrations().await.unwrap();

        let config = Arc::new(AppConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            db_path,
            payment_providers: Vec::new(),
            admin_token: Some("test-admin-token".to_string()),
            system_config_cache_ttl_secs: 1,
            platform_base_url: platform_base_url.clone(),
            platform_timeout_secs: 2,
            min_recharge_amount: 1.0,
            max_recharge_amount: 1000.0,
            max_daily_recharge_amount: 10000.0,
            pay_help_image_url: None,
            pay_help_text: None,
            stripe_publishable_key: None,
        });

        let system_config = SystemConfigService::new(db.clone(), Duration::from_secs(1));
        let platform = platform_base_url.map(|base_url| PlatformClient::new(base_url, 2));

        AppState {
            config: Arc::clone(&config),
            db: db.clone(),
            system_config: system_config.clone(),
            platform: platform.clone(),
            order_service: OrderService::new(
                Arc::clone(&config),
                OrderRepository::new(db.clone()),
                AuditLogRepository::new(db.clone()),
                SubscriptionPlanRepository::new(db.clone()),
                system_config,
                platform,
            ),
        }
    }

    fn admin_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer test-admin-token".parse().unwrap());
        headers
    }

    async fn start_mock_platform() -> (String, JoinHandle<()>) {
        async fn user(Path(user_id): Path<i64>) -> Json<serde_json::Value> {
            Json(json!({
                "data": {
                    "id": user_id,
                    "username": "demo",
                    "email": "demo@example.com",
                    "status": "active",
                    "balance": 12.5
                }
            }))
        }

        let app = Router::new().route("/api/v1/admin/users/{id}", get(user));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn returns_user_balance() {
        let (base_url, handle) = start_mock_platform().await;
        let state = test_state(Some(base_url)).await;
        state
            .system_config
            .set_many(&[UpsertSystemConfig {
                key: "PLATFORM_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        let response = get_user_balance(
            State(state),
            admin_headers(),
            Query(UserBalanceQuery {
                token: None,
                lang: None,
                user_id: Some("42".to_string()),
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(response.balance, 12.5);
        handle.abort();
    }
}
