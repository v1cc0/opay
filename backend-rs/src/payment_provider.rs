use std::{collections::HashMap, env, fs, path::Path, time::Duration};

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use hmac::digest::KeyInit;
use hmac::{Mac, SimpleHmac};
use md5 as md5sum;
use reqwest::Client;
use serde::Deserialize;
use serde_json::from_str;
use sha2::Sha256;

use crate::{AppState, crypto, provider_instances::ProviderInstanceRepository};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CreatePaymentRequest {
    pub order_id: String,
    pub amount_cents: i64,
    pub payment_type: String,
    pub subject: String,
    pub client_ip: Option<String>,
    pub is_mobile: bool,
    pub provider_instance_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreatePaymentResponse {
    pub provider_name: String,
    pub trade_no: String,
    pub pay_url: Option<String>,
    pub qr_code: Option<String>,
    pub client_secret: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RefundPaymentRequest {
    pub trade_no: String,
    pub order_id: String,
    pub amount_cents: i64,
    pub payment_type: String,
    #[allow(dead_code)]
    pub reason: String,
    pub provider_instance_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RefundPaymentResponse {
    pub provider_name: String,
    pub refund_id: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct VerifiedPaymentNotification {
    pub provider_name: String,
    pub trade_no: String,
    pub order_id: String,
    pub amount_cents: i64,
    pub success: bool,
}

#[async_trait]
pub trait PaymentProvider: Send + Sync {
    fn name(&self) -> &str;
    fn supported_types(&self) -> &'static [&'static str];
    async fn create_payment(&self, request: &CreatePaymentRequest)
    -> Result<CreatePaymentResponse>;
    async fn refund(&self, request: &RefundPaymentRequest) -> Result<RefundPaymentResponse>;
}

pub async fn create_payment_for_order(
    state: &AppState,
    request: &CreatePaymentRequest,
) -> Result<CreatePaymentResponse> {
    let provider = build_provider(
        state,
        &request.payment_type,
        request.provider_instance_id.as_deref(),
    )
    .await?;
    provider.create_payment(request).await
}

pub async fn refund_payment(
    state: &AppState,
    request: &RefundPaymentRequest,
) -> Result<RefundPaymentResponse> {
    let provider = build_provider(
        state,
        &request.payment_type,
        request.provider_instance_id.as_deref(),
    )
    .await?;
    provider.refund(request).await
}

pub async fn verify_easypay_notification(
    state: &AppState,
    raw_query: &str,
    instance_id: Option<&str>,
) -> Result<Option<VerifiedPaymentNotification>> {
    let mut params = serde_urlencoded::from_str::<HashMap<String, String>>(raw_query)
        .map_err(|error| anyhow!("failed to decode EasyPay notification query: {}", error))?;
    let inst = params.remove("inst");

    let config =
        load_provider_instance_config(state, "easypay", instance_id.or(inst.as_deref())).await?;
    let pid = config
        .get("pid")
        .cloned()
        .or_else(|| read_env_value("EASY_PAY_PID"))
        .ok_or_else(|| anyhow!("EasyPay pid not configured"))?;
    let pkey = config
        .get("pkey")
        .cloned()
        .or_else(|| read_env_value("EASY_PAY_PKEY"))
        .ok_or_else(|| anyhow!("EasyPay pkey not configured"))?;

    let sign = params
        .get("sign")
        .cloned()
        .ok_or_else(|| anyhow!("EasyPay notification missing sign"))?;

    let params_for_sign = params
        .iter()
        .filter(|(key, value)| *key != "sign" && *key != "sign_type" && !value.is_empty())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();

    let expected = generate_easy_pay_sign(&params_for_sign, &pkey);
    if !secure_equals(&expected, &sign) {
        return Err(anyhow!(
            "EasyPay notification signature verification failed"
        ));
    }

    if params.get("pid").is_some_and(|value| value != &pid) {
        return Err(anyhow!("EasyPay notification pid mismatch"));
    }

    let amount_cents = params
        .get("money")
        .and_then(|value| value.parse::<f64>().ok())
        .map(amount_to_cents)
        .filter(|value| *value > 0)
        .ok_or_else(|| anyhow!("EasyPay notification invalid amount"))?;

    Ok(Some(VerifiedPaymentNotification {
        provider_name: "easy-pay".to_string(),
        trade_no: params.get("trade_no").cloned().unwrap_or_default(),
        order_id: params.get("out_trade_no").cloned().unwrap_or_default(),
        amount_cents,
        success: params
            .get("trade_status")
            .map(|value| value == "TRADE_SUCCESS")
            .unwrap_or(false),
    }))
}

pub async fn verify_stripe_webhook(
    state: &AppState,
    raw_body: &[u8],
    signature_header: &str,
) -> Result<Option<VerifiedPaymentNotification>> {
    let payload = std::str::from_utf8(raw_body)
        .map_err(|error| anyhow!("invalid Stripe webhook body: {}", error))?;

    let candidates = stripe_webhook_secrets(state).await?;
    let mut verified = false;
    for secret in candidates {
        if verify_stripe_signature(&secret, payload, signature_header)? {
            verified = true;
            break;
        }
    }
    if !verified {
        return Err(anyhow!("Stripe webhook signature verification failed"));
    }

    let event: StripeEvent = serde_json::from_slice(raw_body)
        .map_err(|error| anyhow!("invalid Stripe event JSON: {}", error))?;

    match event.event_type.as_str() {
        "payment_intent.succeeded" => Ok(Some(VerifiedPaymentNotification {
            provider_name: "stripe".to_string(),
            trade_no: event.data.object.id,
            order_id: event
                .data
                .object
                .metadata
                .get("orderId")
                .cloned()
                .unwrap_or_default(),
            amount_cents: event.data.object.amount,
            success: true,
        })),
        "payment_intent.payment_failed" => Ok(Some(VerifiedPaymentNotification {
            provider_name: "stripe".to_string(),
            trade_no: event.data.object.id,
            order_id: event
                .data
                .object
                .metadata
                .get("orderId")
                .cloned()
                .unwrap_or_default(),
            amount_cents: event.data.object.amount,
            success: false,
        })),
        _ => Ok(None),
    }
}

async fn build_provider(
    state: &AppState,
    payment_type: &str,
    provider_instance_id: Option<&str>,
) -> Result<Box<dyn PaymentProvider>> {
    let provider_key = provider_key_for_type(payment_type)
        .ok_or_else(|| anyhow!("unsupported payment type {}", payment_type))?;

    let instance_config = if let Some(provider_instance_id) = provider_instance_id {
        let repo = ProviderInstanceRepository::new(state.db.clone());
        let record = repo
            .get(provider_instance_id)
            .await?
            .ok_or_else(|| anyhow!("payment provider instance not found"))?;
        let plaintext = crypto::decrypt(state.config.admin_token.as_deref(), &record.config)?;
        Some(
            from_str::<HashMap<String, String>>(&plaintext)
                .map_err(|error| anyhow!("invalid provider instance config JSON: {}", error))?,
        )
    } else {
        None
    };

    let provider: Box<dyn PaymentProvider> = match provider_key {
        "easypay" => Box::new(EasyPayProvider::new(instance_config)?),
        "alipay" => Box::new(AlipayProvider::new(instance_config)),
        "wxpay" => Box::new(WxpayProvider::new(instance_config)),
        "stripe" => Box::new(StripeProvider::new(instance_config)),
        _ => bail!("unsupported provider key {}", provider_key),
    };

    if !provider
        .supported_types()
        .iter()
        .any(|supported| *supported == request_base_payment_type(payment_type))
    {
        bail!(
            "provider {} does not support payment type {}",
            provider.name(),
            payment_type
        );
    }

    Ok(provider)
}

fn provider_key_for_type(payment_type: &str) -> Option<&'static str> {
    if payment_type == "alipay" {
        Some("easypay")
    } else if payment_type == "wxpay" {
        Some("easypay")
    } else if payment_type == "alipay_direct" {
        Some("alipay")
    } else if payment_type == "wxpay_direct" {
        Some("wxpay")
    } else if payment_type == "stripe" {
        Some("stripe")
    } else {
        None
    }
}

fn request_base_payment_type(payment_type: &str) -> &'static str {
    if payment_type.starts_with("alipay") {
        "alipay"
    } else if payment_type.starts_with("wxpay") {
        "wxpay"
    } else if payment_type.starts_with("stripe") {
        "stripe"
    } else {
        ""
    }
}

#[derive(Debug, Deserialize)]
struct EasyPayCreateResponse {
    code: i64,
    msg: Option<String>,
    trade_no: String,
    payurl: Option<String>,
    payurl2: Option<String>,
    qrcode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StripeCreatePaymentIntentResponse {
    id: String,
    client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EasyPayRefundResponse {
    code: i64,
    msg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StripeRefundResponse {
    id: String,
    status: String,
}

struct EasyPayProvider {
    config: Option<HashMap<String, String>>,
    http: Client,
}

#[derive(Debug, Deserialize)]
struct StripeEvent {
    #[serde(rename = "type")]
    event_type: String,
    data: StripeEventData,
}

#[derive(Debug, Deserialize)]
struct StripeEventData {
    object: StripePaymentIntent,
}

#[derive(Debug, Deserialize)]
struct StripePaymentIntent {
    id: String,
    amount: i64,
    #[serde(default)]
    metadata: HashMap<String, String>,
}

impl EasyPayProvider {
    fn new(config: Option<HashMap<String, String>>) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|error| anyhow!("failed to build reqwest client for EasyPay: {}", error))?;
        Ok(Self { config, http })
    }

    fn pid(&self) -> Option<String> {
        self.config
            .as_ref()
            .and_then(|config| config.get("pid").cloned())
            .or_else(|| read_env_value("EASY_PAY_PID"))
    }

    fn pkey(&self) -> Option<String> {
        self.config
            .as_ref()
            .and_then(|config| config.get("pkey").cloned())
            .or_else(|| read_env_value("EASY_PAY_PKEY"))
    }

    fn api_base(&self) -> Option<String> {
        self.config
            .as_ref()
            .and_then(|config| config.get("apiBase").cloned())
            .or_else(|| read_env_value("EASY_PAY_API_BASE"))
    }

    fn notify_url(&self) -> Option<String> {
        self.config
            .as_ref()
            .and_then(|config| config.get("notifyUrl").cloned())
            .or_else(|| read_env_value("EASY_PAY_NOTIFY_URL"))
    }

    fn return_url(&self) -> Option<String> {
        self.config
            .as_ref()
            .and_then(|config| config.get("returnUrl").cloned())
            .or_else(|| read_env_value("EASY_PAY_RETURN_URL"))
    }

    fn resolve_cid(&self, payment_type: &str) -> Option<String> {
        let config = self.config.as_ref();
        let raw = match payment_type {
            "alipay" => config
                .and_then(|config| config.get("cidAlipay").cloned())
                .or_else(|| config.and_then(|config| config.get("cid").cloned()))
                .or_else(|| read_env_value("EASY_PAY_CID_ALIPAY"))
                .or_else(|| read_env_value("EASY_PAY_CID")),
            _ => config
                .and_then(|config| config.get("cidWxpay").cloned())
                .or_else(|| config.and_then(|config| config.get("cid").cloned()))
                .or_else(|| read_env_value("EASY_PAY_CID_WXPAY"))
                .or_else(|| read_env_value("EASY_PAY_CID")),
        }?;

        let normalized = raw
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
            .join(",");
        if normalized.is_empty() {
            None
        } else {
            Some(normalized)
        }
    }
}

#[async_trait]
impl PaymentProvider for EasyPayProvider {
    fn name(&self) -> &str {
        "easy-pay"
    }

    fn supported_types(&self) -> &'static [&'static str] {
        &["alipay", "wxpay"]
    }

    async fn create_payment(
        &self,
        request: &CreatePaymentRequest,
    ) -> Result<CreatePaymentResponse> {
        let pid = self
            .pid()
            .ok_or_else(|| anyhow!("EASY_PAY_PID is required for EasyPay"))?;
        let pkey = self
            .pkey()
            .ok_or_else(|| anyhow!("EASY_PAY_PKEY is required for EasyPay"))?;
        let api_base = self
            .api_base()
            .ok_or_else(|| anyhow!("EASY_PAY_API_BASE is required for EasyPay"))?;
        let notify_url = self
            .notify_url()
            .ok_or_else(|| anyhow!("EASY_PAY_NOTIFY_URL is required for EasyPay"))?;
        let return_url = self
            .return_url()
            .ok_or_else(|| anyhow!("EASY_PAY_RETURN_URL is required for EasyPay"))?;

        let amount = cents_to_amount(request.amount_cents);
        let client_ip = request
            .client_ip
            .clone()
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let payment_type = request_base_payment_type(&request.payment_type).to_string();

        let mut params = vec![
            ("pid".to_string(), pid.clone()),
            ("type".to_string(), payment_type.clone()),
            ("out_trade_no".to_string(), request.order_id.clone()),
            ("notify_url".to_string(), notify_url),
            ("return_url".to_string(), return_url),
            ("name".to_string(), request.subject.clone()),
            ("money".to_string(), format!("{:.2}", amount)),
            ("clientip".to_string(), client_ip),
        ];

        if let Some(cid) = self.resolve_cid(&payment_type) {
            params.push(("cid".to_string(), cid));
        }
        if request.is_mobile {
            params.push(("device".to_string(), "mobile".to_string()));
        }

        let sign = generate_easy_pay_sign(&params, &pkey);
        params.push(("sign".to_string(), sign));
        params.push(("sign_type".to_string(), "MD5".to_string()));

        let response = self
            .http
            .post(format!("{}/mapi.php", api_base.trim_end_matches('/')))
            .form(&params)
            .send()
            .await
            .map_err(|error| anyhow!("EasyPay create payment request failed: {}", error))?;

        let data = response
            .json::<EasyPayCreateResponse>()
            .await
            .map_err(|error| anyhow!("EasyPay create payment response decode failed: {}", error))?;

        if data.code != 1 {
            return Err(anyhow!(
                "EasyPay create payment failed: {}",
                data.msg.unwrap_or_else(|| "unknown error".to_string())
            ));
        }

        let pay_url = if request.is_mobile {
            data.payurl2.clone().or(data.payurl.clone())
        } else {
            data.payurl.clone()
        };

        Ok(CreatePaymentResponse {
            provider_name: self.name().to_string(),
            trade_no: data.trade_no,
            pay_url,
            qr_code: data.qrcode,
            client_secret: None,
        })
    }

    async fn refund(&self, request: &RefundPaymentRequest) -> Result<RefundPaymentResponse> {
        let pid = self
            .pid()
            .ok_or_else(|| anyhow!("EASY_PAY_PID is required for EasyPay"))?;
        let pkey = self
            .pkey()
            .ok_or_else(|| anyhow!("EASY_PAY_PKEY is required for EasyPay"))?;
        let api_base = self
            .api_base()
            .ok_or_else(|| anyhow!("EASY_PAY_API_BASE is required for EasyPay"))?;

        let params = vec![
            ("pid".to_string(), pid),
            ("key".to_string(), pkey),
            ("trade_no".to_string(), request.trade_no.clone()),
            ("out_trade_no".to_string(), request.order_id.clone()),
            (
                "money".to_string(),
                format!("{:.2}", cents_to_amount(request.amount_cents)),
            ),
        ];

        let response = self
            .http
            .post(format!(
                "{}/api.php?act=refund",
                api_base.trim_end_matches('/')
            ))
            .form(&params)
            .send()
            .await
            .map_err(|error| anyhow!("EasyPay refund request failed: {}", error))?;

        let data = response
            .json::<EasyPayRefundResponse>()
            .await
            .map_err(|error| anyhow!("EasyPay refund response decode failed: {}", error))?;

        if data.code != 1 {
            return Err(anyhow!(
                "EasyPay refund failed: {}",
                data.msg.unwrap_or_else(|| "unknown error".to_string())
            ));
        }

        Ok(RefundPaymentResponse {
            provider_name: self.name().to_string(),
            refund_id: format!("{}-refund", request.trade_no),
            status: "success".to_string(),
        })
    }
}

struct AlipayProvider {
    config: Option<HashMap<String, String>>,
}

impl AlipayProvider {
    fn new(config: Option<HashMap<String, String>>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PaymentProvider for AlipayProvider {
    fn name(&self) -> &str {
        "alipay-direct"
    }

    fn supported_types(&self) -> &'static [&'static str] {
        &["alipay"]
    }

    async fn create_payment(
        &self,
        request: &CreatePaymentRequest,
    ) -> Result<CreatePaymentResponse> {
        let app_id = self
            .config
            .as_ref()
            .and_then(|config| config.get("appId").cloned())
            .or_else(|| read_env_value("ALIPAY_APP_ID"))
            .unwrap_or_else(|| "mock-app-id".to_string());
        let trade_no = request.order_id.clone();
        let amount = cents_to_amount(request.amount_cents);
        let deep_link = format!(
            "alipay://pay?app_id={}&out_trade_no={}&amount={:.2}",
            app_id, request.order_id, amount
        );

        if request.is_mobile {
            Ok(CreatePaymentResponse {
                provider_name: self.name().to_string(),
                trade_no,
                pay_url: Some(deep_link),
                qr_code: None,
                client_secret: None,
            })
        } else {
            Ok(CreatePaymentResponse {
                provider_name: self.name().to_string(),
                trade_no,
                pay_url: Some(deep_link.clone()),
                qr_code: Some(deep_link),
                client_secret: None,
            })
        }
    }

    async fn refund(&self, _request: &RefundPaymentRequest) -> Result<RefundPaymentResponse> {
        bail!("refund is not implemented for alipay-direct in Rust MVP")
    }
}

struct WxpayProvider {
    config: Option<HashMap<String, String>>,
}

impl WxpayProvider {
    fn new(config: Option<HashMap<String, String>>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl PaymentProvider for WxpayProvider {
    fn name(&self) -> &str {
        "wxpay-direct"
    }

    fn supported_types(&self) -> &'static [&'static str] {
        &["wxpay"]
    }

    async fn create_payment(
        &self,
        request: &CreatePaymentRequest,
    ) -> Result<CreatePaymentResponse> {
        let app_id = self
            .config
            .as_ref()
            .and_then(|config| config.get("appId").cloned())
            .or_else(|| read_env_value("WXPAY_APP_ID"))
            .unwrap_or_else(|| "mock-wx-app-id".to_string());
        let trade_no = request.order_id.clone();
        let amount = cents_to_amount(request.amount_cents);
        let link = format!(
            "weixin://wxpay/bizpayurl?appid={}&out_trade_no={}&amount={:.2}",
            app_id, request.order_id, amount
        );

        if request.is_mobile {
            Ok(CreatePaymentResponse {
                provider_name: self.name().to_string(),
                trade_no,
                pay_url: Some(link),
                qr_code: None,
                client_secret: None,
            })
        } else {
            Ok(CreatePaymentResponse {
                provider_name: self.name().to_string(),
                trade_no,
                pay_url: None,
                qr_code: Some(link),
                client_secret: None,
            })
        }
    }

    async fn refund(&self, _request: &RefundPaymentRequest) -> Result<RefundPaymentResponse> {
        bail!("refund is not implemented for wxpay-direct in Rust MVP")
    }
}

struct StripeProvider {
    config: Option<HashMap<String, String>>,
    http: Client,
}

impl StripeProvider {
    fn new(config: Option<HashMap<String, String>>) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("failed to build reqwest client for Stripe");
        Self { config, http }
    }

    fn api_base(&self) -> String {
        self.config
            .as_ref()
            .and_then(|config| config.get("apiBase").cloned())
            .or_else(|| read_env_value("STRIPE_API_BASE"))
            .unwrap_or_else(|| "https://api.stripe.com".to_string())
    }
}

#[async_trait]
impl PaymentProvider for StripeProvider {
    fn name(&self) -> &str {
        "stripe"
    }

    fn supported_types(&self) -> &'static [&'static str] {
        &["stripe"]
    }

    async fn create_payment(
        &self,
        request: &CreatePaymentRequest,
    ) -> Result<CreatePaymentResponse> {
        let secret_key = self
            .config
            .as_ref()
            .and_then(|config| config.get("secretKey").cloned())
            .or_else(|| read_env_value("STRIPE_SECRET_KEY"))
            .ok_or_else(|| anyhow!("STRIPE_SECRET_KEY is required for Stripe"))?;

        let params = vec![
            ("amount".to_string(), request.amount_cents.to_string()),
            ("currency".to_string(), "cny".to_string()),
            (
                "automatic_payment_methods[enabled]".to_string(),
                "true".to_string(),
            ),
            ("description".to_string(), request.subject.clone()),
            ("metadata[orderId]".to_string(), request.order_id.clone()),
        ];

        let response = self
            .http
            .post(format!(
                "{}/v1/payment_intents",
                self.api_base().trim_end_matches('/')
            ))
            .bearer_auth(secret_key)
            .form(&params)
            .send()
            .await
            .map_err(|error| anyhow!("Stripe create payment request failed: {}", error))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Stripe create payment failed ({}): {}",
                status,
                body
            ));
        }

        let data = response
            .json::<StripeCreatePaymentIntentResponse>()
            .await
            .map_err(|error| anyhow!("Stripe create payment response decode failed: {}", error))?;

        Ok(CreatePaymentResponse {
            provider_name: self.name().to_string(),
            trade_no: data.id,
            pay_url: None,
            qr_code: None,
            client_secret: data.client_secret,
        })
    }

    async fn refund(&self, request: &RefundPaymentRequest) -> Result<RefundPaymentResponse> {
        let secret_key = self
            .config
            .as_ref()
            .and_then(|config| config.get("secretKey").cloned())
            .or_else(|| read_env_value("STRIPE_SECRET_KEY"))
            .ok_or_else(|| anyhow!("STRIPE_SECRET_KEY is required for Stripe"))?;

        let params = vec![
            ("payment_intent".to_string(), request.trade_no.clone()),
            ("amount".to_string(), request.amount_cents.to_string()),
            ("reason".to_string(), "requested_by_customer".to_string()),
        ];

        let response = self
            .http
            .post(format!(
                "{}/v1/refunds",
                self.api_base().trim_end_matches('/')
            ))
            .bearer_auth(secret_key)
            .form(&params)
            .send()
            .await
            .map_err(|error| anyhow!("Stripe refund request failed: {}", error))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Stripe refund failed ({}): {}", status, body));
        }

        let data = response
            .json::<StripeRefundResponse>()
            .await
            .map_err(|error| anyhow!("Stripe refund response decode failed: {}", error))?;

        Ok(RefundPaymentResponse {
            provider_name: self.name().to_string(),
            refund_id: data.id,
            status: data.status,
        })
    }
}

