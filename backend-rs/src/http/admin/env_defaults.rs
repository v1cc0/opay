use std::{collections::HashMap, env, fs, path::Path};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::HeaderMap,
    routing::get,
};
use serde::Serialize;

use crate::{AppState, admin_auth::verify_admin_values, error::AppResult, payment};

const RUST_MVP_PROVIDERS: &[(&str, &[&str])] =
    &[("easypay", &["alipay", "wxpay"]), ("stripe", &["stripe"])];

fn provider_name(key: &str) -> &'static str {
    match key {
        "easypay" => "EasyPay",
        "alipay" => "Alipay Official",
        "wxpay" => "WxPay Official",
        "stripe" => "Stripe",
        _ => "Unknown",
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/admin/config/env-defaults", get(get_env_defaults))
}

#[derive(Debug, serde::Deserialize)]
struct EnvDefaultsQuery {
    token: Option<String>,
    lang: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvDefaultsResponse {
    available_payment_types: Vec<String>,
    providers: Vec<ProviderAvailability>,
    instance_defaults: HashMap<String, InstanceDefault>,
    defaults: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
struct ProviderAvailability {
    key: String,
    configured: bool,
    types: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstanceDefault {
    name: String,
    config: HashMap<String, String>,
    supported_types: String,
}

async fn get_env_defaults(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Query<EnvDefaultsQuery>,
) -> AppResult<Json<EnvDefaultsResponse>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let configured_providers =
        payment::filter_provider_keys_for_rust_mvp(&state.config.payment_providers);
    let available_payment_types = payment::supported_types_for_provider_keys(&configured_providers);

    let providers = RUST_MVP_PROVIDERS
        .iter()
        .map(|(key, types)| ProviderAvailability {
            key: (*key).to_string(),
            configured: configured_providers.iter().any(|provider| provider == key),
            types: types.iter().map(|item| (*item).to_string()).collect(),
        })
        .collect::<Vec<_>>();

    let mut instance_defaults = HashMap::new();
    for provider_key in &configured_providers {
        if let Some(config) = build_instance_config(provider_key) {
            let supported_types = payment::provider_supported_types(provider_key)
                .iter()
                .map(|item| (*item).to_string())
                .collect::<Vec<_>>()
                .join(",");
            instance_defaults.insert(
                provider_key.clone(),
                InstanceDefault {
                    name: provider_name(provider_key).to_string(),
                    config,
                    supported_types,
                },
            );
        }
    }

    let mut defaults = HashMap::new();
    defaults.insert(
        "ENABLED_PAYMENT_TYPES".to_string(),
        available_payment_types.join(","),
    );
    defaults.insert(
        "RECHARGE_MIN_AMOUNT".to_string(),
        state.config.min_recharge_amount.to_string(),
    );
    defaults.insert(
        "RECHARGE_MAX_AMOUNT".to_string(),
        state.config.max_recharge_amount.to_string(),
    );
    defaults.insert(
        "DAILY_RECHARGE_LIMIT".to_string(),
        state.config.max_daily_recharge_amount.to_string(),
    );
    defaults.insert(
        "ORDER_TIMEOUT_MINUTES".to_string(),
        env::var("ORDER_TIMEOUT_MINUTES").unwrap_or_else(|_| "5".to_string()),
    );
    defaults.insert(
        "IFRAME_ALLOW_ORIGINS".to_string(),
        env::var("IFRAME_ALLOW_ORIGINS").unwrap_or_default(),
    );
    defaults.insert("MAX_PENDING_ORDERS".to_string(), "3".to_string());

    Ok(Json(EnvDefaultsResponse {
        available_payment_types,
        providers,
        instance_defaults,
        defaults,
    }))
}

fn build_instance_config(provider_key: &str) -> Option<HashMap<String, String>> {
    match provider_key {
        "easypay" => {
            let pid = read_env_value("EASY_PAY_PID")?;
            let pkey = read_env_value("EASY_PAY_PKEY")?;
            let mut config = HashMap::from([("pid".to_string(), pid), ("pkey".to_string(), pkey)]);
            insert_optional(&mut config, "apiBase", "EASY_PAY_API_BASE");
            insert_optional(&mut config, "notifyUrl", "EASY_PAY_NOTIFY_URL");
            insert_optional(&mut config, "returnUrl", "EASY_PAY_RETURN_URL");
            insert_optional(&mut config, "cidAlipay", "EASY_PAY_CID_ALIPAY");
            insert_optional(&mut config, "cidWxpay", "EASY_PAY_CID_WXPAY");
            Some(config)
        }
        "alipay" => {
            let app_id = read_env_value("ALIPAY_APP_ID")?;
            let private_key = read_env_value("ALIPAY_PRIVATE_KEY")?;
            let mut config = HashMap::from([
                ("appId".to_string(), app_id),
                ("privateKey".to_string(), private_key),
            ]);
            insert_optional(&mut config, "publicKey", "ALIPAY_PUBLIC_KEY");
            insert_optional(&mut config, "notifyUrl", "ALIPAY_NOTIFY_URL");
            insert_optional(&mut config, "returnUrl", "ALIPAY_RETURN_URL");
            Some(config)
        }
        "wxpay" => {
            let app_id = read_env_value("WXPAY_APP_ID")?;
            let mch_id = read_env_value("WXPAY_MCH_ID")?;
            let private_key = read_env_value("WXPAY_PRIVATE_KEY")?;
            let mut config = HashMap::from([
                ("appId".to_string(), app_id),
                ("mchId".to_string(), mch_id),
                ("privateKey".to_string(), private_key),
            ]);
            insert_optional(&mut config, "apiV3Key", "WXPAY_API_V3_KEY");
            insert_optional(&mut config, "publicKey", "WXPAY_PUBLIC_KEY");
            insert_optional(&mut config, "publicKeyId", "WXPAY_PUBLIC_KEY_ID");
            insert_optional(&mut config, "certSerial", "WXPAY_CERT_SERIAL");
            insert_optional(&mut config, "notifyUrl", "WXPAY_NOTIFY_URL");
            Some(config)
        }
        "stripe" => {
            let secret_key = read_env_value("STRIPE_SECRET_KEY")?;
            let mut config = HashMap::from([("secretKey".to_string(), secret_key)]);
            insert_optional(&mut config, "publishableKey", "STRIPE_PUBLISHABLE_KEY");
            insert_optional(&mut config, "webhookSecret", "STRIPE_WEBHOOK_SECRET");
            Some(config)
        }
        _ => None,
    }
}

fn insert_optional(target: &mut HashMap<String, String>, target_key: &str, env_key: &str) {
    if let Some(value) = read_env_value(env_key) {
        target.insert(target_key.to_string(), value);
    }
}

fn read_env_value(key: &str) -> Option<String> {
    let value = env::var(key).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    if looks_like_file_path(trimmed) && Path::new(trimmed).exists() {
        return fs::read_to_string(trimmed)
            .ok()
            .map(|contents| contents.trim().to_string())
            .filter(|contents| !contents.is_empty());
    }

    Some(trimmed.to_string())
}

fn looks_like_file_path(value: &str) -> bool {
    value.starts_with('/') || value.chars().nth(1) == Some(':')
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::extract::{Query, State};
    use uuid::Uuid;

    use super::*;
    use crate::{
        config::AppConfig,
        db::DatabaseHandle,
        order::{audit::AuditLogRepository, repository::OrderRepository, service::OrderService},
        subscription_plan::SubscriptionPlanRepository,
        system_config::SystemConfigService,
    };

    #[tokio::test]
    async fn env_defaults_only_exposes_rust_mvp_providers() {
        let state = test_state(vec![
            "easypay".to_string(),
            "alipay".to_string(),
            "wxpay".to_string(),
            "stripe".to_string(),
        ])
        .await;

        let response = get_env_defaults(
            State(state),
            HeaderMap::new(),
            Query(EnvDefaultsQuery {
                token: Some("test-admin-token".to_string()),
                lang: None,
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(
            response.available_payment_types,
            vec![
                "alipay".to_string(),
                "wxpay".to_string(),
                "stripe".to_string()
            ]
        );
        assert_eq!(response.providers.len(), 2);
        assert_eq!(response.providers[0].key, "easypay");
        assert_eq!(response.providers[1].key, "stripe");
    }

    async fn test_state(payment_providers: Vec<String>) -> AppState {
        let path =
            std::env::temp_dir().join(format!("sub2apipay-env-defaults-{}.db", Uuid::new_v4()));
        let db = DatabaseHandle::open_local(&path).await.unwrap();
        db.run_migrations().await.unwrap();

        let config = Arc::new(AppConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            db_path: path,
            payment_providers,
            admin_token: Some("test-admin-token".to_string()),
            system_config_cache_ttl_secs: 1,
            sub2api_base_url: None,
            sub2api_timeout_secs: 2,
            min_recharge_amount: 1.0,
            max_recharge_amount: 1000.0,
            max_daily_recharge_amount: 10000.0,
            pay_help_image_url: None,
            pay_help_text: None,
            stripe_publishable_key: None,
        });

        let system_config = SystemConfigService::new(db.clone(), Duration::from_secs(1));

        AppState {
            config: Arc::clone(&config),
            db: db.clone(),
            system_config: system_config.clone(),
            sub2api: None,
            order_service: OrderService::new(
                Arc::clone(&config),
                OrderRepository::new(db.clone()),
                AuditLogRepository::new(db.clone()),
                SubscriptionPlanRepository::new(db.clone()),
                system_config,
                None,
            ),
        }
    }
}
