use std::collections::{HashMap, HashSet};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Mac, SimpleHmac, digest::KeyInit};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::{
    AppState,
    admin_auth::verify_admin_values,
    error::{AppError, AppResult},
    http::common::{
        amount_to_cents, cents_to_amount, message, optional_timestamp_to_rfc3339, resolve_locale,
        timestamp_to_rfc3339,
    },
    order::{
        repository::OrderRepository,
        service::{CancelOrderOutcome, CreatePendingOrderInput},
        status::{RechargeStatus, derive_order_state, is_recharge_retryable},
    },
    payment,
    payment_provider::{self, CreatePaymentRequest},
    provider_instances::ProviderInstanceRepository,
    subscription_plan::{
        SubscriptionPlanRecord, SubscriptionPlanRepository, compute_validity_days,
    },
};

const VALID_MY_ORDER_PAGE_SIZES: &[i64] = &[20, 50, 100];
const ORDER_STATUS_ACCESS_PURPOSE: &str = "order-status-access:v2";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/orders", post(create_order))
        .route("/api/orders/my", get(list_my_orders))
        .route("/api/orders/{id}", get(get_order_status))
        .route("/api/orders/{id}/cancel", post(cancel_order))
        .route("/api/orders/{id}/refund-request", post(request_refund))
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CreateOrderRequest {
    token: String,
    amount: f64,
    payment_type: String,
    src_host: Option<String>,
    src_url: Option<String>,
    is_mobile: Option<bool>,
    order_type: Option<String>,
    plan_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateOrderQuery {
    lang: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RefundRequestQuery {
    token: Option<String>,
    lang: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MyOrdersQuery {
    token: Option<String>,
    lang: Option<String>,
    page: Option<i64>,
    page_size: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct OrderStatusQuery {
    token: Option<String>,
    lang: Option<String>,
    access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CancelOrderRequest {
    token: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateOrderResponse {
    order_id: String,
    amount: f64,
    pay_amount: f64,
    fee_rate: f64,
    status: String,
    payment_type: String,
    expires_at: i64,
    pay_url: Option<String>,
    qr_code: Option<String>,
    client_secret: Option<String>,
}

#[derive(Debug, Serialize)]
struct RefundRequestResponse {
    success: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MyOrdersResponse {
    user: MyOrdersUser,
    orders: Vec<MyOrderItem>,
    summary: MyOrdersSummary,
    page: i64,
    page_size: i64,
    total_pages: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MyOrdersUser {
    id: i64,
    username: Option<String>,
    email: Option<String>,
    display_name: String,
    balance: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MyOrderItem {
    id: String,
    amount: f64,
    status: String,
    payment_type: String,
    created_at: String,
    order_type: String,
    can_refund_request: bool,
    refund_requested_at: Option<String>,
    refund_request_reason: Option<String>,
    refund_amount: Option<f64>,
    payment_success: bool,
    recharge_success: bool,
    recharge_status: String,
    recharge_retryable: bool,
}

#[derive(Debug, Serialize)]
struct MyOrdersSummary {
    total: i64,
    pending: i64,
    completed: i64,
    failed: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrderStatusResponse {
    id: String,
    status: String,
    expires_at: String,
    payment_success: bool,
    recharge_success: bool,
    recharge_status: String,
    failed_reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct CancelOrderResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

struct SubscriptionOrderContext {
    plan: SubscriptionPlanRecord,
    payment_subject: String,
    amount_cents: i64,
    subscription_days: i64,
}

async fn create_order(
    State(state): State<AppState>,
    Query(query): Query<CreateOrderQuery>,
    Json(body): Json<CreateOrderRequest>,
) -> AppResult<Json<CreateOrderResponse>> {
    let locale = resolve_locale(query.lang.as_deref());

    let token = body.token.trim();
    if token.is_empty() {
        return Err(AppError::unauthorized(message(
            locale,
            "无效的 token，请重新登录",
            "Invalid token, please login again",
        )));
    }

    if !body.amount.is_finite() || body.amount <= 0.0 {
        return Err(AppError::bad_request(message(
            locale,
            "参数错误",
            "Invalid parameters",
        )));
    }

    let order_type = body.order_type.unwrap_or_else(|| "balance".to_string());
    if order_type != "balance" && order_type != "subscription" {
        return Err(AppError::bad_request(message(
            locale,
            "参数错误",
            "Invalid parameters",
        )));
    }

    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| AppError::internal(anyhow::anyhow!("PLATFORM_BASE_URL is not configured")))?;
    let token_user = platform
        .get_current_user_by_token(token)
        .await
        .map_err(|error| {
            let error_message = error.to_string();
            if error_message.starts_with("Failed to get current user:") {
                AppError::unauthorized(message(
                    locale,
                    "无效的 token，请重新登录",
                    "Invalid token, please login again",
                ))
            } else {
                AppError::internal(error)
            }
        })?;

    let payment_config = payment::resolve_user_payment_config(&state)
        .await
        .map_err(AppError::internal)?;
    if !payment_config
        .enabled_payment_types
        .iter()
        .any(|item| item == &body.payment_type)
    {
        return Err(AppError::bad_request(format!(
            "{}: {}",
            message(locale, "不支持的支付方式", "Unsupported payment type"),
            body.payment_type
        )));
    }

    let method_limit = payment_config
        .method_limits
        .get(&body.payment_type)
        .ok_or_else(|| {
            AppError::bad_request(format!(
                "{}: {}",
                message(locale, "不支持的支付方式", "Unsupported payment type"),
                body.payment_type
            ))
        })?;
    if !method_limit.available {
        return Err(AppError::bad_request(message(
            locale,
            "当前支付方式暂不可用",
            "Selected payment type is temporarily unavailable",
        )));
    }

    let subscription_context = if order_type == "subscription" {
        Some(resolve_subscription_order(&state, locale, body.plan_id.as_deref(), platform).await?)
    } else {
        None
    };
    let amount_cents = subscription_context
        .as_ref()
        .map(|context| context.amount_cents)
        .unwrap_or_else(|| amount_to_cents(body.amount));
    let effective_amount = cents_to_amount(amount_cents);

    if effective_amount < method_limit.single_min {
        return Err(AppError::bad_request(format!(
            "{} {:.2}",
            message(
                locale,
                "充值金额低于该支付方式最小限制",
                "Amount is below the minimum for this payment type",
            ),
            method_limit.single_min
        )));
    }
    if method_limit.single_max > 0.0 && effective_amount > method_limit.single_max {
        return Err(AppError::bad_request(format!(
            "{} {:.2}",
            message(
                locale,
                "充值金额超过该支付方式上限",
                "Amount exceeds the maximum for this payment type",
            ),
            method_limit.single_max
        )));
    }

    let payment_selection =
        payment::resolve_payment_selection(&state, &body.payment_type, amount_cents)
            .await
            .map_err(|error| AppError::bad_request(error.to_string()))?;
    let order = state
        .order_service
        .create_pending_order(CreatePendingOrderInput {
            user_id: token_user.id,
            amount_cents,
            pay_amount_cents: Some(payment_selection.pay_amount_cents),
            fee_rate_bps: Some(payment_selection.fee_rate_bps),
            payment_type: body.payment_type.clone(),
            order_type: order_type.clone(),
            plan_id: subscription_context
                .as_ref()
                .map(|context| context.plan.id.clone()),
            subscription_group_id: subscription_context
                .as_ref()
                .and_then(|context| context.plan.group_id),
            subscription_days: subscription_context
                .as_ref()
                .map(|context| context.subscription_days),
            provider_instance_id: payment_selection.provider_instance_id,
        })
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?;

    let subject = subscription_context
        .as_ref()
        .map(|context| context.payment_subject.clone())
        .unwrap_or_else(|| {
            format!(
                "OPay {:.2} CNY",
                cents_to_amount(order.pay_amount_cents.unwrap_or(order.amount_cents))
            )
        });
    let payment_result = match payment_provider::create_payment_for_order(
        &state,
        &CreatePaymentRequest {
            order_id: order.id.clone(),
            amount_cents: order.pay_amount_cents.unwrap_or(order.amount_cents),
            payment_type: order.payment_type.clone(),
            subject,
            client_ip: None,
            is_mobile: body.is_mobile.unwrap_or(false),
            provider_instance_id: order.provider_instance_id.clone(),
        },
    )
    .await
    {
        Ok(payment_result) => payment_result,
        Err(error) => {
            let _ = state.order_service.delete_order(&order.id).await;
            return Err(AppError::bad_request(format!(
                "{}: {}",
                message(
                    locale,
                    "支付渠道暂不可用",
                    "Payment method is temporarily unavailable"
                ),
                error
            )));
        }
    };

    state
        .order_service
        .attach_payment_creation(&order.id, &payment_result)
        .await
        .map_err(AppError::internal)?;

    Ok(Json(CreateOrderResponse {
        order_id: order.id,
        amount: cents_to_amount(order.amount_cents),
        pay_amount: cents_to_amount(order.pay_amount_cents.unwrap_or(order.amount_cents)),
        fee_rate: payment_selection.fee_rate,
        status: order.status,
        payment_type: order.payment_type,
        expires_at: order.expires_at,
        pay_url: payment_result.pay_url,
        qr_code: payment_result.qr_code,
        client_secret: payment_result.client_secret,
    }))
}

async fn list_my_orders(
    State(state): State<AppState>,
    Query(query): Query<MyOrdersQuery>,
) -> AppResult<Json<MyOrdersResponse>> {
    let locale = resolve_locale(query.lang.as_deref());
    let token = query
        .token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::bad_request(if locale == "en" {
                "token is required"
            } else {
                "缺少 token 参数"
            })
        })?;

    let page = query.page.unwrap_or(1).max(1);
    let raw_page_size = query.page_size.unwrap_or(20);
    let page_size = if VALID_MY_ORDER_PAGE_SIZES.contains(&raw_page_size) {
        raw_page_size
    } else {
        20
    };

    let platform = state.platform.as_ref().ok_or_else(|| {
        AppError::public_internal(message(locale, "获取订单失败", "Failed to load orders"))
    })?;
    let token_user = platform
        .get_current_user_by_token(token)
        .await
        .map_err(|error| {
            let error_message = error.to_string();
            if error_message.starts_with("Failed to get current user:") {
                AppError::unauthorized(if locale == "en" {
                    "unauthorized"
                } else {
                    "未授权"
                })
            } else {
                AppError::public_internal(message(locale, "获取订单失败", "Failed to load orders"))
            }
        })?;

    let order_repo = OrderRepository::new(state.db.clone());
    let provider_repo = ProviderInstanceRepository::new(state.db.clone());
    let offset = (page - 1) * page_size;
    let (orders, total, status_counts, provider_instances) = tokio::try_join!(
        order_repo.list_by_user(token_user.id, offset, page_size),
        order_repo.count_by_user_total(token_user.id),
        order_repo.count_statuses_by_user(token_user.id),
        provider_repo.list(None),
    )
    .map_err(AppError::internal)?;

    let instance_ids: HashSet<String> = orders
        .iter()
        .filter_map(|order| order.provider_instance_id.clone())
        .collect();
    let refund_enabled_map: HashMap<String, bool> = provider_instances
        .into_iter()
        .filter(|item| instance_ids.contains(&item.id))
        .map(|item| (item.id, item.refund_enabled))
        .collect();
    let status_map: HashMap<String, i64> = status_counts
        .into_iter()
        .map(|item| (item.status, item.count))
        .collect();

    let items = orders
        .into_iter()
        .map(|order| {
            let derived = derive_order_state(&order);
            let recharge_retryable = is_recharge_retryable(&order);
            let instance_refund_enabled = order
                .provider_instance_id
                .as_ref()
                .and_then(|id| refund_enabled_map.get(id))
                .copied()
                .unwrap_or(false);

            MyOrderItem {
                id: order.id,
                amount: cents_to_amount(order.amount_cents),
                status: order.status.clone(),
                payment_type: order.payment_type,
                created_at: timestamp_to_rfc3339(order.created_at),
                order_type: order.order_type.clone(),
                can_refund_request: order.order_type == "balance"
                    && order.status == "COMPLETED"
                    && instance_refund_enabled,
                refund_requested_at: optional_timestamp_to_rfc3339(order.refund_requested_at),
                refund_request_reason: order.refund_request_reason,
                refund_amount: order.refund_amount_cents.map(cents_to_amount),
                payment_success: derived.payment_success,
                recharge_success: derived.recharge_success,
                recharge_status: recharge_status_text(derived.recharge_status).to_string(),
                recharge_retryable,
            }
        })
        .collect();

    Ok(Json(MyOrdersResponse {
        user: MyOrdersUser {
            id: token_user.id,
            username: token_user.username.clone(),
            email: token_user.email.clone(),
            display_name: token_user
                .username
                .clone()
                .or(token_user.email.clone())
                .unwrap_or_else(|| format!("User #{}", token_user.id)),
            balance: token_user.balance.unwrap_or(0.0),
        },
        orders: items,
        summary: MyOrdersSummary {
            total,
            pending: *status_map.get("PENDING").unwrap_or(&0),
            completed: status_map.get("COMPLETED").copied().unwrap_or(0)
                + status_map.get("PAID").copied().unwrap_or(0)
                + status_map.get("RECHARGING").copied().unwrap_or(0),
            failed: status_map.get("FAILED").copied().unwrap_or(0)
                + status_map.get("CANCELLED").copied().unwrap_or(0)
                + status_map.get("EXPIRED").copied().unwrap_or(0),
        },
        page,
        page_size,
        total_pages: total_pages(total, page_size),
    }))
}

async fn get_order_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<OrderStatusQuery>,
) -> AppResult<Json<OrderStatusResponse>> {
    let locale = resolve_locale(query.lang.as_deref());
    let access_ok = verify_order_status_access_token(
        state.config.admin_token.as_deref(),
        &id,
        query.access_token.as_deref(),
    );
    let admin_ok = verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await
    .is_ok();

    if !access_ok && !admin_ok {
        return Err(AppError::unauthorized(message(
            locale,
            "未授权访问该订单状态",
            "Unauthorized to access this order status",
        )));
    }

    let order = OrderRepository::new(state.db.clone())
        .get_by_id(&id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found(message(locale, "订单不存在", "Order not found")))?;
    let derived = derive_order_state(&order);

    Ok(Json(OrderStatusResponse {
        id: order.id,
        status: order.status,
        expires_at: timestamp_to_rfc3339(order.expires_at),
        payment_success: derived.payment_success,
        recharge_success: derived.recharge_success,
        recharge_status: recharge_status_text(derived.recharge_status).to_string(),
        failed_reason: order.failed_reason,
    }))
}

async fn cancel_order(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CancelOrderRequest>,
) -> AppResult<Json<CancelOrderResponse>> {
    let token = body
        .token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::bad_request("缺少 token 参数"))?;

    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| AppError::public_internal("取消订单失败"))?;
    let token_user = platform
        .get_current_user_by_token(token)
        .await
        .map_err(|error| {
            if error.to_string().starts_with("Failed to get current user:") {
                AppError::unauthorized("登录态已失效，无法取消订单")
            } else {
                AppError::public_internal("取消订单失败")
            }
        })?;

    match state
        .order_service
        .cancel_order(&id, token_user.id)
        .await
        .map_err(map_cancel_order_error)?
    {
        CancelOrderOutcome::Cancelled => Ok(Json(CancelOrderResponse {
            success: true,
            status: None,
            message: None,
        })),
        CancelOrderOutcome::AlreadyPaid => Ok(Json(CancelOrderResponse {
            success: true,
            status: Some("PAID".to_string()),
            message: Some("订单已支付完成".to_string()),
        })),
    }
}

async fn request_refund(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<RefundRequestQuery>,
    Json(body): Json<serde_json::Value>,
) -> AppResult<Json<RefundRequestResponse>> {
    let locale = resolve_locale(query.lang.as_deref());
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

    let body = body
        .as_object()
        .ok_or_else(|| AppError::bad_request(message(locale, "参数错误", "Invalid parameters")))?;
    let amount = body
        .get("amount")
        .and_then(serde_json::Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| AppError::bad_request(message(locale, "参数错误", "Invalid parameters")))?;
    let reason = match body.get("reason") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(value)) => Some(value.clone()),
        _ => {
            return Err(AppError::bad_request(message(
                locale,
                "参数错误",
                "Invalid parameters",
            )));
        }
    };

    let platform = state.platform.as_ref().ok_or_else(|| {
        AppError::public_internal(message(locale, "退款申请失败", "Refund request failed"))
    })?;
    let token_user = platform
        .get_current_user_by_token(token)
        .await
        .map_err(|error| {
            let error_message = error.to_string();
            if error_message.starts_with("Failed to get current user:") {
                AppError::unauthorized(message(locale, "无效的 token", "Invalid token"))
            } else {
                AppError::public_internal(message(locale, "退款申请失败", "Refund request failed"))
            }
        })?;

    state
        .order_service
        .request_refund(crate::order::service::RequestRefundInput {
            order_id: id,
            user_id: token_user.id,
            amount_cents: amount_to_cents(amount),
            reason,
        })
        .await
        .map_err(|error| map_request_refund_error(error, locale))?;

    Ok(Json(RefundRequestResponse { success: true }))
}

async fn resolve_subscription_order(
    state: &AppState,
    locale: &str,
    plan_id: Option<&str>,
    platform: &crate::platform::PlatformClient,
) -> AppResult<SubscriptionOrderContext> {
    let plan_id = plan_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::bad_request(message(
                locale,
                "订阅订单必须指定套餐",
                "Subscription order requires a plan",
            ))
        })?;

    let repo = SubscriptionPlanRepository::new(state.db.clone());
    let plan = repo
        .get_by_id(plan_id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| {
            AppError::not_found(message(
                locale,
                "该套餐不存在或未上架",
                "Plan not found or not for sale",
            ))
        })?;

    if !plan.for_sale {
        return Err(AppError::not_found(message(
            locale,
            "该套餐不存在或未上架",
            "Plan not found or not for sale",
        )));
    }

    let group_id = plan.group_id.ok_or_else(|| {
        AppError::bad_request(message(
            locale,
            "该套餐尚未绑定分组，无法购买",
            "Plan is not bound to a group",
        ))
    })?;

    let admin_api_key = state
        .system_config
        .get("PLATFORM_ADMIN_API_KEY")
        .await
        .map_err(AppError::internal)?
        .unwrap_or_default();
    let admin_api_key = admin_api_key.trim().to_string();
    if admin_api_key.is_empty() {
        return Err(AppError::internal(anyhow::anyhow!(
            "PLATFORM_ADMIN_API_KEY is not configured"
        )));
    }

    let group = platform
        .get_group(group_id, &admin_api_key)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| {
            AppError::bad_request(message(
                locale,
                "订阅分组已下架，无法购买",
                "Subscription group is no longer available",
            ))
        })?;

    if group.status != "active" {
        return Err(AppError::bad_request(message(
            locale,
            "订阅分组已下架，无法购买",
            "Subscription group is no longer available",
        )));
    }

    if group.subscription_type.as_deref() != Some("subscription") {
        return Err(AppError::bad_request(message(
            locale,
            "该分组不是订阅类型，无法购买订阅",
            "This group is not a subscription type",
        )));
    }

    let subscription_days = compute_validity_days(plan.validity_days, &plan.validity_unit, None)
        .map_err(|error| {
            AppError::bad_request(format!(
                "{}: {}",
                message(
                    locale,
                    "订阅套餐配置无效",
                    "Invalid subscription plan configuration"
                ),
                error
            ))
        })?;
    let payment_subject = plan.product_name.clone().unwrap_or_else(|| {
        format!(
            "OPay 订阅 {}",
            if group.name.is_empty() {
                plan.name.clone()
            } else {
                group.name.clone()
            }
        )
    });

    Ok(SubscriptionOrderContext {
        amount_cents: plan.price_cents,
        payment_subject,
        plan,
        subscription_days,
    })
}

