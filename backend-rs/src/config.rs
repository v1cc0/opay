use std::{
    collections::HashMap,
    env, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use toml::Value as TomlValue;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub db_path: PathBuf,
    pub payment_providers: Vec<String>,
    pub admin_token: Option<String>,
    pub system_config_cache_ttl_secs: u64,
    pub sub2api_base_url: Option<String>,
    pub sub2api_timeout_secs: u64,
    pub min_recharge_amount: f64,
    pub max_recharge_amount: f64,
    pub max_daily_recharge_amount: f64,
    pub pay_help_image_url: Option<String>,
    pub pay_help_text: Option<String>,
    pub stripe_publishable_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub app: AppConfig,
    env_vars: HashMap<String, String>,
}

impl RuntimeConfig {
    pub fn load() -> Result<Self> {
        let path = default_config_path()?;
        Self::load_from_path(&path)
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let config_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let file_config = toml::from_str::<FileConfig>(&raw)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;

        let mut legacy_env = collect_legacy_env(&file_config, config_dir)?;
        let app = AppConfig::from_file_sections(&file_config.app, &legacy_env, config_dir)?;

        legacy_env.insert("APP_HOST".to_string(), app.host.clone());
        legacy_env.insert("APP_PORT".to_string(), app.port.to_string());
        legacy_env.insert(
            "TURSO_DB_PATH".to_string(),
            app.db_path.to_string_lossy().to_string(),
        );
        legacy_env.insert(
            "PAYMENT_PROVIDERS".to_string(),
            app.payment_providers.join(","),
        );
        legacy_env.insert(
            "SYSTEM_CONFIG_CACHE_TTL_SECS".to_string(),
            app.system_config_cache_ttl_secs.to_string(),
        );
        legacy_env.insert(
            "SUB2API_TIMEOUT_SECS".to_string(),
            app.sub2api_timeout_secs.to_string(),
        );
        legacy_env.insert(
            "MIN_RECHARGE_AMOUNT".to_string(),
            app.min_recharge_amount.to_string(),
        );
        legacy_env.insert(
            "MAX_RECHARGE_AMOUNT".to_string(),
            app.max_recharge_amount.to_string(),
        );
        legacy_env.insert(
            "MAX_DAILY_RECHARGE_AMOUNT".to_string(),
            app.max_daily_recharge_amount.to_string(),
        );

        insert_optional_env(&mut legacy_env, "ADMIN_TOKEN", app.admin_token.clone());
        insert_optional_env(
            &mut legacy_env,
            "SUB2API_BASE_URL",
            app.sub2api_base_url.clone(),
        );
        insert_optional_env(
            &mut legacy_env,
            "PAY_HELP_IMAGE_URL",
            app.pay_help_image_url.clone(),
        );
        insert_optional_env(&mut legacy_env, "PAY_HELP_TEXT", app.pay_help_text.clone());
        insert_optional_env(
            &mut legacy_env,
            "STRIPE_PUBLISHABLE_KEY",
            app.stripe_publishable_key.clone(),
        );
        insert_optional_env(
            &mut legacy_env,
            "RUST_LOG",
            file_config
                .runtime
                .rust_log
                .clone()
                .and_then(normalize_optional_string),
        );

        Ok(Self {
            app,
            env_vars: legacy_env,
        })
    }

    pub fn apply_process_env(&self) {
        for (key, value) in &self.env_vars {
            // SAFETY: config loading happens during startup before worker threads are spawned.
            unsafe {
                env::set_var(key, value);
            }
        }
    }
}

impl AppConfig {
    fn from_file_sections(
        app: &FileAppConfig,
        legacy_env: &HashMap<String, String>,
        config_dir: &Path,
    ) -> Result<Self> {
        let host = app
            .host
            .clone()
            .or_else(|| legacy_env.get("APP_HOST").cloned())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "0.0.0.0".to_string());

        let port = app
            .port
            .or_else(|| {
                legacy_env
                    .get("APP_PORT")
                    .and_then(|value| value.parse::<u16>().ok())
            })
            .unwrap_or(8080);

        let db_path_raw = app
            .db_path
            .clone()
            .or_else(|| legacy_env.get("TURSO_DB_PATH").cloned());
        let db_path = db_path_raw
            .as_deref()
            .map(|value| resolve_path(config_dir, value))
            .unwrap_or_else(default_db_path);

        let payment_providers = app
            .payment_providers
            .clone()
            .or_else(|| {
                legacy_env
                    .get("PAYMENT_PROVIDERS")
                    .map(|value| parse_payment_providers(value))
            })
            .unwrap_or_default();

        let admin_token = app
            .admin_token
            .clone()
            .or_else(|| legacy_env.get("ADMIN_TOKEN").cloned())
            .and_then(normalize_optional_string);

        let system_config_cache_ttl_secs = app
            .system_config_cache_ttl_secs
            .or_else(|| {
                legacy_env
                    .get("SYSTEM_CONFIG_CACHE_TTL_SECS")
                    .and_then(|value| value.parse::<u64>().ok())
            })
            .unwrap_or(30);

