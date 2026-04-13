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
    error::{AppError, AppResult},
    http::common::timestamp_to_rfc3339,
    platform::PlatformGroup,
    subscription_plan::{
        SubscriptionPlanRecord, SubscriptionPlanRepository, SubscriptionPlanWrite,
    },
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/admin/subscription-plans",
            get(list_plans).post(create_plan),
        )
        .route(
            "/api/admin/subscription-plans/{id}",
            get(get_plan).put(update_plan).delete(delete_plan),
        )
}

#[derive(Debug, Deserialize)]
struct AdminPlanQuery {
    token: Option<String>,
    lang: Option<String>,
}

#[derive(Debug, Serialize)]
struct AdminPlanListResponse {
    plans: Vec<AdminPlanView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminPlanView {
    id: String,
    group_id: Option<String>,
    group_name: Option<String>,
    name: String,
    description: Option<String>,
    price: f64,
    original_price: Option<f64>,
    valid_days: i64,
    validity_unit: String,
    features: Vec<String>,
    sort_order: i64,
    enabled: bool,
    group_exists: bool,
    group_platform: Option<String>,
    group_rate_multiplier: Option<f64>,
    group_daily_limit: Option<f64>,
    group_weekly_limit: Option<f64>,
    group_monthly_limit: Option<f64>,
    group_model_scopes: Option<Vec<String>>,
    group_allow_messages_dispatch: bool,
    group_default_mapped_model: Option<String>,
    product_name: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct DeleteSuccessResponse {
    success: bool,
}

async fn list_plans(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminPlanQuery>,
) -> AppResult<Json<AdminPlanListResponse>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let repo = SubscriptionPlanRepository::new(state.db.clone());
    let plans = repo.list_all().await.map_err(AppError::internal)?;
    let platform = state.platform.as_ref();
    let admin_api_key = platform_admin_api_key(&state).await.ok();

    let mut items = Vec::new();
    for plan in plans {
        let group = match (plan.group_id, platform, admin_api_key.as_deref()) {
            (Some(group_id), Some(client), Some(key)) => {
                client.get_group(group_id, key).await.ok().flatten()
            }
            _ => None,
        };
        items.push(to_admin_plan_view(plan, group));
    }

    Ok(Json(AdminPlanListResponse { plans: items }))
}

async fn get_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminPlanQuery>,
    Path(id): Path<String>,
) -> AppResult<Json<AdminPlanView>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let repo = SubscriptionPlanRepository::new(state.db.clone());
    let plan = repo
        .get_by_id(&id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("订阅套餐不存在"))?;
    let group = match (
        plan.group_id,
        state.platform.as_ref(),
        platform_admin_api_key(&state).await.ok(),
    ) {
        (Some(group_id), Some(client), Some(key)) => {
            client.get_group(group_id, &key).await.ok().flatten()
        }
        _ => None,
    };

    Ok(Json(to_admin_plan_view(plan, group)))
}

async fn create_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminPlanQuery>,
    Json(body): Json<JsonValue>,
) -> AppResult<(axum::http::StatusCode, Json<AdminPlanView>)> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let group_id = required_positive_i64(&body, "group_id", "缺少必填字段: group_id, price")?;
    let group = ensure_group_exists(&state, group_id).await?;
    let write = parse_create_body(&body, group_id)?;
    let created = SubscriptionPlanRepository::new(state.db.clone())
        .create(write)
        .await
        .map_err(AppError::internal)?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(to_admin_plan_view(created, Some(group))),
    ))
}

async fn update_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminPlanQuery>,
    Path(id): Path<String>,
    Json(body): Json<JsonValue>,
) -> AppResult<Json<AdminPlanView>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let repo = SubscriptionPlanRepository::new(state.db.clone());
    let existing = repo
        .get_by_id(&id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("订阅套餐不存在"))?;

    let final_group_id = if body.get("group_id").is_some() {
        optional_positive_i64(body.get("group_id"), "必须关联一个 Platform 分组")?
    } else {
        existing.group_id
    };
    let Some(group_id) = final_group_id else {
        return Err(AppError::bad_request("必须关联一个 Platform 分组"));
    };

    let group = match ensure_group_exists(&state, group_id).await {
        Ok(group) => group,
        Err(_) => {
            let rebound = SubscriptionPlanWrite {
                group_id: None,
                name: existing.name.clone(),
                description: existing.description.clone(),
                price_cents: existing.price_cents,
                original_price_cents: existing.original_price_cents,
                validity_days: existing.validity_days,
                validity_unit: existing.validity_unit.clone(),
                features: existing.features.clone(),
                product_name: existing.product_name.clone(),
                for_sale: false,
                sort_order: existing.sort_order,
            };
            let _ = repo.replace(&id, rebound).await;
            return Err(AppError::conflict(
                "该分组在 Platform 中已被删除，已自动解绑，请重新选择分组",
            ));
        }
    };

    let next = merge_update_body(existing, &body, group_id)?;
    let updated = repo
        .replace(&id, next)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("订阅套餐不存在"))?;

    Ok(Json(to_admin_plan_view(updated, Some(group))))
}

