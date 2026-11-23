use super::types::AppState;
use crate::{deepseek, resolver, services::thread_review, topic};

use axum::{extract::State, http::StatusCode, Json};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

static POST_INPUT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:https?://(?:bbs\.hupu\.com/|m\.hupu\.com/bbs/))?([0-9]{9})(?:-[0-9]+)?(?:\.html)?(?:[?#][^\s]*)?$").unwrap()
});

#[derive(Deserialize)]
pub struct Request {
    post: String,
    #[serde(default)]
    generate: bool,
    cookie: Option<String>,
    api_key: Option<String>,
    provider: Option<String>,
}

fn supplied_provider(key: Option<&str>, name: Option<&str>) -> Option<deepseek::AiProvider> {
    key.map(str::trim)
        .filter(|key| !key.is_empty())
        .map(|key| deepseek::AiProvider::from_user_input(name.unwrap_or("deepseek"), key))
}

pub async fn opinions(
    State(state): State<Arc<AppState>>,
    Json(req): Json<Request>,
) -> std::result::Result<Json<Value>, (StatusCode, Json<Value>)> {
    let invalid = |msg: &str| (StatusCode::BAD_REQUEST, Json(json!({"error": msg})));
    let caps = POST_INPUT
        .captures(req.post.trim())
        .ok_or_else(|| invalid("请输入虎扑帖子链接或九位帖子 ID"))?;
    let tid = &caps[1];
    let client = resolver::create_hupu_client(req.cookie.as_deref()).map_err(|e| invalid(&e))?;
    let thread = topic::full::fetch(&client, tid).await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error":format!("帖子没读完：{e:#}")})),
        )
    })?;
    let mut review = None;
    let mut analysis_error = None;
    let mut hot_points = std::collections::HashMap::new();
    let mut hot_error = None;
    if req.generate {
        let supplied = supplied_provider(req.api_key.as_deref(), req.provider.as_deref());
        match resolver::resolve_ai_provider_with_concurrency(supplied) {
            Ok((provider, concurrency)) => {
                let (main, hot) = tokio::join!(
                    thread_review::generate(&state.http_client, &provider, concurrency, &thread),
                    thread_review::hot_points(&state.http_client, &provider, &thread)
                );
                match main {
                    Ok(result) => review = Some(result),
                    Err(e) => analysis_error = Some(e.to_string()),
                }
                match hot {
                    Ok(points) => hot_points = points,
                    Err(e) => hot_error = Some(e.to_string()),
                }
            }
            Err(error) => analysis_error = Some(error.to_string()),
        }
    }
    Ok(Json(
        json!({"title":thread.title,"total_replies":thread.total_replies,"pages":thread.pages,
        "sources":thread.sources,"floor_count":thread.floor_count,"hot_ids":thread.hot_ids,"hot_points":hot_points,"hot_error":hot_error,"review":review,"analysis_error":analysis_error}),
    ))
}

#[derive(Deserialize)]
pub struct BranchRequest {
    title: String,
    root_id: String,
    root_text: String,
    items: Vec<thread_review::BranchItem>,
    api_key: Option<String>,
    provider: Option<String>,
}

pub async fn branch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BranchRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let error = |msg: String| (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":msg})));
    if req.items.is_empty() || req.items.len() > 24 {
        return Err(error("请选择一组引用回复".into()));
    }
    let provider = resolver::resolve_ai_provider(supplied_provider(
        req.api_key.as_deref(),
        req.provider.as_deref(),
    ))
    .map_err(|e| error(e.into()))?;
    let positions = thread_review::classify(
        &state.http_client,
        &provider,
        &req.title,
        &req.root_id,
        &req.root_text,
        &req.items,
    )
    .await
    .map_err(|e| error(e.to_string()))?;
    Ok(Json(json!({"stances":positions})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_post_urls_and_ids() {
        for s in [
            "638890883",
            "https://bbs.hupu.com/638890883-2.html?pid=123",
            "https://m.hupu.com/bbs/638890883.html",
        ] {
            assert_eq!(&POST_INPUT.captures(s).unwrap()[1], "638890883");
        }
        for s in [
            "278",
            "https://example.com/638890883.html",
            "https://bbs.hupu.com.example.org/638890883.html",
        ] {
            assert!(!POST_INPUT.is_match(s));
        }
    }
}
