pub mod channels;
pub mod config;
pub mod dashboard;
pub mod env_defaults;
pub mod orders;
pub mod provider_instances;
pub mod refund;
pub mod sub2api;
pub mod subscription_plans;
pub mod subscriptions;
pub mod user_balance;

use axum::Router;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(channels::router())
        .merge(config::router())
        .merge(dashboard::router())
        .merge(env_defaults::router())
        .merge(orders::router())
        .merge(provider_instances::router())
        .merge(refund::router())
        .merge(subscription_plans::router())
        .merge(sub2api::router())
        .merge(subscriptions::router())
        .merge(user_balance::router())
}