fn map_request_refund_error(error: anyhow::Error, locale: &str) -> AppError {
    match error.to_string().as_str() {
        "order not found" => AppError::not_found(message(locale, "订单不存在", "Order not found")),
        "forbidden" => AppError::forbidden(message(locale, "无权申请该订单退款", "Forbidden")),
        "only balance orders support refund request" => AppError::bad_request(message(
            locale,
            "仅余额充值订单支持退款申请",
            "Only balance orders can request refund",
        )),
        "only completed orders can request refund" => AppError::bad_request(message(
            locale,
            "仅已完成订单可申请退款",
            "Only completed orders can request refund",
        )),
        "refund amount must be positive" => AppError::bad_request(message(
            locale,
            "退款金额必须大于 0",
            "Refund amount must be greater than 0",
        )),
        "refund amount exceeds recharge amount" => AppError::bad_request(message(
            locale,
            "退款金额不能超过充值金额",
            "Refund amount cannot exceed recharge amount",
        )),
        "refund amount exceeds current balance" => AppError::bad_request(message(
            locale,
            "退款金额不能超过当前余额",
            "Refund amount cannot exceed current balance",
        )),
        "order status changed, refresh and retry" => AppError::conflict(message(
            locale,
            "订单状态已变更，请刷新后重试",
            "Order status changed, refresh and retry",
        )),
        _ => AppError::public_internal(message(locale, "退款申请失败", "Refund request failed")),
    }
}

