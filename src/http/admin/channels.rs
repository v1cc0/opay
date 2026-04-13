use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{
    AppState,
    admin_auth::verify_admin_values,
    channel::{ChannelRecord, ChannelRepository, ChannelWrite},
    error::{AppError, AppResult},
    http::common::timestamp_to_rfc3339,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/admin/channels",
            get(list_channels).post(create_channel),
        )
        .route(
            "/api/admin/channels/{id}",
            get(get_channel).put(update_channel).delete(delete_channel),
        )
}

#[derive(Debug, Deserialize)]
struct AdminChannelsQuery {
    token: Option<String>,
    lang: Option<String>,
}

#[derive(Debug, Serialize)]
struct AdminChannelsResponse {
    channels: Vec<AdminChannelView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminChannelView {
    id: String,
    group_id: Option<i64>,
    name: String,
    platform: String,
    rate_multiplier: f64,
    description: Option<String>,
    models: Option<String>,
    features: Option<String>,
    sort_order: i64,
    enabled: bool,
    group_exists: bool,
    created_at: String,
    updated_at: String,
}

async fn list_channels(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminChannelsQuery>,
) -> AppResult<Json<AdminChannelsResponse>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let repo = ChannelRepository::new(state.db.clone());
    let channels = repo.list_all().await.map_err(AppError::internal)?;
    let platform = state.platform.as_ref();
    let admin_api_key = platform_admin_api_key(&state).await.ok();

    let mut views = Vec::new();
    for channel in channels {
        let group_exists = match (channel.group_id, platform, admin_api_key.as_deref()) {
            (Some(group_id), Some(client), Some(key)) => {
                matches!(client.get_group(group_id, key).await, Ok(Some(_)))
            }
            (Some(_), _, _) => false,
            (None, _, _) => false,
        };
        views.push(to_admin_channel_view(channel, group_exists));
    }

    Ok(Json(AdminChannelsResponse { channels: views }))
}

async fn create_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminChannelsQuery>,
    Json(body): Json<JsonValue>,
) -> AppResult<(axum::http::StatusCode, Json<AdminChannelView>)> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let repo = ChannelRepository::new(state.db.clone());
    let write = parse_create_body(&body)?;
    ensure_group_id_unique(&repo, write.group_id, None).await?;
    let created = repo.create(write).await.map_err(AppError::internal)?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(to_admin_channel_view(created, false)),
    ))
}

async fn get_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminChannelsQuery>,
    Path(id): Path<String>,
) -> AppResult<Json<AdminChannelView>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let repo = ChannelRepository::new(state.db.clone());
    let channel = repo
        .get(&id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("渠道不存在"))?;
    let group_exists = match (
        channel.group_id,
        state.platform.as_ref(),
        platform_admin_api_key(&state).await.ok(),
    ) {
        (Some(group_id), Some(client), Some(key)) => {
            matches!(client.get_group(group_id, &key).await, Ok(Some(_)))
        }
        _ => false,
    };

    Ok(Json(to_admin_channel_view(channel, group_exists)))
}

async fn update_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminChannelsQuery>,
    Path(id): Path<String>,
    Json(body): Json<JsonValue>,
) -> AppResult<Json<AdminChannelView>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let repo = ChannelRepository::new(state.db.clone());
    let existing = repo
        .get(&id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("渠道不存在"))?;
    let next = merge_update_body(existing.clone(), &body)?;
    ensure_group_id_unique(&repo, next.group_id, Some(&id)).await?;
    let updated = repo
        .replace(&id, next)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("渠道不存在"))?;

    Ok(Json(to_admin_channel_view(updated, false)))
}

async fn delete_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminChannelsQuery>,
    Path(id): Path<String>,
) -> AppResult<Json<crate::response::SuccessResponse>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let repo = ChannelRepository::new(state.db.clone());
    let existing = repo
        .get(&id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("渠道不存在"))?;
    let _ = repo
        .delete(&existing.id)
        .await
        .map_err(AppError::internal)?;
    Ok(crate::response::success())
}

