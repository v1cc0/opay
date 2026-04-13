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
    payment,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/user", get(get_user))
}

#[derive(Debug, Deserialize)]
struct UserQuery {
    user_id: i64,
    token: Option<String>,
    lang: Option<String>,
}

#[derive(Debug, Serialize)]
struct UserResponse {
    user: UserSummary,
    config: UserConfig,
}

#[derive(Debug, Serialize)]
struct UserSummary {
    id: i64,
    status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UserConfig {
    enabled_payment_types: Vec<String>,
    min_amount: f64,
    max_amount: f64,
    max_daily_amount: f64,
    method_limits: HashMap<String, payment::MethodLimitStatus>,
    help_image_url: Option<String>,
    help_text: Option<String>,
    stripe_publishable_key: Option<String>,
    balance_disabled: bool,
    max_pending_orders: i64,
    sublabel_overrides: Option<HashMap<String, String>>,
}

async fn get_user(
    State(state): State<AppState>,
    Query(query): Query<UserQuery>,
) -> AppResult<Json<UserResponse>> {
    let locale = resolve_locale(query.lang.as_deref());
    if query.user_id <= 0 {
        return Err(AppError::bad_request(message(
            locale,
            "无效的用户 ID",
            "Invalid user ID",
        )));
    }

    let token = query
        .token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::unauthorized(message(
                locale,
                "缺少 token 参数",
                "Missing token parameter",
            ))
        })?;

    let sub2api = state
        .sub2api
        .as_ref()
        .ok_or_else(|| AppError::internal(anyhow::anyhow!("SUB2API_BASE_URL is not configured")))?;

    let token_user = sub2api
        .get_current_user_by_token(token)
        .await
        .map_err(|error| {
            let error_message = error.to_string();
            if error_message.starts_with("Failed to get current user:") {
                AppError::unauthorized(message(locale, "无效的 token", "Invalid token"))
            } else {
                AppError::internal(error)
            }
        })?;

    if token_user.id != query.user_id {
        return Err(AppError::forbidden(message(
            locale,
            "无权访问该用户信息",
            "Forbidden to access this user",
        )));
    }

    let configs = state
        .system_config
        .get_many(&[
            "BALANCE_PAYMENT_DISABLED",
            "MAX_PENDING_ORDERS",
            "RECHARGE_MIN_AMOUNT",
            "RECHARGE_MAX_AMOUNT",
            "DAILY_RECHARGE_LIMIT",
        ])
        .await
        .map_err(AppError::internal)?;
    let payment_config = payment::resolve_user_payment_config(&state)
        .await
        .map_err(AppError::internal)?;

    let balance_disabled = configs
        .get("BALANCE_PAYMENT_DISABLED")
        .map(|value| value == "true")
        .unwrap_or(false);
    let max_pending_orders = configs
        .get("MAX_PENDING_ORDERS")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(3);
    let min_amount = configs
        .get("RECHARGE_MIN_AMOUNT")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(state.config.min_recharge_amount);
    let max_amount = configs
        .get("RECHARGE_MAX_AMOUNT")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(state.config.max_recharge_amount);
    let max_daily_amount = configs
        .get("DAILY_RECHARGE_LIMIT")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(state.config.max_daily_recharge_amount);

    Ok(Json(UserResponse {
        user: UserSummary {
            id: token_user.id,
            status: token_user.status,
        },
        config: UserConfig {
            enabled_payment_types: payment_config.enabled_payment_types,
            min_amount,
            max_amount,
            max_daily_amount,
            method_limits: payment_config.method_limits,
            help_image_url: state.config.pay_help_image_url.clone(),
            help_text: state.config.pay_help_text.clone(),
            stripe_publishable_key: payment_config.stripe_publishable_key,
            balance_disabled,
            max_pending_orders,
            sublabel_overrides: None,
        },
    }))
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::{Json, Router};
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
        system_config::SystemConfigService,
    };

    #[derive(Clone)]
    struct MockUser {
        id: i64,
        balance: f64,
    }

    #[tokio::test]
    async fn get_user_hides_direct_payment_types_in_rust_mvp() {
        let (base_url, handle) = start_mock_sub2api(MockUser {
            id: 42,
            balance: 88.0,
        })
        .await;
        let state = test_state(
            Some(base_url),
            vec![
                "easypay".to_string(),
                "alipay".to_string(),
                "wxpay".to_string(),
                "stripe".to_string(),
            ],
        )
        .await;

        let response = get_user(
            State(state),
            Query(UserQuery {
                user_id: 42,
                token: Some("user-token".to_string()),
                lang: None,
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(
            response.config.enabled_payment_types,
            vec![
                "alipay".to_string(),
                "wxpay".to_string(),
                "stripe".to_string()
            ]
        );
        assert!(
            !response
                .config
                .enabled_payment_types
                .iter()
                .any(|item| item == "alipay_direct" || item == "wxpay_direct")
        );
        assert!(response.config.method_limits.contains_key("alipay"));
        assert!(response.config.method_limits.contains_key("wxpay"));
        assert!(response.config.method_limits.contains_key("stripe"));

        handle.abort();
    }

    async fn test_state(
        sub2api_base_url: Option<String>,
        payment_providers: Vec<String>,
    ) -> AppState {
        let path = std::env::temp_dir().join(format!("opay-user-{}.db", Uuid::new_v4()));
        let db = DatabaseHandle::open_local(&path).await.unwrap();
        db.run_migrations().await.unwrap();

        let config = Arc::new(AppConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            db_path: path,
            payment_providers,
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

    async fn start_mock_sub2api(user: MockUser) -> (String, JoinHandle<()>) {
        async fn auth_me(State(user): State<MockUser>) -> Json<serde_json::Value> {
            Json(json!({
                "data": {
                    "id": user.id,
                    "status": "active",
                    "balance": user.balance,
                    "email": "user@example.com",
                    "username": "test-user"
                }
            }))
        }

        let app = Router::new()
            .route("/api/v1/auth/me", axum::routing::get(auth_me))
            .with_state(user);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{}", addr), handle)
    }
}
