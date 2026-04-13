use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    error::{AppError, AppResult},
    http::common::{message, resolve_locale},
    sub2api::Sub2ApiSubscription,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/subscriptions/my", get(get_my_subscriptions))
}

#[derive(Debug, Deserialize)]
struct MySubscriptionsQuery {
    token: Option<String>,
    lang: Option<String>,
}

#[derive(Debug, Serialize)]
struct MySubscriptionsResponse {
    subscriptions: Vec<MySubscriptionView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct MySubscriptionView {
    id: i64,
    user_id: i64,
    group_id: i64,
    starts_at: String,
    expires_at: String,
    status: String,
    daily_usage_usd: f64,
    weekly_usage_usd: f64,
    monthly_usage_usd: f64,
    daily_window_start: Option<String>,
    weekly_window_start: Option<String>,
    monthly_window_start: Option<String>,
    notes: Option<String>,
    group_name: Option<String>,
    platform: Option<String>,
}

async fn get_my_subscriptions(
    State(state): State<AppState>,
    Query(query): Query<MySubscriptionsQuery>,
) -> AppResult<Json<MySubscriptionsResponse>> {
    let locale = resolve_locale(query.lang.as_deref());
    let token = query
        .token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::unauthorized(message(locale, "缺少 token", "Missing token")))?;

    let sub2api = state.sub2api.as_ref().ok_or_else(|| {
        AppError::public_internal(message(
            locale,
            "获取订阅信息失败",
            "Failed to get subscriptions",
        ))
    })?;

    let token_user = sub2api
        .get_current_user_by_token(token)
        .await
        .map_err(|error| {
            if error.to_string().starts_with("Failed to get current user:") {
                AppError::unauthorized(message(locale, "无效的 token", "Invalid token"))
            } else {
                AppError::public_internal(message(
                    locale,
                    "获取订阅信息失败",
                    "Failed to get subscriptions",
                ))
            }
        })?;

    let admin_api_key = sub2api_admin_api_key(&state).await?;
    let (subscriptions, groups) = tokio::try_join!(
        sub2api.get_user_subscriptions(token_user.id, &admin_api_key),
        sub2api.get_all_groups(&admin_api_key),
    )
    .map_err(AppError::internal)?;

    let group_map: HashMap<i64, (Option<String>, Option<String>)> = groups
        .into_iter()
        .map(|group| {
            (
                group.id,
                (
                    if group.name.is_empty() {
                        None
                    } else {
                        Some(group.name)
                    },
                    group.platform,
                ),
            )
        })
        .collect();

    Ok(Json(MySubscriptionsResponse {
        subscriptions: subscriptions
            .into_iter()
            .map(|item| to_my_subscription_view(item, &group_map))
            .collect(),
    }))
}

fn to_my_subscription_view(
    item: Sub2ApiSubscription,
    group_map: &HashMap<i64, (Option<String>, Option<String>)>,
) -> MySubscriptionView {
    let (group_name, platform) = group_map
        .get(&item.group_id)
        .cloned()
        .unwrap_or((None, None));

    MySubscriptionView {
        id: item.id,
        user_id: item.user_id,
        group_id: item.group_id,
        starts_at: item.starts_at,
        expires_at: item.expires_at,
        status: item.status,
        daily_usage_usd: item.daily_usage_usd,
        weekly_usage_usd: item.weekly_usage_usd,
        monthly_usage_usd: item.monthly_usage_usd,
        daily_window_start: item.daily_window_start,
        weekly_window_start: item.weekly_window_start,
        monthly_window_start: item.monthly_window_start,
        notes: item.notes,
        group_name,
        platform,
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
        return Err(AppError::public_internal("Failed to get subscriptions"));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::{Json, Router, extract::State, routing::get};
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

    #[derive(Clone)]
    struct MockUser {
        id: i64,
    }

    async fn test_state(sub2api_base_url: Option<String>) -> AppState {
        let db_path =
            std::env::temp_dir().join(format!("opay-my-subs-route-{}.db", Uuid::new_v4()));
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

    async fn start_mock_sub2api() -> (String, JoinHandle<()>) {
        async fn auth_me(State(user): State<MockUser>) -> Json<serde_json::Value> {
            Json(json!({
                "data": {
                    "id": user.id,
                    "status": "active",
                    "email": "user@example.com",
                    "username": "tester",
                    "balance": 0
                }
            }))
        }

        async fn user_subscriptions() -> Json<serde_json::Value> {
            Json(json!({
                "data": [
                    {
                        "id": 101,
                        "user_id": 1,
                        "group_id": 9,
                        "starts_at": "2026-04-01T00:00:00Z",
                        "expires_at": "2026-05-01T00:00:00Z",
                        "status": "active",
                        "daily_usage_usd": 1.2,
                        "weekly_usage_usd": 4.5,
                        "monthly_usage_usd": 12.3,
                        "daily_window_start": "2026-04-12T00:00:00Z",
                        "weekly_window_start": "2026-04-07T00:00:00Z",
                        "monthly_window_start": "2026-04-01T00:00:00Z",
                        "notes": "vip"
                    }
                ]
            }))
        }

        async fn groups_all() -> Json<serde_json::Value> {
            Json(json!({
                "data": [
                    {
                        "id": 9,
                        "name": "OpenAI Group",
                        "status": "active",
                        "platform": "openai",
                        "subscription_type": "subscription"
                    }
                ]
            }))
        }

        let app = Router::new()
            .route("/api/v1/auth/me", get(auth_me))
            .route(
                "/api/v1/admin/users/1/subscriptions",
                get(user_subscriptions),
            )
            .route("/api/v1/admin/groups/all", get(groups_all))
            .with_state(MockUser { id: 1 });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn my_subscriptions_enriches_group_name_and_platform() {
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

        let response = get_my_subscriptions(
            State(state),
            Query(MySubscriptionsQuery {
                token: Some("user-token".to_string()),
                lang: None,
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(response.subscriptions.len(), 1);
        assert_eq!(
            response.subscriptions[0].group_name.as_deref(),
            Some("OpenAI Group")
        );
        assert_eq!(
            response.subscriptions[0].platform.as_deref(),
            Some("openai")
        );

        handle.abort();
    }
}
