use axum::{
    extract::Query,
    http::StatusCode,
    response::{Html, Json},
};
use std::sync::Arc;

use super::types::*;

const INDEX_HTML: &str = include_str!("../../web/index.html");

pub async fn get_index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

pub async fn get_user(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<EuidQuery>,
) -> Result<Json<UserResponse>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let replies = crate::db::query_replies(&conn, Some(&params.euid), 1, 0)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let username = replies
        .first()
        .map(|r| r.username.clone())
        .unwrap_or_else(|| "未知用户".to_string());
    Ok(Json(UserResponse {
        euid: params.euid,
        username,
    }))
}

pub async fn get_stats(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<EuidQuery>,
) -> Result<Json<StatsResponse>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let total =
        crate::db::count_replies(&conn, Some(&params.euid)).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let topic_distribution =
        crate::db::query_topic_distribution(&conn, &params.euid).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let time_distribution =
        crate::db::query_time_distribution(&conn, &params.euid).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (repeated, unique, repeat_rate, similarity_available) = {
        let key = format!("{}:{}", params.euid, 0.5);
        let cached_groups = state
            .results
            .lock()
            .ok()
            .and_then(|r| r.get(&key).map(|res| res.groups.clone()));

        match cached_groups {
            Some(groups) => {
                let r: usize = groups.iter().map(|g| g.count).sum();
                let u = total as usize - r;
                let rate = if total > 0 {
                    r as f64 / total as f64
                } else {
                    0.0
                };
                (r, u, rate, true)
            }
            None => {
                if let Ok(conn2) = crate::db::open_db(&state.db_path) {
                    if let Ok(Some(result_json)) =
                        crate::db::get_similarity_analysis(&conn2, &params.euid, 0.5)
                    {
                        if let Ok(db_res) = serde_json::from_str::<AnalyzeResponse>(&result_json) {
                            let r: usize = db_res.groups.iter().map(|g| g.count).sum();
                            let u = total as usize - r;
                            let rate = if total > 0 {
                                r as f64 / total as f64
                            } else {
                                0.0
                            };
                            return Ok(Json(StatsResponse {
                                total_replies: total as usize,
                                unique_replies: u,
                                repeated_replies: r,
                                repeat_rate: rate,
                                topic_distribution,
                                time_distribution,
                                similarity_available: true,
                            }));
                        }
                    }
                }
                (0, 0, 0.0, false)
            }
        }
    };

    Ok(Json(StatsResponse {
        total_replies: total as usize,
        unique_replies: unique,
        repeated_replies: repeated,
        repeat_rate,
        topic_distribution,
        time_distribution,
        similarity_available,
    }))
}

pub async fn get_similarity_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<AnalyzeQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("{}:{}", params.euid, params.threshold);

    {
        let results = state
            .results
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if let Some(res) = results.get(&key) {
            return Ok(Json(serde_json::json!({
                "status": "done",
                "key": key,
                "total_replies": res.total_replies,
                "groups": res.groups,
            })));
        }
    }

    if let Ok(conn) = crate::db::open_db(&state.db_path) {
        if let Ok(Some(result_json)) =
            crate::db::get_similarity_analysis(&conn, &params.euid, params.threshold)
        {
            if let Ok(res) = serde_json::from_str::<AnalyzeResponse>(&result_json) {
                {
                    let mut results = state.results.lock().ok();
                    if let Some(ref mut r) = results {
                        r.insert(key.clone(), res.clone());
                    }
                }
                return Ok(Json(serde_json::json!({
                    "status": "done",
                    "key": key,
                    "total_replies": res.total_replies,
                    "groups": res.groups,
                })));
            }
        }
    }

    Ok(Json(serde_json::json!({
        "status": "not_found",
        "key": key,
    })))
}

pub async fn start_similarity_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<AnalyzeQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("{}:{}", params.euid, params.threshold);

    {
        let progress = state
            .progress
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if let Some(p) = progress.get(&key) {
            if !p.done {
                return Ok(Json(serde_json::json!({
                    "status": "running",
                    "key": key,
                })));
            }
        }
    }

    let state_clone = state.clone();
    let params_clone = AnalyzeQuery {
        euid: params.euid.clone(),
        threshold: params.threshold,
    };
    tokio::spawn(async move {
        crate::services::analyze::run_analysis_background(state_clone, params_clone).await;
    });

    Ok(Json(serde_json::json!({
        "status": "started",
        "key": key,
    })))
}

