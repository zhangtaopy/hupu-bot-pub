use std::sync::Arc;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::Json;

use crate::server::types::{
    AppState, MonitorAnalyzeQuery, MonitorFetchQuery, MonitorPostsQuery, MonitorStatsQuery,
    MonitorTopicQuery, ProgressState,
};

/// POST /api/monitor/fetch — start fetching posts + replies for a topic
pub async fn start_monitor_fetch(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<MonitorFetchQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("monitor_fetch:{}", params.topic_id);

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

    let has_cookie = crate::resolver::has_cookie(params.cookie.as_deref());
    if !has_cookie {
        return Ok(Json(serde_json::json!({
            "status": "error",
            "error": "请先配置 Cookie",
        })));
    }

    let state_clone = state.clone();
    let topic_id = params.topic_id.clone();
    let days = params.days;
    let replies_per_post = params.replies_per_post;
    let cookie = params.cookie.clone();

    tokio::spawn(async move {
        crate::services::monitor::run_fetch_monitor_background(
            state_clone, topic_id, days, replies_per_post, cookie,
        ).await;
    });

    Ok(Json(serde_json::json!({ "status": "started", "key": key })))
}

pub async fn get_monitor_fetch_progress(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<MonitorTopicQuery>,
) -> Result<Json<ProgressState>, StatusCode> {
    let key = format!("monitor_fetch:{}", params.topic_id);
    let progress = state.progress.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(p) = progress.get(&key) {
        Ok(Json(p.clone()))
    } else {
        Ok(Json(ProgressState { phase: "idle".into(), current: 0, total: 0, done: false, error: None }))
    }
}

pub async fn get_monitor_posts(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<MonitorPostsQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let topic_id = params.topic_id.as_str();
    let posts = crate::db::query_monitor_posts(&conn, topic_id, params.date.as_deref(), params.limit, params.offset)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let total = crate::db::count_monitor_posts(&conn, topic_id, params.date.as_deref()).unwrap_or(0);
    Ok(Json(serde_json::json!({ "total": total, "posts": posts })))
}

pub async fn get_monitor_replies(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<MonitorPostsQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let topic_id = params.topic_id.as_str();
    let replies = crate::db::query_monitor_replies(&conn, topic_id, params.date.as_deref(), params.limit, params.offset)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let total = crate::db::count_monitor_replies(&conn, topic_id, params.date.as_deref()).unwrap_or(0);
    Ok(Json(serde_json::json!({ "total": total, "replies": replies })))
}

pub async fn get_monitor_stats(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<MonitorStatsQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let topic_id = params.topic_id.as_str();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let today_posts = crate::db::count_monitor_posts(&conn, topic_id, Some(&today)).unwrap_or(0);
    let today_replies = crate::db::count_monitor_replies(&conn, topic_id, Some(&today)).unwrap_or(0);
    let daily_counts = crate::db::monitor_daily_post_counts(&conn, topic_id, params.days).unwrap_or_default();
    let daily_reply_counts = crate::db::monitor_daily_reply_counts(&conn, topic_id, params.days).unwrap_or_default();
    // Sum daily counts to get range totals
    let range_posts: i64 = daily_counts.iter().filter_map(|v| v["count"].as_i64()).sum();
    let range_replies: i64 = daily_reply_counts.iter().filter_map(|v| v["count"].as_i64()).sum();
    let snapshots = crate::db::get_monitor_snapshots(&conn, topic_id, params.days).unwrap_or_default();
    let topics = crate::db::get_monitor_topics(&conn).unwrap_or_default();
    let covered_dates = crate::db::get_monitor_covered_dates(&conn, topic_id).unwrap_or_default();
    Ok(Json(serde_json::json!({
        "topic_id": topic_id, "today": { "posts": today_posts, "replies": today_replies },
        "range": { "posts": range_posts, "replies": range_replies },
        "daily_counts": daily_counts, "snapshots": snapshots,
        "known_topics": topics, "covered_dates": covered_dates,
    })))
}

pub async fn start_monitor_analyze(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<MonitorAnalyzeQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let key = format!("monitor_analyze:{}", params.topic_id);
    {
        let progress = state.progress.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if let Some(p) = progress.get(&key) {
            if !p.done {
                return Ok(Json(serde_json::json!({ "status": "running", "key": key })));
            }
        }
    }
    let user_provider = match (&params.api_key, &params.provider) {
        (Some(key), Some(prov)) if !key.is_empty() =>
            Some(crate::deepseek::AiProvider::from_user_input(prov, key)),
        _ => None,
    };
    let state_clone = state.clone();
    let topic_id = params.topic_id.clone();
    tokio::spawn(async move {
        crate::services::monitor::run_analyze_monitor_background(state_clone, topic_id, user_provider).await;
    });
    Ok(Json(serde_json::json!({ "status": "started", "key": key })))
}

pub async fn get_monitor_dates(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<MonitorTopicQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = crate::db::open_db(&state.db_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let topic_id = params.topic_id.as_str();
    let dates = crate::db::get_monitor_covered_dates(&conn, topic_id).unwrap_or_default();
    Ok(Json(serde_json::json!({ "topic_id": topic_id, "covered_dates": dates, "count": dates.len() })))
}

pub async fn get_monitor_analyze_progress(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Query(params): Query<MonitorTopicQuery>,
) -> Result<Json<ProgressState>, StatusCode> {
    let key = format!("monitor_analyze:{}", params.topic_id);
    let progress = state.progress.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(p) = progress.get(&key) {
        Ok(Json(p.clone()))
    } else {
        Ok(Json(ProgressState { phase: "idle".into(), current: 0, total: 0, done: false, error: None }))
    }
}
