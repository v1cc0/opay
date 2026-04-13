pub mod admin;
pub mod channels;
pub mod common;
pub mod easy_pay_notify;
pub mod health;
pub mod orders;
pub mod stripe_webhook;
pub mod subscription_plans;
pub mod subscriptions;
pub mod user;

use axum::Router;

use crate::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(admin::router())
        .merge(channels::router())
        .merge(easy_pay_notify::router())
        .merge(health::router())
        .merge(orders::router())
        .merge(stripe_webhook::router())
        .merge(subscription_plans::router())
        .merge(subscriptions::router())
        .merge(user::router())
        .with_state(state)
}
