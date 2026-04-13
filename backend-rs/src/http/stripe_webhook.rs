use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::Serialize;

use crate::{AppState, payment_provider};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/stripe/webhook", post(handle_webhook))
}

#[derive(Debug, Serialize)]
struct WebhookResponse {
    received: bool,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

async fn handle_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<WebhookResponse>, (StatusCode, Json<ErrorResponse>)> {
    let signature = headers
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");

    let notification = payment_provider::verify_stripe_webhook(&state, &body, signature)
        .await
        .map_err(|error| {
            tracing::error!(error = ?error, "stripe webhook verification failed");
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Webhook processing failed".to_string(),
                }),
            )
        })?;

    let Some(notification) = notification else {
        return Ok(Json(WebhookResponse { received: true }));
    };

    let success = state
        .order_service
        .handle_verified_notification(&notification)
        .await
        .map_err(|error| {
            tracing::error!(error = ?error, "stripe webhook handling failed");
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Webhook processing failed".to_string(),
                }),
            )
        })?;

    if !success {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Processing failed, will retry".to_string(),
            }),
        ));
    }

    Ok(Json(WebhookResponse { received: true }))
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::extract::State;
    use hmac::digest::KeyInit;
    use hmac::{Mac, SimpleHmac};
    use serde_json::json;
    use sha2::Sha256;
    use uuid::Uuid;

    use super::*;
    use crate::{
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
    async fn webhook_returns_bad_request_when_signature_is_invalid() {
        let state = test_state().await;

        let error = handle_webhook(
            State(state),
            HeaderMap::new(),
            Bytes::from_static(br#"{"type":"payment_intent.succeeded","data":{"object":{"id":"pi_x","amount":100,"metadata":{"orderId":"order_x"}}}}"#),
        )
        .await
        .unwrap_err();

        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert_eq!(error.1.0.error, "Webhook processing failed");
    }

    #[tokio::test]
    async fn webhook_returns_received_for_unhandled_event() {
        let state = test_state().await;
        insert_provider_instance(
            &state,
            true,
            json!({
                "webhookSecret": "whsec_route_ok",
            }),
        )
        .await;

        let payload = r#"{
  "id": "evt_route_ignored",
  "type": "charge.refunded",
  "data": {
    "object": {
      "id": "ch_route_ignored",
      "amount": 300,
      "metadata": {
        "orderId": "ignored-order"
      }
    }
  }
}"#;

        let response = handle_webhook(
            State(state),
            stripe_headers("whsec_route_ok", payload),
            Bytes::copy_from_slice(payload.as_bytes()),
        )
        .await
        .unwrap();

        assert!(response.0.received);
    }

    #[tokio::test]
    async fn webhook_returns_retryable_500_when_fulfillment_fails() {
        let state = test_state().await;
        insert_provider_instance(
            &state,
            true,
            json!({
                "webhookSecret": "whsec_route_retry",
            }),
        )
        .await;
        let order = insert_order(&state, "PENDING", "stripe", 1099).await;

        let payload = include_str!("../../testdata/stripe_webhook_payment_intent_succeeded.json")
            .replace("\"order_test_succeeded\"", &format!("\"{}\"", order.id));

        let error = handle_webhook(
            State(state.clone()),
            stripe_headers("whsec_route_retry", &payload),
            Bytes::copy_from_slice(payload.as_bytes()),
        )
        .await
        .unwrap_err();

        assert_eq!(error.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.1.0.error, "Processing failed, will retry");

        let saved = OrderRepository::new(state.db.clone())
            .get_by_id(&order.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.status, "FAILED");
    }

    async fn test_state() -> AppState {
        let path = std::env::temp_dir().join(format!(
            "opay-stripe-webhook-route-{}.db",
            Uuid::new_v4()
        ));
        let db = DatabaseHandle::open_local(&path).await.unwrap();
        db.run_migrations().await.unwrap();

        let config = Arc::new(AppConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            db_path: path,
            payment_providers: vec!["stripe".to_string()],
            admin_token: Some("dev-admin-token".to_string()),
            system_config_cache_ttl_secs: 30,
            sub2api_base_url: None,
            sub2api_timeout_secs: 10,
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
            sub2api: None,
            order_service,
        }
    }

    async fn insert_provider_instance(
        state: &AppState,
        enabled: bool,
        config: serde_json::Value,
    ) -> String {
        let repo = ProviderInstanceRepository::new(state.db.clone());
        let encrypted =
            crypto::encrypt(state.config.admin_token.as_deref(), &config.to_string()).unwrap();

        repo.create(ProviderInstanceWrite {
            provider_key: "stripe".to_string(),
            name: "stripe-route-test".to_string(),
            config: encrypted,
            supported_types: r#"["stripe"]"#.to_string(),
            enabled,
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
                user_id: 7,
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

    fn stripe_headers(secret: &str, payload: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "stripe-signature",
            stripe_signature_header(secret, payload).parse().unwrap(),
        );
        headers
    }

    fn stripe_signature_header(secret: &str, payload: &str) -> String {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        type HmacSha256 = SimpleHmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(format!("{}.{}", timestamp, payload).as_bytes());
        let digest = hex_lower(mac.finalize().into_bytes().as_slice());
        format!("t={},v1={}", timestamp, digest)
    }

    fn hex_lower(bytes: &[u8]) -> String {
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write as _;
            let _ = write!(&mut output, "{:02x}", byte);
        }
        output
    }
}
