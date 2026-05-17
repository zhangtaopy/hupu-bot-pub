use axum::{Json, http::StatusCode};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct ConfigStatusResponse {
    pub configured: bool,
    pub has_cookie: bool,
    pub has_api_key: bool,
}

pub async fn get_config_status() -> Json<ConfigStatusResponse> {
    match crate::config::try_get() {
        Some(cfg) => Json(ConfigStatusResponse {
            configured: true,
            has_cookie: !cfg.cookie.is_empty(),
            has_api_key: !cfg.api_key.is_empty(),
        }),
        None => Json(ConfigStatusResponse {
            configured: false,
            has_cookie: false,
            has_api_key: false,
        }),
    }
}

#[derive(Deserialize)]
pub struct SaveConfigRequest {
    pub cookie: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
}

type ApiResult = Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)>;

fn bad_request(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": msg })),
    )
}

fn internal_error(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": msg })),
    )
}

pub async fn save_config(
    Json(body): Json<SaveConfigRequest>,
) -> ApiResult {
    let cookie = body.cookie.trim().to_string();
    let provider = body.provider.trim().to_string();
    let api_key = body.api_key.trim().to_string();
    let model = body.model.trim().to_string();

    // Key-only update: cookie already configured, just update the AI config
    if cookie.is_empty() && !api_key.is_empty() {
        if let Some(mut existing) = crate::config::try_get() {
            existing.provider = provider;
            existing.api_key = api_key;
            existing.model = model;
            return crate::config::save(&existing)
                .map(|_| Json(serde_json::json!({ "success": true })))
                .map_err(|e| internal_error(&format!("保存配置失败: {}", e)));
        }
        return Err(bad_request("无法更新：配置尚未初始化"));
    }

    if cookie.is_empty() {
        return Err(bad_request("Cookie 不能为空"));
    }

    // 校验 smidV2 字段
    if crate::config::Config::parse_smid_from_cookie(&cookie).is_err() {
        return Err(bad_request(
            "Cookie 中缺少 smidV2 字段，请确保已登录虎扑并复制完整 Cookie",
        ));
    }

    // 解析 puid（可选，解析失败不阻断）
    let puid: String = crate::config::Config::parse_puid_from_cookie(&cookie).unwrap_or_default();
    let shumei_id = crate::config::Config::parse_smid_from_cookie(&cookie).unwrap_or_default();

    let config = crate::config::Config {
        cookie,
        shumei_id,
        puid,
        provider,
        api_key,
        model,
        max_concurrency: 3,
    };

    crate::config::save(&config)
        .map(|_| Json(serde_json::json!({ "success": true })))
        .map_err(|e| internal_error(&format!("保存配置失败: {}", e)))
}
