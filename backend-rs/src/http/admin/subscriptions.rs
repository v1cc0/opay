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
    sub2api::{ListSubscriptionsParams, PaginatedSubscriptions, Sub2ApiSubscription},
};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/admin/subscriptions", get(get_subscriptions))
}

#[derive(Debug, Deserialize)]
struct AdminSubscriptionsQuery {
    token: Option<String>,
    lang: Option<String>,
    user_id: Option<String>,
    group_id: Option<String>,
    status: Option<String>,
    page: Option<i64>,
    page_size: Option<i64>,
}

#[derive(Debug, Serialize)]
struct AdminSubscriptionsResponse {
    subscriptions: Vec<Sub2ApiSubscription>,
    total: i64,
    page: i64,
    page_size: i64,
    user: Option<UserView>,
}

#[derive(Debug, Serialize)]
struct UserView {
    id: i64,
    username: Option<String>,
    email: Option<String>,
}

async fn get_subscriptions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminSubscriptionsQuery>,
) -> AppResult<Json<AdminSubscriptionsResponse>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let sub2api = state
        .sub2api
        .as_ref()
        .ok_or_else(|| AppError::public_internal("查询订阅信息失败"))?;
    let admin_api_key = sub2api_admin_api_key(&state).await?;

    if let Some(user_id) = query
        .user_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let parsed_user_id = user_id
            .parse::<i64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| AppError::bad_request("无效的 user_id"))?;

        let (subscriptions, user) = tokio::try_join!(
            sub2api.get_user_subscriptions(parsed_user_id, &admin_api_key),
            async {
                match sub2api.get_user(parsed_user_id, &admin_api_key).await {
                    Ok(user) => Ok(Some(user)),
                    Err(error) if error.to_string() == "USER_NOT_FOUND" => Ok(None),
                    Err(error) => Err(error),
                }
            }
        )
        .map_err(AppError::internal)?;

        let filtered =
            if let Some(group_id) = query.group_id.as_deref().and_then(parse_positive_i64) {
                subscriptions
                    .into_iter()
                    .filter(|item| item.group_id == group_id)
                    .collect::<Vec<_>>()
            } else {
                subscriptions
            };

        return Ok(Json(AdminSubscriptionsResponse {
            total: filtered.len() as i64,
            page: 1,
            page_size: filtered.len() as i64,
            subscriptions: filtered,
            user: user.map(|item| UserView {
                id: item.id,
                username: item.username,
                email: item.email,
            }),
        }));
    }

    let result = sub2api
        .list_subscriptions(
            &ListSubscriptionsParams {
                user_id: None,
                group_id: query.group_id.as_deref().and_then(parse_positive_i64),
                status: query.status.clone(),
                page: query.page.map(|value| value.max(1)),
                page_size: query.page_size.map(|value| value.clamp(1, 200)),
            },
            &admin_api_key,
        )
        .await
        .map_err(AppError::internal)?;

    Ok(Json(to_admin_subscriptions_response(result)))
}

fn parse_positive_i64(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok().filter(|item| *item > 0)
}

fn to_admin_subscriptions_response(result: PaginatedSubscriptions) -> AdminSubscriptionsResponse {
    AdminSubscriptionsResponse {
        subscriptions: result.subscriptions,
        total: result.total,
        page: result.page,
        page_size: result.page_size,
        user: None,
    }
}