pub async fn get_analysis_progress(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<AnalyzeQuery>,
) -> Result<Json<ProgressState>, StatusCode> {
    let key = format!("{}:{}", params.euid, params.threshold);
    let progress = state
        .progress
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        progress.get(&key).cloned().unwrap_or(ProgressState {
            phase: "idle".into(),
            current: 0,
            total: 0,
            done: false,
            error: None,
        }),
    ))
}

pub async fn get_replies(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<RepliesQuery>,
) -> Result<Json<RepliesResponse>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let total =
        crate::db::count_replies(&conn, Some(&params.euid)).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let replies = crate::db::query_replies(&conn, Some(&params.euid), params.limit, params.offset)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let items: Vec<ReplyItem> = replies
        .iter()
        .map(|r| {
            let format_time = if let Some(ft) = &r.format_time {
                if !ft.is_empty() {
                    ft.clone()
                } else {
                    chrono::DateTime::from_timestamp(r.create_time, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_default()
                }
            } else {
                chrono::DateTime::from_timestamp(r.create_time, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default()
            };
            ReplyItem {
                pid: r.pid,
                tid: r.tid,
                content: r.content.clone(),
                title: r.title.clone(),
                topic_name: r.topic_name.clone(),
                create_time: r.create_time,
                light_count: r.light_count,
                format_time,
            }
        })
        .collect();

    Ok(Json(RepliesResponse {
        total,
        replies: items,
    }))
}

pub async fn get_wordcloud(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<EuidQuery>,
) -> Result<Json<Vec<crate::analyze::WordCloudItem>>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let total =
        crate::db::count_replies(&conn, Some(&params.euid)).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let all_replies =
        crate::db::query_replies(&conn, Some(&params.euid), total as usize, 0)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let words = crate::analyze::word_frequency(&all_replies);
    Ok(Json(words))
}

pub async fn get_detailed_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<EuidQuery>,
) -> Result<Json<crate::analyze::DetailedAnalysis>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let total =
        crate::db::count_replies(&conn, Some(&params.euid)).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let all_replies =
        crate::db::query_replies(&conn, Some(&params.euid), total as usize, 0)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let analysis = crate::analyze::detailed_analysis(&all_replies);
    Ok(Json(analysis))
}

// ── Posts ──

pub async fn get_posts(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<PostsQuery>,
) -> Result<Json<PostsResponse>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let total =
        crate::db::count_posts(&conn, Some(&params.euid)).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let posts =
        crate::db::query_posts(&conn, Some(&params.euid), params.limit, params.offset)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let items: Vec<PostItem> = posts
        .iter()
        .map(|p| PostItem {
            tid: p.tid,
            title: p.title.clone(),
            summary: p.summary.clone(),
            topic_name: p.topic_name.clone(),
            forum_name: p.forum_name.clone(),
            create_time: p.create_time,
            replies: p.replies,
            visits: p.visits,
            lights: p.lights,
            recommend_num: p.recommend_num,
            total_pics: p.total_pics,
            has_video: p.has_video,
            share_num: p.share_num,
            format_time: p.format_time.clone().unwrap_or_default(),
            url: p.url(),
        })
        .collect();

    Ok(Json(PostsResponse {
        total,
        posts: items,
    }))
}

// ── AI Analysis ──

pub async fn get_ai_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<AiAnalyzeQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("ai:{}", params.euid);
    let euid = params.euid;

    {
        let results = state
            .ai_results
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if let Some(res) = results.get(&key) {
            return Ok(Json(serde_json::json!({
                "status": "done",
                "key": key,
                "result": res,
            })));
        }
    }

    if let Ok(conn) = crate::db::open_db(&state.db_path) {
        if let Ok(Some(result_json)) = crate::db::get_ai_analysis(&conn, &euid) {
            if let Ok(result) =
                serde_json::from_str::<crate::deepseek::AiAnalysisResult>(&result_json)
            {
                {
                    let mut results = state.ai_results.lock().ok();
                    if let Some(ref mut r) = results {
                        r.insert(key.clone(), result.clone());
                    }
                }
                return Ok(Json(serde_json::json!({
                    "status": "done",
                    "key": key,
                    "result": result,
                })));
            }
        }
    }

    Ok(Json(serde_json::json!({
        "status": "not_found",
        "key": key,
    })))
}