async fn delete_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminPlanQuery>,
    Path(id): Path<String>,
) -> AppResult<Json<DeleteSuccessResponse>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let repo = SubscriptionPlanRepository::new(state.db.clone());
    let existing = repo
        .get_by_id(&id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("订阅套餐不存在"))?;

    let active_count = repo
        .count_active_orders_by_plan(&id)
        .await
        .map_err(AppError::internal)?;
    if active_count > 0 {
        return Err(AppError::conflict(format!(
            "该套餐仍有 {} 个活跃订单，无法删除",
            active_count
        )));
    }

    let _ = repo
        .delete(&existing.id)
        .await
        .map_err(AppError::internal)?;
    Ok(Json(DeleteSuccessResponse { success: true }))
}

async fn ensure_group_exists(state: &AppState, group_id: i64) -> AppResult<PlatformGroup> {
    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| AppError::public_internal("获取分组信息失败"))?;
    let admin_api_key = platform_admin_api_key(state).await?;
    platform
        .get_group(group_id, &admin_api_key)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| {
            AppError::conflict("该分组在 Platform 中已被删除，已自动解绑，请重新选择分组")
        })
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

fn parse_create_body(body: &JsonValue, group_id: i64) -> AppResult<SubscriptionPlanWrite> {
    let name = required_non_empty_string(body.get("name"), "name 不能为空")?;
    if name.len() > 100 {
        return Err(AppError::bad_request("name 不能超过 100 个字符"));
    }

    Ok(SubscriptionPlanWrite {
        group_id: Some(group_id),
        name,
        description: optional_nullable_string(body.get("description"))?,
        price_cents: required_price_cents(
            body.get("price"),
            "price 必须是 0.01 ~ 99999999.99 之间的数值",
        )?,
        original_price_cents: optional_price_cents(
            body.get("original_price"),
            "original_price 必须是 0.01 ~ 99999999.99 之间的数值",
        )?,
        validity_days: optional_positive_i64(
            body.get("validity_days"),
            "validity_days 必须是正整数",
        )?
        .unwrap_or(30),
        validity_unit: normalize_validity_unit(body.get("validity_unit")),
        features: normalize_string_array_json(body.get("features"))?,
        product_name: optional_trimmed_string(body.get("product_name"))?,
        for_sale: optional_bool(body.get("for_sale"))?.unwrap_or(false),
        sort_order: optional_non_negative_i64(body.get("sort_order"), "sort_order 必须是非负整数")?
            .unwrap_or(0),
    })
}

fn merge_update_body(
    existing: SubscriptionPlanRecord,
    body: &JsonValue,
    group_id: i64,
) -> AppResult<SubscriptionPlanWrite> {
    let name = if body.get("name").is_some() {
        let name = required_non_empty_string(body.get("name"), "name 不能为空")?;
        if name.len() > 100 {
            return Err(AppError::bad_request("name 不能超过 100 个字符"));
        }
        name
    } else {
        existing.name
    };

    Ok(SubscriptionPlanWrite {
        group_id: Some(group_id),
        name,
        description: if body.get("description").is_some() {
            optional_nullable_string(body.get("description"))?
        } else {
            existing.description
        },
        price_cents: if body.get("price").is_some() {
            required_price_cents(
                body.get("price"),
                "price 必须是 0.01 ~ 99999999.99 之间的数值",
            )?
        } else {
            existing.price_cents
        },
        original_price_cents: if body.get("original_price").is_some() {
            optional_price_cents(
                body.get("original_price"),
                "original_price 必须是 0.01 ~ 99999999.99 之间的数值",
            )?
        } else {
            existing.original_price_cents
        },
        validity_days: if body.get("validity_days").is_some() {
            optional_positive_i64(body.get("validity_days"), "validity_days 必须是正整数")?
                .unwrap_or(existing.validity_days)
        } else {
            existing.validity_days
        },
        validity_unit: if body.get("validity_unit").is_some() {
            normalize_validity_unit(body.get("validity_unit"))
        } else {
            existing.validity_unit
        },
        features: if body.get("features").is_some() {
            normalize_string_array_json(body.get("features"))?
        } else {
            existing.features
        },
        product_name: if body.get("product_name").is_some() {
            optional_trimmed_string(body.get("product_name"))?
        } else {
            existing.product_name
        },
        for_sale: if body.get("for_sale").is_some() {
            optional_bool(body.get("for_sale"))?
                .ok_or_else(|| AppError::bad_request("for_sale 必须是布尔值"))?
        } else {
            existing.for_sale
        },
        sort_order: if body.get("sort_order").is_some() {
            optional_non_negative_i64(body.get("sort_order"), "sort_order 必须是非负整数")?
                .unwrap_or(existing.sort_order)
        } else {
            existing.sort_order
        },
    })
}