fn map_cancel_order_error(error: anyhow::Error) -> AppError {
    match error.to_string().as_str() {
        "order not found" => AppError::not_found("订单不存在"),
        "forbidden" => AppError::forbidden("无权操作该订单"),
        "order cannot be cancelled" => AppError::bad_request("订单当前状态不可取消"),
        _ => AppError::public_internal("取消订单失败"),
    }
}

fn recharge_status_text(status: RechargeStatus) -> &'static str {
    match status {
        RechargeStatus::NotPaid => "not_paid",
        RechargeStatus::PaidPending => "paid_pending",
        RechargeStatus::Recharging => "recharging",
        RechargeStatus::Success => "success",
        RechargeStatus::Failed => "failed",
        RechargeStatus::Closed => "closed",
    }
}

fn verify_order_status_access_token(
    admin_token: Option<&str>,
    order_id: &str,
    token: Option<&str>,
) -> bool {
    let Some(token) = token.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let Some(admin_token) = admin_token else {
        return false;
    };

    let parts = token.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return false;
    }

    let expires_at_ms = match parts[0].parse::<i64>() {
        Ok(value) if value.is_positive() => value,
        _ => return false,
    };
    let user_id = match parts[1].parse::<i64>() {
        Ok(value) if value >= 0 => value,
        _ => return false,
    };

    if now_timestamp_ms() > expires_at_ms {
        return false;
    }

    let Some(expected) =
        build_order_status_signature(admin_token, order_id, user_id, expires_at_ms)
    else {
        return false;
    };

    secure_equals(&expected, parts[2])
}

