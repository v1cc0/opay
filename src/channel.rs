use anyhow::{Context, Result, anyhow};
use turso::{Value, params::Params};
use uuid::Uuid;

use crate::db::DatabaseHandle;

#[derive(Clone)]
pub struct ChannelRepository {
    db: DatabaseHandle,
}

#[derive(Debug, Clone)]
pub struct ChannelRecord {
    pub id: String,
    pub group_id: Option<i64>,
    pub name: String,
    pub platform: String,
    pub rate_multiplier_bps: i64,
    pub description: Option<String>,
    pub models: Option<String>,
    pub features: Option<String>,
    pub sort_order: i64,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct ChannelWrite {
    pub group_id: Option<i64>,
    pub name: String,
    pub platform: String,
    pub rate_multiplier_bps: i64,
    pub description: Option<String>,
    pub models: Option<String>,
    pub features: Option<String>,
    pub sort_order: i64,
    pub enabled: bool,
}

impl ChannelRepository {
    pub fn new(db: DatabaseHandle) -> Self {
        Self { db }
    }

    pub async fn list_all(&self) -> Result<Vec<ChannelRecord>> {
        self.query_list(false).await
    }

    pub async fn list_enabled(&self) -> Result<Vec<ChannelRecord>> {
        self.query_list(true).await
    }

    pub async fn get(&self, id: &str) -> Result<Option<ChannelRecord>> {
        let conn = self.db.connect_readonly().await?;
        let mut rows = conn
            .query(
                "SELECT id, group_id, name, platform, rate_multiplier_bps, description, models, features, sort_order, enabled, created_at, updated_at
                 FROM channels
                 WHERE id = ?1",
                [id],
            )
            .await
            .with_context(|| format!("failed to query channel {id}"))?;

        match rows.next().await.context("failed to fetch channel row")? {
            Some(row) => Ok(Some(parse_channel_row(row)?)),
            None => Ok(None),
        }
    }

    pub async fn get_by_group_id(&self, group_id: i64) -> Result<Option<ChannelRecord>> {
        let conn = self.db.connect_readonly().await?;
        let mut rows = conn
            .query(
                "SELECT id, group_id, name, platform, rate_multiplier_bps, description, models, features, sort_order, enabled, created_at, updated_at
                 FROM channels
                 WHERE group_id = ?1",
                [group_id],
            )
            .await
            .with_context(|| format!("failed to query channel by group {group_id}"))?;

        match rows.next().await.context("failed to fetch channel row")? {
            Some(row) => Ok(Some(parse_channel_row(row)?)),
            None => Ok(None),
        }
    }

    pub async fn create(&self, input: ChannelWrite) -> Result<ChannelRecord> {
        let id = Uuid::new_v4().to_string();
        let tx = self.db.begin_concurrent().await?;

        tx.execute(
            "INSERT INTO channels (id, group_id, name, platform, rate_multiplier_bps, description, models, features, sort_order, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, unixepoch(), unixepoch())",
            write_params(&id, &input),
        )
        .await
        .with_context(|| format!("failed to create channel {id}"))?;
        tx.commit().await?;

        self.get(&id)
            .await?
            .ok_or_else(|| anyhow!("channel {id} disappeared after create"))
    }

    pub async fn replace(&self, id: &str, input: ChannelWrite) -> Result<Option<ChannelRecord>> {
        let tx = self.db.begin_concurrent().await?;
        tx.execute(
            "UPDATE channels
             SET group_id = ?2,
                 name = ?3,
                 platform = ?4,
                 rate_multiplier_bps = ?5,
                 description = ?6,
                 models = ?7,
                 features = ?8,
                 sort_order = ?9,
                 enabled = ?10,
                 updated_at = unixepoch()
             WHERE id = ?1",
            write_params(id, &input),
        )
        .await
        .with_context(|| format!("failed to update channel {id}"))?;
        tx.commit().await?;

        self.get(id).await
    }

    pub async fn delete(&self, id: &str) -> Result<bool> {
        let tx = self.db.begin_concurrent().await?;
        let affected = tx
            .execute("DELETE FROM channels WHERE id = ?1", [id])
            .await
            .with_context(|| format!("failed to delete channel {id}"))?;
        tx.commit().await?;
        Ok(affected > 0)
    }

