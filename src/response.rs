use axum::Json;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub success: bool,
}

#[derive(Debug, Serialize)]
pub struct UpdatedResponse {
    pub success: bool,
    pub updated: usize,
}

pub fn success() -> Json<SuccessResponse> {
    Json(SuccessResponse { success: true })
}

pub fn updated(updated: usize) -> Json<UpdatedResponse> {
    Json(UpdatedResponse {
        success: true,
        updated,
    })
}
