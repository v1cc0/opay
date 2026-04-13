use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct Sub2ApiClient {
    http: Client,
    base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sub2ApiUser {
    pub id: i64,
    pub status: String,
    pub role: Option<String>,
    pub email: Option<String>,
    pub username: Option<String>,
    pub notes: Option<String>,
    pub balance: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Sub2ApiSearchUser {
    pub id: i64,
    pub email: Option<String>,
    pub username: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Sub2ApiGroup {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    pub subscription_type: Option<String>,
    pub description: Option<String>,
    pub platform: Option<String>,
    pub rate_multiplier: Option<f64>,
    pub daily_limit_usd: Option<f64>,
    pub weekly_limit_usd: Option<f64>,
    pub monthly_limit_usd: Option<f64>,
    pub default_validity_days: Option<i64>,
    pub sort_order: Option<i64>,
    pub supported_model_scopes: Option<Vec<String>>,
    #[serde(default)]
    pub allow_messages_dispatch: bool,
    pub default_mapped_model: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Sub2ApiSubscription {
    pub id: i64,
    pub user_id: i64,
    pub group_id: i64,
    #[serde(default)]
    pub starts_at: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub expires_at: String,
    #[serde(default)]
    pub daily_usage_usd: f64,
    #[serde(default)]
    pub weekly_usage_usd: f64,
    #[serde(default)]
    pub monthly_usage_usd: f64,
    pub daily_window_start: Option<String>,
    pub weekly_window_start: Option<String>,
    pub monthly_window_start: Option<String>,
    pub assigned_by: Option<i64>,
    pub assigned_at: Option<String>,
    pub notes: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sub2ApiRedeemCode {
    pub id: Option<i64>,
    pub code: String,
    #[serde(rename = "type")]
    pub redeem_type: Option<String>,
    pub value: Option<f64>,
    pub status: Option<String>,
    pub used_by: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DataEnvelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct PaginatedSubscriptionsEnvelope {
    data: PaginatedSubscriptionsData,
}

#[derive(Debug, Deserialize)]
struct PaginatedSubscriptionsData {
    #[serde(default)]
    items: Vec<Sub2ApiSubscription>,
    #[serde(default)]
    total: i64,
    #[serde(default = "default_page")]
    page: i64,
    #[serde(default = "default_page_size")]
    page_size: i64,
}

#[derive(Debug, Deserialize)]
struct PaginatedUsersEnvelope {
    data: PaginatedUsersData,
}

#[derive(Debug, Deserialize)]
struct PaginatedUsersData {
    #[serde(default)]
    items: Vec<Sub2ApiSearchUser>,
}

#[derive(Debug, Serialize)]
struct CreateAndRedeemRequest<'a> {
    code: &'a str,
    #[serde(rename = "type")]
    redeem_type: &'a str,
    value: f64,
    user_id: i64,
    notes: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    group_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    validity_days: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CreateAndRedeemResponse {
    redeem_code: Option<Sub2ApiRedeemCode>,
}

#[derive(Debug, Serialize)]
struct BalanceOperationRequest<'a> {
    operation: &'a str,
    balance: f64,
    notes: &'a str,
}

#[derive(Debug, Serialize)]
struct ExtendSubscriptionRequest {
    days: i64,
}

#[derive(Debug, Clone, Copy)]
enum Sub2ApiRedeemKind {
    Balance,
    Subscription { group_id: i64, validity_days: i64 },
}

#[derive(Debug, Clone, Default)]
pub struct ListSubscriptionsParams {
    pub user_id: Option<i64>,
    pub group_id: Option<i64>,
    pub status: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct PaginatedSubscriptions {
    pub subscriptions: Vec<Sub2ApiSubscription>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

impl Sub2ApiClient {
    pub fn new(base_url: String, timeout_secs: u64) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .connect_timeout(Duration::from_secs(timeout_secs.min(5)))
            .read_timeout(Duration::from_secs(timeout_secs))
            .build()
            .expect("failed to build reqwest client");

        Self { http, base_url }
    }

    pub async fn get_current_user_by_token(&self, token: &str) -> Result<Sub2ApiUser> {
        let response = self
            .http
            .get(format!("{}/api/v1/auth/me", self.base_url))
            .bearer_auth(token)
            .send()
            .await
            .context("failed to call Sub2API auth/me")?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to get current user: {}",
                response.status().as_u16()
            ));
        }

        let data = response
            .json::<DataEnvelope<Sub2ApiUser>>()
            .await
            .context("failed to decode Sub2API auth/me response")?;

        Ok(data.data)
    }

    pub async fn get_user(&self, user_id: i64, admin_api_key: &str) -> Result<Sub2ApiUser> {
        let response = self
            .http
            .get(format!("{}/api/v1/admin/users/{}", self.base_url, user_id))
            .header("x-api-key", admin_api_key)
            .send()
            .await
            .with_context(|| format!("failed to call Sub2API admin/users/{}", user_id))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(anyhow!("USER_NOT_FOUND"));
        }

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to get user: {}",
                response.status().as_u16()
            ));
        }

        let data = response
            .json::<DataEnvelope<Sub2ApiUser>>()
            .await
            .with_context(|| format!("failed to decode Sub2API admin user {}", user_id))?;

        Ok(data.data)
    }

    pub async fn get_group(
        &self,
        group_id: i64,
        admin_api_key: &str,
    ) -> Result<Option<Sub2ApiGroup>> {
        let response = self
            .http
            .get(format!(
                "{}/api/v1/admin/groups/{}",
                self.base_url, group_id
            ))
            .header("x-api-key", admin_api_key)
            .send()
            .await
            .with_context(|| format!("failed to call Sub2API admin/groups/{}", group_id))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to get group {}: {}",
                group_id,
                response.status().as_u16()
            ));
        }

        let data = response
            .json::<DataEnvelope<Sub2ApiGroup>>()
            .await
            .with_context(|| format!("failed to decode Sub2API group {}", group_id))?;

        Ok(Some(data.data))
    }

    pub async fn get_all_groups(&self, admin_api_key: &str) -> Result<Vec<Sub2ApiGroup>> {
        let response = self
            .http
            .get(format!("{}/api/v1/admin/groups/all", self.base_url))
            .header("x-api-key", admin_api_key)
            .send()
            .await
            .context("failed to call Sub2API admin/groups/all")?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to get groups: {}",
                response.status().as_u16()
            ));
        }

        let data = response
            .json::<DataEnvelope<Vec<Sub2ApiGroup>>>()
            .await
            .context("failed to decode Sub2API groups response")?;

        Ok(data.data)
    }

    pub async fn get_user_subscriptions(
        &self,
        user_id: i64,
        admin_api_key: &str,
    ) -> Result<Vec<Sub2ApiSubscription>> {
        let response = self
            .http
            .get(format!(
                "{}/api/v1/admin/users/{}/subscriptions",
                self.base_url, user_id
            ))
            .header("x-api-key", admin_api_key)
            .send()
            .await
            .with_context(|| {
                format!("failed to call Sub2API admin/users/{user_id}/subscriptions")
            })?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to get user subscriptions: {}",
                response.status().as_u16()
            ));
        }

        let data = response
            .json::<DataEnvelope<Vec<Sub2ApiSubscription>>>()
            .await
            .with_context(|| format!("failed to decode subscriptions for user {}", user_id))?;

        Ok(data.data)
    }

    pub async fn search_users(
        &self,
        keyword: &str,
        admin_api_key: &str,
    ) -> Result<Vec<Sub2ApiSearchUser>> {
        let trimmed = keyword.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let query =
            serde_urlencoded::to_string([("search", trimmed), ("page", "1"), ("page_size", "30")])
                .context("failed to encode user search query")?;
        let url = format!("{}/api/v1/admin/users?{query}", self.base_url);
        let response = self
            .http
            .get(url)
            .header("x-api-key", admin_api_key)
            .send()
            .await
            .context("failed to call Sub2API admin user search")?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to search users: {}",
                response.status().as_u16()
            ));
        }

        let data = response
            .json::<PaginatedUsersEnvelope>()
            .await
            .context("failed to decode Sub2API user search response")?;

        Ok(data.data.items)
    }

    pub async fn list_subscriptions(
        &self,
        params: &ListSubscriptionsParams,
        admin_api_key: &str,
    ) -> Result<PaginatedSubscriptions> {
        let query = build_list_subscriptions_query(params);
        let url = if query.is_empty() {
            format!("{}/api/v1/admin/subscriptions", self.base_url)
        } else {
            format!(
                "{}/api/v1/admin/subscriptions?{}",
                self.base_url,
                serde_urlencoded::to_string(&query)
                    .context("failed to encode subscription list query")?
            )
        };
        let response = self
            .http
            .get(url)
            .header("x-api-key", admin_api_key)
            .send()
            .await
            .context("failed to call Sub2API admin/subscriptions")?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to list subscriptions: {}",
                response.status().as_u16()
            ));
        }

        let data = response
            .json::<PaginatedSubscriptionsEnvelope>()
            .await
            .context("failed to decode Sub2API subscriptions list response")?;

        Ok(PaginatedSubscriptions {
            subscriptions: data.data.items,
            total: data.data.total,
            page: data.data.page,
            page_size: data.data.page_size,
        })
    }

    pub async fn subtract_balance(
        &self,
        user_id: i64,
        amount: f64,
        notes: &str,
        idempotency_key: &str,
        admin_api_key: &str,
    ) -> Result<()> {
        self.adjust_balance(
            user_id,
            amount,
            notes,
            idempotency_key,
            admin_api_key,
            "subtract",
        )
        .await
    }

    pub async fn add_balance(
        &self,
        user_id: i64,
        amount: f64,
        notes: &str,
        idempotency_key: &str,
        admin_api_key: &str,
    ) -> Result<()> {
        self.adjust_balance(
            user_id,
            amount,
            notes,
            idempotency_key,
            admin_api_key,
            "add",
        )
        .await
    }

    async fn adjust_balance(
        &self,
        user_id: i64,
        amount: f64,
        notes: &str,
        idempotency_key: &str,
        admin_api_key: &str,
        operation: &str,
    ) -> Result<()> {
        let response = self
            .http
            .post(format!(
                "{}/api/v1/admin/users/{}/balance",
                self.base_url, user_id
            ))
            .header("x-api-key", admin_api_key)
            .header("Idempotency-Key", idempotency_key)
            .json(&BalanceOperationRequest {
                operation,
                balance: amount,
                notes,
            })
            .send()
            .await
            .with_context(|| {
                format!("failed to call Sub2API balance operation {operation} for user {user_id}")
            })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "{} balance failed ({status}): {}",
                capitalize(operation),
                body.trim()
            ));
        }

        Ok(())
    }

    pub async fn extend_subscription(
        &self,
        subscription_id: i64,
        days: i64,
        idempotency_key: &str,
        admin_api_key: &str,
    ) -> Result<()> {
        let response = self
            .http
            .post(format!(
                "{}/api/v1/admin/subscriptions/{}/extend",
                self.base_url, subscription_id
            ))
            .header("x-api-key", admin_api_key)
            .header("Idempotency-Key", idempotency_key)
            .json(&ExtendSubscriptionRequest { days })
            .send()
            .await
            .with_context(|| {
                format!(
                    "failed to call Sub2API extend subscription {}",
                    subscription_id
                )
            })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Extend subscription failed ({status}): {}",
                body.trim()
            ));
        }

        Ok(())
    }

    pub async fn create_and_redeem_balance(
        &self,
        code: &str,
        value: f64,
        user_id: i64,
        notes: &str,
        admin_api_key: &str,
    ) -> Result<Sub2ApiRedeemCode> {
        self.create_and_redeem(
            code,
            value,
            user_id,
            notes,
            admin_api_key,
            Sub2ApiRedeemKind::Balance,
        )
        .await
    }

    pub async fn create_and_redeem_subscription(
        &self,
        code: &str,
        value: f64,
        user_id: i64,
        notes: &str,
        group_id: i64,
        validity_days: i64,
        admin_api_key: &str,
    ) -> Result<Sub2ApiRedeemCode> {
        self.create_and_redeem(
            code,
            value,
            user_id,
            notes,
            admin_api_key,
            Sub2ApiRedeemKind::Subscription {
                group_id,
                validity_days,
            },
        )
        .await
    }

    async fn create_and_redeem(
        &self,
        code: &str,
        value: f64,
        user_id: i64,
        notes: &str,
        admin_api_key: &str,
        redeem_kind: Sub2ApiRedeemKind,
    ) -> Result<Sub2ApiRedeemCode> {
        let (redeem_type, group_id, validity_days) = match redeem_kind {
            Sub2ApiRedeemKind::Balance => ("balance", None, None),
            Sub2ApiRedeemKind::Subscription {
                group_id,
                validity_days,
            } => ("subscription", Some(group_id), Some(validity_days)),
        };

        let response = self
            .http
            .post(format!(
                "{}/api/v1/admin/redeem-codes/create-and-redeem",
                self.base_url
            ))
            .header("x-api-key", admin_api_key)
            .header("Idempotency-Key", format!("sub2apipay:recharge:{code}"))
            .json(&CreateAndRedeemRequest {
                code,
                redeem_type,
                value,
                user_id,
                notes,
                group_id,
                validity_days,
            })
            .send()
            .await
            .context("failed to call Sub2API create-and-redeem")?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Recharge failed ({status}): {}", body.trim()));
        }

        let data = response
            .json::<CreateAndRedeemResponse>()
            .await
            .context("failed to decode Sub2API create-and-redeem response")?;

        data.redeem_code
            .ok_or_else(|| anyhow!("Sub2API create-and-redeem response missing redeem_code"))
    }
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn default_page() -> i64 {
    1
}

fn default_page_size() -> i64 {
    50
}

fn build_list_subscriptions_query(params: &ListSubscriptionsParams) -> Vec<(&str, String)> {
    let mut query = Vec::new();
    if let Some(user_id) = params.user_id {
        query.push(("user_id", user_id.to_string()));
    }
    if let Some(group_id) = params.group_id {
        query.push(("group_id", group_id.to_string()));
    }
    if let Some(status) = params
        .status
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        query.push(("status", status.to_string()));
    }
    if let Some(page) = params.page {
        query.push(("page", page.to_string()));
    }
    if let Some(page_size) = params.page_size {
        query.push(("page_size", page_size.to_string()));
    }
    query
}
