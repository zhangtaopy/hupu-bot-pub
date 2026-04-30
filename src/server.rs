use anyhow::Result;
use axum::{
    extract::Query,
    http::StatusCode,
    response::{Html, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::Mutex as SyncMutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::task::JoinSet;
use tower_http::cors::CorsLayer;

const INDEX_HTML: &str = include_str!("../web/index.html");

// ── Query params ──

#[derive(Deserialize)]
pub struct EuidQuery {
    euid: String,
}

#[derive(Deserialize)]
pub struct AnalyzeQuery {
    euid: String,
    #[serde(default = "default_threshold")]
    threshold: f64,
}

fn default_threshold() -> f64 {
    0.5
}

#[derive(Deserialize)]
pub struct AiAnalyzeQuery {
    euid: String,
}

#[derive(Deserialize)]
pub struct RepliesQuery {
    euid: String,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
}

#[derive(Deserialize)]
pub struct AiPostAnalyzeQuery {
    euid: String,
}

fn default_limit() -> usize {
    1000
}

// ── Fetch data query params ──

#[derive(Deserialize)]
pub struct FetchRepliesQuery {
    euid: String,
    #[serde(default = "default_fetch_replies_max_pages")]
    max_pages: u32,
    #[serde(default = "default_fetch_replies_page_size")]
    page_size: u32,
}

#[derive(Deserialize)]
pub struct FetchPostsPagesQuery {
    euid: String,
    #[serde(default = "default_fetch_posts_max_pages")]
    max_pages: u32,
}

#[derive(Deserialize)]
pub struct FetchPostsProgressQuery {
    euid: String,
}

#[derive(Deserialize)]
pub struct FetchRepliesProgressQuery {
    euid: String,
}

fn default_fetch_replies_max_pages() -> u32 { 50 }
fn default_fetch_replies_page_size() -> u32 { 10 }
fn default_fetch_posts_max_pages() -> u32 { 5 }

// ── Response types ──

#[derive(Serialize)]
pub struct StatsResponse {
    pub total_replies: usize,
    pub unique_replies: usize,
    pub repeated_replies: usize,
    pub repeat_rate: f64,
    pub topic_distribution: HashMap<String, usize>,
    pub time_distribution: BTreeMap<String, usize>,
    pub similarity_available: bool,
}

#[derive(Serialize)]
pub struct RepliesResponse {
    pub total: i64,
    pub replies: Vec<ReplyItem>,
}

#[derive(Serialize)]
pub struct ReplyItem {
    pub pid: i64,
    pub tid: i64,
    pub content: String,
    pub title: String,
    pub topic_name: Option<String>,
    pub create_time: i64,
    pub light_count: i64,
    pub format_time: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AnalyzeResponse {
    pub total_replies: usize,
    pub groups: Vec<crate::analyze::ReplyGroup>,
}

// ── Shared state ──

#[derive(Clone, Serialize)]
pub struct ProgressState {
    pub phase: String,
    pub current: usize,
    pub total: usize,
    pub done: bool,
    pub error: Option<String>,
}

pub struct AppState {
    pub db_path: std::path::PathBuf,
    pub progress: SyncMutex<HashMap<String, ProgressState>>,
    pub results: SyncMutex<HashMap<String, AnalyzeResponse>>,
    pub ai_results: SyncMutex<HashMap<String, crate::deepseek::AiAnalysisResult>>,
    pub ai_post_results: SyncMutex<HashMap<String, crate::deepseek::AiPostAnalysisResult>>,
    pub http_client: reqwest::Client,
}

#[derive(Serialize)]
pub struct PostsResponse {
    pub total: i64,
    pub posts: Vec<PostItem>,
}

#[derive(Serialize)]
pub struct PostItem {
    pub tid: i64,
    pub title: String,
    pub summary: String,
    pub topic_name: String,
    pub forum_name: String,
    pub create_time: i64,
    pub replies: i64,
    pub visits: i64,
    pub lights: i64,
    pub recommend_num: i64,
    pub total_pics: i64,
    pub has_video: bool,
    pub share_num: i64,
    pub format_time: String,
    pub url: String,
}

#[derive(Serialize)]
pub struct UserResponse {
    pub euid: String,
    pub username: String,
}

// ── API handlers ──

async fn get_index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn get_user(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<EuidQuery>,
) -> Result<Json<UserResponse>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let replies = crate::db::query_replies(&conn, Some(&params.euid), 1, 0)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let username = replies.first().map(|r| r.username.clone()).unwrap_or_else(|| "未知用户".to_string());
    Ok(Json(UserResponse {
        euid: params.euid,
        username,
    }))
}

async fn get_stats(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<EuidQuery>,
) -> Result<Json<StatsResponse>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let total = crate::db::count_replies(&conn, Some(&params.euid))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let topic_distribution = query_topic_distribution(&conn, &params.euid)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let time_distribution = query_time_distribution(&conn, &params.euid)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Only use cached similarity results, never auto-compute
    let (repeated, unique, repeat_rate, similarity_available) = {
        let key = format!("{}:{}", params.euid, 0.5);
        let cached_groups = state.results.lock().ok()
            .and_then(|r| r.get(&key).map(|res| res.groups.clone()));

        match cached_groups {
            Some(groups) => {
                let r: usize = groups.iter().map(|g| g.count).sum();
                let u = total as usize - r;
                let rate = if total > 0 { r as f64 / total as f64 } else { 0.0 };
                (r, u, rate, true)
            }
            None => {
                // Check database
                if let Ok(conn2) = crate::db::open_db(&state.db_path) {
                    if let Ok(Some(result_json)) = crate::db::get_similarity_analysis(&conn2, &params.euid, 0.5) {
                        if let Ok(db_res) = serde_json::from_str::<AnalyzeResponse>(&result_json) {
                            let r: usize = db_res.groups.iter().map(|g| g.count).sum();
                            let u = total as usize - r;
                            let rate = if total > 0 { r as f64 / total as f64 } else { 0.0 };
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

/// Read-only: check in-memory cache, then DB. Never starts analysis.
async fn get_similarity_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<AnalyzeQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("{}:{}", params.euid, params.threshold);

    // Check in-memory cache
    {
        let results = state.results.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if let Some(res) = results.get(&key) {
            return Ok(Json(serde_json::json!({
                "status": "done",
                "key": key,
                "total_replies": res.total_replies,
                "groups": res.groups,
            })));
        }
    }

    // Check database
    if let Ok(conn) = crate::db::open_db(&state.db_path) {
        if let Ok(Some(result_json)) = crate::db::get_similarity_analysis(&conn, &params.euid, params.threshold) {
            if let Ok(res) = serde_json::from_str::<AnalyzeResponse>(&result_json) {
                // Warm the in-memory cache
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

/// POST: start similarity analysis (or return existing if already running)
async fn start_similarity_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<AnalyzeQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("{}:{}", params.euid, params.threshold);

    // Check if already running
    {
        let progress = state.progress.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if let Some(p) = progress.get(&key) {
            if !p.done {
                return Ok(Json(serde_json::json!({
                    "status": "running",
                    "key": key,
                })));
            }
        }
    }

    // Start background analysis
    let state_clone = state.clone();
    let params_clone = AnalyzeQuery {
        euid: params.euid.clone(),
        threshold: params.threshold,
    };
    tokio::spawn(async move {
        run_analysis_background(state_clone, params_clone).await;
    });

    Ok(Json(serde_json::json!({
        "status": "started",
        "key": key,
    })))
}

async fn get_analysis_progress(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<AnalyzeQuery>,
) -> Result<Json<ProgressState>, StatusCode> {
    let key = format!("{}:{}", params.euid, params.threshold);
    let progress = state.progress.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(progress.get(&key).cloned().unwrap_or(ProgressState {
        phase: "idle".into(),
        current: 0,
        total: 0,
        done: false,
        error: None,
    })))
}

async fn run_analysis_background(state: Arc<AppState>, params: AnalyzeQuery) {
    let key = format!("{}:{}", params.euid, params.threshold);

    let set_progress = |s: &AppState, phase: &str, current: usize, total: usize, done: bool, error: Option<String>| {
        if let Ok(mut p) = s.progress.lock() {
            p.insert(key.clone(), ProgressState {
                phase: phase.into(),
                current,
                total,
                done,
                error,
            });
        }
    };

    set_progress(&state, "读取数据中", 0, 0, false, None);

    let conn = match crate::db::open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("数据库打开失败: {}", e)));
            return;
        }
    };

    let total = match crate::db::count_replies(&conn, Some(&params.euid)) {
        Ok(t) => t,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("查询失败: {}", e)));
            return;
        }
    };

    let all_replies = match crate::db::query_replies(&conn, Some(&params.euid), total as usize, 0) {
        Ok(r) => r,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("读取数据失败: {}", e)));
            return;
        }
    };

    set_progress(&state, "计算相似度中", 0, 0, false, None);

    // Run CPU-intensive clustering on a blocking thread so progress HTTP
    // requests can still be served by the async runtime.
    let cb_state = state.clone();
    let cb_key = key.clone();
    let threshold = params.threshold;
    let all_replies_clone = all_replies.clone();

    let groups = tokio::task::spawn_blocking(move || {
        let cb: crate::analyze::ProgressFn = Box::new(move |current, total, phase| {
            let _ = cb_state.progress.lock().map(|mut p| {
                p.insert(cb_key.clone(), ProgressState {
                    phase: phase.into(),
                    current,
                    total,
                    done: false,
                    error: None,
                });
            });
        });
        crate::analyze::cluster_replies_with_progress(&all_replies_clone, threshold, Some(cb))
    }).await.unwrap_or_default();

    let response = AnalyzeResponse {
        total_replies: total as usize,
        groups,
    };

    if let Ok(mut results) = state.results.lock() {
        results.insert(key.clone(), response.clone());
    }

    // Save to database
    if let Ok(conn) = crate::db::open_db(&state.db_path) {
        if let Ok(result_json) = serde_json::to_string(&response) {
            let _ = crate::db::save_similarity_analysis(&conn, &params.euid, params.threshold, &result_json);
        }
    }

    set_progress(&state, "完成", 1, 1, true, None);
}

