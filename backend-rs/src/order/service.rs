use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, anyhow, bail};
use serde_json::json;

use crate::{
    AppState,
    config::AppConfig,
    order::{
        audit::{AuditLogRepository, NewAuditLog},
        repository::{
            MarkPaidInput, MarkRefundRequestedInput, MarkRefundedInput, NewPendingOrder,
            OrderRecord, OrderRepository,
        },
        status::is_refund_status,
    },
    payment_provider::{CreatePaymentResponse, RefundPaymentRequest, VerifiedPaymentNotification},
    sub2api::Sub2ApiClient,
    subscription_plan::{SubscriptionPlanRepository, compute_validity_days},
    system_config::SystemConfigService,
};

const DEFAULT_MAX_PENDING_ORDERS: i64 = 3;
const DEFAULT_ORDER_TIMEOUT_MINUTES: i64 = 5;
const PAYMENT_CONFIRM_GRACE_SECONDS: i64 = 5 * 60;

#[derive(Clone)]
pub struct OrderService {
    config: Arc<AppConfig>,
    orders: OrderRepository,
    audit_logs: AuditLogRepository,
    subscription_plans: SubscriptionPlanRepository,
    system_config: SystemConfigService,
    sub2api: Option<Sub2ApiClient>,
}

#[derive(Debug, Clone)]
pub struct CreatePendingOrderInput {
    pub user_id: i64,
    pub amount_cents: i64,
    pub pay_amount_cents: Option<i64>,
    pub fee_rate_bps: Option<i64>,
    pub payment_type: String,
    pub order_type: String,
    pub plan_id: Option<String>,
    pub subscription_group_id: Option<i64>,
    pub subscription_days: Option<i64>,
    pub provider_instance_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmPaymentOutcome {
    Confirmed,
    RetryLater,
    StopRetry,
    AmountMismatch,
    OrderNotFound,
}

#[derive(Debug, Clone)]
pub struct RequestRefundInput {
    pub order_id: String,
    pub user_id: i64,
    pub amount_cents: i64,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProcessRefundInput {
    pub order_id: String,
    pub amount_cents: Option<i64>,
    pub reason: Option<String>,
    pub force: bool,
    pub deduct_balance: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRefundResult {
    pub success: bool,
    pub warning: Option<String>,
    pub require_force: bool,
    pub balance_deducted_cents: i64,
    pub subscription_days_deducted: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOrderOutcome {
    Cancelled,
    AlreadyPaid,
}

#[derive(Debug, Clone)]
struct DeductionPlan {
    balance_amount_cents: i64,
    subscription_days: i64,
    subscription_id: Option<i64>,
}

impl OrderService {
    pub fn new(
        config: Arc<AppConfig>,
        orders: OrderRepository,
        audit_logs: AuditLogRepository,
        subscription_plans: SubscriptionPlanRepository,
        system_config: SystemConfigService,
        sub2api: Option<Sub2ApiClient>,
    ) -> Self {
        Self {
            config,
            orders,
            audit_logs,
            subscription_plans,
            system_config,
            sub2api,
        }
    }

    pub async fn create_pending_order(
        &self,
        input: CreatePendingOrderInput,
    ) -> Result<OrderRecord> {
        self.create_pending_order_at(input, now_timestamp()).await
    }

    pub async fn create_pending_order_at(
        &self,
        input: CreatePendingOrderInput,
        now_ts: i64,
    ) -> Result<OrderRecord> {
        if input.user_id <= 0 {
            bail!("invalid user id");
        }
        if input.amount_cents <= 0 {
            bail!("amount must be positive");
        }

        match input.order_type.as_str() {
            "balance" => {
                let balance_disabled = self
                    .system_config
                    .get("BALANCE_PAYMENT_DISABLED")
                    .await?
                    .map(|value| value == "true")
                    .unwrap_or(false);
                if balance_disabled {
                    bail!("balance payment is disabled");
                }
            }
            "subscription" => {
                if input.plan_id.as_deref().is_none_or(str::is_empty) {
                    bail!("subscription plan is required");
                }
                if input.subscription_group_id.unwrap_or_default() <= 0 {
                    bail!("subscription group is required");
                }
                if input.subscription_days.unwrap_or_default() <= 0 {
                    bail!("subscription validity is required");
                }
            }
            other => bail!("unsupported order type {other}"),
        }

        if input.order_type == "balance" {
            let min_amount_cents = self
                .system_config
                .get("RECHARGE_MIN_AMOUNT")
                .await?
                .and_then(|value| amount_string_to_cents(&value))
                .unwrap_or_else(|| amount_to_cents(self.config.min_recharge_amount));
            let max_amount_cents = self
                .system_config
                .get("RECHARGE_MAX_AMOUNT")
                .await?
                .and_then(|value| amount_string_to_cents(&value))
                .unwrap_or_else(|| amount_to_cents(self.config.max_recharge_amount));

            if input.amount_cents < min_amount_cents || input.amount_cents > max_amount_cents {
                bail!("amount out of allowed range");
            }
        }

        let max_pending_orders = self
            .system_config
            .get("MAX_PENDING_ORDERS")
            .await?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(DEFAULT_MAX_PENDING_ORDERS);
        let pending_count = self.orders.count_pending_by_user(input.user_id).await?;
        if pending_count >= max_pending_orders {
            bail!("too many pending orders");
        }

        let timeout_minutes = self
            .system_config
            .get("ORDER_TIMEOUT_MINUTES")
            .await?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(DEFAULT_ORDER_TIMEOUT_MINUTES);
        let expires_at = now_ts + timeout_minutes.max(1) * 60;

        let order = self
            .orders
            .insert_pending(NewPendingOrder {
                user_id: input.user_id,
                amount_cents: input.amount_cents,
                pay_amount_cents: input.pay_amount_cents,
                fee_rate_bps: input.fee_rate_bps,
                status: "PENDING".to_string(),
                payment_type: input.payment_type,
                order_type: input.order_type,
                plan_id: input.plan_id,
                subscription_group_id: input.subscription_group_id,
                subscription_days: input.subscription_days,
                provider_instance_id: input.provider_instance_id,
                expires_at,
            })
            .await?;

        self.audit_logs
            .append(NewAuditLog {
                order_id: order.id.clone(),
                action: "ORDER_CREATED".to_string(),
                detail: Some(
                    json!({
                        "userId": order.user_id,
                        "amountCents": order.amount_cents,
                        "payAmountCents": order.pay_amount_cents.unwrap_or(order.amount_cents),
                        "feeRateBps": order.fee_rate_bps.unwrap_or(0),
                        "paymentType": order.payment_type,
                        "orderType": order.order_type,
                        "planId": order.plan_id,
                        "subscriptionGroupId": order.subscription_group_id,
                        "subscriptionDays": order.subscription_days,
                    })
                    .to_string(),
                ),
                operator: Some(format!("user:{}", order.user_id)),
            })
            .await?;

        Ok(order)
    }

    pub async fn expire_pending_orders(&self, batch_size: i64) -> Result<usize> {
        self.expire_pending_orders_at(batch_size, now_timestamp())
            .await
    }

    pub async fn expire_pending_orders_at(&self, batch_size: i64, now_ts: i64) -> Result<usize> {
        let orders = self.orders.list_expired_pending(now_ts, batch_size).await?;
        if orders.is_empty() {
            return Ok(0);
        }

        let mut expired = 0usize;
        for order in orders {
            if self
                .orders
                .mark_status_if_pending(&order.id, "EXPIRED")
                .await?
            {
                expired += 1;
                self.audit_logs
                    .append(NewAuditLog {
                        order_id: order.id,
                        action: "ORDER_EXPIRED".to_string(),
                        detail: Some("Order expired".to_string()),
                        operator: Some("timeout".to_string()),
                    })
                    .await?;
            }
        }

        Ok(expired)
    }

    pub async fn confirm_payment(
        &self,
        order_id: &str,
        trade_no: &str,
        paid_amount_cents: i64,
        provider_name: &str,
    ) -> Result<ConfirmPaymentOutcome> {
        self.confirm_payment_at(
            order_id,
            trade_no,
            paid_amount_cents,
            provider_name,
            now_timestamp(),
        )
        .await
    }

    pub async fn confirm_payment_at(
        &self,
        order_id: &str,
        trade_no: &str,
        paid_amount_cents: i64,
        provider_name: &str,
        now_ts: i64,
    ) -> Result<ConfirmPaymentOutcome> {
        let Some(order) = self.orders.get_by_id(order_id).await? else {
            return Ok(ConfirmPaymentOutcome::OrderNotFound);
        };

        if paid_amount_cents <= 0 {
            return Ok(ConfirmPaymentOutcome::StopRetry);
        }

        let expected_cents = order.pay_amount_cents.unwrap_or(order.amount_cents);
        let diff = (paid_amount_cents - expected_cents).abs();
        if diff > 1 {
            self.audit_logs
                .append(NewAuditLog {
                    order_id: order.id,
                    action: "PAYMENT_AMOUNT_MISMATCH".to_string(),
                    detail: Some(format!(
                        "{{\"expected\":{},\"paid\":{},\"diff\":{},\"tradeNo\":\"{}\"}}",
                        expected_cents, paid_amount_cents, diff, trade_no
                    )),
                    operator: Some(provider_name.to_string()),
                })
                .await?;
            return Ok(ConfirmPaymentOutcome::AmountMismatch);
        }

        let grace_updated_at_gte = now_ts - PAYMENT_CONFIRM_GRACE_SECONDS;
        if self
            .orders
            .mark_paid_if_pending_or_recent_expired(MarkPaidInput {
                order_id: order_id.to_string(),
                trade_no: trade_no.to_string(),
                paid_amount_cents,
                paid_at: now_ts,
                grace_updated_at_gte,
            })
            .await?
        {
            self.audit_logs
                .append(NewAuditLog {
                    order_id: order_id.to_string(),
                    action: "ORDER_PAID".to_string(),
                    detail: Some(format!(
                        "{{\"previousStatus\":\"{}\",\"tradeNo\":\"{}\",\"expectedAmountCents\":{},\"expectedPayAmountCents\":{},\"paidAmountCents\":{}}}",
                        order.status,
                        trade_no,
                        order.amount_cents,
                        expected_cents,
                        paid_amount_cents
                    )),
                    operator: Some(provider_name.to_string()),
                })
                .await?;
            return Ok(ConfirmPaymentOutcome::Confirmed);
        }

        let current = self.orders.get_by_id(order_id).await?;
        let Some(current) = current else {
            return Ok(ConfirmPaymentOutcome::StopRetry);
        };

        let outcome = match current.status.as_str() {
            "COMPLETED" | "REFUNDED" => ConfirmPaymentOutcome::StopRetry,
            "FAILED" | "PAID" | "RECHARGING" => ConfirmPaymentOutcome::RetryLater,
            _ => ConfirmPaymentOutcome::StopRetry,
        };
        Ok(outcome)
    }

    pub async fn attach_payment_creation(
        &self,
        order_id: &str,
        payment: &CreatePaymentResponse,
    ) -> Result<()> {
        self.orders
            .set_payment_details(
                order_id,
                &payment.trade_no,
                payment.pay_url.as_deref(),
                payment.qr_code.as_deref(),
            )
            .await?;

        self.audit_logs
            .append(NewAuditLog {
                order_id: order_id.to_string(),
                action: "PAYMENT_CREATED".to_string(),
                detail: Some(format!(
                    "{{\"provider\":\"{}\",\"tradeNo\":\"{}\",\"hasPayUrl\":{},\"hasQrCode\":{},\"hasClientSecret\":{}}}",
                    payment.provider_name,
                    payment.trade_no,
                    payment.pay_url.is_some(),
                    payment.qr_code.is_some(),
                    payment.client_secret.is_some()
                )),
                operator: Some(payment.provider_name.clone()),
            })
            .await?;

        Ok(())
    }

    pub async fn delete_order(&self, order_id: &str) -> Result<()> {
        let _ = self.orders.delete_by_id(order_id).await?;
        Ok(())
    }

    pub async fn admin_cancel_order(&self, order_id: &str) -> Result<CancelOrderOutcome> {
        let Some(order) = self.orders.get_by_id(order_id).await? else {
            bail!("order not found");
        };

        if order.status != "PENDING" {
            bail!("order cannot be cancelled");
        }

        if self
            .orders
            .mark_status_if_pending(order_id, "CANCELLED")
            .await?
        {
            self.audit_logs
                .append(NewAuditLog {
                    order_id: order_id.to_string(),
                    action: "ORDER_CANCELLED".to_string(),
                    detail: Some("Admin cancelled order".to_string()),
                    operator: Some("admin".to_string()),
                })
                .await?;
            return Ok(CancelOrderOutcome::Cancelled);
        }

        let Some(current) = self.orders.get_by_id(order_id).await? else {
            return Ok(CancelOrderOutcome::Cancelled);
        };

        match current.status.as_str() {
            "PAID" | "FAILED" => {
                let _ = self.execute_paid_fulfillment(order_id).await?;
                Ok(CancelOrderOutcome::AlreadyPaid)
            }
            "RECHARGING" | "COMPLETED" | "REFUNDED" => Ok(CancelOrderOutcome::AlreadyPaid),
            _ => Ok(CancelOrderOutcome::Cancelled),
        }
    }

    pub async fn cancel_order(&self, order_id: &str, user_id: i64) -> Result<CancelOrderOutcome> {
        let Some(order) = self.orders.get_by_id(order_id).await? else {
            bail!("order not found");
        };

        if order.user_id != user_id {
            bail!("forbidden");
        }
        if order.status != "PENDING" {
            bail!("order cannot be cancelled");
        }

        if self
            .orders
            .mark_status_if_pending(order_id, "CANCELLED")
            .await?
        {
            self.audit_logs
                .append(NewAuditLog {
                    order_id: order_id.to_string(),
                    action: "ORDER_CANCELLED".to_string(),
                    detail: Some("User cancelled order".to_string()),
                    operator: Some(format!("user:{user_id}")),
                })
                .await?;
            return Ok(CancelOrderOutcome::Cancelled);
        }

        let Some(current) = self.orders.get_by_id(order_id).await? else {
            return Ok(CancelOrderOutcome::Cancelled);
        };

        match current.status.as_str() {
            "PAID" | "FAILED" | "RECHARGING" | "COMPLETED" | "REFUNDED" => {
                let _ = self.execute_paid_fulfillment(order_id).await?;
                Ok(CancelOrderOutcome::AlreadyPaid)
            }
            _ => Ok(CancelOrderOutcome::Cancelled),
        }
    }

    pub async fn retry_recharge(&self, order_id: &str) -> Result<()> {
        let Some(order) = self.orders.get_by_id(order_id).await? else {
            bail!("order not found");
        };

        self.assert_retry_allowed(&order)?;

        if !self.orders.reset_to_paid_if_retryable(order_id).await? {
            let Some(latest) = self.orders.get_by_id(order_id).await? else {
                bail!("order not found");
            };

            match latest.status.as_str() {
                "PAID" | "RECHARGING" => bail!("order is recharging, retry later"),
                "COMPLETED" | "REFUNDED" => bail!("order already completed"),
                status if is_refund_status(status) => {
                    bail!("refund-related order cannot retry")
                }
                _ => bail!("order status changed, refresh and retry"),
            }
        }

        self.audit_logs
            .append(NewAuditLog {
                order_id: order_id.to_string(),
                action: "RECHARGE_RETRY".to_string(),
                detail: Some("Admin manual retry recharge".to_string()),
                operator: Some("admin".to_string()),
            })
            .await?;

        if self.execute_paid_fulfillment(order_id).await? {
            return Ok(());
        }

        let latest = self.orders.get_by_id(order_id).await?;
        let reason = latest
            .and_then(|item| item.failed_reason)
            .unwrap_or_else(|| "recharge retry failed".to_string());
        bail!(reason);
    }

    pub async fn request_refund(&self, input: RequestRefundInput) -> Result<()> {
        let Some(order) = self.orders.get_by_id(&input.order_id).await? else {
            bail!("order not found");
        };

        if order.user_id != input.user_id {
            bail!("forbidden");
        }
        if order.order_type != "balance" {
            bail!("only balance orders support refund request");
        }
        if order.status != "COMPLETED" {
            bail!("only completed orders can request refund");
        }
        if input.amount_cents <= 0 {
            bail!("refund amount must be positive");
        }
        if input.amount_cents > order.amount_cents {
            bail!("refund amount exceeds recharge amount");
        }

        let admin_api_key = self.sub2api_admin_api_key().await?;
        let sub2api = self
            .sub2api
            .as_ref()
            .ok_or_else(|| anyhow!("SUB2API_BASE_URL is not configured"))?;
        let user = sub2api.get_user(order.user_id, &admin_api_key).await?;
        let balance_cents = user
            .balance
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(amount_to_cents)
            .unwrap_or(0);
        if balance_cents < input.amount_cents {
            bail!("refund amount exceeds current balance");
        }

        let normalized_reason = input
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        if !self
            .orders
            .mark_refund_requested(MarkRefundRequestedInput {
                order_id: input.order_id.clone(),
                user_id: input.user_id,
                refund_amount_cents: input.amount_cents,
                refund_request_reason: normalized_reason.clone(),
                refund_requested_by: input.user_id,
                refund_requested_at: now_timestamp(),
            })
            .await?
        {
            bail!("order status changed, refresh and retry");
        }

        self.audit_logs
            .append(NewAuditLog {
                order_id: input.order_id,
                action: "REFUND_REQUESTED".to_string(),
                detail: Some(
                    json!({
                        "amountCents": input.amount_cents,
                        "reason": normalized_reason,
                        "requestedBy": input.user_id,
                    })
                    .to_string(),
                ),
                operator: Some(format!("user:{}", input.user_id)),
            })
            .await?;

        Ok(())
    }

    pub async fn process_refund(
        &self,
        state: &AppState,
        input: ProcessRefundInput,
    ) -> Result<ProcessRefundResult> {
        let Some(order) = self.orders.get_by_id(&input.order_id).await? else {
            bail!("order not found");
        };

        if !matches!(
            order.status.as_str(),
            "COMPLETED" | "REFUND_REQUESTED" | "REFUND_FAILED"
        ) {
            bail!("order is not refundable in current status");
        }

        let recharge_amount_cents = order.amount_cents;
        let gateway_full_refund_cents = order.pay_amount_cents.unwrap_or(order.amount_cents);
        let refund_amount_cents = input.amount_cents.unwrap_or(recharge_amount_cents);
        if refund_amount_cents <= 0 {
            bail!("refund amount must be positive");
        }
        if refund_amount_cents > recharge_amount_cents {
            bail!("refund amount exceeds recharge amount");
        }

        let gateway_refund_amount_cents = if input.amount_cents.is_some() {
            refund_amount_cents
        } else {
            gateway_full_refund_cents
        };
        let refund_reason = input
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| order.refund_request_reason.clone())
            .unwrap_or_else(|| format!("sub2apipay refund order:{}", order.id));

        let admin_api_key = self.sub2api_admin_api_key().await?;
        let sub2api = self
            .sub2api
            .as_ref()
            .ok_or_else(|| anyhow!("SUB2API_BASE_URL is not configured"))?;

        let deduction = match self
            .prepare_deduction(
                &order,
                refund_amount_cents,
                input.deduct_balance,
                input.force,
                sub2api,
                &admin_api_key,
            )
            .await?
        {
            Some(plan) => plan,
            None => {
                return Ok(ProcessRefundResult {
                    success: false,
                    warning: Some(if order.order_type == "subscription" {
                        "cannot fetch subscription info, use force".to_string()
                    } else {
                        "cannot fetch user balance, use force".to_string()
                    }),
                    require_force: true,
                    balance_deducted_cents: 0,
                    subscription_days_deducted: 0,
                });
            }
        };

        if !self
            .orders
            .mark_refunding_if_refundable(&input.order_id)
            .await?
        {
            bail!("order status changed, refresh and retry");
        }

        match self
            .process_refund_locked(
                state,
                &order,
                &refund_reason,
                refund_amount_cents,
                gateway_refund_amount_cents,
                &deduction,
                input.force,
                input.deduct_balance,
                sub2api,
                &admin_api_key,
            )
            .await
        {
            Ok(result) => Ok(result),
            Err(error) => {
                let now_ts = now_timestamp();
                let _ = self
                    .orders
                    .mark_refund_failed(&input.order_id, &error.to_string(), now_ts)
                    .await;
                self.audit_logs
                    .append(NewAuditLog {
                        order_id: input.order_id,
                        action: "REFUND_FAILED".to_string(),
                        detail: Some(error.to_string()),
                        operator: Some("admin".to_string()),
                    })
                    .await?;
                Err(error)
            }
        }
    }

    pub async fn handle_verified_notification(
        &self,
        notification: &VerifiedPaymentNotification,
    ) -> Result<bool> {
        if !notification.success {
            return Ok(true);
        }

        let outcome = self
            .confirm_payment(
                &notification.order_id,
                &notification.trade_no,
                notification.amount_cents,
                &notification.provider_name,
            )
            .await?;

        match outcome {
            ConfirmPaymentOutcome::Confirmed | ConfirmPaymentOutcome::RetryLater => {
                self.execute_paid_fulfillment(&notification.order_id).await
            }
            ConfirmPaymentOutcome::StopRetry => Ok(true),
            ConfirmPaymentOutcome::AmountMismatch | ConfirmPaymentOutcome::OrderNotFound => {
                Ok(false)
            }
        }
    }

    async fn execute_paid_fulfillment(&self, order_id: &str) -> Result<bool> {
        let Some(order) = self.orders.get_by_id(order_id).await? else {
            return Ok(false);
        };

        match order.status.as_str() {
            "COMPLETED" | "REFUNDED" => return Ok(true),
            "RECHARGING" => return Ok(false),
            status if is_refund_status(status) => return Ok(true),
            "PAID" | "FAILED" => {}
            _ => return Ok(true),
        }

        if !self
            .orders
            .mark_recharging_if_paid_or_failed(order_id)
            .await?
        {
            let Some(current) = self.orders.get_by_id(order_id).await? else {
                return Ok(false);
            };

            return Ok(matches!(current.status.as_str(), "COMPLETED" | "REFUNDED"));
        }

        let now_ts = now_timestamp();
        let admin_api_key = match self.sub2api_admin_api_key().await {
            Ok(value) => value,
            Err(error) => match order.order_type.as_str() {
                "subscription" => {
                    return self
                        .record_subscription_failure(order_id, error.to_string(), now_ts)
                        .await;
                }
                _ => {
                    return self
                        .record_recharge_failure(order_id, error.to_string(), now_ts)
                        .await;
                }
            },
        };
        let sub2api = match self.sub2api.as_ref() {
            Some(client) => client,
            None => match order.order_type.as_str() {
                "subscription" => {
                    return self
                        .record_subscription_failure(
                            order_id,
                            "SUB2API_BASE_URL is not configured".to_string(),
                            now_ts,
                        )
                        .await;
                }
                _ => {
                    return self
                        .record_recharge_failure(
                            order_id,
                            "SUB2API_BASE_URL is not configured".to_string(),
                            now_ts,
                        )
                        .await;
                }
            },
        };

        match order.order_type.as_str() {
            "balance" => {
                self.execute_balance_fulfillment(&order, sub2api, &admin_api_key, now_ts)
                    .await
            }
            "subscription" => {
                self.execute_subscription_fulfillment(&order, sub2api, &admin_api_key, now_ts)
                    .await
            }
            other => {
                self.record_recharge_failure(
                    order_id,
                    format!("unsupported order type {other} in Rust fulfillment"),
                    now_ts,
                )
                .await
            }
        }
    }

    async fn execute_balance_fulfillment(
        &self,
        order: &OrderRecord,
        sub2api: &Sub2ApiClient,
        admin_api_key: &str,
        now_ts: i64,
    ) -> Result<bool> {
        let recharge_result = sub2api
            .create_and_redeem_balance(
                &order.recharge_code,
                cents_to_amount(order.amount_cents),
                order.user_id,
                &format!("sub2apipay recharge order:{}", order.id),
                admin_api_key,
            )
            .await;

        match recharge_result {
            Ok(_) => {
                if !self
                    .orders
                    .mark_completed_after_fulfillment(&order.id, now_ts)
                    .await?
                {
                    let current = self.orders.get_by_id(&order.id).await?;
                    return Ok(current
                        .as_ref()
                        .is_some_and(|saved| saved.status == "COMPLETED"));
                }

                self.audit_logs
                    .append(NewAuditLog {
                        order_id: order.id.clone(),
                        action: "RECHARGE_SUCCESS".to_string(),
                        detail: Some(
                            json!({
                                "rechargeCode": order.recharge_code,
                                "amountCents": order.amount_cents,
                            })
                            .to_string(),
                        ),
                        operator: Some("system".to_string()),
                    })
                    .await?;

                Ok(true)
            }
            Err(error) => {
                self.record_recharge_failure(&order.id, error.to_string(), now_ts)
                    .await
            }
        }
    }

    async fn execute_subscription_fulfillment(
        &self,
        order: &OrderRecord,
        sub2api: &Sub2ApiClient,
        admin_api_key: &str,
        now_ts: i64,
    ) -> Result<bool> {
        let group_id = match order.subscription_group_id {
            Some(value) if value > 0 => value,
            _ => {
                return self
                    .record_subscription_failure(
                        &order.id,
                        "Missing subscription group info on order".to_string(),
                        now_ts,
                    )
                    .await;
            }
        };
        let base_validity_days = match order.subscription_days {
            Some(value) if value > 0 => value,
            _ => {
                return self
                    .record_subscription_failure(
                        &order.id,
                        "Missing subscription validity on order".to_string(),
                        now_ts,
                    )
                    .await;
            }
        };

        let group = match sub2api.get_group(group_id, admin_api_key).await {
            Ok(Some(group)) if group.status == "active" => group,
            Ok(_) => {
                return self
                    .record_subscription_failure(
                        &order.id,
                        format!(
                            "SUBSCRIPTION_GROUP_GONE: Subscription group {group_id} no longer exists or inactive"
                        ),
                        now_ts,
                    )
                    .await;
            }
            Err(error) => {
                return self
                    .record_subscription_failure(&order.id, error.to_string(), now_ts)
                    .await;
            }
        };

        if group.subscription_type.as_deref() != Some("subscription") {
            return self
                .record_subscription_failure(
                    &order.id,
                    format!("Subscription group {group_id} is not a subscription type"),
                    now_ts,
                )
                .await;
        }

        let user_subscriptions = match sub2api
            .get_user_subscriptions(order.user_id, admin_api_key)
            .await
        {
            Ok(items) => items,
            Err(error) => {
                return self
                    .record_subscription_failure(&order.id, error.to_string(), now_ts)
                    .await;
            }
        };

        let mut granted_days = base_validity_days;
        let mut fulfill_method = "new";
        let mut renewed_subscription_id = None;

        if let Some(active_subscription) = user_subscriptions
            .into_iter()
            .find(|item| item.group_id == group_id && item.status == "active")
        {
            fulfill_method = "renew";
            renewed_subscription_id = Some(active_subscription.id);

            if let Some(plan_id) = order.plan_id.as_deref() {
                match self.subscription_plans.get_by_id(plan_id).await {
                    Ok(Some(plan)) => match compute_validity_days(
                        plan.validity_days,
                        &plan.validity_unit,
                        Some(&active_subscription.expires_at),
                    ) {
                        Ok(days) => granted_days = days,
                        Err(error) => {
                            return self
                                .record_subscription_failure(&order.id, error.to_string(), now_ts)
                                .await;
                        }
                    },
                    Ok(None) => {}
                    Err(error) => {
                        return self
                            .record_subscription_failure(&order.id, error.to_string(), now_ts)
                            .await;
                    }
                }
            }
        }

        let redeem_result = sub2api
            .create_and_redeem_subscription(
                &order.recharge_code,
                cents_to_amount(order.amount_cents),
                order.user_id,
                &format!("sub2apipay subscription order:{}", order.id),
                group_id,
                granted_days,
                admin_api_key,
            )
            .await;

        match redeem_result {
            Ok(_) => {
                if !self
                    .orders
                    .mark_completed_after_fulfillment(&order.id, now_ts)
                    .await?
                {
                    let current = self.orders.get_by_id(&order.id).await?;
                    return Ok(current
                        .as_ref()
                        .is_some_and(|saved| saved.status == "COMPLETED"));
                }

                self.audit_logs
                    .append(NewAuditLog {
                        order_id: order.id.clone(),
                        action: "SUBSCRIPTION_SUCCESS".to_string(),
                        detail: Some(
                            json!({
                                "groupId": group_id,
                                "days": base_validity_days,
                                "grantedDays": granted_days,
                                "amountCents": order.amount_cents,
                                "method": fulfill_method,
                                "renewedSubscriptionId": renewed_subscription_id,
                            })
                            .to_string(),
                        ),
                        operator: Some("system".to_string()),
                    })
                    .await?;

                Ok(true)
            }
            Err(error) => {
                self.record_subscription_failure(&order.id, error.to_string(), now_ts)
                    .await
            }
        }
    }

    async fn prepare_deduction(
        &self,
        order: &OrderRecord,
        refund_amount_cents: i64,
        deduct_balance: bool,
        force: bool,
        sub2api: &Sub2ApiClient,
        admin_api_key: &str,
    ) -> Result<Option<DeductionPlan>> {
        if !deduct_balance {
            return Ok(Some(DeductionPlan {
                balance_amount_cents: 0,
                subscription_days: 0,
                subscription_id: None,
            }));
        }

        if order.order_type == "subscription" {
            let Some(group_id) = order.subscription_group_id else {
                return Ok(Some(DeductionPlan {
                    balance_amount_cents: 0,
                    subscription_days: 0,
                    subscription_id: None,
                }));
            };
            let Some(order_subscription_days) = order.subscription_days else {
                return Ok(Some(DeductionPlan {
                    balance_amount_cents: 0,
                    subscription_days: 0,
                    subscription_id: None,
                }));
            };

            let user_subscriptions = match sub2api
                .get_user_subscriptions(order.user_id, admin_api_key)
                .await
            {
                Ok(items) => items,
                Err(error) => {
                    if force {
                        let _ = error;
                        return Ok(Some(DeductionPlan {
                            balance_amount_cents: 0,
                            subscription_days: 0,
                            subscription_id: None,
                        }));
                    }
                    return Ok(None);
                }
            };

            let Some(active_subscription) = user_subscriptions
                .into_iter()
                .find(|item| item.group_id == group_id && item.status == "active")
            else {
                return Ok(Some(DeductionPlan {
                    balance_amount_cents: 0,
                    subscription_days: 0,
                    subscription_id: None,
                }));
            };

            let remaining_days =
                remaining_days_from_rfc3339(&active_subscription.expires_at, now_timestamp())?;
            return Ok(Some(DeductionPlan {
                balance_amount_cents: 0,
                subscription_days: order_subscription_days.min(remaining_days),
                subscription_id: Some(active_subscription.id),
            }));
        }

        let user = match sub2api.get_user(order.user_id, admin_api_key).await {
            Ok(user) => user,
            Err(error) => {
                if force {
                    let _ = error;
                    return Ok(Some(DeductionPlan {
                        balance_amount_cents: 0,
                        subscription_days: 0,
                        subscription_id: None,
                    }));
                }
                return Ok(None);
            }
        };
        let user_balance_cents = user
            .balance
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(amount_to_cents)
            .unwrap_or(0);

        Ok(Some(DeductionPlan {
            balance_amount_cents: refund_amount_cents.min(user_balance_cents),
            subscription_days: 0,
            subscription_id: None,
        }))
    }

    async fn process_refund_locked(
        &self,
        state: &AppState,
        order: &OrderRecord,
        refund_reason: &str,
        refund_amount_cents: i64,
        gateway_refund_amount_cents: i64,
        deduction: &DeductionPlan,
        force: bool,
        deduct_balance: bool,
        sub2api: &Sub2ApiClient,
        admin_api_key: &str,
    ) -> Result<ProcessRefundResult> {
        self.execute_deduction(order, deduction, sub2api, admin_api_key)
            .await?;

        let gateway_refund = if let Some(trade_no) = order.payment_trade_no.as_deref() {
            match crate::payment_provider::refund_payment(
                state,
                &RefundPaymentRequest {
                    trade_no: trade_no.to_string(),
                    order_id: order.id.clone(),
                    amount_cents: gateway_refund_amount_cents,
                    payment_type: order.payment_type.clone(),
                    reason: refund_reason.to_string(),
                    provider_instance_id: order.provider_instance_id.clone(),
                },
            )
            .await
            {
                Ok(response) => Some(response),
                Err(gateway_error) => {
                    let rollback_ok = self
                        .rollback_deduction(
                            order,
                            deduction,
                            sub2api,
                            admin_api_key,
                            &gateway_error,
                        )
                        .await?;

                    if rollback_ok {
                        let restore_status = if order.status == "REFUND_REQUESTED" {
                            "REFUND_REQUESTED"
                        } else {
                            "COMPLETED"
                        };
                        let _ = self
                            .orders
                            .restore_after_refund_gateway_failure(&order.id, restore_status)
                            .await?;
                        self.audit_logs
                            .append(NewAuditLog {
                                order_id: order.id.clone(),
                                action: "REFUND_GATEWAY_FAILED".to_string(),
                                detail: Some(format!(
                                    "Gateway refund failed, deduction rolled back: {}",
                                    gateway_error
                                )),
                                operator: Some("admin".to_string()),
                            })
                            .await?;

                        return Ok(ProcessRefundResult {
                            success: false,
                            warning: Some(format!(
                                "gateway refund failed: {}, deduction rolled back",
                                gateway_error
                            )),
                            require_force: false,
                            balance_deducted_cents: 0,
                            subscription_days_deducted: 0,
                        });
                    }

                    bail!(
                        "Gateway refund failed and rollback also failed: {}",
                        gateway_error
                    );
                }
            }
        } else {
            self.audit_logs
                .append(NewAuditLog {
                    order_id: order.id.clone(),
                    action: "REFUND_NO_TRADE_NO".to_string(),
                    detail: Some("No paymentTradeNo, skipped gateway refund".to_string()),
                    operator: Some("admin".to_string()),
                })
                .await?;
            None
        };

        let final_status = if refund_amount_cents < order.amount_cents {
            "PARTIALLY_REFUNDED"
        } else {
            "REFUNDED"
        };
        let _ = self
            .orders
            .mark_refunded(MarkRefundedInput {
                order_id: order.id.clone(),
                status: final_status.to_string(),
                refund_amount_cents,
                refund_reason: refund_reason.to_string(),
                refund_at: now_timestamp(),
                force_refund: force,
            })
            .await?;

        self.audit_logs
            .append(NewAuditLog {
                order_id: order.id.clone(),
                action: if final_status == "PARTIALLY_REFUNDED" {
                    "PARTIAL_REFUND_SUCCESS".to_string()
                } else {
                    "REFUND_SUCCESS".to_string()
                },
                detail: Some(
                    json!({
                        "rechargeAmountCents": order.amount_cents,
                        "refundAmountCents": refund_amount_cents,
                        "gatewayRefundAmountCents": gateway_refund_amount_cents,
                        "gatewayProvider": gateway_refund.as_ref().map(|item| item.provider_name.clone()),
                        "gatewayRefundId": gateway_refund.as_ref().map(|item| item.refund_id.clone()),
                        "gatewayStatus": gateway_refund.as_ref().map(|item| item.status.clone()),
                        "reason": refund_reason,
                        "force": force,
                        "deductBalance": deduct_balance,
                        "balanceDeductedCents": deduction.balance_amount_cents,
                        "subscriptionDaysDeducted": deduction.subscription_days,
                    })
                    .to_string(),
                ),
                operator: Some("admin".to_string()),
            })
            .await?;

        Ok(ProcessRefundResult {
            success: true,
            warning: None,
            require_force: false,
            balance_deducted_cents: deduction.balance_amount_cents,
            subscription_days_deducted: deduction.subscription_days,
        })
    }

    async fn execute_deduction(
        &self,
        order: &OrderRecord,
        deduction: &DeductionPlan,
        sub2api: &Sub2ApiClient,
        admin_api_key: &str,
    ) -> Result<()> {
        let now_ts = now_timestamp();
        if let Some(subscription_id) = deduction.subscription_id {
            if deduction.subscription_days > 0 {
                sub2api
                    .extend_subscription(
                        subscription_id,
                        -deduction.subscription_days,
                        &format!("sub2apipay:refund-sub:{}:{}", order.id, now_ts),
                        admin_api_key,
                    )
                    .await?;
            }
            return Ok(());
        }

        if deduction.balance_amount_cents > 0 {
            sub2api
                .subtract_balance(
                    order.user_id,
                    cents_to_amount(deduction.balance_amount_cents),
                    &format!("sub2apipay refund order:{}", order.id),
                    &format!("sub2apipay:refund:{}:{}", order.id, now_ts),
                    admin_api_key,
                )
                .await?;
        }
        Ok(())
    }

    async fn rollback_deduction(
        &self,
        order: &OrderRecord,
        deduction: &DeductionPlan,
        sub2api: &Sub2ApiClient,
        admin_api_key: &str,
        gateway_error: &anyhow::Error,
    ) -> Result<bool> {
        let now_ts = now_timestamp();

        if let Some(subscription_id) = deduction.subscription_id {
            if deduction.subscription_days <= 0 {
                return Ok(true);
            }

            match sub2api
                .extend_subscription(
                    subscription_id,
                    deduction.subscription_days,
                    &format!("sub2apipay:refund-sub-rollback:{}:{}", order.id, now_ts),
                    admin_api_key,
                )
                .await
            {
                Ok(_) => return Ok(true),
                Err(rollback_error) => {
                    self.audit_logs
                        .append(NewAuditLog {
                            order_id: order.id.clone(),
                            action: "REFUND_ROLLBACK_FAILED".to_string(),
                            detail: Some(
                                json!({
                                    "gatewayError": gateway_error.to_string(),
                                    "rollbackError": rollback_error.to_string(),
                                    "subscriptionDaysDeducted": deduction.subscription_days,
                                })
                                .to_string(),
                            ),
                            operator: Some("admin".to_string()),
                        })
                        .await?;
                    return Ok(false);
                }
            }
        }

        if deduction.balance_amount_cents <= 0 {
            return Ok(true);
        }

        match sub2api
            .add_balance(
                order.user_id,
                cents_to_amount(deduction.balance_amount_cents),
                &format!("sub2apipay refund rollback order:{}", order.id),
                &format!("sub2apipay:refund-rollback:{}:{}", order.id, now_ts),
                admin_api_key,
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(rollback_error) => {
                self.audit_logs
                    .append(NewAuditLog {
                        order_id: order.id.clone(),
                        action: "REFUND_ROLLBACK_FAILED".to_string(),
                        detail: Some(
                            json!({
                                "gatewayError": gateway_error.to_string(),
                                "rollbackError": rollback_error.to_string(),
                                "balanceDeductedCents": deduction.balance_amount_cents,
                                "needsBalanceCompensation": true,
                            })
                            .to_string(),
                        ),
                        operator: Some("admin".to_string()),
                    })
                    .await?;
                Ok(false)
            }
        }
    }

    async fn record_recharge_failure(
        &self,
        order_id: &str,
        reason: String,
        failed_at: i64,
    ) -> Result<bool> {
        self.record_fulfillment_failure(order_id, reason, failed_at, "RECHARGE_FAILED")
            .await
    }

    async fn record_subscription_failure(
        &self,
        order_id: &str,
        reason: String,
        failed_at: i64,
    ) -> Result<bool> {
        self.record_fulfillment_failure(order_id, reason, failed_at, "SUBSCRIPTION_FAILED")
            .await
    }

    async fn record_fulfillment_failure(
        &self,
        order_id: &str,
        reason: String,
        failed_at: i64,
        audit_action: &str,
    ) -> Result<bool> {
        let _ = self
            .orders
            .mark_failed_if_recharging(order_id, &reason, failed_at)
            .await?;

        self.audit_logs
            .append(NewAuditLog {
                order_id: order_id.to_string(),
                action: audit_action.to_string(),
                detail: Some(reason),
                operator: Some("system".to_string()),
            })
            .await?;

        Ok(false)
    }

    fn assert_retry_allowed(&self, order: &OrderRecord) -> Result<()> {
        if order.paid_at.is_none() {
            bail!("order is not paid, retry denied");
        }

        if is_refund_status(&order.status) {
            bail!("refund-related order cannot retry");
        }

        match order.status.as_str() {
            "FAILED" | "PAID" => Ok(()),
            "RECHARGING" => bail!("order is recharging, retry later"),
            "COMPLETED" => bail!("order already completed"),
            _ => bail!("only paid and failed orders can retry"),
        }
    }

    async fn sub2api_admin_api_key(&self) -> Result<String> {
        let value = self
            .system_config
            .get("SUB2API_ADMIN_API_KEY")
            .await?
            .unwrap_or_default();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            bail!("SUB2API_ADMIN_API_KEY is not configured");
        }
        Ok(trimmed.to_string())
    }
}

fn amount_string_to_cents(value: &str) -> Option<i64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(amount_to_cents)
}

fn amount_to_cents(value: f64) -> i64 {
    (value * 100.0).round() as i64
}

fn cents_to_amount(value: i64) -> f64 {
    (value as f64) / 100.0
}

fn remaining_days_from_rfc3339(value: &str, now_ts: i64) -> Result<i64> {
    let expires_at = chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|error| anyhow!("failed to parse subscription expiry {value}: {}", error))?
        .timestamp();
    let seconds = (expires_at - now_ts).max(0);
    Ok((seconds + 86_400 - 1) / 86_400)
}

fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock drifted before unix epoch")
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, VecDeque},
        sync::{Arc, Mutex},
        time::Duration,
    };

    use axum::{
        Form, Json, Router,
        extract::{Path, State},
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::{get, post},
    };
    use serde::{Deserialize, Serialize};
    use serde_json::json;
    use tokio::{net::TcpListener, task::JoinHandle};

    use super::*;
    use crate::{
        AppState,
        config::AppConfig,
        crypto,
        db::DatabaseHandle,
        provider_instances::{ProviderInstanceRepository, ProviderInstanceWrite},
        sub2api::Sub2ApiClient,
        subscription_plan::SubscriptionPlanRepository,
        system_config::{SystemConfigService, UpsertSystemConfig},
    };

    #[derive(Debug, Clone)]
    enum MockRedeemReply {
        Success,
        Failure {
            status: StatusCode,
            body: serde_json::Value,
        },
    }

    #[derive(Debug, Clone)]
    enum MockApiReply {
        Success,
        Failure {
            status: StatusCode,
            body: serde_json::Value,
        },
    }

    #[derive(Debug, Clone)]
    enum MockStripeReply {
        Success,
        Failure { status: StatusCode, body: String },
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    struct MockRedeemRequest {
        code: String,
        #[serde(rename = "type")]
        redeem_type: String,
        value: f64,
        user_id: i64,
        notes: String,
        group_id: Option<i64>,
        validity_days: Option<i64>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    struct MockBalanceRequest {
        operation: String,
        balance: f64,
        notes: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    struct MockExtendRequest {
        days: i64,
    }

    #[derive(Debug, Clone)]
    struct CapturedRedeemRequest {
        x_api_key: Option<String>,
        idempotency_key: Option<String>,
        body: MockRedeemRequest,
    }

    #[derive(Debug, Clone)]
    struct CapturedBalanceRequest {
        user_id: i64,
        x_api_key: Option<String>,
        idempotency_key: Option<String>,
        body: MockBalanceRequest,
    }

    #[derive(Debug, Clone)]
    struct CapturedExtendRequest {
        subscription_id: i64,
        x_api_key: Option<String>,
        idempotency_key: Option<String>,
        body: MockExtendRequest,
    }

    #[derive(Debug, Clone)]
    struct CapturedStripeRefund {
        authorization: Option<String>,
        body: HashMap<String, String>,
    }

    #[derive(Debug, Clone, Serialize)]
    struct MockGroup {
        id: i64,
        name: String,
        status: String,
        subscription_type: String,
    }

    #[derive(Debug, Clone, Serialize)]
    struct MockSubscription {
        id: i64,
        user_id: i64,
        group_id: i64,
        status: String,
        expires_at: String,
    }

    #[derive(Debug, Clone, Serialize)]
    struct MockUser {
        id: i64,
        status: String,
        balance: f64,
        email: Option<String>,
        username: Option<String>,
        notes: Option<String>,
    }

    #[derive(Debug, Clone, Default)]
    struct MockSub2ApiFixture {
        replies: Vec<MockRedeemReply>,
        groups: Vec<MockGroup>,
        users: Vec<MockUser>,
        user_subscriptions: Vec<(i64, Vec<MockSubscription>)>,
        balance_replies: Vec<MockApiReply>,
        extend_replies: Vec<MockApiReply>,
    }

    #[derive(Clone)]
    struct MockSub2ApiState {
        replies: Arc<Mutex<VecDeque<MockRedeemReply>>>,
        balance_replies: Arc<Mutex<VecDeque<MockApiReply>>>,
        extend_replies: Arc<Mutex<VecDeque<MockApiReply>>>,
        captured: Arc<Mutex<Vec<CapturedRedeemRequest>>>,
        captured_balance: Arc<Mutex<Vec<CapturedBalanceRequest>>>,
        captured_extend: Arc<Mutex<Vec<CapturedExtendRequest>>>,
        groups: Arc<HashMap<i64, MockGroup>>,
        users: Arc<HashMap<i64, MockUser>>,
        user_subscriptions: Arc<HashMap<i64, Vec<MockSubscription>>>,
    }

    #[derive(Clone)]
    struct MockStripeState {
        replies: Arc<Mutex<VecDeque<MockStripeReply>>>,
        captured: Arc<Mutex<Vec<CapturedStripeRefund>>>,
    }

    async fn mock_create_and_redeem(
        State(state): State<MockSub2ApiState>,
        headers: HeaderMap,
        Json(body): Json<MockRedeemRequest>,
    ) -> impl IntoResponse {
        state.captured.lock().unwrap().push(CapturedRedeemRequest {
            x_api_key: headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            idempotency_key: headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            body: body.clone(),
        });

        let reply = state
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(MockRedeemReply::Success);

        match reply {
            MockRedeemReply::Success => (
                StatusCode::OK,
                Json(json!({
                    "redeem_code": {
                        "code": body.code,
                        "type": body.redeem_type,
                        "value": body.value,
                        "used_by": body.user_id
                    }
                })),
            )
                .into_response(),
            MockRedeemReply::Failure { status, body } => (status, Json(body)).into_response(),
        }
    }

    async fn mock_get_group(
        Path(group_id): Path<i64>,
        State(state): State<MockSub2ApiState>,
    ) -> impl IntoResponse {
        match state.groups.get(&group_id) {
            Some(group) => (StatusCode::OK, Json(json!({ "data": group }))).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn mock_get_user_subscriptions(
        Path(user_id): Path<i64>,
        State(state): State<MockSub2ApiState>,
    ) -> impl IntoResponse {
        let items = state
            .user_subscriptions
            .get(&user_id)
            .cloned()
            .unwrap_or_default();
        (StatusCode::OK, Json(json!({ "data": items }))).into_response()
    }

    async fn mock_get_user(
        Path(user_id): Path<i64>,
        State(state): State<MockSub2ApiState>,
    ) -> impl IntoResponse {
        match state.users.get(&user_id) {
            Some(user) => (StatusCode::OK, Json(json!({ "data": user }))).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn mock_balance_operation(
        Path(user_id): Path<i64>,
        State(state): State<MockSub2ApiState>,
        headers: HeaderMap,
        Json(body): Json<MockBalanceRequest>,
    ) -> impl IntoResponse {
        state
            .captured_balance
            .lock()
            .unwrap()
            .push(CapturedBalanceRequest {
                user_id,
                x_api_key: headers
                    .get("x-api-key")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string),
                idempotency_key: headers
                    .get("idempotency-key")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string),
                body,
            });

        match state
            .balance_replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(MockApiReply::Success)
        {
            MockApiReply::Success => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
            MockApiReply::Failure { status, body } => (status, Json(body)).into_response(),
        }
    }

    async fn mock_extend_subscription(
        Path(subscription_id): Path<i64>,
        State(state): State<MockSub2ApiState>,
        headers: HeaderMap,
        Json(body): Json<MockExtendRequest>,
    ) -> impl IntoResponse {
        state
            .captured_extend
            .lock()
            .unwrap()
            .push(CapturedExtendRequest {
                subscription_id,
                x_api_key: headers
                    .get("x-api-key")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string),
                idempotency_key: headers
                    .get("idempotency-key")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string),
                body,
            });

        match state
            .extend_replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(MockApiReply::Success)
        {
            MockApiReply::Success => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
            MockApiReply::Failure { status, body } => (status, Json(body)).into_response(),
        }
    }

    async fn start_mock_sub2api(
        fixture: MockSub2ApiFixture,
    ) -> (String, MockSub2ApiState, JoinHandle<()>) {
        let state = MockSub2ApiState {
            replies: Arc::new(Mutex::new(VecDeque::from(fixture.replies))),
            balance_replies: Arc::new(Mutex::new(VecDeque::from(fixture.balance_replies))),
            extend_replies: Arc::new(Mutex::new(VecDeque::from(fixture.extend_replies))),
            captured: Arc::new(Mutex::new(Vec::new())),
            captured_balance: Arc::new(Mutex::new(Vec::new())),
            captured_extend: Arc::new(Mutex::new(Vec::new())),
            groups: Arc::new(
                fixture
                    .groups
                    .into_iter()
                    .map(|item| (item.id, item))
                    .collect(),
            ),
            users: Arc::new(
                fixture
                    .users
                    .into_iter()
                    .map(|item| (item.id, item))
                    .collect(),
            ),
            user_subscriptions: Arc::new(fixture.user_subscriptions.into_iter().collect()),
        };

        let app = Router::new()
            .route(
                "/api/v1/admin/redeem-codes/create-and-redeem",
                post(mock_create_and_redeem),
            )
            .route("/api/v1/admin/groups/{group_id}", get(mock_get_group))
            .route("/api/v1/admin/users/{user_id}", get(mock_get_user))
            .route(
                "/api/v1/admin/users/{user_id}/balance",
                post(mock_balance_operation),
            )
            .route(
                "/api/v1/admin/users/{user_id}/subscriptions",
                get(mock_get_user_subscriptions),
            )
            .route(
                "/api/v1/admin/subscriptions/{subscription_id}/extend",
                post(mock_extend_subscription),
            )
            .with_state(state.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{addr}"), state, handle)
    }

    async fn mock_stripe_refund(
        State(state): State<MockStripeState>,
        headers: HeaderMap,
        Form(body): Form<HashMap<String, String>>,
    ) -> impl IntoResponse {
        state.captured.lock().unwrap().push(CapturedStripeRefund {
            authorization: headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            body,
        });

        match state
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(MockStripeReply::Success)
        {
            MockStripeReply::Success => (
                StatusCode::OK,
                Json(json!({ "id": "re_test_123", "status": "succeeded" })),
            )
                .into_response(),
            MockStripeReply::Failure { status, body } => (status, body).into_response(),
        }
    }

    async fn start_mock_stripe(
        replies: Vec<MockStripeReply>,
    ) -> (String, MockStripeState, JoinHandle<()>) {
        let state = MockStripeState {
            replies: Arc::new(Mutex::new(VecDeque::from(replies))),
            captured: Arc::new(Mutex::new(Vec::new())),
        };

        let app = Router::new()
            .route("/v1/refunds", post(mock_stripe_refund))
            .with_state(state.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{addr}"), state, handle)
    }

    async fn test_service(
        sub2api_base_url: Option<String>,
    ) -> (
        OrderService,
        AuditLogRepository,
        OrderRepository,
        SystemConfigService,
        DatabaseHandle,
    ) {
        let path =
            std::env::temp_dir().join(format!("sub2apipay-service-{}.db", uuid::Uuid::new_v4()));
        let db = DatabaseHandle::open_local(&path).await.unwrap();
        db.run_migrations().await.unwrap();

        let config = Arc::new(AppConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            db_path: path,
            payment_providers: vec!["easypay".to_string()],
            admin_token: Some("dev-admin-token".to_string()),
            system_config_cache_ttl_secs: 30,
            sub2api_base_url: sub2api_base_url.clone(),
            sub2api_timeout_secs: 10,
            min_recharge_amount: 1.0,
            max_recharge_amount: 1000.0,
            max_daily_recharge_amount: 10000.0,
            pay_help_image_url: None,
            pay_help_text: None,
            stripe_publishable_key: None,
        });

        let orders = OrderRepository::new(db.clone());
        let audit = AuditLogRepository::new(db.clone());
        let subscription_plans = SubscriptionPlanRepository::new(db.clone());
        let system_config = SystemConfigService::new(db.clone(), Duration::from_secs(30));

        (
            OrderService::new(
                config,
                orders.clone(),
                audit.clone(),
                subscription_plans,
                system_config.clone(),
                sub2api_base_url.map(|base_url| Sub2ApiClient::new(base_url, 10)),
            ),
            audit,
            orders,
            system_config,
            db,
        )
    }

    async fn test_app_state(
        sub2api_base_url: Option<String>,
    ) -> (
        AppState,
        AuditLogRepository,
        OrderRepository,
        SystemConfigService,
        DatabaseHandle,
    ) {
        let path =
            std::env::temp_dir().join(format!("sub2apipay-state-{}.db", uuid::Uuid::new_v4()));
        let db = DatabaseHandle::open_local(&path).await.unwrap();
        db.run_migrations().await.unwrap();

        let config = Arc::new(AppConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            db_path: path,
            payment_providers: vec!["easypay".to_string(), "stripe".to_string()],
            admin_token: Some("dev-admin-token".to_string()),
            system_config_cache_ttl_secs: 30,
            sub2api_base_url: sub2api_base_url.clone(),
            sub2api_timeout_secs: 10,
            min_recharge_amount: 1.0,
            max_recharge_amount: 1000.0,
            max_daily_recharge_amount: 10000.0,
            pay_help_image_url: None,
            pay_help_text: None,
            stripe_publishable_key: None,
        });

        let orders = OrderRepository::new(db.clone());
        let audit = AuditLogRepository::new(db.clone());
        let subscription_plans = SubscriptionPlanRepository::new(db.clone());
        let system_config = SystemConfigService::new(db.clone(), Duration::from_secs(30));
        let sub2api = sub2api_base_url.map(|base_url| Sub2ApiClient::new(base_url, 10));
        let order_service = OrderService::new(
            Arc::clone(&config),
            orders.clone(),
            audit.clone(),
            subscription_plans,
            system_config.clone(),
            sub2api.clone(),
        );

        (
            AppState {
                config,
                db: db.clone(),
                system_config: system_config.clone(),
                sub2api,
                order_service,
            },
            audit,
            orders,
            system_config,
            db,
        )
    }

    async fn insert_subscription_plan(
        db: &DatabaseHandle,
        id: &str,
        group_id: i64,
        price_cents: i64,
        validity_days: i64,
        validity_unit: &str,
        product_name: Option<&str>,
    ) {
        db.connect()
            .unwrap()
            .execute(
                "INSERT INTO subscription_plans (id, group_id, name, price_cents, validity_days, validity_unit, product_name, for_sale)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
                turso::params![
                    id,
                    group_id,
                    format!("plan-{id}"),
                    price_cents,
                    validity_days,
                    validity_unit,
                    product_name
                ],
            )
            .await
            .unwrap();
    }

    async fn insert_stripe_instance(state: &AppState, api_base: &str) -> String {
        let repo = ProviderInstanceRepository::new(state.db.clone());
        let encrypted = crypto::encrypt(
            state.config.admin_token.as_deref(),
            &json!({
                "secretKey": "sk_test_refund",
                "apiBase": api_base,
            })
            .to_string(),
        )
        .unwrap();

        repo.create(ProviderInstanceWrite {
            provider_key: "stripe".to_string(),
            name: "Stripe Refund".to_string(),
            config: encrypted,
            supported_types: "stripe".to_string(),
            enabled: true,
            sort_order: 0,
            limits: None,
            refund_enabled: true,
        })
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn create_order_writes_audit_log() {
        let (service, audit, _, _, _) = test_service(None).await;
        let order = service
            .create_pending_order_at(
                CreatePendingOrderInput {
                    user_id: 1,
                    amount_cents: 500,
                    pay_amount_cents: Some(550),
                    fee_rate_bps: Some(1000),
                    payment_type: "alipay".to_string(),
                    order_type: "balance".to_string(),
                    plan_id: None,
                    subscription_group_id: None,
                    subscription_days: None,
                    provider_instance_id: None,
                },
                1000,
            )
            .await
            .unwrap();

        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "ORDER_CREATED")
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn expire_pending_orders_marks_and_audits() {
        let (service, audit, orders, _, _) = test_service(None).await;
        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 2,
                amount_cents: 500,
                pay_amount_cents: Some(550),
                fee_rate_bps: Some(1000),
                status: "PENDING".to_string(),
                payment_type: "alipay".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        let expired = service.expire_pending_orders_at(10, 2000).await.unwrap();
        assert_eq!(expired, 1);

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "EXPIRED");
        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "ORDER_EXPIRED")
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn confirm_payment_marks_paid_and_audits() {
        let (service, audit, orders, _, _) = test_service(None).await;
        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 3,
                amount_cents: 500,
                pay_amount_cents: Some(550),
                fee_rate_bps: Some(1000),
                status: "PENDING".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        let outcome = service
            .confirm_payment_at(&order.id, "pi_123", 550, "stripe", 2000)
            .await
            .unwrap();
        assert_eq!(outcome, ConfirmPaymentOutcome::Confirmed);

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "PAID");
        assert_eq!(saved.payment_trade_no.as_deref(), Some("pi_123"));
        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "ORDER_PAID")
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn verified_notification_completes_balance_fulfillment() {
        let (base_url, mock_state, handle) = start_mock_sub2api(MockSub2ApiFixture {
            replies: vec![MockRedeemReply::Success],
            ..Default::default()
        })
        .await;
        let (service, audit, orders, system_config, _) = test_service(Some(base_url)).await;

        system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 8,
                amount_cents: 500,
                pay_amount_cents: Some(550),
                fee_rate_bps: Some(1000),
                status: "PENDING".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        let handled = service
            .handle_verified_notification(&VerifiedPaymentNotification {
                provider_name: "stripe".to_string(),
                trade_no: "pi_success".to_string(),
                order_id: order.id.clone(),
                amount_cents: 550,
                success: true,
            })
            .await
            .unwrap();

        assert!(handled);

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "COMPLETED");
        assert!(saved.paid_at.is_some());
        assert!(saved.completed_at.is_some());
        assert_eq!(saved.failed_reason, None);
        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "RECHARGE_SUCCESS")
                .await
                .unwrap(),
            1
        );

        let captured = mock_state.captured.lock().unwrap().clone();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].x_api_key.as_deref(), Some("test-admin-key"));
        assert_eq!(
            captured[0].idempotency_key.as_deref(),
            Some(format!("sub2apipay:recharge:{}", order.recharge_code).as_str())
        );
        assert_eq!(captured[0].body.redeem_type, "balance");
        assert_eq!(captured[0].body.user_id, 8);
        assert!((captured[0].body.value - 5.0).abs() < f64::EPSILON);

        handle.abort();
    }

    #[tokio::test]
    async fn failed_fulfillment_retries_on_next_notification() {
        let (base_url, mock_state, handle) = start_mock_sub2api(MockSub2ApiFixture {
            replies: vec![
                MockRedeemReply::Failure {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    body: json!({ "error": "temporary upstream failure" }),
                },
                MockRedeemReply::Success,
            ],
            ..Default::default()
        })
        .await;
        let (service, audit, orders, system_config, _) = test_service(Some(base_url)).await;

        system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 9,
                amount_cents: 500,
                pay_amount_cents: Some(550),
                fee_rate_bps: Some(1000),
                status: "PENDING".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        let first = service
            .handle_verified_notification(&VerifiedPaymentNotification {
                provider_name: "stripe".to_string(),
                trade_no: "pi_retry".to_string(),
                order_id: order.id.clone(),
                amount_cents: 550,
                success: true,
            })
            .await
            .unwrap();
        assert!(!first);

        let failed = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(failed.status, "FAILED");
        assert!(failed.failed_reason.is_some());
        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "RECHARGE_FAILED")
                .await
                .unwrap(),
            1
        );

        let second = service
            .handle_verified_notification(&VerifiedPaymentNotification {
                provider_name: "stripe".to_string(),
                trade_no: "pi_retry".to_string(),
                order_id: order.id.clone(),
                amount_cents: 550,
                success: true,
            })
            .await
            .unwrap();
        assert!(second);

        let completed = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(completed.status, "COMPLETED");
        assert!(completed.completed_at.is_some());
        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "RECHARGE_SUCCESS")
                .await
                .unwrap(),
            1
        );

        let captured = mock_state.captured.lock().unwrap().clone();
        assert_eq!(captured.len(), 2);

        handle.abort();
    }

    #[tokio::test]
    async fn admin_cancel_order_marks_pending_order_as_cancelled() {
        let (service, audit, orders, _, _) = test_service(None).await;

        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 17,
                amount_cents: 500,
                pay_amount_cents: Some(550),
                fee_rate_bps: Some(1000),
                status: "PENDING".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        let outcome = service.admin_cancel_order(&order.id).await.unwrap();
        assert_eq!(outcome, CancelOrderOutcome::Cancelled);

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "CANCELLED");
        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "ORDER_CANCELLED")
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn cancel_order_rejects_other_users() {
        let (service, _, orders, _, _) = test_service(None).await;

        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 19,
                amount_cents: 500,
                pay_amount_cents: Some(550),
                fee_rate_bps: Some(1000),
                status: "PENDING".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        let error = service.cancel_order(&order.id, 20).await.unwrap_err();
        assert_eq!(error.to_string(), "forbidden");
    }

    #[tokio::test]
    async fn retry_recharge_replays_failed_paid_order() {
        let (base_url, mock_state, handle) = start_mock_sub2api(MockSub2ApiFixture {
            replies: vec![MockRedeemReply::Success],
            ..Default::default()
        })
        .await;
        let (service, audit, orders, system_config, _) = test_service(Some(base_url)).await;

        system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 18,
                amount_cents: 600,
                pay_amount_cents: Some(660),
                fee_rate_bps: Some(1000),
                status: "PENDING".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();
        orders
            .mark_paid_if_pending_or_recent_expired(MarkPaidInput {
                order_id: order.id.clone(),
                trade_no: "pi_retry_manual".to_string(),
                paid_amount_cents: 660,
                paid_at: 2000,
                grace_updated_at_gte: 0,
            })
            .await
            .unwrap();
        orders
            .mark_recharging_if_paid_or_failed(&order.id)
            .await
            .unwrap();
        orders
            .mark_failed_if_recharging(&order.id, "temporary upstream failure", 3000)
            .await
            .unwrap();

        service.retry_recharge(&order.id).await.unwrap();

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "COMPLETED");
        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "RECHARGE_RETRY")
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "RECHARGE_SUCCESS")
                .await
                .unwrap(),
            1
        );

        let captured = mock_state.captured.lock().unwrap().clone();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].body.code, order.recharge_code);

        handle.abort();
    }

    #[tokio::test]
    async fn verified_notification_completes_subscription_fulfillment() {
        let (base_url, mock_state, handle) = start_mock_sub2api(MockSub2ApiFixture {
            replies: vec![MockRedeemReply::Success],
            groups: vec![MockGroup {
                id: 5,
                name: "Claude Pro".to_string(),
                status: "active".to_string(),
                subscription_type: "subscription".to_string(),
            }],
            ..Default::default()
        })
        .await;
        let (service, audit, orders, system_config, db) = test_service(Some(base_url)).await;

        system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        insert_subscription_plan(&db, "plan_sub", 5, 1999, 30, "day", Some("Sub2API Pro")).await;

        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 10,
                amount_cents: 1999,
                pay_amount_cents: Some(2099),
                fee_rate_bps: Some(500),
                status: "PENDING".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "subscription".to_string(),
                plan_id: Some("plan_sub".to_string()),
                subscription_group_id: Some(5),
                subscription_days: Some(30),
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        let handled = service
            .handle_verified_notification(&VerifiedPaymentNotification {
                provider_name: "stripe".to_string(),
                trade_no: "pi_sub_success".to_string(),
                order_id: order.id.clone(),
                amount_cents: 2099,
                success: true,
            })
            .await
            .unwrap();
        assert!(handled);

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "COMPLETED");
        assert!(saved.completed_at.is_some());
        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "SUBSCRIPTION_SUCCESS")
                .await
                .unwrap(),
            1
        );

        let captured = mock_state.captured.lock().unwrap().clone();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].body.redeem_type, "subscription");
        assert_eq!(captured[0].body.group_id, Some(5));
        assert_eq!(captured[0].body.validity_days, Some(30));
        assert!((captured[0].body.value - 19.99).abs() < f64::EPSILON);

        handle.abort();
    }

    #[tokio::test]
    async fn subscription_renewal_recomputes_month_validity_from_active_expiry() {
        let (base_url, mock_state, handle) = start_mock_sub2api(MockSub2ApiFixture {
            replies: vec![MockRedeemReply::Success],
            groups: vec![MockGroup {
                id: 9,
                name: "Claude Max".to_string(),
                status: "active".to_string(),
                subscription_type: "subscription".to_string(),
            }],
            user_subscriptions: vec![(
                11,
                vec![MockSubscription {
                    id: 101,
                    user_id: 11,
                    group_id: 9,
                    status: "active".to_string(),
                    expires_at: "2026-01-31T00:00:00Z".to_string(),
                }],
            )],
            ..Default::default()
        })
        .await;
        let (service, audit, orders, system_config, db) = test_service(Some(base_url)).await;

        system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        insert_subscription_plan(&db, "plan_month", 9, 2999, 1, "month", None).await;

        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 11,
                amount_cents: 2999,
                pay_amount_cents: Some(3099),
                fee_rate_bps: Some(333),
                status: "PENDING".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "subscription".to_string(),
                plan_id: Some("plan_month".to_string()),
                subscription_group_id: Some(9),
                subscription_days: Some(30),
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        let handled = service
            .handle_verified_notification(&VerifiedPaymentNotification {
                provider_name: "stripe".to_string(),
                trade_no: "pi_sub_renew".to_string(),
                order_id: order.id.clone(),
                amount_cents: 3099,
                success: true,
            })
            .await
            .unwrap();
        assert!(handled);

        let detail = audit
            .find_latest_by_order_and_action(&order.id, "SUBSCRIPTION_SUCCESS")
            .await
            .unwrap()
            .unwrap()
            .detail
            .unwrap();
        let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(detail["method"], "renew");
        assert_eq!(detail["renewedSubscriptionId"], 101);
        assert_eq!(detail["grantedDays"], 28);

        let captured = mock_state.captured.lock().unwrap().clone();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].body.validity_days, Some(28));

        handle.abort();
    }

    #[tokio::test]
    async fn request_refund_marks_balance_order_as_requested() {
        let (base_url, _mock_state, handle) = start_mock_sub2api(MockSub2ApiFixture {
            users: vec![MockUser {
                id: 21,
                status: "active".to_string(),
                balance: 10.0,
                email: None,
                username: None,
                notes: None,
            }],
            ..Default::default()
        })
        .await;
        let (service, audit, orders, system_config, _) = test_service(Some(base_url)).await;

        system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 21,
                amount_cents: 500,
                pay_amount_cents: Some(550),
                fee_rate_bps: Some(1000),
                status: "COMPLETED".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        service
            .request_refund(RequestRefundInput {
                order_id: order.id.clone(),
                user_id: 21,
                amount_cents: 300,
                reason: Some("user wants refund".to_string()),
            })
            .await
            .unwrap();

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "REFUND_REQUESTED");
        assert_eq!(saved.refund_amount_cents, Some(300));
        assert_eq!(saved.refund_requested_by, Some(21));
        assert_eq!(
            saved.refund_request_reason.as_deref(),
            Some("user wants refund")
        );
        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "REFUND_REQUESTED")
                .await
                .unwrap(),
            1
        );

        handle.abort();
    }

    #[tokio::test]
    async fn process_refund_balance_partial_success_calls_gateway_and_balance_deduction() {
        let (sub2api_base_url, mock_sub2api, sub2api_handle) =
            start_mock_sub2api(MockSub2ApiFixture {
                users: vec![MockUser {
                    id: 31,
                    status: "active".to_string(),
                    balance: 20.0,
                    email: None,
                    username: None,
                    notes: None,
                }],
                ..Default::default()
            })
            .await;
        let (stripe_base_url, mock_stripe, stripe_handle) =
            start_mock_stripe(vec![MockStripeReply::Success]).await;
        let (state, audit, orders, system_config, _) = test_app_state(Some(sub2api_base_url)).await;

        system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        let instance_id = insert_stripe_instance(&state, &stripe_base_url).await;
        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 31,
                amount_cents: 1000,
                pay_amount_cents: Some(1099),
                fee_rate_bps: Some(990),
                status: "COMPLETED".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: Some(instance_id),
                expires_at: 1000,
            })
            .await
            .unwrap();
        orders
            .set_payment_details(&order.id, "pi_balance_refund", None, None)
            .await
            .unwrap();

        let result = state
            .order_service
            .process_refund(
                &state,
                ProcessRefundInput {
                    order_id: order.id.clone(),
                    amount_cents: Some(500),
                    reason: Some("manual partial".to_string()),
                    force: false,
                    deduct_balance: true,
                },
            )
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.balance_deducted_cents, 500);
        assert_eq!(result.subscription_days_deducted, 0);

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "PARTIALLY_REFUNDED");
        assert_eq!(saved.refund_amount_cents, Some(500));
        assert_eq!(saved.refund_reason.as_deref(), Some("manual partial"));

        let balance_calls = mock_sub2api.captured_balance.lock().unwrap().clone();
        assert_eq!(balance_calls.len(), 1);
        assert_eq!(balance_calls[0].user_id, 31);
        assert_eq!(balance_calls[0].body.operation, "subtract");
        assert!((balance_calls[0].body.balance - 5.0).abs() < f64::EPSILON);

        let stripe_calls = mock_stripe.captured.lock().unwrap().clone();
        assert_eq!(stripe_calls.len(), 1);
        assert_eq!(
            stripe_calls[0].authorization.as_deref(),
            Some("Bearer sk_test_refund")
        );
        assert_eq!(
            stripe_calls[0]
                .body
                .get("payment_intent")
                .map(String::as_str),
            Some("pi_balance_refund")
        );
        assert_eq!(
            stripe_calls[0].body.get("amount").map(String::as_str),
            Some("500")
        );

        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "PARTIAL_REFUND_SUCCESS")
                .await
                .unwrap(),
            1
        );

        sub2api_handle.abort();
        stripe_handle.abort();
    }

    #[tokio::test]
    async fn process_refund_subscription_success_deducts_subscription_days() {
        let (sub2api_base_url, mock_sub2api, sub2api_handle) =
            start_mock_sub2api(MockSub2ApiFixture {
                user_subscriptions: vec![(
                    41,
                    vec![MockSubscription {
                        id: 401,
                        user_id: 41,
                        group_id: 9,
                        status: "active".to_string(),
                        expires_at: "2026-12-31T00:00:00Z".to_string(),
                    }],
                )],
                ..Default::default()
            })
            .await;
        let (stripe_base_url, mock_stripe, stripe_handle) =
            start_mock_stripe(vec![MockStripeReply::Success]).await;
        let (state, audit, orders, system_config, _) = test_app_state(Some(sub2api_base_url)).await;

        system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        let instance_id = insert_stripe_instance(&state, &stripe_base_url).await;
        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 41,
                amount_cents: 3000,
                pay_amount_cents: Some(3099),
                fee_rate_bps: Some(330),
                status: "COMPLETED".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "subscription".to_string(),
                plan_id: Some("plan_sub".to_string()),
                subscription_group_id: Some(9),
                subscription_days: Some(30),
                provider_instance_id: Some(instance_id),
                expires_at: 1000,
            })
            .await
            .unwrap();
        orders
            .set_payment_details(&order.id, "pi_subscription_refund", None, None)
            .await
            .unwrap();

        let result = state
            .order_service
            .process_refund(
                &state,
                ProcessRefundInput {
                    order_id: order.id.clone(),
                    amount_cents: None,
                    reason: None,
                    force: false,
                    deduct_balance: true,
                },
            )
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.balance_deducted_cents, 0);
        assert_eq!(result.subscription_days_deducted, 30);

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "REFUNDED");

        let extend_calls = mock_sub2api.captured_extend.lock().unwrap().clone();
        assert_eq!(extend_calls.len(), 1);
        assert_eq!(extend_calls[0].subscription_id, 401);
        assert_eq!(extend_calls[0].body.days, -30);

        let stripe_calls = mock_stripe.captured.lock().unwrap().clone();
        assert_eq!(stripe_calls.len(), 1);
        assert_eq!(
            stripe_calls[0].body.get("amount").map(String::as_str),
            Some("3099")
        );

        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "REFUND_SUCCESS")
                .await
                .unwrap(),
            1
        );

        sub2api_handle.abort();
        stripe_handle.abort();
    }

    #[tokio::test]
    async fn process_refund_gateway_failure_rolls_back_balance_and_restores_status() {
        let (sub2api_base_url, mock_sub2api, sub2api_handle) =
            start_mock_sub2api(MockSub2ApiFixture {
                users: vec![MockUser {
                    id: 51,
                    status: "active".to_string(),
                    balance: 8.0,
                    email: None,
                    username: None,
                    notes: None,
                }],
                ..Default::default()
            })
            .await;
        let (stripe_base_url, _mock_stripe, stripe_handle) =
            start_mock_stripe(vec![MockStripeReply::Failure {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                body: "gateway down".to_string(),
            }])
            .await;
        let (state, audit, orders, system_config, _) = test_app_state(Some(sub2api_base_url)).await;

        system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        let instance_id = insert_stripe_instance(&state, &stripe_base_url).await;
        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 51,
                amount_cents: 400,
                pay_amount_cents: Some(450),
                fee_rate_bps: Some(1250),
                status: "COMPLETED".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: Some(instance_id),
                expires_at: 1000,
            })
            .await
            .unwrap();
        orders
            .set_payment_details(&order.id, "pi_gateway_fail", None, None)
            .await
            .unwrap();

        let result = state
            .order_service
            .process_refund(
                &state,
                ProcessRefundInput {
                    order_id: order.id.clone(),
                    amount_cents: None,
                    reason: None,
                    force: false,
                    deduct_balance: true,
                },
            )
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.warning.is_some());

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "COMPLETED");

        let balance_calls = mock_sub2api.captured_balance.lock().unwrap().clone();
        assert_eq!(balance_calls.len(), 2);
        assert_eq!(balance_calls[0].body.operation, "subtract");
        assert_eq!(balance_calls[1].body.operation, "add");

        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "REFUND_GATEWAY_FAILED")
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "REFUND_FAILED")
                .await
                .unwrap(),
            0
        );

        sub2api_handle.abort();
        stripe_handle.abort();
    }

    #[tokio::test]
    async fn process_refund_rollback_failure_marks_order_refund_failed() {
        let (sub2api_base_url, _mock_sub2api, sub2api_handle) =
            start_mock_sub2api(MockSub2ApiFixture {
                users: vec![MockUser {
                    id: 61,
                    status: "active".to_string(),
                    balance: 8.0,
                    email: None,
                    username: None,
                    notes: None,
                }],
                balance_replies: vec![
                    MockApiReply::Success,
                    MockApiReply::Failure {
                        status: StatusCode::INTERNAL_SERVER_ERROR,
                        body: json!({ "error": "rollback failed" }),
                    },
                ],
                ..Default::default()
            })
            .await;
        let (stripe_base_url, _mock_stripe, stripe_handle) =
            start_mock_stripe(vec![MockStripeReply::Failure {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                body: "gateway still down".to_string(),
            }])
            .await;
        let (state, audit, orders, system_config, _) = test_app_state(Some(sub2api_base_url)).await;

        system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        let instance_id = insert_stripe_instance(&state, &stripe_base_url).await;
        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 61,
                amount_cents: 400,
                pay_amount_cents: Some(450),
                fee_rate_bps: Some(1250),
                status: "COMPLETED".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: Some(instance_id),
                expires_at: 1000,
            })
            .await
            .unwrap();
        orders
            .set_payment_details(&order.id, "pi_gateway_fail_hard", None, None)
            .await
            .unwrap();

        let error = state
            .order_service
            .process_refund(
                &state,
                ProcessRefundInput {
                    order_id: order.id.clone(),
                    amount_cents: None,
                    reason: None,
                    force: false,
                    deduct_balance: true,
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Gateway refund failed"));

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "REFUND_FAILED");
        assert!(saved.failed_reason.is_some());
        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "REFUND_ROLLBACK_FAILED")
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            audit
                .count_by_order_and_action(&order.id, "REFUND_FAILED")
                .await
                .unwrap(),
            1
        );

        sub2api_handle.abort();
        stripe_handle.abort();
    }
}
