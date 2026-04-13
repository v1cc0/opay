use anyhow::{Context, Result, anyhow};
use serde_json::Value as JsonValue;
use turso::{Value, params::Params};
use uuid::Uuid;

use crate::db::DatabaseHandle;

const PENDING_STATUSES: &[&str] = &["PENDING", "PAID", "RECHARGING"];

#[derive(Clone)]
pub struct ProviderInstanceRepository {
    db: DatabaseHandle,
}

#[derive(Debug, Clone)]
pub struct ProviderInstanceRecord {
    pub id: String,
    pub provider_key: String,
    pub name: String,
    pub config: String,
    pub supported_types: String,
    pub enabled: bool,
    pub sort_order: i64,
    pub limits: Option<String>,
    pub refund_enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct ProviderInstanceWrite {
    pub provider_key: String,
    pub name: String,
    pub config: String,
    pub supported_types: String,
    pub enabled: bool,
    pub sort_order: i64,
    pub limits: Option<String>,
    pub refund_enabled: bool,
}

impl ProviderInstanceRepository {
    pub fn new(db: DatabaseHandle) -> Self {
        Self { db }
    }

    pub async fn list(&self, provider_key: Option<&str>) -> Result<Vec<ProviderInstanceRecord>> {
        let conn = self.db.connect_readonly().await?;
        let mut rows = if let Some(provider_key) = provider_key {
            conn.query(
                "SELECT id, provider_key, name, config, supported_types, enabled, sort_order, limits, refund_enabled, created_at, updated_at FROM payment_provider_instances WHERE provider_key = ?1 ORDER BY sort_order ASC",
                [provider_key],
            )
            .await
            .with_context(|| format!("failed to list provider instances for {provider_key}"))?
        } else {
            conn.query(
                "SELECT id, provider_key, name, config, supported_types, enabled, sort_order, limits, refund_enabled, created_at, updated_at FROM payment_provider_instances ORDER BY sort_order ASC",
                (),
            )
            .await
            .context("failed to list provider instances")?
        };

        let mut items = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .context("failed to iterate provider instance rows")?
        {
            items.push(parse_record(row)?);
        }

        Ok(items)
    }

    pub async fn get(&self, id: &str) -> Result<Option<ProviderInstanceRecord>> {
        let conn = self.db.connect_readonly().await?;
        let mut stmt = conn
            .prepare("SELECT id, provider_key, name, config, supported_types, enabled, sort_order, limits, refund_enabled, created_at, updated_at FROM payment_provider_instances WHERE id = ?1")
            .await
            .with_context(|| format!("failed to prepare provider instance lookup {id}"))?;

        let mut rows = stmt
            .query([id])
            .await
            .with_context(|| format!("failed to execute provider instance lookup {id}"))?;

        match rows
            .next()
            .await
            .context("failed to fetch provider instance row")?
        {
            Some(row) => Ok(Some(parse_record(row)?)),
            None => Ok(None),
        }
    }

    pub async fn create(&self, input: ProviderInstanceWrite) -> Result<ProviderInstanceRecord> {
        let id = Uuid::new_v4().to_string();
        let params = write_params(&id, &input);
        let tx = self.db.begin_concurrent().await?;

        tx.execute(
            "INSERT INTO payment_provider_instances (id, provider_key, name, config, supported_types, enabled, sort_order, limits, refund_enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, unixepoch(), unixepoch())",
            params,
        )
        .await
        .with_context(|| format!("failed to create provider instance {id}"))?;
        tx.commit().await?;

        self.get(&id)
            .await?
            .ok_or_else(|| anyhow!("provider instance {id} disappeared after creation"))
    }

    pub async fn replace(
        &self,
        id: &str,
        input: ProviderInstanceWrite,
    ) -> Result<Option<ProviderInstanceRecord>> {
        let params = write_params(id, &input);
        let tx = self.db.begin_concurrent().await?;

        tx.execute(
            "UPDATE payment_provider_instances
             SET provider_key = ?2,
                 name = ?3,
                 config = ?4,
                 supported_types = ?5,
                 enabled = ?6,
                 sort_order = ?7,
                 limits = ?8,
                 refund_enabled = ?9,
                 updated_at = unixepoch()
             WHERE id = ?1",
            params,
        )
        .await
        .with_context(|| format!("failed to update provider instance {id}"))?;
        tx.commit().await?;

        self.get(id).await
    }

