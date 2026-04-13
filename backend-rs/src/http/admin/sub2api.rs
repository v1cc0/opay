use axum::{
    Json, Router,
    extract::{Query, State},
    http::HeaderMap,
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    admin_auth::verify_admin_values,
    error::{AppError, AppResult},
    sub2api::Sub2ApiSearchUser,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/admin/sub2api/groups", get(get_groups))
        .route("/api/admin/sub2api/search-users", get(search_users))
}

#[derive(Debug, Deserialize)]
struct AdminSub2ApiQuery {
    token: Option<String>,
    lang: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchUsersQuery {
    token: Option<String>,
    lang: Option<String>,
    keyword: Option<String>,
}

#[derive(Debug, Serialize)]
struct GroupsResponse<T> {
    groups: Vec<T>,
}

#[derive(Debug, Serialize)]
struct UsersResponse {
    users: Vec<Sub2ApiSearchUser>,
}

async fn get_groups(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminSub2ApiQuery>,
) -> AppResult<Json<GroupsResponse<crate::sub2api::Sub2ApiGroup>>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let sub2api = state
        .sub2api
        .as_ref()
        .ok_or_else(|| AppError::public_internal("获取 Sub2API 分组列表失败"))?;
    let admin_api_key = sub2api_admin_api_key(&state).await?;
    let groups = sub2api
        .get_all_groups(&admin_api_key)
        .await
        .map_err(|_| AppError::public_internal("获取 Sub2API 分组列表失败"))?;

    Ok(Json(GroupsResponse { groups }))
}

async fn search_users(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchUsersQuery>,
) -> AppResult<Json<UsersResponse>> {
    verify_admin_values(
        &headers,
        query.token.as_deref(),
        query.lang.as_deref(),
        &state,
    )
    .await?;

    let keyword = query.keyword.as_deref().map(str::trim).unwrap_or_default();
    if keyword.is_empty() {
        return Ok(Json(UsersResponse { users: Vec::new() }));
    }

    let sub2api = state
        .sub2api
        .as_ref()
        .ok_or_else(|| AppError::public_internal("搜索用户失败"))?;
    let admin_api_key = sub2api_admin_api_key(&state).await?;
    let users = sub2api
        .search_users(keyword, &admin_api_key)
        .await
        .map_err(|_| AppError::public_internal("搜索用户失败"))?;

    Ok(Json(UsersResponse { users }))
}

async fn sub2api_admin_api_key(state: &AppState) -> AppResult<String> {
    let value = state
        .system_config
        .get("SUB2API_ADMIN_API_KEY")
        .await
        .map_err(AppError::internal)?
        .unwrap_or_default();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::public_internal(
            "Sub2API admin api key is not configured",
        ));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::{Json, Router, routing::get};
    use serde_json::json;
    use tokio::{net::TcpListener, task::JoinHandle};
    use uuid::Uuid;

    use super::*;
    use crate::{
        config::AppConfig,
        db::DatabaseHandle,
        order::{audit::AuditLogRepository, repository::OrderRepository, service::OrderService},
        sub2api::Sub2ApiClient,
        subscription_plan::SubscriptionPlanRepository,
        system_config::{SystemConfigService, UpsertSystemConfig},
    };

    async fn test_state(sub2api_base_url: Option<String>) -> AppState {
        let db_path = std::env::temp_dir().join(format!(
            "sub2apipay-admin-sub2api-routes-{}.db",
            Uuid::new_v4()
        ));
        let db = DatabaseHandle::open_local(&db_path).await.unwrap();
        db.run_migrations().await.unwrap();

        let config = Arc::new(AppConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            db_path,
            payment_providers: Vec::new(),
            admin_token: Some("test-admin-token".to_string()),
            system_config_cache_ttl_secs: 1,
            sub2api_base_url: sub2api_base_url.clone(),
            sub2api_timeout_secs: 2,
            min_recharge_amount: 1.0,
            max_recharge_amount: 1000.0,
            max_daily_recharge_amount: 10000.0,
            pay_help_image_url: None,
            pay_help_text: None,
            stripe_publishable_key: None,
        });

        let system_config = SystemConfigService::new(db.clone(), Duration::from_secs(1));
        let sub2api = sub2api_base_url.map(|base_url| Sub2ApiClient::new(base_url, 2));

        AppState {
            config: Arc::clone(&config),
            db: db.clone(),
            system_config: system_config.clone(),
            sub2api: sub2api.clone(),
            order_service: OrderService::new(
                Arc::clone(&config),
                OrderRepository::new(db.clone()),
                AuditLogRepository::new(db.clone()),
                SubscriptionPlanRepository::new(db.clone()),
                system_config,
                sub2api,
            ),
        }
    }

    fn admin_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer test-admin-token".parse().unwrap());
        headers
    }

    async fn start_mock_sub2api() -> (String, JoinHandle<()>) {
        async fn groups() -> Json<serde_json::Value> {
            Json(json!({
                "data": [
                    {
                        "id": 1,
                        "name": "OpenAI",
                        "status": "active",
                        "platform": "openai",
                        "subscription_type": "subscription"
                    }
                ]
            }))
        }

        async fn users() -> Json<serde_json::Value> {
            Json(json!({
                "data": {
                    "items": [
                        {
                            "id": 7,
                            "email": "demo@example.com",
                            "username": "demo",
                            "notes": "vip"
                        }
                    ]
                }
            }))
        }

        let app = Router::new()
            .route("/api/v1/admin/groups/all", get(groups))
            .route("/api/v1/admin/users", get(users));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn returns_groups_and_search_users() {
        let (base_url, handle) = start_mock_sub2api().await;
        let state = test_state(Some(base_url)).await;
        state
            .system_config
            .set_many(&[UpsertSystemConfig {
                key: "SUB2API_ADMIN_API_KEY".to_string(),
                value: "test-admin-key".to_string(),
                group: Some("payment".to_string()),
                label: None,
            }])
            .await
            .unwrap();

        let groups = get_groups(
            State(state.clone()),
            admin_headers(),
            Query(AdminSub2ApiQuery {
                token: None,
                lang: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(groups.groups.len(), 1);

        let users = search_users(
            State(state),
            admin_headers(),
            Query(SearchUsersQuery {
                token: None,
                lang: None,
                keyword: Some("demo".to_string()),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(users.users.len(), 1);
        assert_eq!(users.users[0].username.as_deref(), Some("demo"));

        handle.abort();
    }
}