async fn get_replies(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<RepliesQuery>,
) -> Result<Json<RepliesResponse>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let total = crate::db::count_replies(&conn, Some(&params.euid))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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

// ── Word Cloud ──

async fn get_wordcloud(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<EuidQuery>,
) -> Result<Json<Vec<crate::analyze::WordCloudItem>>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let total = crate::db::count_replies(&conn, Some(&params.euid))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let all_replies = crate::db::query_replies(&conn, Some(&params.euid), total as usize, 0)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let words = crate::analyze::word_frequency(&all_replies);
    Ok(Json(words))
}

// ── Detailed Analysis ──

async fn get_detailed_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<EuidQuery>,
) -> Result<Json<crate::analyze::DetailedAnalysis>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let total = crate::db::count_replies(&conn, Some(&params.euid))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let all_replies = crate::db::query_replies(&conn, Some(&params.euid), total as usize, 0)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let analysis = crate::analyze::detailed_analysis(&all_replies);
    Ok(Json(analysis))
}

// ── AI Analysis ──

/// Read-only: check DB first, then in-memory cache. Never starts analysis.
async fn get_ai_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<AiAnalyzeQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("ai:{}", params.euid);
    let euid = params.euid;

    // Check in-memory cache first
    {
        let results = state.ai_results.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if let Some(res) = results.get(&key) {
            return Ok(Json(serde_json::json!({
                "status": "done",
                "key": key,
                "result": res,
            })));
        }
    }

    // Check database
    if let Ok(conn) = crate::db::open_db(&state.db_path) {
        if let Ok(Some(result_json)) = crate::db::get_ai_analysis(&conn, &euid) {
            if let Ok(result) = serde_json::from_str::<crate::deepseek::AiAnalysisResult>(&result_json) {
                // Warm the in-memory cache
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

/// POST: start AI analysis (or return existing if already running)
async fn start_ai_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<AiAnalyzeQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("ai:{}", params.euid);

    // Check if already running
    {
        let progress = state.progress.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if let Some(p) = progress.get(&key) {
            if !p.done {
                return Ok(Json(serde_json::json!({
                    "status": "running",
                    "key": key,
                })));
            }
        }
    }

    // Check API key
    let cfg: crate::config::Config = crate::config::get();
    if cfg.deepseek_api_key.is_empty() {
        return Ok(Json(serde_json::json!({
            "status": "error",
            "error": "未配置 DeepSeek API Key，请在 config.json 中添加 deepseek_api_key",
        })));
    }

    // Start background task
    let state_clone = state.clone();
    let euid = params.euid.clone();
    tokio::spawn(async move {
        run_ai_analysis_background(state_clone, euid).await;
    });

    Ok(Json(serde_json::json!({
        "status": "started",
        "key": key,
    })))
}

async fn get_ai_progress(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<AiAnalyzeQuery>,
) -> Result<Json<ProgressState>, StatusCode> {
    let key = format!("ai:{}", params.euid);
    let progress = state.progress.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(progress.get(&key).cloned().unwrap_or(ProgressState {
        phase: "idle".into(),
        current: 0,
        total: 0,
        done: false,
        error: None,
    })))
}

async fn run_ai_analysis_background(state: Arc<AppState>, euid: String) {
    let key = format!("ai:{}", euid);

    let set_progress = |s: &AppState, phase: &str, current: usize, total: usize, done: bool, error: Option<String>| {
        if let Ok(mut p) = s.progress.lock() {
            p.insert(key.clone(), ProgressState {
                phase: phase.into(),
                current,
                total,
                done,
                error,
            });
        }
    };

    set_progress(&state, "读取数据中", 0, 0, false, None);

    let conn = match crate::db::open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("数据库打开失败: {}", e)));
            return;
        }
    };

    let total = match crate::db::count_replies(&conn, Some(&euid)) {
        Ok(t) => t,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("查询失败: {}", e)));
            return;
        }
    };

    if total == 0 {
        set_progress(&state, "error", 0, 0, true, Some("该用户没有回帖数据".into()));
        return;
    }

    let all_replies = match crate::db::query_replies(&conn, Some(&euid), total as usize, 0) {
        Ok(r) => r,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("读取数据失败: {}", e)));
            return;
        }
    };

    // Load posts for additional identity context
    let posts_total = crate::db::count_posts(&conn, Some(&euid)).unwrap_or(0);
    let all_posts = if posts_total > 0 {
        crate::db::query_posts(&conn, Some(&euid), posts_total as usize, 0).unwrap_or_default()
    } else {
        Vec::new()
    };
    let posts_context = crate::deepseek::format_posts_context(&all_posts);
    let posts_ctx_opt = if posts_context.is_empty() { None } else { Some(posts_context) };

    let cfg = crate::config::get();
    if cfg.deepseek_api_key.is_empty() {
        set_progress(&state, "error", 0, 0, true, Some("未配置 DeepSeek API Key".into()));
        return;
    }

    // Sort by time and chunk
    let mut sorted = all_replies.clone();
    sorted.sort_by(|a, b| a.create_time.cmp(&b.create_time));
    let chunks = crate::deepseek::chunk_replies(&sorted);
    let total_chunks = chunks.len();

    set_progress(&state, "AI分批分析中", 0, total_chunks, false, None);

    // Interleave task spawning and result collection so progress updates
    // incrementally instead of jumping at the end.
    let max_concurrency = cfg.deepseek_max_concurrency.max(1);
    let completed = Arc::new(AtomicUsize::new(0));
    let failed_count = Arc::new(AtomicUsize::new(0));
    let api_key = cfg.deepseek_api_key.clone();
    let client = state.http_client.clone();
    let db_path = state.db_path.clone();
    let euid_clone = euid.clone();

    let mut join_set: JoinSet<(usize, Result<(serde_json::Value, String)>)> = JoinSet::new();
    let mut next_idx: usize = 0;
    let initial_batch = max_concurrency.min(total_chunks);

    while next_idx < initial_batch {
        let i = next_idx;
        let chunk = chunks[i].clone();
        let api_key = api_key.clone();
        let client = client.clone();
        let db_path = db_path.clone();
        let euid_c = euid_clone.clone();
        let posts_ctx = posts_ctx_opt.clone();
        join_set.spawn(async move {
            let ctx_ref = posts_ctx.as_deref();
            let result = crate::deepseek::analyze_batch(&client, &api_key, &chunk, ctx_ref).await;
            if let Err(ref e) = result {
                if let Ok(conn) = crate::db::open_db(&db_path) {
                    let _ = crate::db::save_batch_error(&conn, &euid_c, "reply", i, &e.to_string(), Some(&e.to_string()));
                }
            }
            (i, result)
        });
        next_idx += 1;
    }

    let mut batch_results: Vec<(usize, serde_json::Value)> = Vec::new();
    while let Some(join_result) = join_set.join_next().await {
        match join_result {
            Ok((i, Ok((value, _raw)))) => batch_results.push((i, value)),
            Ok((_, Err(_))) => { failed_count.fetch_add(1, Ordering::Relaxed); }
            Err(_) => { failed_count.fetch_add(1, Ordering::Relaxed); }
        }
        let n = completed.fetch_add(1, Ordering::Relaxed) + 1;
        let failed_n = failed_count.load(Ordering::Relaxed);
        let phase = if failed_n > 0 {
            format!("AI分批分析 {}/{} (失败{}批)", n, total_chunks, failed_n)
        } else {
            format!("AI分批分析 {}/{}", n, total_chunks)
        };
        set_progress(&state, &phase, n, total_chunks, false, None);

        if next_idx < total_chunks {
            let i = next_idx;
            let chunk = chunks[i].clone();
            let api_key = api_key.clone();
            let client = client.clone();
            let db_path = db_path.clone();
            let euid_c = euid_clone.clone();
            let posts_ctx = posts_ctx_opt.clone();
            join_set.spawn(async move {
                let ctx_ref = posts_ctx.as_deref();
                let result = crate::deepseek::analyze_batch(&client, &api_key, &chunk, ctx_ref).await;
                if let Err(ref e) = result {
                    if let Ok(conn) = crate::db::open_db(&db_path) {
                        let _ = crate::db::save_batch_error(&conn, &euid_c, "reply", i, &e.to_string(), Some(&e.to_string()));
                    }
                }
                (i, result)
            });
            next_idx += 1;
        }
    }

    batch_results.sort_by_key(|(i, _)| *i);
    let failed_count = failed_count.load(Ordering::Relaxed);
    let batch_results: Vec<serde_json::Value> = batch_results.into_iter().map(|(_, v)| v).collect();

    if batch_results.is_empty() {
        set_progress(&state, "error", 0, 0, true, Some("所有批次分析均失败".to_string()));
        return;
    }

    // Synthesis
    let synth_phase = if failed_count > 0 {
        format!("综合生成用户画像中 ({}批成功, {}批失败)", batch_results.len(), failed_count)
    } else {
        format!("综合生成用户画像中 ({}批完成)", batch_results.len())
    };
    set_progress(&state, &synth_phase, total_chunks, total_chunks, false, None);
    let synthesis_result = match crate::deepseek::synthesize_results(&client, &cfg.deepseek_api_key, &batch_results).await {
        Ok(r) => r,
        Err(e) => {
            set_progress(&state, "error", total_chunks, total_chunks, true, Some(format!("AI综合失败: {}", e)));
            return;
        }
    };

    // Cache result
    if let Ok(mut results) = state.ai_results.lock() {
        results.insert(key.clone(), synthesis_result.clone());
    }

    // Save to database
    if let Ok(conn) = crate::db::open_db(&state.db_path) {
        if let Ok(result_json) = serde_json::to_string(&synthesis_result) {
            let _ = crate::db::save_ai_analysis(&conn, &euid, &result_json);
        }
    }

    set_progress(&state, "完成", total_chunks, total_chunks, true, None);
}

