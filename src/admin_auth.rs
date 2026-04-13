use axum::{extract::Query, http::HeaderMap};
use serde::Deserialize;

use crate::{
    AppState,
    error::{AppError, AppResult},
};

#[derive(Debug, Deserialize, Default)]
pub struct AdminTokenQuery {
    pub token: Option<String>,
    pub lang: Option<String>,
}

pub async fn verify_admin(
    headers: &HeaderMap,
    Query(query): Query<AdminTokenQuery>,
    state: &AppState,
) -> AppResult<()> {
    verify_admin_values(
        headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        state,
    )
    .await
}

pub async fn verify_admin_values(
    headers: &HeaderMap,
    query_token: Option<&str>,
    lang: Option<&str>,
    state: &AppState,
) -> AppResult<()> {
    let token = extract_token(headers, query_token)
        .ok_or_else(|| AppError::unauthorized(unauthorized_message(lang)))?;

    let expected = state
        .config
        .admin_token
        .as_deref()
        .ok_or_else(|| AppError::unauthorized("Admin auth is not configured"))?;

    if secure_equals(expected, token) {
        return Ok(());
    }

    Err(AppError::unauthorized(unauthorized_message(lang)))
}

fn extract_token<'a>(headers: &'a HeaderMap, query_token: Option<&'a str>) -> Option<&'a str> {
    if let Some(value) = headers.get(axum::http::header::AUTHORIZATION) {
        if let Ok(text) = value.to_str() {
            if let Some(token) = text.strip_prefix("Bearer ") {
                let trimmed = token.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
    }

    query_token.map(str::trim).filter(|token| !token.is_empty())
}

fn unauthorized_message(lang: Option<&str>) -> &'static str {
    match lang.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("en") => "Unauthorized",
        _ => "未授权",
    }
}

fn secure_equals(expected: &str, received: &str) -> bool {
    let expected = expected.as_bytes();
    let received = received.as_bytes();

    let max_len = expected.len().max(received.len());
    let mut diff = expected.len() ^ received.len();

    for index in 0..max_len {
        let left = *expected.get(index).unwrap_or(&0);
        let right = *received.get(index).unwrap_or(&0);
        diff |= (left ^ right) as usize;
    }

    diff == 0
}
