use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use tokio::sync::RwLock;
use turso::{Value, params::Params};

use crate::db::DatabaseHandle;

#[derive(Debug, Clone, Serialize)]
pub struct SystemConfigEntry {
    pub key: String,
    pub value: String,
    pub group: String,
    pub label: Option<String>,
    pub updated_at: i64,
}

#[derive(Clone)]
pub struct SystemConfigService {
    db: DatabaseHandle,
    ttl: Duration,
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    value: String,
    expires_at: Instant,
}

impl SystemConfigService {
    pub fn new(db: DatabaseHandle, ttl: Duration) -> Self {
        Self {
            db,
            ttl,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        if let Some(value) = self.get_cached(key).await {
            return Ok(Some(value));
        }

        let conn = self.db.connect()?;
        let mut stmt = conn
            .prepare("SELECT value FROM system_configs WHERE key = ?1")
            .await
            .with_context(|| format!("failed to prepare system config lookup for {key}"))?;

        let mut rows = stmt
            .query([key])
            .await
            .with_context(|| format!("failed to execute system config lookup for {key}"))?;

        if let Some(row) = rows
            .next()
            .await
            .with_context(|| format!("failed to fetch system config row for {key}"))?
        {
            let value = read_required_string(&row.get_value(0)?)?;
            self.set_cached(key.to_string(), value.clone()).await;
            Ok(Some(value))
        } else {
            let env_value = std::env::var(key).ok();
            if let Some(value) = &env_value {
                self.set_cached(key.to_string(), value.clone()).await;
            }
            Ok(env_value)
        }
    }

    pub async fn get_many(&self, keys: &[&str]) -> Result<HashMap<String, String>> {
        let mut result = HashMap::new();
        for key in keys {
            if let Some(value) = self.get(key).await? {
                result.insert((*key).to_string(), value);
            }
        }
        Ok(result)
    }

    pub async fn list_all(&self) -> Result<Vec<SystemConfigEntry>> {
        let conn = self.db.connect()?;
        let mut rows = conn
            .query(
                "SELECT key, value, group_name, label, updated_at FROM system_configs ORDER BY group_name ASC, key ASC",
                (),
            )
            .await
            .context("failed to query system configs")?;

        let mut items = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .context("failed to iterate system config rows")?
        {
            items.push(SystemConfigEntry {
                key: read_required_string(&row.get_value(0)?)?,
                value: read_required_string(&row.get_value(1)?)?,
                group: read_required_string(&row.get_value(2)?)?,
                label: read_optional_string(&row.get_value(3)?)?,
                updated_at: read_required_i64(&row.get_value(4)?)?,
            });
        }

        Ok(items)
    }

    pub async fn set_many(&self, items: &[UpsertSystemConfig]) -> Result<()> {
        let tx = self.db.begin_concurrent().await?;

        for item in items {
            let params = Params::Positional(vec![
                Value::Text(item.key.clone()),
                Value::Text(item.value.clone()),
                Value::Text(item.group.clone().unwrap_or_else(|| "general".to_string())),
                item.label.clone().map(Value::Text).unwrap_or(Value::Null),
            ]);

            tx.execute(
                "INSERT INTO system_configs (key, value, group_name, label, updated_at)
                 VALUES (?1, ?2, ?3, ?4, unixepoch())
                 ON CONFLICT(key) DO UPDATE SET
                    value = excluded.value,
                    group_name = excluded.group_name,
                    label = excluded.label,
                    updated_at = unixepoch()",
                params,
            )
            .await
            .with_context(|| format!("failed to upsert system config {}", item.key))?;
        }
        tx.commit().await?;
        for item in items {
            self.invalidate(Some(&item.key)).await;
        }

        Ok(())
    }

    pub async fn invalidate(&self, key: Option<&str>) {
        let mut cache = self.cache.write().await;
        if let Some(key) = key {
            cache.remove(key);
        } else {
            cache.clear();
        }
    }

    async fn get_cached(&self, key: &str) -> Option<String> {
        let mut cache = self.cache.write().await;
        let entry = cache.get(key)?.clone();
        if Instant::now() > entry.expires_at {
            cache.remove(key);
            return None;
        }
        Some(entry.value)
    }

    async fn set_cached(&self, key: String, value: String) {
        let mut cache = self.cache.write().await;
        cache.insert(
            key,
            CacheEntry {
                value,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }
}

#[derive(Debug, Clone)]
pub struct UpsertSystemConfig {
    pub key: String,
    pub value: String,
    pub group: Option<String>,
    pub label: Option<String>,
}

fn read_required_string(value: &Value) -> Result<String> {
    match value {
        Value::Text(value) => Ok(value.clone()),
        Value::Integer(value) => Ok(value.to_string()),
        Value::Real(value) => Ok(value.to_string()),
        Value::Null => Err(anyhow!("unexpected NULL value")),
        Value::Blob(_) => Err(anyhow!("unexpected BLOB value")),
    }
}

fn read_optional_string(value: &Value) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        _ => read_required_string(value).map(Some),
    }
}

fn read_required_i64(value: &Value) -> Result<i64> {
    match value {
        Value::Integer(value) => Ok(*value),
        Value::Text(value) => value
            .parse::<i64>()
            .context("failed to parse integer from text"),
        _ => Err(anyhow!("unexpected non-integer value")),
    }
}