    async fn query_list(&self, enabled_only: bool) -> Result<Vec<ChannelRecord>> {
        let conn = self.db.connect_readonly().await?;
        let mut rows = if enabled_only {
            conn.query(
                "SELECT id, group_id, name, platform, rate_multiplier_bps, description, models, features, sort_order, enabled, created_at, updated_at
                 FROM channels
                 WHERE enabled = 1
                 ORDER BY sort_order ASC, created_at ASC",
                Params::Positional(vec![]),
            )
            .await
            .context("failed to query enabled channels")?
        } else {
            conn.query(
                "SELECT id, group_id, name, platform, rate_multiplier_bps, description, models, features, sort_order, enabled, created_at, updated_at
                 FROM channels
                 ORDER BY sort_order ASC, created_at ASC",
                Params::Positional(vec![]),
            )
            .await
            .context("failed to query channels")?
        };

        let mut items = Vec::new();
        while let Some(row) = rows.next().await.context("failed to fetch channel row")? {
            items.push(parse_channel_row(row)?);
        }

        Ok(items)
    }
}

fn write_params(id: &str, input: &ChannelWrite) -> Params {
    Params::Positional(vec![
        Value::Text(id.to_string()),
        input.group_id.map(Value::Integer).unwrap_or(Value::Null),
        Value::Text(input.name.clone()),
        Value::Text(input.platform.clone()),
        Value::Integer(input.rate_multiplier_bps),
        input
            .description
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
        input.models.clone().map(Value::Text).unwrap_or(Value::Null),
        input
            .features
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
        Value::Integer(input.sort_order),
        Value::Integer(if input.enabled { 1 } else { 0 }),
    ])
}

fn parse_channel_row(row: turso::Row) -> Result<ChannelRecord> {
    Ok(ChannelRecord {
        id: read_required_string(&row.get_value(0)?)?,
        group_id: read_optional_i64(&row.get_value(1)?)?,
        name: read_required_string(&row.get_value(2)?)?,
        platform: read_required_string(&row.get_value(3)?)?,
        rate_multiplier_bps: read_required_i64(&row.get_value(4)?)?,
        description: read_optional_string(&row.get_value(5)?)?,
        models: read_optional_string(&row.get_value(6)?)?,
        features: read_optional_string(&row.get_value(7)?)?,
        sort_order: read_required_i64(&row.get_value(8)?)?,
        enabled: read_required_bool(&row.get_value(9)?)?,
        created_at: read_required_i64(&row.get_value(10)?)?,
        updated_at: read_required_i64(&row.get_value(11)?)?,
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

fn read_optional_i64(value: &Value) -> Result<Option<i64>> {
    match value {
        Value::Null => Ok(None),
        _ => read_required_i64(value).map(Some),
    }
}

fn read_required_bool(value: &Value) -> Result<bool> {
    match value {
        Value::Integer(value) => Ok(*value != 0),
        Value::Text(value) => Ok(matches!(value.as_str(), "1" | "true" | "TRUE")),
        Value::Real(value) => Ok(*value != 0.0),
        Value::Null => Err(anyhow!("unexpected NULL value")),
        Value::Blob(_) => Err(anyhow!("unexpected BLOB value")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DatabaseHandle;

    async fn test_repo() -> ChannelRepository {
        let path =
            std::env::temp_dir().join(format!("opay-channel-repo-{}.db", uuid::Uuid::new_v4()));
        let db = DatabaseHandle::open_local(&path).await.unwrap();
        db.run_migrations().await.unwrap();
        ChannelRepository::new(db)
    }

    #[tokio::test]
    async fn create_list_replace_delete_channel() {
        let repo = test_repo().await;
        let created = repo
            .create(ChannelWrite {
                group_id: Some(7),
                name: "OpenAI".to_string(),
                platform: "openai".to_string(),
                rate_multiplier_bps: 1500,
                description: Some("desc".to_string()),
                models: Some("[\"gpt-4.1\"]".to_string()),
                features: Some("[\"fast\"]".to_string()),
                sort_order: 1,
                enabled: true,
            })
            .await
            .unwrap();

        assert_eq!(created.group_id, Some(7));
        assert_eq!(repo.list_all().await.unwrap().len(), 1);
        assert_eq!(repo.list_enabled().await.unwrap().len(), 1);
        assert!(repo.get_by_group_id(7).await.unwrap().is_some());

        let replaced = repo
            .replace(
                &created.id,
                ChannelWrite {
                    group_id: None,
                    name: "Claude".to_string(),
                    platform: "claude".to_string(),
                    rate_multiplier_bps: 2500,
                    description: None,
                    models: None,
                    features: None,
                    sort_order: 2,
                    enabled: false,
                },
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(replaced.name, "Claude");
        assert_eq!(replaced.platform, "claude");
        assert!(!replaced.enabled);
        assert_eq!(repo.list_enabled().await.unwrap().len(), 0);
        assert!(repo.delete(&created.id).await.unwrap());
        assert!(repo.get(&created.id).await.unwrap().is_none());
    }
}
