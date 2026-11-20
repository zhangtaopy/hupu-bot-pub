use super::types::AppState;
use crate::services::history::{self, HistoryRequest};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::sync::Arc;

pub async fn history(
    State(state): State<Arc<AppState>>,
    Json(mut request): Json<HistoryRequest>,
) -> Response {
    if let Err(e) = request.normalize() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response();
    }
    match run(state, request).await {
        Ok(result) => Json(result).into_response(),
        Err(e) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn run(state: Arc<AppState>, request: HistoryRequest) -> anyhow::Result<history::Preview> {
    let path = state.db_path.clone();
    let query = request.clone();
    let inputs =
        tokio::task::spawn_blocking(move || history::load(&crate::db::open_db(&path)?, &query))
            .await??;
    if !request.generate
        || (inputs.preview.cached && !inputs.preview.stale && !request.refresh)
        || inputs.evidence.is_empty()
    {
        return Ok(inputs.preview);
    }
    let supplied = request
        .api_key
        .as_deref()
        .filter(|key| !key.trim().is_empty())
        .map(|key| {
            crate::deepseek::AiProvider::from_user_input(
                request.provider.as_deref().unwrap_or("deepseek"),
                key.trim(),
            )
        });
    let provider = crate::resolver::resolve_ai_provider(supplied).map_err(anyhow::Error::msg)?;
    // A generation starts with a fresh pool, independent of stale connections
    // held by a long-running server after network/proxy changes.
    let http_client = crate::deepseek::history_http_client()?;
    let provider_label = match &provider {
        crate::deepseek::AiProvider::DeepSeek { .. } => "DeepSeek",
        crate::deepseek::AiProvider::Ollama { .. } => "Ollama",
        crate::deepseek::AiProvider::OpenRouter { .. } => "OpenRouter",
        crate::deepseek::AiProvider::OpenCode { .. } => "OpenCode",
    };
    let analysis = history::generate(&http_client, &provider, &request, &inputs)
        .await
        .map_err(|error| {
            // generate returns only authored, redacted error messages, never upstream bodies.
            eprintln!("[history] provider={provider_label} generation failed: {error}");
            if inputs.preview.report.is_some() {
                anyhow::anyhow!("{error}；已有报告仍保留")
            } else {
                error
            }
        })?;
    let path = state.db_path.clone();
    tokio::task::spawn_blocking(move || {
        history::save(&crate::db::open_db(&path)?, &request, inputs, analysis)
    })
    .await?
}
