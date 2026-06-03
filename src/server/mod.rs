mod handlers;
mod config_handlers;
mod monitor_handlers;
pub mod types;

use axum::{routing::{get, post}, Router};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

use types::AppState;

pub async fn start_server(port: u16, deploy_mode: bool) -> anyhow::Result<()> {
    let db_path = std::path::PathBuf::from("hupu.db");
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .expect("Failed to create HTTP client");
    let state = Arc::new(AppState {
        db_path,
        progress: std::sync::Mutex::new(std::collections::HashMap::new()),
        results: std::sync::Mutex::new(std::collections::HashMap::new()),
        ai_results: std::sync::Mutex::new(std::collections::HashMap::new()),
        ai_post_results: std::sync::Mutex::new(std::collections::HashMap::new()),
        http_client,
        deploy_mode,
    });

    let app = Router::new()
        .route("/api/user", get(handlers::get_user))
        .route("/api/stats", get(handlers::get_stats))
        .route("/api/analyze/similarity", get(handlers::get_similarity_analysis))
        .route("/api/analyze/similarity/start", post(handlers::start_similarity_analysis))
        .route("/api/analyze/progress", get(handlers::get_analysis_progress))
        .route("/api/analyze/wordcloud", get(handlers::get_wordcloud))
        .route("/api/analyze/detailed", get(handlers::get_detailed_analysis))
        .route("/api/analyze/ai", get(handlers::get_ai_analysis))
        .route("/api/analyze/ai/start", post(handlers::start_ai_analysis))
        .route("/api/analyze/ai-progress", get(handlers::get_ai_progress))
        .route("/api/replies", get(handlers::get_replies))
        .route("/api/replies/fetch", post(handlers::fetch_replies_handler))
        .route("/api/replies/fetch-progress", get(handlers::get_fetch_replies_progress))
        .route("/api/posts", get(handlers::get_posts))
        .route("/api/posts/fetch", post(handlers::fetch_posts_handler))
        .route("/api/posts/fetch-progress", get(handlers::get_fetch_posts_progress))
        .route("/api/posts/analyze/ai", get(handlers::get_ai_post_analysis))
        .route("/api/posts/analyze/ai/start", post(handlers::start_ai_post_analysis))
        .route("/api/posts/analyze/ai-progress", get(handlers::get_ai_post_progress))
        .route("/api/euids", get(handlers::get_all_euids))
        .route("/api/qa/ask", post(handlers::qa_ask))
        .route("/api/monitor/fetch", post(monitor_handlers::start_monitor_fetch))
        .route("/api/monitor/fetch-progress", get(monitor_handlers::get_monitor_fetch_progress))
        .route("/api/monitor/posts", get(monitor_handlers::get_monitor_posts))
        .route("/api/monitor/replies", get(monitor_handlers::get_monitor_replies))
        .route("/api/monitor/dates", get(monitor_handlers::get_monitor_dates))
        .route("/api/monitor/stats", get(monitor_handlers::get_monitor_stats))
        .route("/api/monitor/analyze", post(monitor_handlers::start_monitor_analyze))
        .route("/api/monitor/analyze-progress", get(monitor_handlers::get_monitor_analyze_progress))
        .route("/api/config/status", get(config_handlers::get_config_status))
        .route("/api/config/save", post(config_handlers::save_config))
        .route("/", get(handlers::get_index))
        .route("/*path", get(handlers::static_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    println!("服务已启动: http://localhost:{}", port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
