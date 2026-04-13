use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use serde_json::Value as JsonValue;
use turso::{Value, params::Params};

use crate::{AppState, crypto, provider_instances::ProviderInstanceRepository};

const STATUS_PAID: &str = "PAID";
const STATUS_RECHARGING: &str = "RECHARGING";
const STATUS_COMPLETED: &str = "COMPLETED";
const BIZ_OFFSET_SECONDS: i64 = 8 * 60 * 60;
const RUST_MVP_PROVIDER_KEYS: &[&str] = &["easypay", "stripe"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MethodLimitStatus {
    pub daily_limit: f64,
    pub used: f64,
    pub remaining: Option<f64>,
    pub available: bool,
    pub single_min: f64,
    pub single_max: f64,
    pub fee_rate: f64,
}

#[derive(Debug, Clone)]
pub struct UserPaymentConfig {
    pub enabled_payment_types: Vec<String>,
    pub method_limits: HashMap<String, MethodLimitStatus>,
    pub stripe_publishable_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PaymentSelection {
    pub fee_rate: f64,
    pub fee_rate_bps: i64,
    pub pay_amount_cents: i64,
    pub provider_instance_id: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct MethodDefaultLimits {
    single_max_cents: i64,
    daily_max_cents: i64,
}

#[derive(Debug, Clone)]
struct ActiveInstance {
    id: String,
    provider_key: String,
    sort_order: i64,
    supported_types: Vec<String>,
    limits: Option<JsonValue>,
    encrypted_config: String,
}

#[derive(Debug, Clone)]
struct InstanceAggregate {
    single_min_cents: i64,
    single_max_cents: i64,
    all_instances_daily_blocked: bool,
    max_remaining_capacity_cents: Option<i64>,
    has_instances: bool,
}

pub async fn resolve_user_payment_config(state: &AppState) -> Result<UserPaymentConfig> {
    let override_enabled = state
        .system_config
        .get("OVERRIDE_ENV_ENABLED")
        .await?
        .map(|value| value == "true")
        .unwrap_or(false);

    let enabled_provider_keys = if override_enabled {
        state
            .system_config
            .get("ENABLED_PROVIDERS")
            .await?
            .map(|value| split_csv(&value))
            .unwrap_or_else(|| state.config.payment_providers.clone())
    } else {
        state.config.payment_providers.clone()
    };
    let enabled_provider_keys = filter_provider_keys_for_rust_mvp(&enabled_provider_keys);

    let supported_types = supported_types_for_provider_keys(&enabled_provider_keys);
    let configured_types = state.system_config.get("ENABLED_PAYMENT_TYPES").await?;
    let mut enabled_types =
        resolve_enabled_payment_types(&supported_types, configured_types.as_deref());

    let repo = ProviderInstanceRepository::new(state.db.clone());
    let active_instances = repo
        .list(None)
        .await?
        .into_iter()
        .filter(|record| record.enabled)
        .map(|record| ActiveInstance {
            id: record.id,
            provider_key: record.provider_key,
            sort_order: record.sort_order,
            supported_types: split_csv(&record.supported_types),
            limits: record
                .limits
                .and_then(|value| serde_json::from_str::<JsonValue>(&value).ok()),
            encrypted_config: record.config,
        })
        .collect::<Vec<_>>();

    if override_enabled && !enabled_types.is_empty() {
        enabled_types.retain(|payment_type| {
            let Some(provider_key) = provider_key_for_type(payment_type) else {
                return false;
            };
            active_instances.iter().any(|instance| {
                instance.provider_key == provider_key
                    && instance_supports_type(instance, payment_type)
            })
        });
    }

    let method_limits =
        query_method_limits(state, &enabled_types, &active_instances, override_enabled).await?;
    let stripe_publishable_key =
        resolve_stripe_publishable_key(state, &enabled_types, &active_instances)?;

    Ok(UserPaymentConfig {
        enabled_payment_types: enabled_types,
        method_limits,
        stripe_publishable_key,
    })
}

pub async fn resolve_payment_selection(
    state: &AppState,
    payment_type: &str,
    amount_cents: i64,
) -> Result<PaymentSelection> {
    if amount_cents <= 0 {
        return Err(anyhow!("amount must be positive"));
    }

    let fee_rate = get_method_fee_rate(payment_type);
    let fee_rate_bps = fee_rate_to_bps(fee_rate);
    let pay_amount_cents = calculate_pay_amount_cents(amount_cents, fee_rate_bps);

    let repo = ProviderInstanceRepository::new(state.db.clone());
    let mut instances = repo
        .list(None)
        .await?
        .into_iter()
        .filter(|record| record.enabled)
        .map(|record| ActiveInstance {
            id: record.id,
            provider_key: record.provider_key,
            sort_order: record.sort_order,
            supported_types: split_csv(&record.supported_types),
            limits: record
                .limits
                .and_then(|value| serde_json::from_str::<JsonValue>(&value).ok()),
            encrypted_config: record.config,
        })
        .collect::<Vec<_>>();
    instances.sort_by_key(|instance| instance.sort_order);

    let Some(provider_key) = provider_key_for_type(payment_type) else {
        return Ok(PaymentSelection {
            fee_rate,
            fee_rate_bps,
            pay_amount_cents,
            provider_instance_id: None,
        });
    };

    let matching_instances = instances
        .into_iter()
        .filter(|instance| {
            instance.provider_key == provider_key && instance_supports_type(instance, payment_type)
        })
        .collect::<Vec<_>>();

    if matching_instances.is_empty() {
        return Ok(PaymentSelection {
            fee_rate,
            fee_rate_bps,
            pay_amount_cents,
            provider_instance_id: None,
        });
    }

    let usage_by_instance = query_usage_by_provider_instance(state).await?;
    let selected = matching_instances
        .iter()
        .find(|instance| {
            instance_accepts_amount(instance, payment_type, pay_amount_cents, &usage_by_instance)
        })
        .map(|instance| instance.id.clone())
        .ok_or_else(|| anyhow!("no available payment instance for {}", payment_type))?;

    Ok(PaymentSelection {
        fee_rate,
        fee_rate_bps,
        pay_amount_cents,
        provider_instance_id: Some(selected),
    })
}

fn resolve_enabled_payment_types(
    supported_types: &[String],
    configured_types: Option<&str>,
) -> Vec<String> {
    match configured_types {
        None => supported_types.to_vec(),
        Some(configured_types) => {
            let configured = split_csv(configured_types);
            if configured.is_empty() {
                supported_types.to_vec()
            } else {
                supported_types
                    .iter()
                    .filter(|payment_type| {
                        configured
                            .iter()
                            .any(|configured| configured == *payment_type)
                    })
                    .cloned()
                    .collect()
            }
        }
    }
}

pub fn supported_types_for_provider_keys(provider_keys: &[String]) -> Vec<String> {
    let mut types = Vec::new();
    for provider_key in filter_provider_keys_for_rust_mvp(provider_keys) {
        for payment_type in provider_supported_types(&provider_key) {
            if !types.iter().any(|existing| existing == payment_type) {
                types.push(payment_type.to_string());
            }
        }
    }
    types
}

pub fn filter_provider_keys_for_rust_mvp(provider_keys: &[String]) -> Vec<String> {
    let mut filtered = Vec::new();
    for provider_key in provider_keys {
        if !provider_enabled_in_rust_mvp(provider_key) {
            continue;
        }
        if !filtered.iter().any(|existing| existing == provider_key) {
            filtered.push(provider_key.clone());
        }
    }
    filtered
}

pub fn ignored_provider_keys_for_rust_mvp(provider_keys: &[String]) -> Vec<String> {
    let mut ignored = Vec::new();
    for provider_key in provider_keys {
        if provider_enabled_in_rust_mvp(provider_key)
            || ignored.iter().any(|existing| existing == provider_key)
        {
            continue;
        }
        ignored.push(provider_key.clone());
    }
    ignored
}

pub fn provider_enabled_in_rust_mvp(provider_key: &str) -> bool {
    RUST_MVP_PROVIDER_KEYS
        .iter()
        .any(|supported| *supported == provider_key)
}

pub fn provider_supported_types(provider_key: &str) -> &'static [&'static str] {
    match provider_key {
        "easypay" => &["alipay", "wxpay"],
        "alipay" => &["alipay_direct"],
        "wxpay" => &["wxpay_direct"],
        "stripe" => &["stripe"],
        _ => &[],
    }
}

fn provider_key_for_type(payment_type: &str) -> Option<&'static str> {
    if payment_type.starts_with("alipay_direct") {
        Some("alipay")
    } else if payment_type == "alipay" {
        Some("easypay")
    } else if payment_type.starts_with("wxpay_direct") {
        Some("wxpay")
    } else if payment_type == "wxpay" {
        Some("easypay")
    } else if payment_type.starts_with("stripe") {
        Some("stripe")
    } else {
        None
    }
}

fn provider_default_limit(payment_type: &str) -> MethodDefaultLimits {
    match payment_type {
        "alipay" | "wxpay" | "alipay_direct" | "wxpay_direct" => MethodDefaultLimits {
            single_max_cents: 100_000,
            daily_max_cents: 1_000_000,
        },
        "stripe" => MethodDefaultLimits {
            single_max_cents: 0,
            daily_max_cents: 0,
        },
        _ => MethodDefaultLimits {
            single_max_cents: 0,
            daily_max_cents: 0,
        },
    }
}

fn fee_rate_to_bps(fee_rate: f64) -> i64 {
    (fee_rate * 100.0).round() as i64
}

fn calculate_pay_amount_cents(amount_cents: i64, fee_rate_bps: i64) -> i64 {
    if fee_rate_bps <= 0 {
        return amount_cents;
    }
    let fee_cents = (amount_cents * fee_rate_bps + 9_999) / 10_000;
    amount_cents + fee_cents
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn instance_supports_type(instance: &ActiveInstance, payment_type: &str) -> bool {
    instance.supported_types.is_empty()
        || instance
            .supported_types
            .iter()
            .any(|supported| supported == payment_type)
}

fn instance_accepts_amount(
    instance: &ActiveInstance,
    payment_type: &str,
    pay_amount_cents: i64,
    usage_by_instance: &HashMap<String, i64>,
) -> bool {
    let channel_limits = instance
        .limits
        .as_ref()
        .and_then(|limits| limits.get(payment_type))
        .and_then(JsonValue::as_object);

    let single_min_cents = channel_limits
        .and_then(|limits| limits.get("singleMin"))
        .and_then(json_number_to_cents)
        .unwrap_or(0);
    let single_max_cents = channel_limits
        .and_then(|limits| limits.get("singleMax"))
        .and_then(json_number_to_cents)
        .unwrap_or(0);
    let daily_limit_cents = channel_limits
        .and_then(|limits| limits.get("dailyLimit"))
        .and_then(json_number_to_cents)
        .unwrap_or(0);

    if single_min_cents > 0 && pay_amount_cents < single_min_cents {
        return false;
    }
    if single_max_cents > 0 && pay_amount_cents > single_max_cents {
        return false;
    }
    if daily_limit_cents > 0 {
        let used_cents = *usage_by_instance.get(&instance.id).unwrap_or(&0);
        if used_cents + pay_amount_cents > daily_limit_cents {
            return false;
        }
    }
    true
}

async fn query_method_limits(
    state: &AppState,
    payment_types: &[String],
    active_instances: &[ActiveInstance],
    override_enabled: bool,
) -> Result<HashMap<String, MethodLimitStatus>> {
    let usage_by_type = query_usage_by_payment_type(state, payment_types).await?;
    let usage_by_instance = query_usage_by_provider_instance(state).await?;
    let instance_agg =
        aggregate_instance_limits(payment_types, active_instances, &usage_by_instance);

    let mut result = HashMap::new();
    for payment_type in payment_types {
        let global_daily_limit =
            get_method_daily_limit(state, payment_type, override_enabled).await?;
        let global_single_max =
            get_method_single_limit(state, payment_type, override_enabled).await?;
        let fee_rate = get_method_fee_rate(payment_type);
        let used_cents = *usage_by_type.get(payment_type).unwrap_or(&0);
        let remaining_cents = if global_daily_limit > 0 {
            Some((global_daily_limit - used_cents).max(0))
        } else {
            None
        };

        let instance = instance_agg
            .get(payment_type)
            .cloned()
            .unwrap_or(InstanceAggregate {
                single_min_cents: 0,
                single_max_cents: 0,
                all_instances_daily_blocked: false,
                max_remaining_capacity_cents: None,
                has_instances: false,
            });

        let global_available = global_daily_limit == 0 || used_cents < global_daily_limit;
        let instance_available = !instance.has_instances || !instance.all_instances_daily_blocked;

        let single_min_cents = instance.single_min_cents;
        let mut single_max_cents = global_single_max;
        if instance.has_instances && instance.single_max_cents > 0 {
            single_max_cents = if single_max_cents > 0 {
                single_max_cents.min(instance.single_max_cents)
            } else {
                instance.single_max_cents
            };
        }
        if let Some(max_remaining_capacity_cents) = instance.max_remaining_capacity_cents {
            single_max_cents = if single_max_cents > 0 {
                single_max_cents.min(max_remaining_capacity_cents)
            } else {
                max_remaining_capacity_cents
            };
        }
        if let Some(remaining_cents) = remaining_cents {
            single_max_cents = if single_max_cents > 0 {
                single_max_cents.min(remaining_cents)
            } else {
                remaining_cents
            };
        }

        let effectively_available = global_available
            && instance_available
            && (single_min_cents == 0 || single_max_cents >= single_min_cents);

        result.insert(
            payment_type.clone(),
            MethodLimitStatus {
                daily_limit: cents_to_amount(global_daily_limit),
                used: cents_to_amount(used_cents),
                remaining: remaining_cents.map(cents_to_amount),
                available: effectively_available,
                single_min: cents_to_amount(single_min_cents),
                single_max: cents_to_amount(single_max_cents),
                fee_rate,
            },
        );
    }

    Ok(result)
}

async fn get_method_daily_limit(
    state: &AppState,
    payment_type: &str,
    override_enabled: bool,
) -> Result<i64> {
    let key = format!("MAX_DAILY_AMOUNT_{}", payment_type.to_ascii_uppercase());
    if let Some(value) = state.system_config.get(&key).await? {
        if let Some(cents) = amount_string_to_cents(&value) {
            return Ok(cents);
        }
    }

    if override_enabled {
        return Ok(0);
    }

    Ok(provider_default_limit(payment_type).daily_max_cents)
}

async fn get_method_single_limit(
    state: &AppState,
    payment_type: &str,
    override_enabled: bool,
) -> Result<i64> {
    let key = format!("MAX_SINGLE_AMOUNT_{}", payment_type.to_ascii_uppercase());
    if let Some(value) = state.system_config.get(&key).await? {
        if let Some(cents) = amount_string_to_cents(&value) {
            return Ok(cents);
        }
    }

    if override_enabled {
        return Ok(0);
    }

    Ok(provider_default_limit(payment_type).single_max_cents)
}

fn get_method_fee_rate(payment_type: &str) -> f64 {
    let method_key = format!("FEE_RATE_{}", payment_type.to_ascii_uppercase());
    if let Ok(value) = std::env::var(&method_key) {
        if let Ok(value) = value.parse::<f64>() {
            if value.is_finite() && value >= 0.0 {
                return value;
            }
        }
    }

    if let Some(provider_key) = provider_key_for_type(payment_type) {
        let provider_key = format!("FEE_RATE_PROVIDER_{}", provider_key.to_ascii_uppercase());
        if let Ok(value) = std::env::var(&provider_key) {
            if let Ok(value) = value.parse::<f64>() {
                if value.is_finite() && value >= 0.0 {
                    return value;
                }
            }
        }
    }

    0.0
}

fn aggregate_instance_limits(
    payment_types: &[String],
    active_instances: &[ActiveInstance],
    usage_by_instance: &HashMap<String, i64>,
) -> HashMap<String, InstanceAggregate> {
    let mut result = HashMap::new();

    for payment_type in payment_types {
        let provider_key = provider_key_for_type(payment_type);
        let supporting = active_instances
            .iter()
            .filter(|instance| {
                provider_key
                    .map(|provider_key| instance.provider_key == provider_key)
                    .unwrap_or(false)
                    && instance_supports_type(instance, payment_type)
            })
            .collect::<Vec<_>>();

        if supporting.is_empty() {
            result.insert(
                payment_type.clone(),
                InstanceAggregate {
                    single_min_cents: 0,
                    single_max_cents: 0,
                    all_instances_daily_blocked: false,
                    max_remaining_capacity_cents: None,
                    has_instances: false,
                },
            );
            continue;
        }

        let mut agg_single_min_cents = i64::MAX;
        let mut agg_single_max_cents = 0;
        let mut all_blocked = true;
        let mut max_remaining_capacity_cents: Option<i64> = None;

        for instance in supporting {
            let channel_limits = instance
                .limits
                .as_ref()
                .and_then(|limits| limits.get(payment_type))
                .and_then(JsonValue::as_object);

            let single_min_cents = channel_limits
                .and_then(|limits| limits.get("singleMin"))
                .and_then(json_number_to_cents)
                .unwrap_or(0);
            let single_max_cents = channel_limits
                .and_then(|limits| limits.get("singleMax"))
                .and_then(json_number_to_cents)
                .unwrap_or(0);
            let daily_limit_cents = channel_limits
                .and_then(|limits| limits.get("dailyLimit"))
                .and_then(json_number_to_cents)
                .unwrap_or(0);

            if single_min_cents == 0 {
                agg_single_min_cents = 0;
            } else if single_min_cents < agg_single_min_cents {
                agg_single_min_cents = single_min_cents;
            }
            if single_max_cents == 0 {
                agg_single_max_cents = 0;
            } else if agg_single_max_cents != 0 {
                agg_single_max_cents = agg_single_max_cents.max(single_max_cents);
            } else {
                agg_single_max_cents = single_max_cents;
            }

            if daily_limit_cents <= 0 {
                all_blocked = false;
                max_remaining_capacity_cents = None;
                continue;
            }

            let used_cents = *usage_by_instance.get(&instance.id).unwrap_or(&0);
            let remaining_cents = (daily_limit_cents - used_cents).max(0);
            let effective_single_min_cents = single_min_cents.max(0);

            if remaining_cents > effective_single_min_cents {
                all_blocked = false;
                if let Some(current) = max_remaining_capacity_cents {
                    max_remaining_capacity_cents = Some(current.max(remaining_cents));
                } else {
                    max_remaining_capacity_cents = Some(remaining_cents);
                }
            }
        }

        if agg_single_min_cents == i64::MAX {
            agg_single_min_cents = 0;
        }

        result.insert(
            payment_type.clone(),
            InstanceAggregate {
                single_min_cents: agg_single_min_cents,
                single_max_cents: agg_single_max_cents,
                all_instances_daily_blocked: all_blocked,
                max_remaining_capacity_cents,
                has_instances: true,
            },
        );
    }

    result
}

async fn query_usage_by_payment_type(
    state: &AppState,
    payment_types: &[String],
) -> Result<HashMap<String, i64>> {
    let biz_day_start = get_biz_day_start_utc_timestamp();
    let conn = state.db.connect_readonly().await?;
    let mut rows = conn
        .query(
            "SELECT payment_type, COALESCE(SUM(amount_cents), 0) FROM orders WHERE status IN (?1, ?2, ?3) AND paid_at >= ?4 GROUP BY payment_type",
            Params::Positional(vec![
                Value::Text(STATUS_PAID.to_string()),
                Value::Text(STATUS_RECHARGING.to_string()),
                Value::Text(STATUS_COMPLETED.to_string()),
                Value::Integer(biz_day_start),
            ]),
        )
        .await
        .context("failed to query usage by payment type")?;

    let mut usage = HashMap::new();
    while let Some(row) = rows
        .next()
        .await
        .context("failed to iterate payment type usage rows")?
    {
        let payment_type = read_required_string(&row.get_value(0)?)?;
        if payment_types
            .iter()
            .any(|candidate| candidate == &payment_type)
        {
            usage.insert(payment_type, read_required_i64(&row.get_value(1)?)?);
        }
    }
    Ok(usage)
}

async fn query_usage_by_provider_instance(state: &AppState) -> Result<HashMap<String, i64>> {
    let biz_day_start = get_biz_day_start_utc_timestamp();
    let conn = state.db.connect_readonly().await?;
    let mut rows = conn
        .query(
            "SELECT provider_instance_id, COALESCE(SUM(pay_amount_cents), 0) FROM orders WHERE provider_instance_id IS NOT NULL AND status IN (?1, ?2, ?3) AND paid_at >= ?4 GROUP BY provider_instance_id",
            Params::Positional(vec![
                Value::Text(STATUS_PAID.to_string()),
                Value::Text(STATUS_RECHARGING.to_string()),
                Value::Text(STATUS_COMPLETED.to_string()),
                Value::Integer(biz_day_start),
            ]),
        )
        .await
        .context("failed to query usage by provider instance")?;

    let mut usage = HashMap::new();
    while let Some(row) = rows
        .next()
        .await
        .context("failed to iterate provider instance usage rows")?
    {
        usage.insert(
            read_required_string(&row.get_value(0)?)?,
            read_required_i64(&row.get_value(1)?)?,
        );
    }
    Ok(usage)
}

fn resolve_stripe_publishable_key(
    state: &AppState,
    enabled_types: &[String],
    active_instances: &[ActiveInstance],
) -> Result<Option<String>> {
    if !enabled_types
        .iter()
        .any(|payment_type| payment_type == "stripe")
    {
        return Ok(None);
    }

    for instance in active_instances {
        if instance.provider_key != "stripe" {
            continue;
        }
        let plaintext = crypto::decrypt(
            state.config.admin_token.as_deref(),
            &instance.encrypted_config,
        )?;
        let config: HashMap<String, String> = serde_json::from_str(&plaintext)
            .context("failed to parse stripe provider instance config")?;
        if let Some(value) = config
            .get("publishableKey")
            .cloned()
            .filter(|value| !value.is_empty())
        {
            return Ok(Some(value));
        }
    }

    Ok(state.config.stripe_publishable_key.clone())
}

fn get_biz_day_start_utc_timestamp() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock drifted before unix epoch")
        .as_secs() as i64;
    let biz_day = (now + BIZ_OFFSET_SECONDS) / 86_400;
    biz_day * 86_400 - BIZ_OFFSET_SECONDS
}

fn amount_string_to_cents(value: &str) -> Option<i64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(amount_to_cents)
}

fn json_number_to_cents(value: &JsonValue) -> Option<i64> {
    value
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(amount_to_cents)
}

fn amount_to_cents(value: f64) -> i64 {
    (value * 100.0).round() as i64
}

fn cents_to_amount(value: i64) -> f64 {
    (value as f64) / 100.0
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
        Value::Real(value) => Ok(*value as i64),
        _ => Err(anyhow!("unexpected non-integer value")),
    }
}
