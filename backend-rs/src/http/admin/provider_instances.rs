use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::{
    AppState,
    admin_auth::verify_admin_values,
    crypto,
    error::{AppError, AppResult},
    provider_instances::{
        ProviderInstanceRecord, ProviderInstanceRepository, ProviderInstanceWrite,
    },
    response,
};

const SENSITIVE_PATTERNS: &[&str] = &["key", "pkey", "secret", "private", "password"];
const VALID_PROVIDERS: &[&str] = &["easypay", "alipay", "wxpay", "stripe"];

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/admin/provider-instances",
            get(list_instances).post(create_instance),
        )
        .route(
            "/api/admin/provider-instances/{id}",
            get(get_instance)
                .put(update_instance)
                .delete(delete_instance),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInstanceQuery {
    provider_key: Option<String>,
    token: Option<String>,
    lang: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProviderInstanceRequest {
    provider_key: String,
    name: String,
    config: HashMap<String, String>,
    enabled: Option<bool>,
    sort_order: Option<i64>,
    supported_types: Option<String>,
    limits: Option<JsonValue>,
    refund_enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ProviderInstanceListResponse {
    instances: Vec<ProviderInstanceView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInstanceView {
    id: String,
    provider_key: String,
    name: String,
    config: HashMap<String, String>,
    supported_types: String,
    enabled: bool,
    sort_order: i64,
    limits: Option<JsonValue>,
    refund_enabled: bool,
    created_at: i64,
    updated_at: i64,
}

async fn list_instances(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Query<ProviderInstanceQuery>,
) -> AppResult<Json<ProviderInstanceListResponse>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let repo = ProviderInstanceRepository::new(state.db.clone());
    let items = repo
        .list(query.provider_key.as_deref())
        .await
        .map_err(AppError::internal)?;

    Ok(Json(ProviderInstanceListResponse {
        instances: items
            .into_iter()
            .map(|item| to_view(&state, item))
            .collect::<AppResult<Vec<_>>>()?,
    }))
}

async fn create_instance(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Query<ProviderInstanceQuery>,
    Json(body): Json<CreateProviderInstanceRequest>,
) -> AppResult<(axum::http::StatusCode, Json<ProviderInstanceView>)> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    validate_provider_key(&body.provider_key)?;
    validate_name(&body.name)?;
    validate_sort_order(body.sort_order)?;

    let encrypted_config = crypto::encrypt(
        state.config.admin_token.as_deref(),
        &serde_json::to_string(&body.config).map_err(AppError::internal)?,
    )
    .map_err(AppError::internal)?;

    let repo = ProviderInstanceRepository::new(state.db.clone());
    let record = repo
        .create(ProviderInstanceWrite {
            provider_key: body.provider_key,
            name: body.name.trim().to_string(),
            config: encrypted_config,
            supported_types: body.supported_types.unwrap_or_default(),
            enabled: body.enabled.unwrap_or(true),
            sort_order: body.sort_order.unwrap_or(0),
            limits: normalize_limits(body.limits)?,
            refund_enabled: body.refund_enabled.unwrap_or(false),
        })
        .await
        .map_err(AppError::internal)?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(to_view(&state, record)?),
    ))
}

async fn get_instance(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Query<ProviderInstanceQuery>,
    Path(id): Path<String>,
) -> AppResult<Json<ProviderInstanceView>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let repo = ProviderInstanceRepository::new(state.db.clone());
    let record = repo
        .get(&id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("支付实例不存在"))?;

    Ok(Json(to_view(&state, record)?))
}

