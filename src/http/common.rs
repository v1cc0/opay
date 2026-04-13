use chrono::{TimeZone, Utc};

pub fn amount_to_cents(value: f64) -> i64 {
    (value * 100.0).round() as i64
}

pub fn cents_to_amount(value: i64) -> f64 {
    (value as f64) / 100.0
}

pub fn resolve_locale(lang: Option<&str>) -> &'static str {
    match lang.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("en") => "en",
        _ => "zh",
    }
}

pub fn message(locale: &str, zh: &'static str, en: &'static str) -> &'static str {
    if locale == "en" { en } else { zh }
}

pub fn is_false(value: &bool) -> bool {
    !*value
}

pub fn timestamp_to_rfc3339(value: i64) -> String {
    Utc.timestamp_opt(value, 0)
        .single()
        .map(|item| item.to_rfc3339())
        .unwrap_or_else(|| value.to_string())
}

pub fn optional_timestamp_to_rfc3339(value: Option<i64>) -> Option<String> {
    value.map(timestamp_to_rfc3339)
}
