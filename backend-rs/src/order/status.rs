#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    Pending,
    Paid,
    Recharging,
    Completed,
    Expired,
    Cancelled,
    Failed,
    RefundRequested,
    Refunding,
    PartiallyRefunded,
    Refunded,
    RefundFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RechargeStatus {
    NotPaid,
    PaidPending,
    Recharging,
    Success,
    Failed,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedOrderState {
    pub payment_success: bool,
    pub recharge_success: bool,
    pub recharge_status: RechargeStatus,
}

pub trait OrderStatusLike {
    fn status(&self) -> &str;
    fn paid_at(&self) -> Option<i64>;
    fn completed_at(&self) -> Option<i64>;
}

pub fn is_refund_status(status: &str) -> bool {
    matches!(
        status,
        "REFUND_REQUESTED" | "REFUNDING" | "PARTIALLY_REFUNDED" | "REFUNDED" | "REFUND_FAILED"
    )
}

pub fn is_recharge_retryable<T: OrderStatusLike>(order: &T) -> bool {
    order.paid_at().is_some() && order.status() == "FAILED" && !is_refund_status(order.status())
}

pub fn derive_order_state<T: OrderStatusLike>(order: &T) -> DerivedOrderState {
    let payment_success = order.paid_at().is_some();
    let recharge_success = order.completed_at().is_some() || order.status() == "COMPLETED";

    if recharge_success {
        return DerivedOrderState {
            payment_success,
            recharge_success: true,
            recharge_status: RechargeStatus::Success,
        };
    }

    if order.status() == "RECHARGING" {
        return DerivedOrderState {
            payment_success,
            recharge_success: false,
            recharge_status: RechargeStatus::Recharging,
        };
    }

    if order.status() == "FAILED" {
        return DerivedOrderState {
            payment_success,
            recharge_success: false,
            recharge_status: RechargeStatus::Failed,
        };
    }

    if matches!(
        order.status(),
        "EXPIRED"
            | "CANCELLED"
            | "REFUND_REQUESTED"
            | "REFUNDING"
            | "PARTIALLY_REFUNDED"
            | "REFUNDED"
            | "REFUND_FAILED"
    ) {
        return DerivedOrderState {
            payment_success,
            recharge_success: false,
            recharge_status: RechargeStatus::Closed,
        };
    }

    if payment_success {
        return DerivedOrderState {
            payment_success,
            recharge_success: false,
            recharge_status: RechargeStatus::PaidPending,
        };
    }

    DerivedOrderState {
        payment_success: false,
        recharge_success: false,
        recharge_status: RechargeStatus::NotPaid,
    }
}

impl OrderStatusLike for crate::order::repository::OrderRecord {
    fn status(&self) -> &str {
        &self.status
    }

    fn paid_at(&self) -> Option<i64> {
        self.paid_at
    }

    fn completed_at(&self) -> Option<i64> {
        self.completed_at
    }
}