async fn update_instance(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Query<ProviderInstanceQuery>,
    Path(id): Path<String>,
    Json(body): Json<JsonValue>,
) -> AppResult<Json<ProviderInstanceView>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let body = body
        .as_object()
        .ok_or_else(|| AppError::bad_request("请求体必须是 JSON 对象"))?;

    let repo = ProviderInstanceRepository::new(state.db.clone());
    let existing = repo
        .get(&id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("支付实例不存在"))?;

    let mut next = ProviderInstanceWrite {
        provider_key: existing.provider_key.clone(),
        name: existing.name.clone(),
        config: existing.config.clone(),
        supported_types: existing.supported_types.clone(),
        enabled: existing.enabled,
        sort_order: existing.sort_order,
        limits: existing.limits.clone(),
        refund_enabled: existing.refund_enabled,
    };

    if let Some(provider_key) = optional_string_field(body, "providerKey")? {
        validate_provider_key(&provider_key)?;
        next.provider_key = provider_key;
    }

    if let Some(name) = optional_string_field(body, "name")? {
        validate_name(&name)?;
        next.name = name.trim().to_string();
    }

    if let Some(enabled) = optional_bool_field(body, "enabled")? {
        next.enabled = enabled;
    }

    if let Some(sort_order) = optional_i64_field(body, "sortOrder")? {
        validate_sort_order(Some(sort_order))?;
        next.sort_order = sort_order;
    }

    if let Some(supported_types) = optional_string_field(body, "supportedTypes")? {
        next.supported_types = supported_types;
    }

    if has_field(body, "limits") {
        next.limits = normalize_limits(optional_json_field(body, "limits")?)?;
    }

    if let Some(refund_enabled) = optional_bool_field(body, "refundEnabled")? {
        next.refund_enabled = refund_enabled;
    }

    if has_field(body, "config") {
        let new_config = required_string_map_field(body, "config")?;
        let existing_config = decrypt_config_map(&state, &existing.config)?;
        let merged = merge_config_with_existing(new_config, &existing_config);

        if has_credential_change(&merged, &existing_config) {
            let pending_count = repo
                .count_pending_orders(&id)
                .await
                .map_err(AppError::internal)?;
            if pending_count > 0 {
                return Err(AppError::conflict(format!(
                    "该实例有 {} 个进行中的订单，修改凭证可能导致回调验签失败。请等待订单完成后再修改，或先禁用该实例。",
                    pending_count
                )));
            }
        }

        next.config = crypto::encrypt(
            state.config.admin_token.as_deref(),
            &serde_json::to_string(&merged).map_err(AppError::internal)?,
        )
        .map_err(AppError::internal)?;
    }

    let updated = repo
        .replace(&id, next)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("支付实例不存在"))?;

    Ok(Json(to_view(&state, updated)?))
}

async fn delete_instance(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Query<ProviderInstanceQuery>,
    Path(id): Path<String>,
) -> AppResult<Json<response::SuccessResponse>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let repo = ProviderInstanceRepository::new(state.db.clone());
    let existing = repo
        .get(&id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("支付实例不存在"))?;

    let pending_count = repo
        .count_pending_orders(&existing.id)
        .await
        .map_err(AppError::internal)?;
    if pending_count > 0 {
        return Err(AppError::conflict(format!(
            "该实例有 {} 个进行中的订单，无法删除。请等待订单完成或先禁用该实例。",
            pending_count
        )));
    }

    repo.delete(&existing.id)
        .await
        .map_err(AppError::internal)?;
    Ok(response::success())
}

fn to_view(state: &AppState, record: ProviderInstanceRecord) -> AppResult<ProviderInstanceView> {
    Ok(ProviderInstanceView {
        id: record.id,
        provider_key: record.provider_key,
        name: record.name,
        config: decrypt_and_mask_config(state, &record.config)?,
        supported_types: record.supported_types,
        enabled: record.enabled,
        sort_order: record.sort_order,
        limits: record
            .limits
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(AppError::internal)?,
        refund_enabled: record.refund_enabled,
        created_at: record.created_at,
        updated_at: record.updated_at,
    })
}

fn decrypt_and_mask_config(
    state: &AppState,
    encrypted: &str,
) -> AppResult<HashMap<String, String>> {
    let plaintext = crypto::decrypt(state.config.admin_token.as_deref(), encrypted)
        .map_err(AppError::internal)?;
    let config: HashMap<String, String> =
        serde_json::from_str(&plaintext).map_err(AppError::internal)?;

    Ok(config
        .into_iter()
        .map(|(key, value)| {
            let masked = if is_sensitive_field(&key) {
                mask_value(&value)
            } else {
                value
            };
            (key, masked)
        })
        .collect())
}

fn decrypt_config_map(state: &AppState, encrypted: &str) -> AppResult<HashMap<String, String>> {
    let plaintext = crypto::decrypt(state.config.admin_token.as_deref(), encrypted)
        .map_err(AppError::internal)?;
    serde_json::from_str(&plaintext).map_err(AppError::internal)
}

fn is_sensitive_field(field_name: &str) -> bool {
    let lower = field_name.to_ascii_lowercase();
    SENSITIVE_PATTERNS
        .iter()
        .any(|pattern| lower.contains(pattern))
}