fn to_admin_plan_view(plan: SubscriptionPlanRecord, group: Option<PlatformGroup>) -> AdminPlanView {
    let (
        group_exists,
        group_name,
        group_platform,
        group_rate_multiplier,
        group_daily_limit,
        group_weekly_limit,
        group_monthly_limit,
        group_model_scopes,
        group_allow_messages_dispatch,
        group_default_mapped_model,
    ) = match group {
        Some(group) => (
            true,
            if group.name.is_empty() {
                None
            } else {
                Some(group.name)
            },
            group.platform,
            group.rate_multiplier,
            group.daily_limit_usd,
            group.weekly_limit_usd,
            group.monthly_limit_usd,
            group.supported_model_scopes,
            group.allow_messages_dispatch,
            group.default_mapped_model,
        ),
        None => (false, None, None, None, None, None, None, None, false, None),
    };

    AdminPlanView {
        id: plan.id,
        group_id: if group_exists {
            plan.group_id.map(|value| value.to_string())
        } else {
            None
        },
        group_name,
        name: plan.name,
        description: plan.description,
        price: cents_to_amount(plan.price_cents),
        original_price: plan.original_price_cents.map(cents_to_amount),
        valid_days: plan.validity_days,
        validity_unit: plan.validity_unit,
        features: parse_features(plan.features.as_deref()),
        sort_order: plan.sort_order,
        enabled: if group_exists { plan.for_sale } else { false },
        group_exists,
        group_platform,
        group_rate_multiplier,
        group_daily_limit,
        group_weekly_limit,
        group_monthly_limit,
        group_model_scopes,
        group_allow_messages_dispatch,
        group_default_mapped_model,
        product_name: plan.product_name,
        created_at: timestamp_to_rfc3339(plan.created_at),
        updated_at: timestamp_to_rfc3339(plan.updated_at),
    }
}

fn parse_features(value: Option<&str>) -> Vec<String> {
    value
        .and_then(|item| serde_json::from_str::<Vec<String>>(item).ok())
        .unwrap_or_default()
}

fn cents_to_amount(value: i64) -> f64 {
    (value as f64) / 100.0
}

fn amount_to_cents(value: f64) -> i64 {
    (value * 100.0).round() as i64
}

fn required_non_empty_string(value: Option<&JsonValue>, error: &str) -> AppResult<String> {
    value
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .ok_or_else(|| AppError::bad_request(error))
}

fn optional_trimmed_string(value: Option<&JsonValue>) -> AppResult<Option<String>> {
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

fn optional_nullable_string(value: Option<&JsonValue>) -> AppResult<Option<String>> {
    optional_trimmed_string(value)
}

fn required_price_cents(value: Option<&JsonValue>, error: &str) -> AppResult<i64> {
    let value = value
        .and_then(JsonValue::as_f64)
        .filter(|item| item.is_finite() && *item > 0.0 && *item <= 99_999_999.99)
        .ok_or_else(|| AppError::bad_request(error))?;
    Ok(amount_to_cents(value))
}

fn optional_price_cents(value: Option<&JsonValue>, error: &str) -> AppResult<Option<i64>> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(_) => required_price_cents(value, error).map(Some),
    }
}

fn required_positive_i64(value: &JsonValue, field: &str, missing_error: &str) -> AppResult<i64> {
    value
        .get(field)
        .and_then(JsonValue::as_i64)
        .filter(|item| *item > 0)
        .ok_or_else(|| AppError::bad_request(missing_error))
}

fn optional_positive_i64(value: Option<&JsonValue>, error: &str) -> AppResult<Option<i64>> {
    match value {
        None => Ok(None),
        Some(JsonValue::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .filter(|item| *item > 0)
            .map(Some)
            .ok_or_else(|| AppError::bad_request(error)),
    }
}

fn optional_non_negative_i64(value: Option<&JsonValue>, error: &str) -> AppResult<Option<i64>> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .filter(|item| *item >= 0)
            .map(Some)
            .ok_or_else(|| AppError::bad_request(error)),
    }
}

