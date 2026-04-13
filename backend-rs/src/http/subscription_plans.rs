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
    sub2api::Sub2ApiGroup,
    subscription_plan::{SubscriptionPlanRecord, SubscriptionPlanRepository},
};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/subscription-plans", get(list_subscription_plans))
}

#[derive(Debug, Deserialize)]
struct SubscriptionPlansQuery {
    token: Option<String>,
    lang: Option<String>,
}

#[derive(Debug, Serialize)]
struct SubscriptionPlansResponse {
    plans: Vec<SubscriptionPlanView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubscriptionPlanView {
    id: String,
    group_id: i64,
    group_name: Option<String>,
    name: String,
    description: Option<String>,
    price: f64,
    original_price: Option<f64>,
    validity_days: i64,
    validity_unit: String,
    features: Vec<String>,
    product_name: Option<String>,
    platform: Option<String>,
    rate_multiplier: Option<f64>,
    limits: Option<PlanLimits>,
    allow_messages_dispatch: bool,
    default_mapped_model: Option<String>,
}

#[derive(Debug, Serialize)]
struct PlanLimits {
    daily_limit_usd: Option<f64>,
    weekly_limit_usd: Option<f64>,
    monthly_limit_usd: Option<f64>,
}

async fn list_subscription_plans(
    State(state): State<AppState>,
    Query(query): Query<SubscriptionPlansQuery>,
) -> AppResult<Json<SubscriptionPlansResponse>> {
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
            "获取订阅套餐失败",
            "Failed to list subscription plans",
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
                    "获取订阅套餐失败",
                    "Failed to list subscription plans",
                ))
            }
        })?;

    let admin_api_key = sub2api_admin_api_key(&state).await?;
    let repo = SubscriptionPlanRepository::new(state.db.clone());
    let plans = repo.list_for_sale().await.map_err(AppError::internal)?;

    let mut views = Vec::new();
    for plan in plans {
        let Some(group_id) = plan.group_id else {
            continue;
        };

        let group = match sub2api.get_group(group_id, &admin_api_key).await {
            Ok(Some(group)) if group.status == "active" => group,
            _ => continue,
        };

        views.push(to_subscription_plan_view(plan, group));
    }

    Ok(Json(SubscriptionPlansResponse { plans: views }))
}

fn to_subscription_plan_view(
    plan: SubscriptionPlanRecord,
    group: Sub2ApiGroup,
) -> SubscriptionPlanView {
    SubscriptionPlanView {
        id: plan.id,
        group_id: plan.group_id.unwrap_or_default(),
        group_name: if group.name.is_empty() {
            None
        } else {
            Some(group.name)
        },
        name: plan.name,
        description: plan.description,
        price: cents_to_amount(plan.price_cents),
        original_price: plan.original_price_cents.map(cents_to_amount),
        validity_days: plan.validity_days,
        validity_unit: plan.validity_unit,
        features: parse_features(plan.features.as_deref()),
        product_name: plan.product_name,
        platform: group.platform,
        rate_multiplier: group.rate_multiplier,
        limits: Some(PlanLimits {
            daily_limit_usd: group.daily_limit_usd,
            weekly_limit_usd: group.weekly_limit_usd,
            monthly_limit_usd: group.monthly_limit_usd,
        }),
        allow_messages_dispatch: group.allow_messages_dispatch,
        default_mapped_model: group.default_mapped_model,
    }
}

fn parse_features(value: Option<&str>) -> Vec<String> {
    value
        .and_then(|text| serde_json::from_str::<Vec<String>>(text).ok())
        .unwrap_or_default()
}

fn cents_to_amount(value: i64) -> f64 {
    (value as f64) / 100.0
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
        return Err(AppError::public_internal(
            "Failed to list subscription plans",
        ));
    }
    Ok(trimmed.to_string())
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
    struct MockAuthUser {
        id: i64,
    }

    async fn test_state(sub2api_base_url: Option<String>) -> AppState {
        let db_path = std::env::temp_dir().join(format!(
            "opay-subscription-plans-route-{}.db",
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
        async fn auth_me(State(user): State<MockAuthUser>) -> Json<serde_json::Value> {
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
            let body = match group_id {
                11 => json!({
                    "data": {
                        "id": 11,
                        "name": "OpenAI Pro",
                        "status": "active",
                        "subscription_type": "subscription",
                        "platform": "openai",
                        "rate_multiplier": 2.0,
                        "daily_limit_usd": 10.0,
                        "weekly_limit_usd": 30.0,
                        "monthly_limit_usd": 100.0,
                        "allow_messages_dispatch": true,
                        "default_mapped_model": "gpt-4.1"
                    }
                }),
                _ => json!({
                    "data": {
                        "id": group_id,
                        "name": "inactive",
                        "status": "disabled",
                        "subscription_type": "subscription"
                    }
                }),
            };
            Json(body)
        }

        let app = Router::new()
            .route("/api/v1/auth/me", get(auth_me))
            .route("/api/v1/admin/groups/{id}", get(group))
            .with_state(MockAuthUser { id: 1 });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn returns_only_active_plans_with_group_details() {
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
            "INSERT INTO subscription_plans (id, group_id, name, description, price_cents, original_price_cents, validity_days, validity_unit, features, product_name, for_sale, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            Params::Positional(vec![
                Value::Text("plan_ok".to_string()),
                Value::Integer(11),
                Value::Text("OpenAI Pro 月包".to_string()),
                Value::Text("desc".to_string()),
                Value::Integer(1999),
                Value::Integer(2999),
                Value::Integer(30),
                Value::Text("day".to_string()),
                Value::Text("[\"GPT\",\"Fast\"]".to_string()),
                Value::Text("OpenAI Pro".to_string()),
                Value::Integer(1),
                Value::Integer(1),
            ]),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO subscription_plans (id, group_id, name, price_cents, validity_days, validity_unit, for_sale, sort_order)
             VALUES ('plan_skip', 99, 'Skip', 999, 30, 'day', 1, 2)",
            (),
        )
        .await
        .unwrap();

        let response = list_subscription_plans(
            State(state),
            Query(SubscriptionPlansQuery {
                token: Some("user-token".to_string()),
                lang: None,
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(response.plans.len(), 1);
        assert_eq!(response.plans[0].id, "plan_ok");
        assert_eq!(response.plans[0].group_name.as_deref(), Some("OpenAI Pro"));
        assert_eq!(response.plans[0].features, vec!["GPT", "Fast"]);
        assert_eq!(response.plans[0].platform.as_deref(), Some("openai"));
        assert!(response.plans[0].allow_messages_dispatch);

        handle.abort();
    }
}