fn to_admin_channel_view(channel: ChannelRecord, group_exists: bool) -> AdminChannelView {
    AdminChannelView {
        id: channel.id,
        group_id: channel.group_id,
        name: channel.name,
        platform: channel.platform,
        rate_multiplier: bps_to_rate(channel.rate_multiplier_bps),
        description: channel.description,
        models: channel.models,
        features: channel.features,
        sort_order: channel.sort_order,
        enabled: channel.enabled,
        group_exists,
        created_at: timestamp_to_rfc3339(channel.created_at),
        updated_at: timestamp_to_rfc3339(channel.updated_at),
    }
}

fn parse_create_body(body: &JsonValue) -> AppResult<ChannelWrite> {
    let object = body
        .as_object()
        .ok_or_else(|| AppError::bad_request("请求体必须是 JSON 对象"))?;

    let name = required_non_empty_string(object, "name", "name 必须非空")?;
    let platform = required_non_empty_string(object, "platform", "platform 必须非空")?;
    let rate_multiplier = required_positive_rate(object.get("rate_multiplier"))?;
    let sort_order = optional_non_negative_i64(object.get("sort_order"))?.unwrap_or(0);

    Ok(ChannelWrite {
        group_id: optional_group_id(object.get("group_id"))?,
        name,
        platform,
        rate_multiplier_bps: rate_to_bps(rate_multiplier),
        description: optional_nullable_string(object.get("description"))?,
        models: normalize_json_text(object.get("models"))?,
        features: normalize_json_text(object.get("features"))?,
        sort_order,
        enabled: optional_bool(object.get("enabled"))?.unwrap_or(true),
    })
}

fn merge_update_body(existing: ChannelRecord, body: &JsonValue) -> AppResult<ChannelWrite> {
    let object = body
        .as_object()
        .ok_or_else(|| AppError::bad_request("请求体必须是 JSON 对象"))?;

    let group_id = if object.contains_key("group_id") {
        optional_group_id(object.get("group_id"))?
    } else {
        existing.group_id
    };
    let name = if object.contains_key("name") {
        let value = required_non_empty_string(object, "name", "name 不能为空")?;
        if value.len() > 100 {
            return Err(AppError::bad_request("name 不能超过 100 个字符"));
        }
        value
    } else {
        existing.name
    };
    let platform = if object.contains_key("platform") {
        let value = required_non_empty_string(object, "platform", "platform 不能为空")?;
        if value.len() > 50 {
            return Err(AppError::bad_request("platform 不能超过 50 个字符"));
        }
        value
    } else {
        existing.platform
    };
    let rate_multiplier_bps = if object.contains_key("rate_multiplier") {
        rate_to_bps(required_positive_rate(object.get("rate_multiplier"))?)
    } else {
        existing.rate_multiplier_bps
    };
    let description = if object.contains_key("description") {
        optional_nullable_string(object.get("description"))?
    } else {
        existing.description
    };
    let models = if object.contains_key("models") {
        normalize_json_text(object.get("models"))?
    } else {
        existing.models
    };
    let features = if object.contains_key("features") {
        normalize_json_text(object.get("features"))?
    } else {
        existing.features
    };
    let sort_order = if object.contains_key("sort_order") {
        optional_non_negative_i64(object.get("sort_order"))?.unwrap_or(0)
    } else {
        existing.sort_order
    };
    let enabled = if object.contains_key("enabled") {
        optional_bool(object.get("enabled"))?
            .ok_or_else(|| AppError::bad_request("enabled 必须是布尔值"))?
    } else {
        existing.enabled
    };

    Ok(ChannelWrite {
        group_id,
        name,
        platform,
        rate_multiplier_bps,
        description,
        models,
        features,
        sort_order,
        enabled,
    })
}

async fn ensure_group_id_unique(
    repo: &ChannelRepository,
    group_id: Option<i64>,
    skip_channel_id: Option<&str>,
) -> AppResult<()> {
    let Some(group_id) = group_id else {
        return Ok(());
    };
    let Some(existing) = repo
        .get_by_group_id(group_id)
        .await
        .map_err(AppError::internal)?
    else {
        return Ok(());
    };

    if skip_channel_id.is_some_and(|id| id == existing.id) {
        return Ok(());
    }

    Err(AppError::conflict(format!(
        "分组 ID {} 已被渠道「{}」使用",
        group_id, existing.name
    )))
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
        return Err(AppError::public_internal(
            "Platform admin api key is not configured",
        ));
    }
    Ok(trimmed.to_string())
}

