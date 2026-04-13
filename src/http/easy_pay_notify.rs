use axum::{
    Router,
    extract::{RawQuery, State},
    http::HeaderMap,
    routing::get,
};

use crate::{AppState, payment_provider};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/easy-pay/notify", get(handle_notify))
}

async fn handle_notify(
    State(state): State<AppState>,
    headers: HeaderMap,
    raw_query: RawQuery,
) -> (axum::http::StatusCode, &'static str) {
    let raw_query = raw_query.0.unwrap_or_default();
    let inst = parse_inst(&raw_query);

    let result = async {
        let notification =
            payment_provider::verify_easypay_notification(&state, &raw_query, inst.as_deref())
                .await?;
        let Some(notification) = notification else {
            return Ok::<bool, anyhow::Error>(true);
        };
        state
            .order_service
            .handle_verified_notification(&notification)
            .await
    }
    .await;

    match result {
        Ok(true) => (axum::http::StatusCode::OK, "success"),
        Ok(false) => (axum::http::StatusCode::OK, "fail"),
        Err(error) => {
            tracing::error!(error = ?error, "easy-pay notify failed");
            let _ = headers;
            (axum::http::StatusCode::OK, "fail")
        }
    }
}

fn parse_inst(raw_query: &str) -> Option<String> {
    serde_urlencoded::from_str::<std::collections::HashMap<String, String>>(raw_query)
        .ok()
        .and_then(|params| params.get("inst").cloned())
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use md5 as md5sum;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::{
        AppState,
        config::AppConfig,
        crypto,
        db::DatabaseHandle,
        order::{
            audit::AuditLogRepository,
            repository::{NewPendingOrder, OrderRepository},
            service::OrderService,
        },
        provider_instances::{ProviderInstanceRepository, ProviderInstanceWrite},
        subscription_plan::SubscriptionPlanRepository,
        system_config::SystemConfigService,
    };

    #[tokio::test]
    async fn notify_returns_success_for_already_completed_order() {
        let state = test_state().await;
        let instance_id = insert_provider_instance(
            &state,
            json!({
                "pid": "easy_route_pid",
                "pkey": "easy_route_key",
            }),
        )
        .await;
        let order = insert_order(&state, "COMPLETED", "alipay", 1234).await;
        let query = signed_query(
            &[
                ("pid", "easy_route_pid"),
                ("trade_no", "easy_route_trade_ok"),
                ("out_trade_no", order.id.as_str()),
                ("money", "12.34"),
                ("trade_status", "TRADE_SUCCESS"),
            ],
            &[("inst", instance_id.as_str())],
            "easy_route_key",
        );

        let response = handle_notify(
            State(state),
            HeaderMap::new(),
            axum::extract::RawQuery(Some(query)),
        )
        .await;

        assert_eq!(response.0, axum::http::StatusCode::OK);
        assert_eq!(response.1, "success");
    }

    #[tokio::test]
    async fn notify_returns_fail_when_fulfillment_needs_retry() {
        let state = test_state().await;
        let instance_id = insert_provider_instance(
            &state,
            json!({
                "pid": "easy_route_pid",
                "pkey": "easy_route_key",
            }),
        )
        .await;
        let order = insert_order(&state, "PENDING", "alipay", 1234).await;
        let query = signed_query(
            &[
                ("pid", "easy_route_pid"),
                ("trade_no", "easy_route_trade_retry"),
                ("out_trade_no", order.id.as_str()),
                ("money", "12.34"),
                ("trade_status", "TRADE_SUCCESS"),
            ],
            &[("inst", instance_id.as_str())],
            "easy_route_key",
        );

        let response = handle_notify(
            State(state.clone()),
            HeaderMap::new(),
            axum::extract::RawQuery(Some(query)),
        )
        .await;

        assert_eq!(response.0, axum::http::StatusCode::OK);
        assert_eq!(response.1, "fail");

        let saved = OrderRepository::new(state.db.clone())
            .get_by_id(&order.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.status, "FAILED");
    }

    #[tokio::test]
    async fn notify_returns_fail_for_invalid_signature() {
        let state = test_state().await;
        let instance_id = insert_provider_instance(
            &state,
            json!({
                "pid": "easy_route_pid",
                "pkey": "easy_route_key",
            }),
        )
        .await;
        let query = serde_urlencoded::to_string([
            ("pid", "easy_route_pid"),
            ("trade_no", "easy_route_trade_bad"),
            ("out_trade_no", "order_bad"),
            ("money", "12.34"),
            ("trade_status", "TRADE_SUCCESS"),
            ("inst", instance_id.as_str()),
            ("sign", "bad-signature"),
            ("sign_type", "MD5"),
        ])
        .unwrap();

        let response = handle_notify(
            State(state),
            HeaderMap::new(),
            axum::extract::RawQuery(Some(query)),
        )
        .await;

        assert_eq!(response.0, axum::http::StatusCode::OK);
        assert_eq!(response.1, "fail");
    }

    async fn test_state() -> AppState {
        let path = std::env::temp_dir().join(format!(
            "opay-easy-pay-notify-route-{}.db",
            Uuid::new_v4()
        ));
        let db = DatabaseHandle::open_local(&path).await.unwrap();
        db.run_migrations().await.unwrap();

        let config = Arc::new(AppConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            db_path: path,
            payment_providers: vec!["easypay".to_string()],
            admin_token: Some("dev-admin-token".to_string()),
            system_config_cache_ttl_secs: 30,
            platform_base_url: None,
            platform_timeout_secs: 10,
            min_recharge_amount: 1.0,
            max_recharge_amount: 1000.0,
            max_daily_recharge_amount: 10000.0,
            pay_help_image_url: None,
            pay_help_text: None,
            stripe_publishable_key: None,
        });

        let system_config = SystemConfigService::new(db.clone(), Duration::from_secs(30));
        let order_service = OrderService::new(
            Arc::clone(&config),
            OrderRepository::new(db.clone()),
            AuditLogRepository::new(db.clone()),
            SubscriptionPlanRepository::new(db.clone()),
            system_config.clone(),
            None,
        );

        AppState {
            config,
            db,
            system_config,
            platform: None,
            order_service,
        }
    }

    async fn insert_provider_instance(state: &AppState, config: serde_json::Value) -> String {
        let repo = ProviderInstanceRepository::new(state.db.clone());
        let encrypted =
            crypto::encrypt(state.config.admin_token.as_deref(), &config.to_string()).unwrap();

        repo.create(ProviderInstanceWrite {
            provider_key: "easypay".to_string(),
            name: "easy-pay-route-test".to_string(),
            config: encrypted,
            supported_types: r#"["alipay","wxpay"]"#.to_string(),
            enabled: true,
            sort_order: 0,
            limits: None,
            refund_enabled: true,
        })
        .await
        .unwrap()
        .id
    }

    async fn insert_order(
        state: &AppState,
        status: &str,
        payment_type: &str,
        pay_amount_cents: i64,
    ) -> crate::order::repository::OrderRecord {
        OrderRepository::new(state.db.clone())
            .insert_pending(NewPendingOrder {
                user_id: 9,
                amount_cents: pay_amount_cents,
                pay_amount_cents: Some(pay_amount_cents),
                fee_rate_bps: Some(0),
                status: status.to_string(),
                payment_type: payment_type.to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 2000,
            })
            .await
            .unwrap()
    }

    fn signed_query(
        signed_params: &[(&str, &str)],
        extra_params: &[(&str, &str)],
        pkey: &str,
    ) -> String {
        let mut sign_params = signed_params
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<Vec<_>>();
        let sign = generate_sign(&sign_params, pkey);

        sign_params.extend(
            extra_params
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string())),
        );
        sign_params.push(("sign".to_string(), sign));
        sign_params.push(("sign_type".to_string(), "MD5".to_string()));
        serde_urlencoded::to_string(sign_params).unwrap()
    }

    fn generate_sign(params: &[(String, String)], pkey: &str) -> String {
        let mut filtered = params
            .iter()
            .filter(|(key, value)| key != "sign" && key != "sign_type" && !value.is_empty())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        filtered.sort_by(|a, b| a.0.cmp(&b.0));
        let query_string = filtered
            .iter()
            .map(|(key, value)| format!("{}={}", key, value))
            .collect::<Vec<_>>()
            .join("&");
        format!("{:x}", md5sum::compute(format!("{}{}", query_string, pkey)))
    }
}
