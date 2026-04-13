use axum::{
    Json, Router,
    extract::{Query, State},
    http::HeaderMap,
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    admin_auth::{AdminTokenQuery, verify_admin},
    error::{AppError, AppResult},
    provider_instances::ProviderInstanceRepository,
    response,
    system_config::{SystemConfigEntry, UpsertSystemConfig},
};

const SENSITIVE_PATTERNS: &[&str] = &["KEY", "SECRET", "PASSWORD", "PRIVATE"];
const ALLOWED_CONFIG_KEYS: &[&str] = &[
    "ENABLED_PAYMENT_TYPES",
    "RECHARGE_MIN_AMOUNT",
    "RECHARGE_MAX_AMOUNT",
    "DAILY_RECHARGE_LIMIT",
    "ORDER_TIMEOUT_MINUTES",
    "IFRAME_ALLOW_ORIGINS",
    "PRODUCT_NAME_PREFIX",
    "PRODUCT_NAME_SUFFIX",
    "BALANCE_PAYMENT_DISABLED",
    "CANCEL_RATE_LIMIT_ENABLED",
    "CANCEL_RATE_LIMIT_WINDOW",
    "CANCEL_RATE_LIMIT_UNIT",
    "CANCEL_RATE_LIMIT_MAX",
    "CANCEL_RATE_LIMIT_WINDOW_MODE",
    "MAX_PENDING_ORDERS",
    "LOAD_BALANCE_STRATEGY",
    "ENABLED_PROVIDERS",
    "SUB2API_ADMIN_API_KEY",
    "OVERRIDE_ENV_ENABLED",
    "DEFAULT_DEDUCT_BALANCE",
];

pub fn router() -> Router<AppState> {
    Router::new().route("/api/admin/config", get(get_configs).put(put_configs))
}

#[derive(Debug, Serialize)]
struct ConfigListResponse {
    configs: Vec<SystemConfigView>,
}

#[derive(Debug, Serialize)]
struct SystemConfigView {
    key: String,
    value: String,
    group: String,
    label: Option<String>,
    updated_at: i64,
}

#[derive(Debug, Deserialize)]
struct PutConfigsRequest {
    configs: Vec<PutConfigItem>,
}

#[derive(Debug, Deserialize)]
struct PutConfigItem {
    key: String,
    value: String,
    group: Option<String>,
    label: Option<String>,
}

async fn get_configs(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Query<AdminTokenQuery>,
) -> AppResult<Json<ConfigListResponse>> {
    verify_admin(&headers, query, &state).await?;

    let configs = state
        .system_config
        .list_all()
        .await
        .map_err(AppError::internal)?;

    Ok(Json(ConfigListResponse {
        configs: configs.into_iter().map(mask_config).collect(),
    }))
}

async fn put_configs(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Query<AdminTokenQuery>,
    Json(body): Json<PutConfigsRequest>,
) -> AppResult<Json<response::UpdatedResponse>> {
    verify_admin(&headers, query, &state).await?;

    if body.configs.is_empty() {
        return Err(AppError::bad_request("缺少必填字段: configs 数组"));
    }

    for item in &body.configs {
        if item.key.trim().is_empty() {
            return Err(AppError::bad_request("每条配置必须包含 key 和 value"));
        }
        if !ALLOWED_CONFIG_KEYS.contains(&item.key.as_str()) {
            return Err(AppError::bad_request(format!(
                "不允许修改配置项: {}",
                item.key
            )));
        }
    }

    validate_enabled_providers_change(&state, &body).await?;

    let filtered: Vec<UpsertSystemConfig> = body
        .configs
        .into_iter()
        .filter(|item| !is_masked_sensitive_value(&item.key, &item.value))
        .map(|item| UpsertSystemConfig {
            key: item.key,
            value: item.value,
            group: item.group,
            label: item.label,
        })
        .collect();

    state
        .system_config
        .set_many(&filtered)
        .await
        .map_err(AppError::internal)?;

    Ok(response::updated(filtered.len()))
}

fn mask_config(config: SystemConfigEntry) -> SystemConfigView {
    SystemConfigView {
        key: config.key.clone(),
        value: mask_sensitive_value(&config.key, &config.value),
        group: config.group,
        label: config.label,
        updated_at: config.updated_at,
    }
}

fn mask_sensitive_value(key: &str, value: &str) -> String {
    let is_sensitive = SENSITIVE_PATTERNS
        .iter()
        .any(|pattern| key.to_ascii_uppercase().contains(pattern));
    if !is_sensitive {
        return value.to_string();
    }
    if value.len() <= 4 {
        return "****".to_string();
    }
    format!(
        "{}{}",
        "*".repeat(value.len() - 4),
        &value[value.len() - 4..]
    )
}

fn is_masked_sensitive_value(key: &str, value: &str) -> bool {
    let is_sensitive = SENSITIVE_PATTERNS
        .iter()
        .any(|pattern| key.to_ascii_uppercase().contains(pattern));
    is_sensitive && value.chars().filter(|ch| *ch == '*').count() >= 4
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

async fn validate_enabled_providers_change(
    state: &AppState,
    body: &PutConfigsRequest,
) -> AppResult<()> {
    let Some(entry) = body
        .configs
        .iter()
        .find(|item| item.key == "ENABLED_PROVIDERS")
    else {
        return Ok(());
    };

    let Some(current_raw) = state
        .system_config
        .get("ENABLED_PROVIDERS")
        .await
        .map_err(AppError::internal)?
    else {
        return Ok(());
    };

    let next = parse_csv(&entry.value);
    let current = parse_csv(&current_raw);
    let removed: Vec<String> = current
        .into_iter()
        .filter(|provider| !next.iter().any(|next_provider| next_provider == provider))
        .collect();

    if removed.is_empty() {
        return Ok(());
    }

    let blocked = find_blocked_providers(state, &removed).await?;
    if blocked.is_empty() {
        return Ok(());
    }

    Err(AppError::conflict(format!(
        "无法关闭服务商类型 [{}]：存在关联实例，请先删除所有实例",
        blocked.join(", ")
    )))
}

async fn find_blocked_providers(
    state: &AppState,
    provider_keys: &[String],
) -> AppResult<Vec<String>> {
    let repo = ProviderInstanceRepository::new(state.db.clone());
    repo.find_existing_provider_keys(provider_keys)
        .await
        .map_err(AppError::internal)
}