fn optional_bool(value: Option<&JsonValue>) -> AppResult<Option<bool>> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Bool(value)) => Ok(Some(*value)),
        _ => Err(AppError::bad_request("字段必须是布尔值")),
    }
}

fn normalize_validity_unit(value: Option<&JsonValue>) -> String {
    value
        .and_then(JsonValue::as_str)
        .filter(|item| matches!(*item, "day" | "week" | "month"))
        .unwrap_or("day")
        .to_string()
}

fn normalize_string_array_json(value: Option<&JsonValue>) -> AppResult<Option<String>> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Array(value)) => serde_json::to_string(value)
            .map(Some)
            .map_err(AppError::internal),
        Some(JsonValue::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        _ => Err(AppError::bad_request("features 字段格式不正确")),
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::{
        Json, Router,
        extract::{Path, State},
        response::IntoResponse,
        routing::get,
    };
    use serde_json::json;
    use tokio::{net::TcpListener, task::JoinHandle};
    use uuid::Uuid;

    use super::*;
    use crate::{
        config::AppConfig,
        db::DatabaseHandle,
        order::{audit::AuditLogRepository, repository::OrderRepository, service::OrderService},
        platform::PlatformClient,
        system_config::SystemConfigService,
        system_config::UpsertSystemConfig,
    };

    fn mock_group(id: i64, name: &str) -> PlatformGroup {
        PlatformGroup {
            id,
            name: name.to_string(),
            status: "active".to_string(),
            subscription_type: Some("subscription".to_string()),
            description: Some("desc".to_string()),
            platform: Some("openai".to_string()),
            rate_multiplier: Some(2.0),
            daily_limit_usd: Some(10.0),
            weekly_limit_usd: Some(30.0),
            monthly_limit_usd: Some(100.0),
            default_validity_days: Some(30),
            sort_order: Some(0),
            supported_model_scopes: Some(vec!["gpt-4.1".to_string()]),
            allow_messages_dispatch: true,
            default_mapped_model: Some("gpt-4.1".to_string()),
        }
    }

    #[derive(Clone)]
    struct MockGroupState;

    async fn test_state(platform_base_url: Option<String>) -> AppState {
        let db_path =
            std::env::temp_dir().join(format!("opay-admin-plan-route-{}.db", Uuid::new_v4()));
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
        async fn group(
            Path(group_id): Path<i64>,
            State(_): State<MockGroupState>,
        ) -> impl axum::response::IntoResponse {
            match group_id {
                11 | 12 => (
                    axum::http::StatusCode::OK,
                    Json(json!({ "data": mock_group(group_id, &format!("Group {group_id}")) })),
                )
                    .into_response(),
                _ => axum::http::StatusCode::NOT_FOUND.into_response(),
            }
        }

        let app = Router::new()
            .route("/api/v1/admin/groups/{id}", get(group))
            .with_state(MockGroupState);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn admin_subscription_plan_crud_roundtrip() {
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

        let created = create_plan(
            State(state.clone()),
            admin_headers(),
            Query(AdminPlanQuery {
                token: None,
                lang: None,
            }),
            Json(json!({
                "group_id": 11,
                "name": "Plan A",
                "description": "desc",
                "price": 19.99,
                "original_price": 29.99,
                "validity_days": 30,
                "validity_unit": "day",
                "features": ["GPT", "Fast"],
                "for_sale": true,
                "sort_order": 1,
                "product_name": "OpenAI Plan"
            })),
        )
        .await
        .unwrap()
        .1
        .0;

        assert_eq!(created.name, "Plan A");
        assert!(created.group_exists);
        assert_eq!(created.price, 19.99);

        let listed = list_plans(
            State(state.clone()),
            admin_headers(),
            Query(AdminPlanQuery {
                token: None,
                lang: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(listed.plans.len(), 1);

        let updated = update_plan(
            State(state.clone()),
            admin_headers(),
            Query(AdminPlanQuery {
                token: None,
                lang: None,
            }),
            Path(created.id.clone()),
            Json(json!({
                "group_id": 12,
                "price": 25.5,
                "for_sale": false
            })),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(updated.group_id.as_deref(), Some("12"));
        assert_eq!(updated.price, 25.5);
        assert!(!updated.enabled);

        let deleted = delete_plan(
            State(state),
            admin_headers(),
            Query(AdminPlanQuery {
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
