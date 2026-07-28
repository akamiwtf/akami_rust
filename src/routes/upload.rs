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

/// The display name for a stored file: the timestamp `upload` appends is stripped
/// again, so a download is saved as `notes.txt` rather than `notes_1785163354170.txt`.
pub fn pretty_name(stored: &str) -> String {
    // Look for the `_<millis>` group and cut it out. Scanning the whole name rather
    // than only the part before the last dot keeps `archive.tar_<ts>.gz` working.
    let bytes = stored.as_bytes();
    for (i, _) in stored.char_indices().filter(|(_, c)| *c == '_') {
        let digits = bytes[i + 1..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        let after = i + 1 + digits;
        let ends_here = after == stored.len() || bytes[after] == b'.';
        if i > 0 && digits >= 10 && ends_here {
            return format!("{}{}", &stored[..i], &stored[after..]);
        }
    }
    stored.to_string()
}

#[cfg(test)]
mod tests {
    use super::pretty_name;

    #[test]
    fn strips_the_stored_timestamp() {
        assert_eq!(pretty_name("notes_1785163354170.txt"), "notes.txt");
        assert_eq!(pretty_name("my_notes_1785163354170.txt"), "my_notes.txt");
        assert_eq!(pretty_name("archive.tar_1785163354170.gz"), "archive.tar.gz");
        assert_eq!(pretty_name("setup_1785163354170"), "setup");
        // Names that were never stamped are left alone.
        assert_eq!(pretty_name("notes.txt"), "notes.txt");
        assert_eq!(pretty_name("build_2.exe"), "build_2.exe");
        assert_eq!(pretty_name("_1785163354170.txt"), "_1785163354170.txt");
    }
}