pub async fn start_ai_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<AiAnalyzeQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("ai:{}", params.euid);

    {
        let progress = state
            .progress
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if let Some(p) = progress.get(&key) {
            if !p.done {
                return Ok(Json(serde_json::json!({
                    "status": "running",
                    "key": key,
                })));
            }
        }
    }

    let cfg = match crate::config::try_get() {
        Some(cfg) => cfg,
        None => {
            return Ok(Json(serde_json::json!({
                "status": "error",
                "error": "请先配置 Cookie",
            })));
        }
    };
    if cfg.deepseek_api_key.is_empty() {
        return Ok(Json(serde_json::json!({
            "status": "error",
            "error": "未配置 DeepSeek API Key，请在 config.json 中添加 deepseek_api_key",
        })));
    }

    let state_clone = state.clone();
    let euid = params.euid.clone();
    tokio::spawn(async move {
        crate::services::ai::run_ai_analysis_background(state_clone, euid).await;
    });

    Ok(Json(serde_json::json!({
        "status": "started",
        "key": key,
    })))
}

pub async fn get_ai_progress(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<AiAnalyzeQuery>,
) -> Result<Json<ProgressState>, StatusCode> {
    let key = format!("ai:{}", params.euid);
    let progress = state
        .progress
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        progress.get(&key).cloned().unwrap_or(ProgressState {
            phase: "idle".into(),
            current: 0,
            total: 0,
            done: false,
            error: None,
        }),
    ))
}

// ── AI Post Analysis ──

pub async fn get_ai_post_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<AiPostAnalyzeQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("ai_post:{}", params.euid);
    let euid = params.euid;

    {
        let results = state
            .ai_post_results
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if let Some(res) = results.get(&key) {
            return Ok(Json(serde_json::json!({
                "status": "done",
                "key": key,
                "result": res,
            })));
        }
    }

    if let Ok(conn) = crate::db::open_db(&state.db_path) {
        if let Ok(Some(result_json)) = crate::db::get_ai_post_analysis(&conn, &euid) {
            if let Ok(result) =
                serde_json::from_str::<crate::deepseek::AiPostAnalysisResult>(&result_json)
            {
                {
                    let mut results = state.ai_post_results.lock().ok();
                    if let Some(ref mut r) = results {
                        r.insert(key.clone(), result.clone());
                    }
                }
                return Ok(Json(serde_json::json!({
                    "status": "done",
                    "key": key,
                    "result": result,
                })));
            }
        }
    }

    Ok(Json(serde_json::json!({
        "status": "not_found",
        "key": key,
    })))
}

pub async fn start_ai_post_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<AiPostAnalyzeQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("ai_post:{}", params.euid);

    {
        let progress = state
            .progress
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if let Some(p) = progress.get(&key) {
            if !p.done {
                return Ok(Json(serde_json::json!({
                    "status": "running",
                    "key": key,
                })));
            }
        }
    }

    let cfg = match crate::config::try_get() {
        Some(cfg) => cfg,
        None => {
            return Ok(Json(serde_json::json!({
                "status": "error",
                "error": "请先配置 Cookie",
            })));
        }
    };
    if cfg.deepseek_api_key.is_empty() {
        return Ok(Json(serde_json::json!({
            "status": "error",
            "error": "未配置 DeepSeek API Key",
        })));
    }

    let state_clone = state.clone();
    let euid = params.euid.clone();
    tokio::spawn(async move {
        crate::services::ai::run_ai_post_analysis_background(state_clone, euid).await;
    });

    Ok(Json(serde_json::json!({
        "status": "started",
        "key": key,
    })))
}