// ── SQL queries ──

#[derive(Deserialize)]
pub struct PostsQuery {
    euid: String,
    #[serde(default = "default_posts_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
}

fn default_posts_limit() -> usize {
    100
}

async fn get_posts(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<PostsQuery>,
) -> Result<Json<PostsResponse>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let total = crate::db::count_posts(&conn, Some(&params.euid))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let posts = crate::db::query_posts(&conn, Some(&params.euid), params.limit, params.offset)
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

// ── AI analysis for posts ──

async fn get_ai_post_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<AiPostAnalyzeQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("ai_post:{}", params.euid);
    let euid = params.euid;

    // Check in-memory cache
    {
        let results = state.ai_post_results.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if let Some(res) = results.get(&key) {
            return Ok(Json(serde_json::json!({
                "status": "done",
                "key": key,
                "result": res,
            })));
        }
    }

    // Check database
    if let Ok(conn) = crate::db::open_db(&state.db_path) {
        if let Ok(Some(result_json)) = crate::db::get_ai_post_analysis(&conn, &euid) {
            if let Ok(result) = serde_json::from_str::<crate::deepseek::AiPostAnalysisResult>(&result_json) {
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

async fn start_ai_post_analysis(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<AiPostAnalyzeQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("ai_post:{}", params.euid);

    // Check if already running
    {
        let progress = state.progress.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if let Some(p) = progress.get(&key) {
            if !p.done {
                return Ok(Json(serde_json::json!({
                    "status": "running",
                    "key": key,
                })));
            }
        }
    }

    // Check API key
    let cfg: crate::config::Config = crate::config::get();
    if cfg.deepseek_api_key.is_empty() {
        return Ok(Json(serde_json::json!({
            "status": "error",
            "error": "未配置 DeepSeek API Key",
        })));
    }

    let state_clone = state.clone();
    let euid = params.euid.clone();
    tokio::spawn(async move {
        run_ai_post_analysis_background(state_clone, euid).await;
    });

    Ok(Json(serde_json::json!({
        "status": "started",
        "key": key,
    })))
}

async fn get_ai_post_progress(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<AiPostAnalyzeQuery>,
) -> Result<Json<ProgressState>, StatusCode> {
    let key = format!("ai_post:{}", params.euid);
    let progress = state.progress.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(progress.get(&key).cloned().unwrap_or(ProgressState {
        phase: "idle".into(),
        current: 0,
        total: 0,
        done: false,
        error: None,
    })))
}

async fn run_ai_post_analysis_background(state: Arc<AppState>, euid: String) {
    let key = format!("ai_post:{}", euid);

    let set_progress = |s: &AppState, phase: &str, current: usize, total: usize, done: bool, error: Option<String>| {
        if let Ok(mut p) = s.progress.lock() {
            p.insert(key.clone(), ProgressState {
                phase: phase.into(),
                current,
                total,
                done,
                error,
            });
        }
    };

    set_progress(&state, "读取发帖数据中", 0, 0, false, None);

    let conn = match crate::db::open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("数据库打开失败: {}", e)));
            return;
        }
    };

    let total = match crate::db::count_posts(&conn, Some(&euid)) {
        Ok(t) => t,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("查询失败: {}", e)));
            return;
        }
    };

    if total == 0 {
        set_progress(&state, "error", 0, 0, true, Some("该用户没有发帖数据".into()));
        return;
    }

    let all_posts = match crate::db::query_posts(&conn, Some(&euid), total as usize, 0) {
        Ok(r) => r,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("读取数据失败: {}", e)));
            return;
        }
    };

    let cfg = crate::config::get();
    if cfg.deepseek_api_key.is_empty() {
        set_progress(&state, "error", 0, 0, true, Some("未配置 DeepSeek API Key".into()));
        return;
    }

    // Sort by time and chunk
    let mut sorted = all_posts.clone();
    sorted.sort_by(|a, b| a.create_time.cmp(&b.create_time));
    let chunks = crate::deepseek::chunk_posts(&sorted);
    let total_chunks = chunks.len();

    set_progress(&state, "AI分批分析帖子中", 0, total_chunks, false, None);

    // Interleave task spawning and result collection so progress updates
    // incrementally instead of jumping at the end.
    let max_concurrency = cfg.deepseek_max_concurrency.max(1);
    let completed = Arc::new(AtomicUsize::new(0));
    let failed_count = Arc::new(AtomicUsize::new(0));
    let api_key = cfg.deepseek_api_key.clone();
    let client = state.http_client.clone();
    let db_path = state.db_path.clone();
    let euid_clone = euid.clone();

    let mut join_set: JoinSet<(usize, Result<(serde_json::Value, String)>)> = JoinSet::new();
    let mut next_idx: usize = 0;
    let initial_batch = max_concurrency.min(total_chunks);

    while next_idx < initial_batch {
        let i = next_idx;
        let chunk = chunks[i].clone();
        let api_key = api_key.clone();
        let client = client.clone();
        let db_path = db_path.clone();
        let euid_c = euid_clone.clone();
        join_set.spawn(async move {
            let result = crate::deepseek::analyze_post_batch(&client, &api_key, &chunk).await;
            if let Err(ref e) = result {
                if let Ok(conn) = crate::db::open_db(&db_path) {
                    let _ = crate::db::save_batch_error(&conn, &euid_c, "post", i, &e.to_string(), Some(&e.to_string()));
                }
            }
            (i, result)
        });
        next_idx += 1;
    }

    let mut batch_results: Vec<(usize, serde_json::Value)> = Vec::new();
    while let Some(join_result) = join_set.join_next().await {
        match join_result {
            Ok((i, Ok((value, _raw)))) => batch_results.push((i, value)),
            Ok((_, Err(_))) => { failed_count.fetch_add(1, Ordering::Relaxed); }
            Err(_) => { failed_count.fetch_add(1, Ordering::Relaxed); }
        }
        let n = completed.fetch_add(1, Ordering::Relaxed) + 1;
        let failed_n = failed_count.load(Ordering::Relaxed);
        let phase = if failed_n > 0 {
            format!("AI分批分析帖子 {}/{} (失败{}批)", n, total_chunks, failed_n)
        } else {
            format!("AI分批分析帖子 {}/{}", n, total_chunks)
        };
        set_progress(&state, &phase, n, total_chunks, false, None);

        if next_idx < total_chunks {
            let i = next_idx;
            let chunk = chunks[i].clone();
            let api_key = api_key.clone();
            let client = client.clone();
            let db_path = db_path.clone();
            let euid_c = euid_clone.clone();
            join_set.spawn(async move {
                let result = crate::deepseek::analyze_post_batch(&client, &api_key, &chunk).await;
                if let Err(ref e) = result {
                    if let Ok(conn) = crate::db::open_db(&db_path) {
                        let _ = crate::db::save_batch_error(&conn, &euid_c, "post", i, &e.to_string(), Some(&e.to_string()));
                    }
                }
                (i, result)
            });
            next_idx += 1;
        }
    }

    batch_results.sort_by_key(|(i, _)| *i);
    let failed_count = failed_count.load(Ordering::Relaxed);
    let batch_results: Vec<serde_json::Value> = batch_results.into_iter().map(|(_, v)| v).collect();

    if batch_results.is_empty() {
        set_progress(&state, "error", 0, 0, true, Some("所有批次分析均失败".to_string()));
        return;
    }

    // Synthesis
    let synth_phase = if failed_count > 0 {
        format!("综合生成发帖分析画像中 ({}批成功, {}批失败)", batch_results.len(), failed_count)
    } else {
        format!("综合生成发帖分析画像中 ({}批完成)", batch_results.len())
    };
    set_progress(&state, &synth_phase, total_chunks, total_chunks, false, None);
    let synthesis_result = match crate::deepseek::synthesize_post_results(&client, &cfg.deepseek_api_key, &batch_results).await {
        Ok(r) => r,
        Err(e) => {
            set_progress(&state, "error", total_chunks, total_chunks, true, Some(format!("AI综合失败: {}", e)));
            return;
        }
    };

    // Cache result
    if let Ok(mut results) = state.ai_post_results.lock() {
        results.insert(key.clone(), synthesis_result.clone());
    }

    // Save to database
    if let Ok(conn) = crate::db::open_db(&state.db_path) {
        if let Ok(result_json) = serde_json::to_string(&synthesis_result) {
            let _ = crate::db::save_ai_post_analysis(&conn, &euid, &result_json);
        }
    }

    set_progress(&state, "完成", total_chunks, total_chunks, true, None);
}