fn build_order_status_signature(
    admin_token: &str,
    order_id: &str,
    user_id: i64,
    expires_at_ms: i64,
) -> Option<String> {
    let mut derive_mac = SimpleHmac::<Sha256>::new_from_slice(admin_token.as_bytes()).ok()?;
    derive_mac.update(b"order-status-access-key");
    let derived_key = derive_mac.finalize().into_bytes();

    let mut mac = SimpleHmac::<Sha256>::new_from_slice(derived_key.as_slice()).ok()?;
    mac.update(
        format!(
            "{}:{}:{}:{}",
            ORDER_STATUS_ACCESS_PURPOSE, order_id, user_id, expires_at_ms
        )
        .as_bytes(),
    );
    Some(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

fn secure_equals(expected: &str, received: &str) -> bool {
    let expected = expected.as_bytes();
    let received = received.as_bytes();
    let max_len = expected.len().max(received.len());
    let mut diff = expected.len() ^ received.len();

    for index in 0..max_len {
        let left = *expected.get(index).unwrap_or(&0);
        let right = *received.get(index).unwrap_or(&0);
        diff |= (left ^ right) as usize;
    }

    diff == 0
}

fn now_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock drifted before unix epoch")
        .as_millis() as i64
}

fn total_pages(total: i64, page_size: i64) -> i64 {
    if total <= 0 || page_size <= 0 {
        0
    } else {
        (total + page_size - 1) / page_size
    }
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
        platform::PlatformClient,
        subscription_plan::SubscriptionPlanRepository,
        system_config::{SystemConfigService, UpsertSystemConfig},
    };

    #[derive(Clone)]
    struct MockUser {
        id: i64,
        balance: f64,
    }

    async fn test_state(platform_base_url: Option<String>) -> AppState {
        test_state_with_payment_providers(platform_base_url, Vec::new()).await
    }

    async fn test_state_with_payment_providers(
        platform_base_url: Option<String>,
        payment_providers: Vec<String>,
    ) -> AppState {
        let db_path = std::env::temp_dir().join(format!(
            "opay-refund-request-route-{}.db",
            Uuid::new_v4()
        ));
        let db = DatabaseHandle::open_local(&db_path).await.unwrap();
        db.run_migrations().await.unwrap();

        let config = Arc::new(AppConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            db_path,
            payment_providers,
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

    async fn start_mock_platform(user: MockUser) -> (String, JoinHandle<()>) {
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

        async fn admin_user(
            State(user): State<MockUser>,
            Path(requested_id): Path<i64>,
        ) -> Json<serde_json::Value> {
            assert_eq!(requested_id, user.id);
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
            .route("/api/v1/auth/me", get(auth_me))
            .route("/api/v1/admin/users/{id}", get(admin_user))
            .with_state(user);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn create_order_rejects_direct_payment_type_in_rust_mvp() {
        let (base_url, handle) = start_mock_platform(MockUser {
            id: 3,
            balance: 50.0,
        })
        .await;
        let state = test_state_with_payment_providers(
            Some(base_url),
            vec![
                "easypay".to_string(),
                "alipay".to_string(),
                "wxpay".to_string(),
                "stripe".to_string(),
            ],
        )
        .await;

        let error = create_order(
            State(state),
            Query(CreateOrderQuery { lang: None }),
            Json(CreateOrderRequest {
                token: "user-token".to_string(),
                amount: 12.34,
                payment_type: "alipay_direct".to_string(),
                src_host: None,
                src_url: None,
                is_mobile: Some(false),
                order_type: None,
                plan_id: None,
            }),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("不支持的支付方式"));
        assert!(error.to_string().contains("alipay_direct"));

        handle.abort();
    }

    #[tokio::test]
    async fn refund_request_marks_order_as_requested() {
        let (base_url, handle) = start_mock_platform(MockUser {
            id: 21,
            balance: 99.0,
        })
        .await;
        let state = test_state(Some(base_url)).await;
        let orders = OrderRepository::new(state.db.clone());

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

        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 21,
                amount_cents: 800,
                pay_amount_cents: Some(800),
                fee_rate_bps: Some(0),
                status: "COMPLETED".to_string(),
                payment_type: "easypay".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        let response = request_refund(
            State(state.clone()),
            axum::extract::Path(order.id.clone()),
            Query(RefundRequestQuery {
                token: Some("user-token".to_string()),
                lang: None,
            }),
            Json(json!({
                "amount": 6.5,
                "reason": "user wants refund"
            })),
        )
        .await
        .unwrap()
        .0;

        assert!(response.success);

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "REFUND_REQUESTED");
        assert_eq!(saved.refund_amount_cents, Some(650));
        assert_eq!(
            saved.refund_request_reason.as_deref(),
            Some("user wants refund")
        );

        handle.abort();
    }

    #[tokio::test]
    async fn my_orders_returns_refund_capability_and_summary() {
        let (base_url, handle) = start_mock_platform(MockUser {
            id: 1,
            balance: 100.0,
        })
        .await;
        let state = test_state(Some(base_url)).await;
        let orders = OrderRepository::new(state.db.clone());
        let providers = ProviderInstanceRepository::new(state.db.clone());

        let instance = providers
            .create(ProviderInstanceWrite {
                provider_key: "stripe".to_string(),
                name: "Stripe #1".to_string(),
                config: crypto::encrypt(state.config.admin_token.as_deref(), "{}").unwrap(),
                supported_types: "stripe".to_string(),
                enabled: true,
                sort_order: 0,
                limits: None,
                refund_enabled: true,
            })
            .await
            .unwrap();

        orders
            .insert_pending(NewPendingOrder {
                user_id: 1,
                amount_cents: 800,
                pay_amount_cents: Some(880),
                fee_rate_bps: Some(1000),
                status: "COMPLETED".to_string(),
                payment_type: "stripe".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: Some(instance.id),
                expires_at: 1000,
            })
            .await
            .unwrap();

        orders
            .insert_pending(NewPendingOrder {
                user_id: 1,
                amount_cents: 500,
                pay_amount_cents: Some(500),
                fee_rate_bps: Some(0),
                status: "PENDING".to_string(),
                payment_type: "alipay".to_string(),
                order_type: "subscription".to_string(),
                plan_id: Some("plan_sub".to_string()),
                subscription_group_id: Some(9),
                subscription_days: Some(30),
                provider_instance_id: None,
                expires_at: 2000,
            })
            .await
            .unwrap();

        let response = list_my_orders(
            State(state),
            Query(MyOrdersQuery {
                token: Some("user-token".to_string()),
                lang: None,
                page: Some(1),
                page_size: Some(20),
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(response.summary.total, 2);
        assert_eq!(response.summary.pending, 1);
        assert_eq!(response.summary.completed, 1);
        assert_eq!(response.orders.len(), 2);
        assert!(response.orders.iter().any(|item| item.can_refund_request));
        assert_eq!(response.user.display_name, "test-user");

        handle.abort();
    }

    #[tokio::test]
    async fn order_status_allows_access_token_polling() {
        let state = test_state(Some("http://127.0.0.1:9".to_string())).await;
        let orders = OrderRepository::new(state.db.clone());

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
                expires_at: 4_102_444_800,
            })
            .await
            .unwrap();

        let expires_at_ms = now_timestamp_ms() + 60_000;
        let signature = build_order_status_signature(
            state.config.admin_token.as_deref().unwrap(),
            &order.id,
            8,
            expires_at_ms,
        )
        .unwrap();
        let access_token = format!("{expires_at_ms}.8.{signature}");

        let response = get_order_status(
            State(state),
            HeaderMap::new(),
            Path(order.id),
            Query(OrderStatusQuery {
                token: None,
                lang: None,
                access_token: Some(access_token),
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(response.status, "PENDING");
        assert!(!response.payment_success);
        assert_eq!(response.recharge_status, "not_paid");
    }

    #[tokio::test]
    async fn cancel_order_route_cancels_pending_user_order() {
        let (base_url, handle) = start_mock_platform(MockUser {
            id: 21,
            balance: 99.0,
        })
        .await;
        let state = test_state(Some(base_url)).await;
        let orders = OrderRepository::new(state.db.clone());

        let order = orders
            .insert_pending(NewPendingOrder {
                user_id: 21,
                amount_cents: 800,
                pay_amount_cents: Some(800),
                fee_rate_bps: Some(0),
                status: "PENDING".to_string(),
                payment_type: "easypay".to_string(),
                order_type: "balance".to_string(),
                plan_id: None,
                subscription_group_id: None,
                subscription_days: None,
                provider_instance_id: None,
                expires_at: 1000,
            })
            .await
            .unwrap();

        let response = cancel_order(
            State(state.clone()),
            Path(order.id.clone()),
            Json(CancelOrderRequest {
                token: Some("user-token".to_string()),
            }),
        )
        .await
        .unwrap()
        .0;

        assert!(response.success);

        let saved = orders.get_by_id(&order.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "CANCELLED");

        handle.abort();
    }

    #[tokio::test]
    async fn refund_request_requires_token() {
        let state = test_state(Some("http://127.0.0.1:9".to_string())).await;

        let error = request_refund(
            State(state),
            axum::extract::Path("order-1".to_string()),
            Query(RefundRequestQuery {
                token: None,
                lang: None,
            }),
            Json(json!({
                "amount": 1.0
            })),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, AppError::Unauthorized(_)));
    }
}
