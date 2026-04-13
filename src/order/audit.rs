use anyhow::{Context, Result, anyhow};
use turso::{Value, params::Params};
use uuid::Uuid;

use crate::db::DatabaseHandle;

#[derive(Clone)]
pub struct AuditLogRepository {
    db: DatabaseHandle,
}

#[derive(Debug, Clone)]
pub struct NewAuditLog {
    pub order_id: String,
    pub action: String,
    pub detail: Option<String>,
    pub operator: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuditLogRecord {
    pub id: String,
    pub order_id: String,
    pub action: String,
    pub detail: Option<String>,
    pub operator: Option<String>,
    pub created_at: i64,
}

impl AuditLogRepository {
    pub fn new(db: DatabaseHandle) -> Self {
        Self { db }
    }

    pub async fn append(&self, entry: NewAuditLog) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let tx = self.db.begin_concurrent().await?;
        let params = Params::Positional(vec![
            Value::Text(id.clone()),
            Value::Text(entry.order_id),
            Value::Text(entry.action),
            entry.detail.map(Value::Text).unwrap_or(Value::Null),
            entry.operator.map(Value::Text).unwrap_or(Value::Null),
        ]);

        tx.execute(
            "INSERT INTO audit_logs (id, order_id, action, detail, operator, created_at) VALUES (?1, ?2, ?3, ?4, ?5, unixepoch())",
            params,
        )
        .await
        .with_context(|| format!("failed to append audit log {id}"))?;
        tx.commit().await?;

        Ok(id)
    }

    pub async fn count_by_order_and_action(&self, order_id: &str, action: &str) -> Result<i64> {
        let conn = self.db.connect_readonly().await?;
        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM audit_logs WHERE order_id = ?1 AND action = ?2")
            .await
            .with_context(|| {
                format!("failed to prepare audit log count for order {order_id} action {action}")
            })?;
        let row = stmt.query_row([order_id, action]).await.with_context(|| {
            format!("failed to count audit logs for order {order_id} action {action}")
        })?;
        row.get::<i64>(0).map_err(|error| anyhow!(error))
    }

    pub async fn list_by_order(&self, order_id: &str) -> Result<Vec<AuditLogRecord>> {
        let conn = self.db.connect_readonly().await?;
        let mut rows = conn
            .query(
                "SELECT id, order_id, action, detail, operator, created_at
                 FROM audit_logs
                 WHERE order_id = ?1
                 ORDER BY created_at DESC, id DESC",
                Params::Positional(vec![Value::Text(order_id.to_string())]),
            )
            .await
            .with_context(|| format!("failed to query audit logs for order {order_id}"))?;

        let mut items = Vec::new();
        while let Some(row) = rows.next().await.context("failed to fetch audit log row")? {
            items.push(AuditLogRecord {
                id: read_required_string(&row.get_value(0)?)?,
                order_id: read_required_string(&row.get_value(1)?)?,
                action: read_required_string(&row.get_value(2)?)?,
                detail: read_optional_string(&row.get_value(3)?)?,
                operator: read_optional_string(&row.get_value(4)?)?,
                created_at: read_required_i64(&row.get_value(5)?)?,
            });
        }

        Ok(items)
    }

    pub async fn find_latest_by_order_and_action(
        &self,
        order_id: &str,
        action: &str,
    ) -> Result<Option<AuditLogRecord>> {
        let conn = self.db.connect_readonly().await?;
        let mut rows = conn
            .query(
                "SELECT id, order_id, action, detail, operator, created_at
                 FROM audit_logs
                 WHERE order_id = ?1 AND action = ?2
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                Params::Positional(vec![
                    Value::Text(order_id.to_string()),
                    Value::Text(action.to_string()),
                ]),
            )
            .await
            .with_context(|| {
                format!("failed to query latest audit log for order {order_id} action {action}")
            })?;

        match rows.next().await.context("failed to fetch audit log row")? {
            Some(row) => Ok(Some(AuditLogRecord {
                id: read_required_string(&row.get_value(0)?)?,
                order_id: read_required_string(&row.get_value(1)?)?,
                action: read_required_string(&row.get_value(2)?)?,
                detail: read_optional_string(&row.get_value(3)?)?,
                operator: read_optional_string(&row.get_value(4)?)?,
                created_at: read_required_i64(&row.get_value(5)?)?,
            })),
            None => Ok(None),
        }
    }
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
        Value::Real(value) => Ok(*value as i64),
        _ => Err(anyhow!("unexpected non-integer value")),
    }
}