// ── SQL queries ──

fn query_topic_distribution(
    conn: &rusqlite::Connection,
    euid: &str,
) -> Result<HashMap<String, usize>> {
    let mut stmt = conn.prepare(
        "SELECT topic_name, COUNT(*) as cnt FROM replies WHERE euid = ? AND topic_name IS NOT NULL GROUP BY topic_name ORDER BY cnt DESC",
    )?;

    let mut dist = HashMap::new();
    let rows = stmt.query_map(rusqlite::params![euid], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
    })?;

    for row in rows {
        let (name, count) = row?;
        dist.insert(name, count);
    }
    Ok(dist)
}

fn query_time_distribution(
    conn: &rusqlite::Connection,
    euid: &str,
) -> Result<BTreeMap<String, usize>> {
    let mut stmt = conn.prepare(
        "SELECT strftime('%Y-%m', create_time, 'unixepoch') as month, COUNT(*) as cnt
         FROM replies WHERE euid = ? GROUP BY month ORDER BY month",
    )?;

    let mut dist = BTreeMap::new();
    let rows = stmt.query_map(rusqlite::params![euid], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
    })?;

    for row in rows {
        let (month, count) = row?;
        dist.insert(month, count);
    }
    Ok(dist)
}

// ── Fetch data handlers ──