async fn sub2api_admin_api_key(state: &AppState) -> AppResult<String> {
    let value = state
        .system_config
        .get("SUB2API_ADMIN_API_KEY")
        .await
        .map_err(AppError::internal)?
        .unwrap_or_default();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::public_internal("查询订阅信息失败"));
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
        sub2api::Sub2ApiClient,
        subscription_plan::SubscriptionPlanRepository,
        system_config::{SystemConfigService, UpsertSystemConfig},
    };

    async fn test_state(sub2api_base_url: Option<String>) -> AppState {
        let db_path =
            std::env::temp_dir().join(format!("sub2apipay-admin-subs-route-{}.db", Uuid::new_v4()));
        let db = DatabaseHandle::open_local(&db_path).await.unwrap();
        db.run_migrations().await.unwrap();

        let config = Arc::new(AppConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            db_path,
            payment_providers: Vec::new(),
            admin_token: Some("test-admin-token".to_string()),
            system_config_cache_ttl_secs: 1,
            sub2api_base_url: sub2api_base_url.clone(),
            sub2api_timeout_secs: 2,
            min_recharge_amount: 1.0,
            max_recharge_amount: 1000.0,
            max_daily_recharge_amount: 10000.0,
            pay_help_image_url: None,
            pay_help_text: None,
            stripe_publishable_key: None,
        });

        let system_config = SystemConfigService::new(db.clone(), Duration::from_secs(1));
        let sub2api = sub2api_base_url.map(|base_url| Sub2ApiClient::new(base_url, 2));

        AppState {
            config: Arc::clone(&config),
            db: db.clone(),
            system_config: system_config.clone(),
            sub2api: sub2api.clone(),
            order_service: OrderService::new(
                Arc::clone(&config),
                OrderRepository::new(db.clone()),
                AuditLogRepository::new(db.clone()),
                SubscriptionPlanRepository::new(db.clone()),
                system_config,
                sub2api,
            ),
        }
    }

    fn admin_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer test-admin-token".parse().unwrap());
        headers
    }

    async fn start_mock_sub2api() -> (String, JoinHandle<()>) {
        async fn user_subscriptions() -> Json<serde_json::Value> {
            Json(json!({
                "data": [
                    {
                        "id": 501,
                        "user_id": 42,
                        "group_id": 7,
                        "starts_at": "2026-04-01T00:00:00Z",
                        "expires_at": "2026-05-01T00:00:00Z",
                        "status": "active",
                        "daily_usage_usd": 1.0,
                        "weekly_usage_usd": 3.0,
                        "monthly_usage_usd": 10.0
                    }
                ]
            }))
        }

        async fn user(Path(user_id): Path<i64>) -> Json<serde_json::Value> {
            Json(json!({
                "data": {
                    "id": user_id,
                    "username": "demo",
                    "email": "demo@example.com",
                    "status": "active",
                    "balance": 0
                }
            }))
        }

        async fn list_subscriptions() -> Json<serde_json::Value> {
            Json(json!({
                "data": {
                    "items": [
                        {
                            "id": 701,
                            "user_id": 99,
                            "group_id": 8,
                            "starts_at": "2026-04-01T00:00:00Z",
                            "expires_at": "2026-06-01T00:00:00Z",
                            "status": "active",
                            "daily_usage_usd": 2.0,
                            "weekly_usage_usd": 4.0,
                            "monthly_usage_usd": 20.0
                        }
                    ],
                    "total": 1,
                    "page": 2,
                    "page_size": 30
                }
            }))
        }

        let app = Router::new()
            .route(
                "/api/v1/admin/users/42/subscriptions",
                get(user_subscriptions),
            )
            .route("/api/v1/admin/users/{id}", get(user))
            .route("/api/v1/admin/subscriptions", get(list_subscriptions));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn admin_subscriptions_supports_user_lookup() {
        let (base_url, handle) = start_mock_sub2api().await;
        let state = test_state(Some(base_url)).await;
        state
            .system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        let response = get_subscriptions(
            State(state),
            admin_headers(),
            Query(AdminSubscriptionsQuery {
                token: None,
                lang: None,
                user_id: Some("42".to_string()),
                group_id: Some("7".to_string()),
                status: None,
                page: None,
                page_size: None,
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(response.subscriptions.len(), 1);
        assert_eq!(response.user.as_ref().unwrap().id, 42);
        assert_eq!(response.total, 1);

        handle.abort();
    }

    #[tokio::test]
    async fn admin_subscriptions_supports_global_listing() {
        let (base_url, handle) = start_mock_sub2api().await;
        let state = test_state(Some(base_url)).await;
        state
            .system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        let response = get_subscriptions(
            State(state),
            admin_headers(),
            Query(AdminSubscriptionsQuery {
                token: None,
                lang: None,
                user_id: None,
                group_id: Some("8".to_string()),
                status: Some("active".to_string()),
                page: Some(2),
                page_size: Some(30),
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(response.subscriptions.len(), 1);
        assert_eq!(response.page, 2);
        assert_eq!(response.page_size, 30);
        assert_eq!(response.total, 1);

        handle.abort();
    }
}
