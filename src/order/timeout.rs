use std::time::Duration;

use tokio::{task::JoinHandle, time::interval};
use tracing::{error, info};

use crate::order::service::OrderService;

const INTERVAL_SECONDS: u64 = 30;
const BATCH_SIZE: i64 = 50;

pub fn start_timeout_scheduler(order_service: OrderService) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(INTERVAL_SECONDS));
        info!(
            interval_seconds = INTERVAL_SECONDS,
            batch_size = BATCH_SIZE,
            "order timeout scheduler started"
        );

        loop {
            ticker.tick().await;
            match order_service.expire_pending_orders(BATCH_SIZE).await {
                Ok(expired) if expired > 0 => {
                    info!(expired, "expired pending orders");
                }
                Ok(_) => {}
                Err(error_value) => {
                    error!(error = ?error_value, "order timeout sweep failed");
                }
            }
        }
    })
}