type ApiResult = Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)>;

fn internal_error(msg: String) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": msg })))
}

async fn fetch_replies_handler(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<FetchRepliesQuery>,
) -> ApiResult {
    let key = format!("fetch_replies:{}", params.euid);

    // Check if already running
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
        run_fetch_replies_background(state_clone, euid, max_pages, page_size).await;
    });

    Ok(Json(serde_json::json!({
        "status": "started",
        "key": key,
    })))
}

async fn fetch_posts_handler(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<FetchPostsPagesQuery>,
) -> ApiResult {
    let key = format!("fetch_posts:{}", params.euid);

    // Check if already running
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

    // Start background fetch
    let state_clone = state.clone();
    let euid = params.euid.clone();
    let max_pages = params.max_pages;
    tokio::spawn(async move {
        run_fetch_posts_background(state_clone, euid, max_pages).await;
    });

    Ok(Json(serde_json::json!({
        "status": "started",
        "key": key,
    })))
}

async fn get_fetch_posts_progress(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<FetchPostsProgressQuery>,
) -> Result<Json<ProgressState>, StatusCode> {
    let key = format!("fetch_posts:{}", params.euid);
    let progress = state.progress.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(progress.get(&key).cloned().unwrap_or(ProgressState {
        phase: "idle".into(),
        current: 0,
        total: 0,
        done: false,
        error: None,
    })))
}

