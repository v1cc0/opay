use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    channel::ChannelRepository,
    error::{AppError, AppResult},
    http::common::{message, resolve_locale},
};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/channels", get(list_channels))
}

#[derive(Debug, Deserialize)]
struct ChannelsQuery {
    token: Option<String>,
    lang: Option<String>,
}

#[derive(Debug, Serialize)]
struct ChannelsResponse {
    channels: Vec<ChannelView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelView {
    id: String,
    group_id: Option<i64>,
    name: String,
    platform: String,
    rate_multiplier: f64,
    description: Option<String>,
    models: Vec<String>,
    features: Vec<String>,
    sort_order: i64,
}

async fn list_channels(
    State(state): State<AppState>,
    Query(query): Query<ChannelsQuery>,
) -> AppResult<Json<ChannelsResponse>> {
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
            "获取渠道列表失败",
            "Failed to list channels",
        ))
    })?;
    sub2api
        .get_current_user_by_token(token)
        .await
        .map_err(|error| {
            if error.to_string().starts_with("Failed to get current user:") {
                AppError::unauthorized(message(locale, "无效的 token", "Invalid token"))
            } else {
                AppError::public_internal(message(
                    locale,
                    "获取渠道列表失败",
                    "Failed to list channels",
                ))
            }
        })?;

    let admin_api_key = sub2api_admin_api_key(&state).await?;
    let channels = ChannelRepository::new(state.db.clone())
        .list_enabled()
        .await
        .map_err(AppError::internal)?;
    let mut views = Vec::new();

    for channel in channels {
        if let Some(group_id) = channel.group_id {
            let active = matches!(
                sub2api.get_group(group_id, &admin_api_key).await,
                Ok(Some(group)) if group.status == "active"
            );
            if !active {
                continue;
            }
        }

        views.push(ChannelView {
            id: channel.id,
            group_id: channel.group_id,
            name: channel.name,
            platform: channel.platform,
            rate_multiplier: (channel.rate_multiplier_bps as f64) / 10_000.0,
            description: channel.description,
            models: parse_string_array(channel.models.as_deref()),
            features: parse_string_array(channel.features.as_deref()),
            sort_order: channel.sort_order,
        });
    }

    Ok(Json(ChannelsResponse { channels: views }))
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
        return Err(AppError::public_internal("Failed to list channels"));
    }
    Ok(trimmed.to_string())
}

fn parse_string_array(value: Option<&str>) -> Vec<String> {
    value
        .and_then(|text| serde_json::from_str::<Vec<String>>(text).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::{
        Json, Router,
        extract::{Path, State},
        routing::get,
    };
    use serde_json::json;
    use tokio::{net::TcpListener, task::JoinHandle};
    use turso::{Value, params::Params};
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
            std::env::temp_dir().join(format!("sub2apipay-channels-route-{}.db", Uuid::new_v4()));
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

        async fn group(Path(group_id): Path<i64>) -> Json<serde_json::Value> {
            let status = if group_id == 9 { "active" } else { "disabled" };
            Json(json!({
                "data": {
                    "id": group_id,
                    "name": format!("Group {group_id}"),
                    "status": status,
                    "subscription_type": "subscription"
                }
            }))
        }

        let app = Router::new()
            .route("/api/v1/auth/me", get(auth_me))
            .route("/api/v1/admin/groups/{id}", get(group))
            .with_state(MockUser { id: 1 });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn filters_out_inactive_group_channels() {
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

        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO channels (id, group_id, name, platform, rate_multiplier_bps, description, models, features, sort_order, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            Params::Positional(vec![
                Value::Text("ch_ok".to_string()),
                Value::Integer(9),
                Value::Text("OpenAI".to_string()),
                Value::Text("openai".to_string()),
                Value::Integer(1500),
                Value::Text("desc".to_string()),
                Value::Text("[\"gpt-4.1\"]".to_string()),
                Value::Text("[\"fast\"]".to_string()),
                Value::Integer(1),
                Value::Integer(1),
            ]),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO channels (id, group_id, name, platform, rate_multiplier_bps, sort_order, enabled)
             VALUES ('ch_skip', 10, 'Skip', 'openai', 1000, 2, 1)",
            (),
        )
        .await
        .unwrap();

        let response = list_channels(
            State(state),
            Query(ChannelsQuery {
                token: Some("user-token".to_string()),
                lang: None,
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(response.channels.len(), 1);
        assert_eq!(response.channels[0].id, "ch_ok");
        assert_eq!(response.channels[0].rate_multiplier, 0.15);
        assert_eq!(response.channels[0].models, vec!["gpt-4.1"]);

        handle.abort();
    }
}