fn mask_value(value: &str) -> String {
    if value.is_empty() {
        return "****".to_string();
    }
    if value.len() > 4 {
        format!(
            "{}{}",
            "*".repeat(value.len() - 4),
            &value[value.len() - 4..]
        )
    } else {
        "****".to_string()
    }
}

fn is_masked_value(value: &str) -> bool {
    value.chars().filter(|ch| *ch == '*').count() >= 4
}

fn merge_config_with_existing(
    new_config: HashMap<String, String>,
    existing: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut merged = HashMap::new();
    for (key, value) in new_config {
        if is_masked_value(&value) {
            if let Some(existing_value) = existing.get(&key) {
                merged.insert(key, existing_value.clone());
                continue;
            }
        }
        merged.insert(key, value);
    }
    merged
}

fn has_credential_change(
    merged: &HashMap<String, String>,
    existing: &HashMap<String, String>,
) -> bool {
    merged
        .iter()
        .any(|(key, value)| is_sensitive_field(key) && existing.get(key) != Some(value))
}

fn validate_provider_key(provider_key: &str) -> AppResult<()> {
    if !VALID_PROVIDERS.contains(&provider_key) {
        return Err(AppError::bad_request(format!(
            "无效的 providerKey，可选值: {}",
            VALID_PROVIDERS.join(", ")
        )));
    }
    Ok(())
}

fn validate_name(name: &str) -> AppResult<()> {
    if name.trim().is_empty() {
        return Err(AppError::bad_request("name 不能为空"));
    }
    Ok(())
}

fn validate_sort_order(sort_order: Option<i64>) -> AppResult<()> {
    if let Some(sort_order) = sort_order {
        if sort_order < 0 {
            return Err(AppError::bad_request("sortOrder 必须是非负整数"));
        }
    }
    Ok(())
}

fn normalize_limits(limits: Option<JsonValue>) -> AppResult<Option<String>> {
    match limits {
        Some(limits) => serde_json::to_string(&limits)
            .map(Some)
            .map_err(AppError::internal),
        None => Ok(None),
    }
}

fn has_field(body: &JsonMap<String, JsonValue>, key: &str) -> bool {
    body.contains_key(key)
}

fn optional_string_field(
    body: &JsonMap<String, JsonValue>,
    key: &str,
) -> AppResult<Option<String>> {
    match body.get(key) {
        None => Ok(None),
        Some(JsonValue::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(AppError::bad_request(format!("{} 必须是字符串", key))),
    }
}

fn optional_bool_field(body: &JsonMap<String, JsonValue>, key: &str) -> AppResult<Option<bool>> {
    match body.get(key) {
        None => Ok(None),
        Some(JsonValue::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(AppError::bad_request(format!("{} 必须是布尔值", key))),
    }
}

fn optional_i64_field(body: &JsonMap<String, JsonValue>, key: &str) -> AppResult<Option<i64>> {
    match body.get(key) {
        None => Ok(None),
        Some(JsonValue::Number(value)) => value
            .as_i64()
            .ok_or_else(|| AppError::bad_request(format!("{} 必须是整数", key)))
            .map(Some),
        Some(_) => Err(AppError::bad_request(format!("{} 必须是整数", key))),
    }
}

fn optional_json_field(
    body: &JsonMap<String, JsonValue>,
    key: &str,
) -> AppResult<Option<JsonValue>> {
    match body.get(key) {
        None => Ok(None),
        Some(JsonValue::Null) => Ok(None),
        Some(value) => Ok(Some(value.clone())),
    }
}

fn required_string_map_field(
    body: &JsonMap<String, JsonValue>,
    key: &str,
) -> AppResult<HashMap<String, String>> {
    let value = body
        .get(key)
        .ok_or_else(|| AppError::bad_request(format!("缺少必填字段: {}", key)))?;

    let object = value
        .as_object()
        .ok_or_else(|| AppError::bad_request(format!("{} 必须是对象", key)))?;

    let mut result = HashMap::new();
    for (child_key, child_value) in object {
        let string_value = child_value
            .as_str()
            .ok_or_else(|| AppError::bad_request(format!("{}.{child_key} 必须是字符串", key)))?;
        result.insert(child_key.clone(), string_value.to_string());
    }
    Ok(result)
}