async fn get_fetch_replies_progress(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<FetchRepliesProgressQuery>,
) -> Result<Json<ProgressState>, StatusCode> {
    let key = format!("fetch_replies:{}", params.euid);
    let progress = state.progress.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(progress.get(&key).cloned().unwrap_or(ProgressState {
        phase: "idle".into(),
        current: 0,
        total: 0,
        done: false,
        error: None,
    })))
}

async fn run_fetch_posts_background(state: Arc<AppState>, euid: String, max_pages: u32) {
    let key = format!("fetch_posts:{}", euid);

    let set_progress = |s: &AppState, phase: &str, current: usize, total: usize, done: bool, error: Option<String>| {
        if let Ok(mut p) = s.progress.lock() {
            p.insert(key.clone(), ProgressState {
                phase: phase.into(),
                current,
                total,
                done,
                error,
            });
        }
    };

    set_progress(&state, "准备中", 0, max_pages as usize, false, None);

    let cfg = crate::config::get();
    let client = match crate::client::HupuClient::new(&cfg.cookie) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("创建客户端失败: {}", e)));
            return;
        }
    };

    let mut total_fetched = 0usize;

    for page in 1..=max_pages {
        let phase = format!("获取第 {} 页", page);
        set_progress(&state, &phase, page as usize, max_pages as usize, false, None);

        let posts = match crate::posts::fetch_posts_page(&client, &euid, page).await {
            Ok(p) => p,
            Err(e) => {
                set_progress(&state, "error", page as usize, max_pages as usize, true, Some(format!("获取第{}页失败: {}", page, e)));
                return;
            }
        };

        let count = posts.len();
        total_fetched += count;

        if !posts.is_empty() {
            match crate::db::open_db(&state.db_path) {
                Ok(conn) => {
                    if let Err(e) = crate::db::upsert_posts(&conn, &posts) {
                        set_progress(&state, "error", page as usize, max_pages as usize, true, Some(format!("数据库写入失败: {}", e)));
                        return;
                    }
                }
                Err(e) => {
                    set_progress(&state, "error", page as usize, max_pages as usize, true, Some(format!("数据库打开失败: {}", e)));
                    return;
                }
            }
        }

        if (count as u32) < crate::posts::PAGE_SIZE {
            break;
        }
    }

    set_progress(&state, "完成", total_fetched, max_pages as usize, true, None);
}