pub async fn get_ai_post_progress(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<AiPostAnalyzeQuery>,
) -> Result<Json<ProgressState>, StatusCode> {
    let key = format!("ai_post:{}", params.euid);
    let progress = state
        .progress
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        progress.get(&key).cloned().unwrap_or(ProgressState {
            phase: "idle".into(),
            current: 0,
            total: 0,
            done: false,
            error: None,
        }),
    ))
}

// ── Fetch handlers ──

pub type ApiResult = Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)>;

fn internal_error(msg: String) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": msg })),
    )
}

pub async fn fetch_replies_handler(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<FetchRepliesQuery>,
) -> ApiResult {
    let key = format!("fetch_replies:{}", params.euid);

    {
        let progress = state.progress.lock().map_err(|e| internal_error(e.to_string()))?;
        if let Some(p) = progress.get(&key) {
            if !p.done {
                return Ok(Json(serde_json::json!({
                    "status": "running",
                    "key": key,
                })));
            }
        }
    }

    let state_clone = state.clone();
    let euid = params.euid.clone();
    let max_pages = params.max_pages;
    let page_size = params.page_size;
    tokio::spawn(async move {
        crate::services::fetch::run_fetch_replies_background(state_clone, euid, max_pages, page_size)
            .await;
    });

    Ok(Json(serde_json::json!({
        "status": "started",
        "key": key,
    })))
}

pub async fn fetch_posts_handler(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<FetchPostsPagesQuery>,
) -> ApiResult {
    let key = format!("fetch_posts:{}", params.euid);

    {
        let progress = state.progress.lock().map_err(|e| internal_error(e.to_string()))?;
        if let Some(p) = progress.get(&key) {
            if !p.done {
                return Ok(Json(serde_json::json!({
                    "status": "running",
                    "key": key,
                })));
            }
        }
    }

    let state_clone = state.clone();
    let euid = params.euid.clone();
    let max_pages = params.max_pages;
    tokio::spawn(async move {
        crate::services::fetch::run_fetch_posts_background(state_clone, euid, max_pages).await;
    });

    Ok(Json(serde_json::json!({
        "status": "started",
        "key": key,
    })))
}

pub async fn get_fetch_posts_progress(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<FetchPostsProgressQuery>,
) -> Result<Json<ProgressState>, StatusCode> {
    let key = format!("fetch_posts:{}", params.euid);
    let progress = state
        .progress
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        progress.get(&key).cloned().unwrap_or(ProgressState {
            phase: "idle".into(),
            current: 0,
            total: 0,
            done: false,
            error: None,
        }),
    ))
}

pub async fn get_fetch_replies_progress(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<FetchRepliesProgressQuery>,
) -> Result<Json<ProgressState>, StatusCode> {
    let key = format!("fetch_replies:{}", params.euid);
    let progress = state
        .progress
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        progress.get(&key).cloned().unwrap_or(ProgressState {
            phase: "idle".into(),
            current: 0,
            total: 0,
            done: false,
            error: None,
        }),
    ))
}

pub async fn get_all_euids(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<Json<Vec<EuidEntry>>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let euids = crate::db::get_all_euids(&conn).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let entries: Vec<EuidEntry> = euids
        .into_iter()
        .map(|(euid, username)| EuidEntry { euid, username })
        .collect();
    Ok(Json(entries))
}

// ── Q&A ──

pub async fn qa_ask(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::extract::Json(body): axum::extract::Json<QaAskRequest>,
) -> Result<Json<QaAskResponse>, (StatusCode, Json<serde_json::Value>)> {
    let cfg = match crate::config::try_get() {
        Some(cfg) => cfg,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "请先配置 Cookie"})),
            ));
        }
    };

    if cfg.deepseek_api_key.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "未配置 DeepSeek API Key，请在 config.json 中添加 deepseek_api_key"})),
        ));
    }

    if body.question.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "问题不能为空"})),
        ));
    }

    let result = crate::services::qa::run_qa(
        &state.db_path,
        &state.http_client,
        &cfg.deepseek_api_key,
        &body.euid,
        &body.question,
        &body.history,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    Ok(Json(QaAskResponse {
        answer: result.answer,
        username: result.username,
        euid: body.euid,
        prompt_detail: result.prompt_detail,
    }))
}
