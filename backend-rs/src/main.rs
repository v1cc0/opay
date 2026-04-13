mod admin_auth;
mod channel;
mod config;
mod crypto;
mod db;
mod error;
mod http;
mod order;
mod payment;
mod payment_provider;
mod provider_instances;
mod response;
mod sub2api;
mod subscription_plan;
mod system_config;

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use axum::serve;
use db::DatabaseHandle;
use order::{
    audit::AuditLogRepository, repository::OrderRepository, service::OrderService,
    timeout::start_timeout_scheduler,
};
use sub2api::Sub2ApiClient;
use subscription_plan::SubscriptionPlanRepository;
use system_config::SystemConfigService;
use tokio::{net::TcpListener, signal};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::AppConfig;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub db: DatabaseHandle,
    pub system_config: SystemConfigService,
    pub sub2api: Option<Sub2ApiClient>,
    pub order_service: OrderService,
}

#[tokio::main]
async fn main() -> Result<()> {
    let runtime_config = config::RuntimeConfig::load()?;
    runtime_config.apply_process_env();
    init_tracing();

    let config = Arc::new(runtime_config.app);
    let ignored_providers = payment::ignored_provider_keys_for_rust_mvp(&config.payment_providers);
    if !ignored_providers.is_empty() {
        warn!(
            ignored_providers = ?ignored_providers,
            "Rust MVP-A ignores unsupported payment providers; only easypay and stripe are active"
        );
    }
    let db = DatabaseHandle::open_local(&config.db_path).await?;
    db.run_migrations().await?;

    let state = AppState {
        config: Arc::clone(&config),
        db: db.clone(),
        system_config: SystemConfigService::new(
            db.clone(),
            Duration::from_secs(config.system_config_cache_ttl_secs),
        ),
        sub2api: config
            .sub2api_base_url
            .clone()
            .map(|base_url| Sub2ApiClient::new(base_url, config.sub2api_timeout_secs)),
        order_service: OrderService::new(
            Arc::clone(&config),
            OrderRepository::new(db.clone()),
            AuditLogRepository::new(db.clone()),
            SubscriptionPlanRepository::new(db.clone()),
            SystemConfigService::new(
                db.clone(),
                Duration::from_secs(config.system_config_cache_ttl_secs),
            ),
            config
                .sub2api_base_url
                .clone()
                .map(|base_url| Sub2ApiClient::new(base_url, config.sub2api_timeout_secs)),
        ),
    };

    let _timeout_task = start_timeout_scheduler(state.order_service.clone());

    let app = http::router(state).layer(TraceLayer::new_for_http());
    let addr = config.socket_addr()?;
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {}", addr))?;

    info!(bind = %addr, db_path = %config.db_path.display(), "opay-rs listening");

    serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum server exited with error")?;

    Ok(())
}

fn init_tracing() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,tower_http=info".into());

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        terminate.recv().await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }

    info!("shutdown signal received");
}