async fn run_fetch_replies_background(state: Arc<AppState>, euid: String, max_pages: u32, page_size: u32) {
    let key = format!("fetch_replies:{}", euid);

    let set_progress = |s: &AppState, phase: &str, current: usize, total: usize, done: bool, error: Option<String>| {
        if let Ok(mut p) = s.progress.lock() {
            p.insert(key.clone(), ProgressState {
                phase: phase.into(),
                current,
                total,
                done,
                error,
            });
        }
    };

    set_progress(&state, "准备中", 0, max_pages as usize, false, None);

    let cfg = crate::config::get();
    let client = match crate::client::HupuClient::new(&cfg.cookie) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("创建客户端失败: {}", e)));
            return;
        }
    };

    let mut all_items = Vec::new();
    let mut total_fetched = 0usize;
    let now_ts = chrono::Local::now().timestamp();
    let mut max_time: Option<i64> = Some(now_ts);

    for page in 1..=max_pages {
        set_progress(&state, &format!("获取第 {} 页", page), page as usize, max_pages as usize, false, None);

        match crate::replies::fetch_replies(&client, &euid, max_time, page, page_size).await {
            Ok(result) => {
                let count = result.items.len();
                total_fetched += count;
                all_items.extend(result.items);

                if !result.has_next_page || page >= max_pages {
                    break;
                }
                max_time = result.max_time;
            }
            Err(e) => {
                set_progress(&state, "error", page as usize, max_pages as usize, true, Some(format!("获取第{}页失败: {}", page, e)));
                return;
            }
        }
    }

    // Write to DB after all fetches complete (Connection is not Send, so don't hold across await)
    set_progress(&state, "写入数据库", total_fetched, max_pages as usize, false, None);

    let conn = match crate::db::open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("数据库打开失败: {}", e)));
            return;
        }
    };

    if let Err(e) = crate::db::upsert_replies(&conn, &all_items) {
        set_progress(&state, "error", 0, 0, true, Some(format!("数据库写入失败: {}", e)));
        return;
    }

    set_progress(&state, "完成", total_fetched, max_pages as usize, true, None);
}

