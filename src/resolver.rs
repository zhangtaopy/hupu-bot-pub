use crate::client::HupuClient;
use crate::deepseek::AiProvider;

// ── Cookie ──

/// Resolve cookie string: user-supplied override → config.json fallback.
/// Returns `Ok(cookie)` or `Err(human-readable error)`.
pub fn resolve_cookie(override_cookie: Option<&str>) -> Result<String, &'static str> {
    if let Some(c) = override_cookie.filter(|c| !c.is_empty()) {
        return Ok(c.to_string());
    }
    match crate::config::try_get() {
        Some(cfg) if !cfg.cookie.is_empty() => Ok(cfg.cookie),
        _ => Err("请先配置 Cookie"),
    }
}

/// Check whether any cookie is available (user override or config), without extracting the value.
pub fn has_cookie(override_cookie: Option<&str>) -> bool {
    override_cookie.filter(|c| !c.is_empty()).is_some()
        || crate::config::try_get().map(|c| !c.cookie.is_empty()).unwrap_or(false)
}

/// Resolve cookie and create a `HupuClient` in one step.
pub fn create_hupu_client(override_cookie: Option<&str>) -> Result<HupuClient, String> {
    let cookie = resolve_cookie(override_cookie).map_err(|e| e.to_string())?;
    HupuClient::new(&cookie).map_err(|e| format!("创建客户端失败: {}", e))
}

// ── AI Provider ──

/// Resolve AI provider: user-supplied → config.json fallback.
pub fn resolve_ai_provider(user_provider: Option<AiProvider>) -> Result<AiProvider, &'static str> {
    if let Some(p) = user_provider {
        return Ok(p);
    }
    let cfg = crate::config::try_get().ok_or("请先配置 AI API Key")?;
    if cfg.api_key.is_empty() {
        return Err("未配置 AI API Key");
    }
    Ok(cfg.ai_provider())
}

/// Resolve AI provider AND `max_concurrency` in one call.
/// Used by the batch-analysis services that need both values.
pub fn resolve_ai_provider_with_concurrency(
    user_provider: Option<AiProvider>,
) -> Result<(AiProvider, usize), &'static str> {
    let provider = resolve_ai_provider(user_provider)?;
    let max_concurrency = crate::config::try_get()
        .map(|c| c.max_concurrency.max(1))
        .unwrap_or(3);
    Ok((provider, max_concurrency))
}