    pub async fn delete(&self, id: &str) -> Result<bool> {
        let tx = self.db.begin_concurrent().await?;
        tx.execute("DELETE FROM payment_provider_instances WHERE id = ?1", [id])
            .await
            .with_context(|| format!("failed to delete provider instance {id}"))?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn count_pending_orders(&self, instance_id: &str) -> Result<i64> {
        let conn = self.db.connect_readonly().await?;
        let mut stmt = conn
            .prepare(
                "SELECT COUNT(*) FROM orders WHERE provider_instance_id = ?1 AND status IN (?2, ?3, ?4)",
            )
            .await
            .with_context(|| format!("failed to prepare pending order count for {instance_id}"))?;

        let row = stmt
            .query_row([
                instance_id,
                PENDING_STATUSES[0],
                PENDING_STATUSES[1],
                PENDING_STATUSES[2],
            ])
            .await
            .with_context(|| format!("failed to count pending orders for {instance_id}"))?;

        row.get::<i64>(0).map_err(|error| anyhow!(error))
    }

    pub async fn find_existing_provider_keys(
        &self,
        provider_keys: &[String],
    ) -> Result<Vec<String>> {
        let all = self.list(None).await?;
        let keys: Vec<String> = all
            .into_iter()
            .filter(|record| provider_keys.iter().any(|key| key == &record.provider_key))
            .map(|record| record.provider_key)
            .collect();
        Ok(keys)
    }
}

fn write_params(id: &str, input: &ProviderInstanceWrite) -> Params {
    Params::Positional(vec![
        Value::Text(id.to_string()),
        Value::Text(input.provider_key.clone()),
        Value::Text(input.name.clone()),
        Value::Text(input.config.clone()),
        Value::Text(input.supported_types.clone()),
        Value::Integer(if input.enabled { 1 } else { 0 }),
        Value::Integer(input.sort_order),
        input.limits.clone().map(Value::Text).unwrap_or(Value::Null),
        Value::Integer(if input.refund_enabled { 1 } else { 0 }),
    ])
}

fn parse_record(row: turso::Row) -> Result<ProviderInstanceRecord> {
    Ok(ProviderInstanceRecord {
        id: read_required_string(&row.get_value(0)?)?,
        provider_key: read_required_string(&row.get_value(1)?)?,
        name: read_required_string(&row.get_value(2)?)?,
        config: read_required_string(&row.get_value(3)?)?,
        supported_types: read_required_string(&row.get_value(4)?)?,
        enabled: read_required_bool(&row.get_value(5)?)?,
        sort_order: read_required_i64(&row.get_value(6)?)?,
        limits: read_optional_json_text(&row.get_value(7)?)?,
        refund_enabled: read_required_bool(&row.get_value(8)?)?,
        created_at: read_required_i64(&row.get_value(9)?)?,
        updated_at: read_required_i64(&row.get_value(10)?)?,
    })
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

fn read_required_i64(value: &Value) -> Result<i64> {
    match value {
        Value::Integer(value) => Ok(*value),
        Value::Text(value) => value
            .parse::<i64>()
            .context("failed to parse integer from text"),
        _ => Err(anyhow!("unexpected non-integer value")),
    }
}

fn read_required_bool(value: &Value) -> Result<bool> {
    match value {
        Value::Integer(value) => Ok(*value != 0),
        Value::Text(value) => Ok(matches!(value.as_str(), "1" | "true" | "TRUE")),
        _ => Err(anyhow!("unexpected non-boolean value")),
    }
}

fn read_optional_json_text(value: &Value) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        Value::Text(value) => {
            let _: JsonValue = serde_json::from_str(value).context("failed to parse JSON text")?;
            Ok(Some(value.clone()))
        }
        _ => Err(anyhow!("unexpected non-text JSON value")),
    }
}
