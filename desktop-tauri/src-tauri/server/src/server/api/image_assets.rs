//! 内联图片的短期下载链接；只驻留内存，以随机能力令牌访问，不落盘或携带网关 Key。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use axum::extract::Path;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;

use crate::server::errors::GatewayError;
use crate::server::logging;

const TTL_MS: i64 = 60 * 60 * 1000;
const MAX_CACHE_BYTES: usize = 128 * 1024 * 1024;

struct Asset {
    bytes: Bytes,
    mime: &'static str,
    expires_at: i64,
}

fn cache() -> &'static Mutex<HashMap<String, Asset>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Asset>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn store(images: Vec<(Vec<u8>, &'static str)>) -> Result<Vec<String>, GatewayError> {
    let mut assets = Vec::with_capacity(images.len());
    let expires_at = logging::now_ms() + TTL_MS;
    for (bytes, mime) in images {
        let mut random = [0u8; 32];
        getrandom::getrandom(&mut random)
            .map_err(|_| GatewayError::with_status(500, "无法生成安全的图片下载令牌"))?;
        let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        assets.push((
            id,
            Asset {
                bytes: Bytes::from(bytes),
                mime,
                expires_at,
            },
        ));
    }
    let mut cache = cache()
        .lock()
        .map_err(|_| GatewayError::with_status(503, "图片下载缓存暂不可用"))?;
    cache.retain(|_, asset| asset.expires_at > logging::now_ms());
    let used: usize = cache.values().map(|asset| asset.bytes.len()).sum();
    let added: usize = assets.iter().map(|(_, asset)| asset.bytes.len()).sum();
    if used.saturating_add(added) > MAX_CACHE_BYTES {
        return Err(GatewayError::with_status(
            503,
            "图片下载缓存已满，请改用 response_format=b64_json",
        ));
    }
    let ids = assets.iter().map(|(id, _)| id.clone()).collect();
    cache.extend(assets);
    Ok(ids)
}

pub(super) fn read(id: &str) -> Option<(Bytes, &'static str)> {
    cache().lock().ok().and_then(|mut cache| {
        cache.retain(|_, asset| asset.expires_at > logging::now_ms());
        cache.get(id).map(|asset| (asset.bytes.clone(), asset.mime))
    })
}

pub async fn download(Path(id): Path<String>) -> Response {
    let Some((bytes, mime)) = read(&id) else {
        return GatewayError::with_status(404, "图片下载链接不存在或已过期").payload_response();
    };
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    (StatusCode::OK, headers, bytes).into_response()
}
