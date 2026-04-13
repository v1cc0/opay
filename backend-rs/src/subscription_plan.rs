use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Months, Utc};
use turso::{Value, params::Params};

use crate::db::DatabaseHandle;

#[derive(Clone)]
pub struct SubscriptionPlanRepository {
    db: DatabaseHandle,
}

#[derive(Debug, Clone)]
pub struct SubscriptionPlanRecord {
    pub id: String,
    pub group_id: Option<i64>,
    pub name: String,
    pub description: Option<String>,
    pub price_cents: i64,
    pub original_price_cents: Option<i64>,
    pub validity_days: i64,
    pub validity_unit: String,
    pub features: Option<String>,
    pub product_name: Option<String>,
    pub for_sale: bool,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct SubscriptionPlanWrite {
    pub group_id: Option<i64>,
    pub name: String,
    pub description: Option<String>,
    pub price_cents: i64,
    pub original_price_cents: Option<i64>,
    pub validity_days: i64,
    pub validity_unit: String,
    pub features: Option<String>,
    pub product_name: Option<String>,
    pub for_sale: bool,
    pub sort_order: i64,
}

impl SubscriptionPlanRepository {
    pub fn new(db: DatabaseHandle) -> Self {
        Self { db }
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<SubscriptionPlanRecord>> {
        let conn = self.db.connect()?;
        let mut rows = conn
            .query(
                "SELECT id, group_id, name, description, price_cents, original_price_cents, validity_days, validity_unit, features, product_name, for_sale, sort_order, created_at, updated_at
                 FROM subscription_plans
                 WHERE id = ?1",
                Params::Positional(vec![Value::Text(id.to_string())]),
            )
            .await
            .with_context(|| format!("failed to query subscription plan {id}"))?;

        match rows
            .next()
            .await
            .context("failed to fetch subscription plan row")?
        {
            Some(row) => Ok(Some(parse_plan_row(row)?)),
            None => Ok(None),
        }
    }

    pub async fn list_for_sale(&self) -> Result<Vec<SubscriptionPlanRecord>> {
        let conn = self.db.connect()?;
        let mut rows = conn
            .query(
                "SELECT id, group_id, name, description, price_cents, original_price_cents, validity_days, validity_unit, features, product_name, for_sale, sort_order, created_at, updated_at
                 FROM subscription_plans
                 WHERE for_sale = 1
                 ORDER BY sort_order ASC, created_at ASC",
                (),
            )
            .await
            .context("failed to query for-sale subscription plans")?;

        let mut items = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .context("failed to fetch subscription plan row")?
        {
            items.push(parse_plan_row(row)?);
        }

        Ok(items)
    }

    pub async fn list_all(&self) -> Result<Vec<SubscriptionPlanRecord>> {
        let conn = self.db.connect()?;
        let mut rows = conn
            .query(
                "SELECT id, group_id, name, description, price_cents, original_price_cents, validity_days, validity_unit, features, product_name, for_sale, sort_order, created_at, updated_at
                 FROM subscription_plans
                 ORDER BY sort_order ASC, created_at ASC",
                (),
            )
            .await
            .context("failed to query all subscription plans")?;

        let mut items = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .context("failed to fetch subscription plan row")?
        {
            items.push(parse_plan_row(row)?);
        }

        Ok(items)
    }

    pub async fn create(&self, input: SubscriptionPlanWrite) -> Result<SubscriptionPlanRecord> {
        let id = uuid::Uuid::new_v4().to_string();
        let conn = self.db.connect()?;
        conn.execute(
            "INSERT INTO subscription_plans (id, group_id, name, description, price_cents, original_price_cents, validity_days, validity_unit, features, product_name, for_sale, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, unixepoch(), unixepoch())",
            write_params(&id, &input),
        )
        .await
        .with_context(|| format!("failed to create subscription plan {id}"))?;

        self.get_by_id(&id)
            .await?
            .ok_or_else(|| anyhow!("subscription plan {id} disappeared after create"))
    }

    pub async fn replace(
        &self,
        id: &str,
        input: SubscriptionPlanWrite,
    ) -> Result<Option<SubscriptionPlanRecord>> {
        let conn = self.db.connect()?;
        conn.execute(
            "UPDATE subscription_plans
             SET group_id = ?2,
                 name = ?3,
                 description = ?4,
                 price_cents = ?5,
                 original_price_cents = ?6,
                 validity_days = ?7,
                 validity_unit = ?8,
                 features = ?9,
                 product_name = ?10,
                 for_sale = ?11,
                 sort_order = ?12,
                 updated_at = unixepoch()
             WHERE id = ?1",
            write_params(id, &input),
        )
        .await
        .with_context(|| format!("failed to update subscription plan {id}"))?;

        self.get_by_id(id).await
    }

    pub async fn delete(&self, id: &str) -> Result<bool> {
        let conn = self.db.connect()?;
        let affected = conn
            .execute("DELETE FROM subscription_plans WHERE id = ?1", [id])
            .await
            .with_context(|| format!("failed to delete subscription plan {id}"))?;
        Ok(affected > 0)
    }

    pub async fn count_active_orders_by_plan(&self, plan_id: &str) -> Result<i64> {
        let conn = self.db.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT COUNT(*) FROM orders WHERE plan_id = ?1 AND status IN ('PENDING', 'PAID', 'RECHARGING')",
            )
            .await
            .with_context(|| format!("failed to prepare active order count for plan {plan_id}"))?;
        let row = stmt
            .query_row([plan_id])
            .await
            .with_context(|| format!("failed to count active orders for plan {plan_id}"))?;
        row.get::<i64>(0).map_err(|error| anyhow!(error))
    }
}

pub fn compute_validity_days(value: i64, unit: &str, from_rfc3339: Option<&str>) -> Result<i64> {
    if value <= 0 {
        bail!("subscription validity must be positive");
    }

    match unit.trim().to_ascii_lowercase().as_str() {
        "day" => Ok(value),
        "week" => Ok(value * 7),
        "month" => {
            let from = match from_rfc3339 {
                Some(value) => DateTime::parse_from_rfc3339(value)
                    .with_context(|| format!("failed to parse subscription timestamp {value}"))?,
                None => Utc::now().fixed_offset(),
            };
            let months = u32::try_from(value).context("subscription month value exceeds u32")?;
            let target = from
                .checked_add_months(Months::new(months))
                .ok_or_else(|| anyhow!("failed to add {months} month(s) to subscription date"))?;
            Ok((target - from).num_days())
        }
        other => bail!("unsupported subscription validity unit {other}"),
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

fn read_optional_i64(value: &Value) -> Result<Option<i64>> {
    match value {
        Value::Null => Ok(None),
        _ => read_required_i64(value).map(Some),
    }
}

fn read_required_bool(value: &Value) -> Result<bool> {
    match value {
        Value::Integer(value) => Ok(*value != 0),
        Value::Text(value) => Ok(value != "0" && !value.eq_ignore_ascii_case("false")),
        Value::Real(value) => Ok(*value != 0.0),
        Value::Null => Err(anyhow!("unexpected NULL value")),
        Value::Blob(_) => Err(anyhow!("unexpected BLOB value")),
    }
}

fn parse_plan_row(row: turso::Row) -> Result<SubscriptionPlanRecord> {
    Ok(SubscriptionPlanRecord {
        id: read_required_string(&row.get_value(0)?)?,
        group_id: read_optional_i64(&row.get_value(1)?)?,
        name: read_required_string(&row.get_value(2)?)?,
        description: read_optional_string(&row.get_value(3)?)?,
        price_cents: read_required_i64(&row.get_value(4)?)?,
        original_price_cents: read_optional_i64(&row.get_value(5)?)?,
        validity_days: read_required_i64(&row.get_value(6)?)?,
        validity_unit: read_required_string(&row.get_value(7)?)?,
        features: read_optional_string(&row.get_value(8)?)?,
        product_name: read_optional_string(&row.get_value(9)?)?,
        for_sale: read_required_bool(&row.get_value(10)?)?,
        sort_order: read_required_i64(&row.get_value(11)?)?,
        created_at: read_required_i64(&row.get_value(12)?)?,
        updated_at: read_required_i64(&row.get_value(13)?)?,
    })
}

fn write_params(id: &str, input: &SubscriptionPlanWrite) -> Params {
    Params::Positional(vec![
        Value::Text(id.to_string()),
        input.group_id.map(Value::Integer).unwrap_or(Value::Null),
        Value::Text(input.name.clone()),
        input
            .description
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
        Value::Integer(input.price_cents),
        input
            .original_price_cents
            .map(Value::Integer)
            .unwrap_or(Value::Null),
        Value::Integer(input.validity_days),
        Value::Text(input.validity_unit.clone()),
        input
            .features
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
        input
            .product_name
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
        Value::Integer(if input.for_sale { 1 } else { 0 }),
        Value::Integer(input.sort_order),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DatabaseHandle;

    async fn test_repo() -> SubscriptionPlanRepository {
        let path = std::env::temp_dir().join(format!(
            "sub2apipay-subscription-plan-{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = DatabaseHandle::open_local(&path).await.unwrap();
        db.run_migrations().await.unwrap();
        SubscriptionPlanRepository::new(db)
    }

    #[tokio::test]
    async fn get_by_id_reads_for_sale_plan() {
        let repo = test_repo().await;
        let conn = repo.db.connect().unwrap();

        conn.execute(
            "INSERT INTO subscription_plans (id, group_id, name, price_cents, validity_days, validity_unit, product_name, for_sale)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            Params::Positional(vec![
                Value::Text("plan_basic".to_string()),
                Value::Integer(5),
                Value::Text("Basic".to_string()),
                Value::Integer(1999),
                Value::Integer(30),
                Value::Text("day".to_string()),
                Value::Text("Sub2API Basic".to_string()),
                Value::Integer(1),
            ]),
        )
        .await
        .unwrap();

        let plan = repo.get_by_id("plan_basic").await.unwrap().unwrap();
        assert_eq!(plan.group_id, Some(5));
        assert_eq!(plan.price_cents, 1999);
        assert_eq!(plan.validity_days, 30);
        assert_eq!(plan.validity_unit, "day");
        assert_eq!(plan.product_name.as_deref(), Some("Sub2API Basic"));
        assert!(plan.for_sale);
    }

    #[tokio::test]
    async fn list_for_sale_returns_sorted_plans() {
        let repo = test_repo().await;
        let conn = repo.db.connect().unwrap();

        conn.execute(
            "INSERT INTO subscription_plans (id, group_id, name, price_cents, validity_days, validity_unit, for_sale, sort_order)
             VALUES ('plan_b', 2, 'B', 2999, 30, 'day', 1, 20)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO subscription_plans (id, group_id, name, price_cents, validity_days, validity_unit, for_sale, sort_order)
             VALUES ('plan_a', 1, 'A', 1999, 30, 'day', 1, 10)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO subscription_plans (id, group_id, name, price_cents, validity_days, validity_unit, for_sale, sort_order)
             VALUES ('plan_hidden', 3, 'Hidden', 999, 30, 'day', 0, 0)",
            (),
        )
        .await
        .unwrap();

        let plans = repo.list_for_sale().await.unwrap();
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].id, "plan_a");
        assert_eq!(plans[1].id, "plan_b");
    }

    #[test]
    fn compute_validity_days_handles_day_week_and_month() {
        assert_eq!(compute_validity_days(30, "day", None).unwrap(), 30);
        assert_eq!(compute_validity_days(2, "week", None).unwrap(), 14);
        assert_eq!(
            compute_validity_days(1, "month", Some("2026-01-31T00:00:00Z")).unwrap(),
            28
        );
    }
}