        let sub2api_base_url = app
            .sub2api_base_url
            .clone()
            .or_else(|| legacy_env.get("SUB2API_BASE_URL").cloned())
            .and_then(normalize_optional_string)
            .map(|value| value.trim_end_matches('/').to_string());

        let sub2api_timeout_secs = app
            .sub2api_timeout_secs
            .or_else(|| {
                legacy_env
                    .get("SUB2API_TIMEOUT_SECS")
                    .and_then(|value| value.parse::<u64>().ok())
            })
            .unwrap_or(10);

        let min_recharge_amount = app
            .min_recharge_amount
            .or_else(|| {
                legacy_env
                    .get("MIN_RECHARGE_AMOUNT")
                    .and_then(|value| value.parse::<f64>().ok())
            })
            .unwrap_or(1.0);

        let max_recharge_amount = app
            .max_recharge_amount
            .or_else(|| {
                legacy_env
                    .get("MAX_RECHARGE_AMOUNT")
                    .and_then(|value| value.parse::<f64>().ok())
            })
            .unwrap_or(1000.0);

        let max_daily_recharge_amount = app
            .max_daily_recharge_amount
            .or_else(|| {
                legacy_env
                    .get("MAX_DAILY_RECHARGE_AMOUNT")
                    .and_then(|value| value.parse::<f64>().ok())
            })
            .unwrap_or(10000.0);

        let pay_help_image_url = app
            .pay_help_image_url
            .clone()
            .or_else(|| legacy_env.get("PAY_HELP_IMAGE_URL").cloned())
            .and_then(normalize_optional_string);

        let pay_help_text = app
            .pay_help_text
            .clone()
            .or_else(|| legacy_env.get("PAY_HELP_TEXT").cloned())
            .and_then(normalize_optional_string);

        let stripe_publishable_key = app
            .stripe_publishable_key
            .clone()
            .or_else(|| legacy_env.get("STRIPE_PUBLISHABLE_KEY").cloned())
            .and_then(normalize_optional_string);

