use axum::{Json, http::StatusCode};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct ConfigStatusResponse {
    pub configured: bool,
    pub has_cookie: bool,
    pub has_deepseek_key: bool,
    pub has_ollama_key: bool,
    pub has_openrouter_key: bool,
}

pub async fn get_config_status() -> Json<ConfigStatusResponse> {
    match crate::config::try_get() {
        Some(cfg) => Json(ConfigStatusResponse {
            configured: true,
            has_cookie: !cfg.cookie.is_empty(),
            has_deepseek_key: !cfg.deepseek_api_key.is_empty(),
            has_ollama_key: !cfg.ollama_api_key.is_empty(),
            has_openrouter_key: !cfg.openrouter_api_key.is_empty(),
        }),
        None => Json(ConfigStatusResponse {
            configured: false,
            has_cookie: false,
            has_deepseek_key: false,
            has_ollama_key: false,
            has_openrouter_key: false,
        }),
    }
}

#[derive(Deserialize)]
pub struct SaveConfigRequest {
    pub cookie: String,
    #[serde(default)]
    pub deepseek_api_key: String,
    #[serde(default)]
    pub ollama_api_key: String,
    #[serde(default)]
    pub ollama_model: String,
    #[serde(default)]
    pub deepseek_model: String,
    #[serde(default)]
    pub openrouter_api_key: String,
    #[serde(default)]
    pub openrouter_model: String,
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
    let deepseek_api_key = body.deepseek_api_key.trim().to_string();
    let ollama_api_key = body.ollama_api_key.trim().to_string();
    let ollama_model = body.ollama_model.trim().to_string();
    let deepseek_model = body.deepseek_model.trim().to_string();
    let openrouter_api_key = body.openrouter_api_key.trim().to_string();
    let openrouter_model = body.openrouter_model.trim().to_string();

    // Key-only update: cookie already configured, just update the API keys
    if cookie.is_empty() && (!deepseek_api_key.is_empty() || !ollama_api_key.is_empty() || !openrouter_api_key.is_empty()) {
        if let Some(mut existing) = crate::config::try_get() {
            if !deepseek_api_key.is_empty() {
                existing.deepseek_api_key = deepseek_api_key;
            }
            if !ollama_api_key.is_empty() {
                existing.ollama_api_key = ollama_api_key;
            }
            if !ollama_model.is_empty() {
                existing.ollama_model = ollama_model;
            }
            if !deepseek_model.is_empty() {
                existing.deepseek_model = deepseek_model;
            }
            if !openrouter_api_key.is_empty() {
                existing.openrouter_api_key = openrouter_api_key;
            }
            if !openrouter_model.is_empty() {
                existing.openrouter_model = openrouter_model;
            }
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
        deepseek_api_key,
        deepseek_max_concurrency: 3,
        ollama_api_key,
        ollama_model,
        deepseek_model,
        openrouter_api_key,
        openrouter_model,
    };

    crate::config::save(&config)
        .map(|_| Json(serde_json::json!({ "success": true })))
        .map_err(|e| internal_error(&format!("保存配置失败: {}", e)))
}