fn generate_easy_pay_sign(params: &[(String, String)], pkey: &str) -> String {
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
    let sign_input = format!("{}{}", query_string, pkey);
    format!("{:x}", md5sum::compute(sign_input.as_bytes()))
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

async fn load_provider_instance_config(
    state: &AppState,
    expected_provider_key: &str,
    instance_id: Option<&str>,
) -> Result<HashMap<String, String>> {
    if let Some(instance_id) = instance_id {
        let repo = ProviderInstanceRepository::new(state.db.clone());
        let record = repo
            .get(instance_id)
            .await?
            .ok_or_else(|| anyhow!("payment provider instance not found"))?;
        if record.provider_key != expected_provider_key {
            return Err(anyhow!("payment provider instance type mismatch"));
        }
        let plaintext = crypto::decrypt(state.config.admin_token.as_deref(), &record.config)?;
        return from_str::<HashMap<String, String>>(&plaintext)
            .map_err(|error| anyhow!("invalid provider instance config JSON: {}", error));
    }

    Ok(HashMap::new())
}

async fn stripe_webhook_secrets(state: &AppState) -> Result<Vec<String>> {
    let mut secrets = Vec::new();
    if let Some(secret) = read_env_value("STRIPE_WEBHOOK_SECRET") {
        secrets.push(secret);
    }

    let repo = ProviderInstanceRepository::new(state.db.clone());
    let instances = repo.list(Some("stripe")).await?;
    for instance in instances.into_iter().filter(|instance| instance.enabled) {
        let plaintext = crypto::decrypt(state.config.admin_token.as_deref(), &instance.config)?;
        let config = from_str::<HashMap<String, String>>(&plaintext)
            .map_err(|error| anyhow!("invalid provider instance config JSON: {}", error))?;
        if let Some(secret) = config
            .get("webhookSecret")
            .cloned()
            .filter(|value| !value.is_empty())
        {
            secrets.push(secret);
        }
    }

    Ok(secrets)
}

fn verify_stripe_signature(secret: &str, payload: &str, signature_header: &str) -> Result<bool> {
    let mut timestamp = None::<i64>;
    let mut v1_signatures = Vec::<String>::new();
    for part in signature_header.split(',') {
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or("").trim();
        let value = kv.next().unwrap_or("").trim();
        match key {
            "t" => {
                timestamp = value.parse::<i64>().ok();
            }
            "v1" => {
                if !value.is_empty() {
                    v1_signatures.push(value.to_string());
                }
            }
            _ => {}
        }
    }

    let timestamp =
        timestamp.ok_or_else(|| anyhow!("Stripe signature header missing timestamp"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| anyhow!("system time error: {}", error))?
        .as_secs() as i64;
    if (now - timestamp).abs() > 300 {
        return Ok(false);
    }

    let signed_payload = format!("{}.{}", timestamp, payload);
    type HmacSha256 = SimpleHmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| anyhow!("invalid Stripe secret"))?;
    mac.update(signed_payload.as_bytes());
    let expected = hex_lower(mac.finalize().into_bytes().as_slice());

    Ok(v1_signatures
        .iter()
        .any(|candidate| secure_equals(candidate, &expected)))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}

fn secure_equals(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let max_len = left.len().max(right.len());
    let mut diff = left.len() ^ right.len();
    for index in 0..max_len {
        let a = *left.get(index).unwrap_or(&0);
        let b = *right.get(index).unwrap_or(&0);
        diff |= (a ^ b) as usize;
    }
    diff == 0
}

fn looks_like_file_path(value: &str) -> bool {
    value.starts_with('/') || value.chars().nth(1) == Some(':')
}

fn amount_to_cents(value: f64) -> i64 {
    (value * 100.0).round() as i64
}

fn cents_to_amount(value: i64) -> f64 {
    (value as f64) / 100.0
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use super::*;
    use crate::{
        AppState,
        config::AppConfig,
        db::DatabaseHandle,
        order::{audit::AuditLogRepository, repository::OrderRepository, service::OrderService},
        provider_instances::{ProviderInstanceRepository, ProviderInstanceWrite},
        subscription_plan::SubscriptionPlanRepository,
        system_config::SystemConfigService,
    };
    use serde_json::json;
    use uuid::Uuid;

    #[tokio::test]
    async fn verify_easypay_notification_accepts_instance_from_query() {
        let state = test_state().await;
        let instance_id = insert_provider_instance(
            &state,
            "easypay",
            true,
            json!({
                "pid": "ep_test_pid",
                "pkey": "ep_test_key",
            }),
        )
        .await;

        let query = signed_easypay_query(
            &[
                ("pid", "ep_test_pid"),
                ("trade_no", "easy_trade_123"),
                ("out_trade_no", "order_easy_123"),
                ("money", "12.34"),
                ("trade_status", "TRADE_SUCCESS"),
            ],
            &[("inst", instance_id.as_str())],
            "ep_test_key",
        );

        let notification = verify_easypay_notification(&state, &query, None)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(notification.provider_name, "easy-pay");
        assert_eq!(notification.trade_no, "easy_trade_123");
        assert_eq!(notification.order_id, "order_easy_123");
        assert_eq!(notification.amount_cents, 1234);
        assert!(notification.success);
    }

    #[tokio::test]
    async fn verify_easypay_notification_rejects_pid_mismatch() {
        let state = test_state().await;
        let instance_id = insert_provider_instance(
            &state,
            "easypay",
            true,
            json!({
                "pid": "ep_expected_pid",
                "pkey": "ep_test_key",
            }),
        )
        .await;

        let query = signed_easypay_query(
            &[
                ("pid", "ep_other_pid"),
                ("trade_no", "easy_trade_bad_pid"),
                ("out_trade_no", "order_easy_bad_pid"),
                ("money", "8.00"),
                ("trade_status", "TRADE_SUCCESS"),
            ],
            &[("inst", instance_id.as_str())],
            "ep_test_key",
        );

        let error = verify_easypay_notification(&state, &query, None)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "EasyPay notification pid mismatch");
    }

    #[tokio::test]
    async fn verify_stripe_webhook_accepts_enabled_instance_secret() {
        let state = test_state().await;
        insert_provider_instance(
            &state,
            "stripe",
            true,
            json!({
                "webhookSecret": "whsec_enabled_test",
            }),
        )
        .await;

        let payload = include_str!("../testdata/stripe_webhook_payment_intent_failed.json");
        let signature = stripe_signature_header("whsec_enabled_test", payload);

        let notification = verify_stripe_webhook(&state, payload.as_bytes(), &signature)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(notification.provider_name, "stripe");
        assert_eq!(notification.trade_no, "pi_test_failed");
        assert_eq!(notification.order_id, "order_test_failed");
        assert_eq!(notification.amount_cents, 2099);
        assert!(!notification.success);
    }

    #[tokio::test]
    async fn verify_stripe_webhook_ignores_disabled_instance_secret() {
        let state = test_state().await;
        insert_provider_instance(
            &state,
            "stripe",
            false,
            json!({
                "webhookSecret": "whsec_disabled_test",
            }),
        )
        .await;
        insert_provider_instance(
            &state,
            "stripe",
            true,
            json!({
                "webhookSecret": "whsec_enabled_other",
            }),
        )
        .await;

        let payload = include_str!("../testdata/stripe_webhook_payment_intent_succeeded.json");
        let signature = stripe_signature_header("whsec_disabled_test", payload);

        let error = verify_stripe_webhook(&state, payload.as_bytes(), &signature)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Stripe webhook signature verification failed"
        );
    }

    #[tokio::test]
    async fn verify_stripe_webhook_returns_none_for_unhandled_event() {
        let state = test_state().await;
        insert_provider_instance(
            &state,
            "stripe",
            true,
            json!({
                "webhookSecret": "whsec_enabled_test",
            }),
        )
        .await;

        let payload = r#"{
  "id": "evt_test_ignored",
  "type": "charge.refunded",
  "data": {
    "object": {
      "id": "ch_test_ignored",
      "amount": 300,
      "metadata": {
        "orderId": "order_ignored"
      }
    }
  }
}"#;
        let signature = stripe_signature_header("whsec_enabled_test", payload);

        let notification = verify_stripe_webhook(&state, payload.as_bytes(), &signature)
            .await
            .unwrap();
        assert!(notification.is_none());
    }

    async fn test_state() -> AppState {
        let path =
            std::env::temp_dir().join(format!("opay-payment-provider-{}.db", Uuid::new_v4()));
        let db = DatabaseHandle::open_local(&path).await.unwrap();
        db.run_migrations().await.unwrap();

        let config = Arc::new(AppConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            db_path: path,
            payment_providers: vec!["easypay".to_string(), "stripe".to_string()],
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

    async fn insert_provider_instance(
        state: &AppState,
        provider_key: &str,
        enabled: bool,
        config: serde_json::Value,
    ) -> String {
        let repo = ProviderInstanceRepository::new(state.db.clone());
        let supported_types = match provider_key {
            "easypay" => r#"["alipay","wxpay"]"#,
            "stripe" => r#"["stripe"]"#,
            "alipay" => r#"["alipay"]"#,
            "wxpay" => r#"["wxpay"]"#,
            _ => "[]",
        };
        let encrypted =
            crate::crypto::encrypt(state.config.admin_token.as_deref(), &config.to_string())
                .unwrap();

        repo.create(ProviderInstanceWrite {
            provider_key: provider_key.to_string(),
            name: format!("{provider_key}-test"),
            config: encrypted,
            supported_types: supported_types.to_string(),
            enabled,
            sort_order: 0,
            limits: None,
            refund_enabled: true,
        })
        .await
        .unwrap()
        .id
    }

    fn signed_easypay_query(
        signed_params: &[(&str, &str)],
        extra_params: &[(&str, &str)],
        pkey: &str,
    ) -> String {
        let mut sign_params = signed_params
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<Vec<_>>();
        let sign = generate_easy_pay_sign(&sign_params, pkey);

        sign_params.extend(
            extra_params
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string())),
        );
        sign_params.push(("sign".to_string(), sign));
        sign_params.push(("sign_type".to_string(), "MD5".to_string()));
        serde_urlencoded::to_string(sign_params).unwrap()
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
}
