use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::{json, Value};

use super::dto::{err, ApiResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct UploadBody {
    filename: Option<String>,
    #[serde(rename = "fileData")]
    file_data: Option<String>,
}

/// Splits a filename into (base, ext) the way Node's path.basename/extname do:
/// ext includes the leading dot; a dotfile with no other dot has empty ext.
fn split_name(filename: &str) -> (String, String) {
    let name = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(filename);
    match name.rfind('.') {
        Some(idx) if idx > 0 => (name[..idx].to_string(), name[idx..].to_string()),
        _ => (name.to_string(), String::new()),
    }
}

pub async fn upload(
    State(_st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UploadBody>,
) -> ApiResult<Json<Value>> {
    let (Some(filename), Some(file_data)) = (body.filename, body.file_data) else {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Filename and fileData (base64) are required",
        ));
    };

    let buffer = STANDARD
        .decode(file_data.as_bytes())
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to upload file"))?;

    let (base, ext) = split_name(&filename);
    let ts = chrono::Utc::now().timestamp_millis();
    let new_filename = format!("{base}_{ts}{ext}");
    let path = std::path::Path::new("uploads").join(&new_filename);

    std::fs::write(&path, &buffer)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to upload file"))?;

    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let url = format!("{proto}://{host}/uploads/{new_filename}");

    Ok(Json(json!({ "url": url })))
}