        Ok(Self {
            host,
            port,
            db_path,
            payment_providers,
            admin_token,
            system_config_cache_ttl_secs,
            sub2api_base_url,
            sub2api_timeout_secs,
            min_recharge_amount,
            max_recharge_amount,
            max_daily_recharge_amount,
            pay_help_image_url,
            pay_help_text,
            stripe_publishable_key,
        })
    }

    pub fn socket_addr(&self) -> Result<SocketAddr> {
        format!("{}:{}", self.host, self.port)
            .parse()
            .with_context(|| format!("invalid bind address: {}:{}", self.host, self.port))
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FileConfig {
    #[serde(default)]
    app: FileAppConfig,
    #[serde(default)]
    runtime: FileRuntimeConfig,
    #[serde(default)]
    env: HashMap<String, TomlValue>,
    #[serde(flatten)]
    legacy: HashMap<String, TomlValue>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FileAppConfig {
    host: Option<String>,
    port: Option<u16>,
    db_path: Option<String>,
    payment_providers: Option<Vec<String>>,
    admin_token: Option<String>,
    system_config_cache_ttl_secs: Option<u64>,
    sub2api_base_url: Option<String>,
    sub2api_timeout_secs: Option<u64>,
    min_recharge_amount: Option<f64>,
    max_recharge_amount: Option<f64>,
    max_daily_recharge_amount: Option<f64>,
    pay_help_image_url: Option<String>,
    pay_help_text: Option<String>,
    stripe_publishable_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FileRuntimeConfig {
    rust_log: Option<String>,
}

fn default_config_path() -> Result<PathBuf> {
    let cwd_path = PathBuf::from("config.toml");
    if cwd_path.exists() {
        return Ok(cwd_path);
    }

    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.toml");
    if manifest_path.exists() {
        return Ok(manifest_path);
    }

    bail!(
        "config.toml not found. Expected either ./config.toml or {}. Copy backend-rs/config.example.toml to config.toml first",
        manifest_path.display()
    );
}

fn collect_legacy_env(
    file_config: &FileConfig,
    config_dir: &Path,
) -> Result<HashMap<String, String>> {
    let mut env_vars = HashMap::new();

    for (key, value) in &file_config.legacy {
        if value.is_table() || value.is_array() {
            return Err(anyhow!(
                "root key `{}` must be a scalar value; move nested config into [app], [runtime], or [env]",
                key
            ));
        }

        env_vars.insert(key.clone(), toml_scalar_to_string(key, value, config_dir)?);
    }

    for (key, value) in &file_config.env {
        if value.is_table() || value.is_array() {
            return Err(anyhow!(
                "[env].{} must be a scalar value; arrays/tables are not supported there",
                key
            ));
        }

        env_vars.insert(key.clone(), toml_scalar_to_string(key, value, config_dir)?);
    }

    Ok(env_vars)
}

fn toml_scalar_to_string(key: &str, value: &TomlValue, config_dir: &Path) -> Result<String> {
    let string_value = match value {
        TomlValue::String(value) => maybe_resolve_value_as_path(key, value, config_dir),
        TomlValue::Integer(value) => value.to_string(),
        TomlValue::Float(value) => value.to_string(),
        TomlValue::Boolean(value) => value.to_string(),
        TomlValue::Datetime(value) => value.to_string(),
        other => {
            return Err(anyhow!(
                "key `{}` only supports scalar TOML values, got {}",
                key,
                other.type_str()
            ));
        }
    };

    Ok(string_value)
}

fn maybe_resolve_value_as_path(key: &str, value: &str, config_dir: &Path) -> String {
    if key == "TURSO_DB_PATH" {
        return resolve_path(config_dir, value)
            .to_string_lossy()
            .to_string();
    }

    if !supports_file_path_value(key) {
        return value.to_string();
    }

    let candidate = PathBuf::from(value);
    if candidate.is_absolute() {
        return value.to_string();
    }

    let resolved = config_dir.join(&candidate);
    if resolved.exists() {
        return resolved.to_string_lossy().to_string();
    }

    value.to_string()
}

fn supports_file_path_value(key: &str) -> bool {
    matches!(
        key,
        "ALIPAY_PRIVATE_KEY" | "ALIPAY_PUBLIC_KEY" | "WXPAY_PRIVATE_KEY" | "WXPAY_PUBLIC_KEY"
    )
}

fn resolve_path(base_dir: &Path, value: &str) -> PathBuf {
    let candidate = PathBuf::from(value);
    if candidate.is_absolute() {
        candidate
    } else {
        base_dir.join(candidate)
    }
}

fn default_db_path() -> PathBuf {
    PathBuf::from(format!("{}/data/opay.db", env!("CARGO_MANIFEST_DIR")))
}

fn normalize_optional_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_payment_providers(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .collect()
}

fn insert_optional_env(target: &mut HashMap<String, String>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        target.insert(key.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use uuid::Uuid;

    #[test]
    fn loads_structured_config_toml() {
        let temp_dir =
            std::env::temp_dir().join(format!("opay-rs-config-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();

        let config_path = temp_dir.join("config.toml");
        fs::write(
            &config_path,
            r#"
[app]
host = "127.0.0.1"
port = 9090
db_path = "data/runtime.db"
payment_providers = ["easypay", "stripe"]
admin_token = "secret-token"
system_config_cache_ttl_secs = 99
sub2api_base_url = "https://sub2api.example.com/"
sub2api_timeout_secs = 30
min_recharge_amount = 5.5
max_recharge_amount = 3000
max_daily_recharge_amount = 20000
pay_help_image_url = "https://cdn.example.com/help.png"
pay_help_text = "hello"
stripe_publishable_key = "pk_live_xxx"

[runtime]
rust_log = "debug"

[env]
ORDER_TIMEOUT_MINUTES = 7
"#,
        )
        .unwrap();

        let runtime = RuntimeConfig::load_from_path(&config_path).unwrap();
        let app = runtime.app;

        assert_eq!(app.host, "127.0.0.1");
        assert_eq!(app.port, 9090);
        assert_eq!(app.db_path, temp_dir.join("data/runtime.db"));
        assert_eq!(app.payment_providers, vec!["easypay", "stripe"]);
        assert_eq!(app.admin_token.as_deref(), Some("secret-token"));
        assert_eq!(app.system_config_cache_ttl_secs, 99);
        assert_eq!(
            app.sub2api_base_url.as_deref(),
            Some("https://sub2api.example.com")
        );
        assert_eq!(app.sub2api_timeout_secs, 30);
        assert_eq!(app.min_recharge_amount, 5.5);
        assert_eq!(app.max_recharge_amount, 3000.0);
        assert_eq!(app.max_daily_recharge_amount, 20000.0);
    }

    #[test]
    fn supports_legacy_root_keys_and_relative_secret_paths() {
        let temp_dir =
            std::env::temp_dir().join(format!("opay-rs-config-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();

        let key_path = temp_dir.join("keys/private.pem");
        fs::create_dir_all(key_path.parent().unwrap()).unwrap();
        fs::write(&key_path, "pem-data").unwrap();

        let config_path = temp_dir.join("config.toml");
        fs::write(
            &config_path,
            r#"
APP_HOST = "0.0.0.0"
APP_PORT = 8081
TURSO_DB_PATH = "data/local.db"
PAYMENT_PROVIDERS = "easypay,stripe"
ALIPAY_PRIVATE_KEY = "keys/private.pem"
"#,
        )
        .unwrap();

        let runtime = RuntimeConfig::load_from_path(&config_path).unwrap();

        assert_eq!(runtime.app.port, 8081);
        assert_eq!(runtime.app.db_path, temp_dir.join("data/local.db"));
        assert_eq!(runtime.app.payment_providers, vec!["easypay", "stripe"]);
        assert_eq!(
            runtime
                .env_vars
                .get("ALIPAY_PRIVATE_KEY")
                .map(PathBuf::from),
            Some(key_path)
        );
    }
}