fn required_non_empty_string(
    object: &serde_json::Map<String, JsonValue>,
    field: &str,
    message: &str,
) -> AppResult<String> {
    let value = object
        .get(field)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::bad_request(message))?;
    Ok(value.to_string())
}

fn optional_group_id(value: Option<&JsonValue>) -> AppResult<Option<i64>> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .filter(|item| *item > 0)
            .map(Some)
            .ok_or_else(|| AppError::bad_request("group_id 必须是正整数或 null")),
    }
}

fn required_positive_rate(value: Option<&JsonValue>) -> AppResult<f64> {
    value
        .and_then(JsonValue::as_f64)
        .filter(|item| item.is_finite() && *item > 0.0)
        .ok_or_else(|| AppError::bad_request("rate_multiplier 必须是正数"))
}

fn optional_non_negative_i64(value: Option<&JsonValue>) -> AppResult<Option<i64>> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .filter(|item| *item >= 0)
            .map(Some)
            .ok_or_else(|| AppError::bad_request("sort_order 必须是非负整数")),
    }
}

fn optional_bool(value: Option<&JsonValue>) -> AppResult<Option<bool>> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Bool(value)) => Ok(Some(*value)),
        _ => Err(AppError::bad_request("enabled 必须是布尔值")),
    }
}

fn optional_nullable_string(value: Option<&JsonValue>) -> AppResult<Option<String>> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        _ => Err(AppError::bad_request("字段必须是字符串或 null")),
    }
}

fn normalize_json_text(value: Option<&JsonValue>) -> AppResult<Option<String>> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Some(JsonValue::Array(value)) => serde_json::to_string(value)
            .map(Some)
            .map_err(AppError::internal),
        Some(JsonValue::Object(value)) => serde_json::to_string(value)
            .map(Some)
            .map_err(AppError::internal),
        _ => Err(AppError::bad_request("字段格式不正确")),
    }
}

fn rate_to_bps(value: f64) -> i64 {
    (value * 10_000.0).round() as i64
}

fn bps_to_rate(value: i64) -> f64 {
    (value as f64) / 10_000.0
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
            "opay-admin-channels-route-{}.db",
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
        async fn group(Path(group_id): Path<i64>) -> Json<serde_json::Value> {
            let exists = group_id == 9;
            if exists {
                Json(json!({
                    "data": {
                        "id": 9,
                        "name": "OpenAI",
                        "status": "active",
                        "subscription_type": "subscription"
                    }
                }))
            } else {
                Json(json!({
                    "data": null
                }))
            }
        }

        let app = Router::new().route("/api/v1/admin/groups/{id}", get(group));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn create_list_update_delete_admin_channel() {
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

        let created = create_channel(
            State(state.clone()),
            admin_headers(),
            Query(AdminChannelsQuery {
                token: None,
                lang: None,
            }),
            Json(json!({
                "group_id": 9,
                "name": "OpenAI",
                "platform": "openai",
                "rate_multiplier": 0.15,
                "description": "desc",
                "models": "[\"gpt-4.1\"]",
                "features": "[\"fast\"]",
                "sort_order": 1,
                "enabled": true
            })),
        )
        .await
        .unwrap()
        .1
        .0;
        assert_eq!(created.group_id, Some(9));
        assert_eq!(created.rate_multiplier, 0.15);

        let listed = list_channels(
            State(state.clone()),
            admin_headers(),
            Query(AdminChannelsQuery {
                token: None,
                lang: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(listed.channels.len(), 1);
        assert!(listed.channels[0].group_exists);

        let updated = update_channel(
            State(state.clone()),
            admin_headers(),
            Query(AdminChannelsQuery {
                token: None,
                lang: None,
            }),
            Path(created.id.clone()),
            Json(json!({
                "enabled": false,
                "rate_multiplier": 0.25
            })),
        )
        .await
        .unwrap()
        .0;
        assert!(!updated.enabled);
        assert_eq!(updated.rate_multiplier, 0.25);

        let deleted = delete_channel(
            State(state),
            admin_headers(),
            Query(AdminChannelsQuery {
                token: None,
                lang: None,
            }),
            Path(created.id),
        )
        .await
        .unwrap()
        .0;
        assert!(deleted.success);

        handle.abort();
    }
}