// ── Euid list ──

#[derive(Serialize)]
pub struct EuidEntry {
    pub euid: String,
    pub username: String,
}

async fn get_all_euids(
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

// ── Server startup ──

pub async fn start_server(port: u16) -> Result<()> {
    let db_path = std::path::PathBuf::from("hupu.db");
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .expect("Failed to create HTTP client");
    let state = Arc::new(AppState {
        db_path,
        progress: SyncMutex::new(HashMap::new()),
        results: SyncMutex::new(HashMap::new()),
        ai_results: SyncMutex::new(HashMap::new()),
        ai_post_results: SyncMutex::new(HashMap::new()),
        http_client,
    });

    let app = Router::new()
        .route("/", get(get_index))
        .route("/api/user", get(get_user))
        .route("/api/stats", get(get_stats))
        .route("/api/analyze/similarity", get(get_similarity_analysis))
        .route("/api/analyze/similarity/start", post(start_similarity_analysis))
        .route("/api/analyze/progress", get(get_analysis_progress))
        .route("/api/analyze/wordcloud", get(get_wordcloud))
        .route("/api/analyze/detailed", get(get_detailed_analysis))
        .route("/api/analyze/ai", get(get_ai_analysis))
        .route("/api/analyze/ai/start", post(start_ai_analysis))
        .route("/api/analyze/ai-progress", get(get_ai_progress))
        .route("/api/replies", get(get_replies))
        .route("/api/replies/fetch", post(fetch_replies_handler))
        .route("/api/replies/fetch-progress", get(get_fetch_replies_progress))
        .route("/api/posts", get(get_posts))
        .route("/api/posts/fetch", post(fetch_posts_handler))
        .route("/api/posts/fetch-progress", get(get_fetch_posts_progress))
        .route("/api/posts/analyze/ai", get(get_ai_post_analysis))
        .route("/api/posts/analyze/ai/start", post(start_ai_post_analysis))
        .route("/api/posts/analyze/ai-progress", get(get_ai_post_progress))
        .route("/api/euids", get(get_all_euids))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    println!("服务已启动: http://localhost:{}", port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}