use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex as SyncMutex;

#[derive(Deserialize)]
pub struct EuidQuery {
    pub euid: String,
}

#[derive(Deserialize)]
pub struct AnalyzeQuery {
    pub euid: String,
    #[serde(default = "default_threshold")]
    pub threshold: f64,
}

fn default_threshold() -> f64 {
    0.5
}

#[derive(Deserialize)]
pub struct AiAnalyzeQuery {
    pub euid: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Deserialize)]
pub struct RepliesQuery {
    pub euid: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

#[derive(Deserialize)]
pub struct AiPostAnalyzeQuery {
    pub euid: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
}

fn default_limit() -> usize {
    1000
}

#[derive(Deserialize)]
pub struct FetchRepliesQuery {
    pub euid: String,
    #[serde(default = "default_fetch_replies_max_pages")]
    pub max_pages: u32,
    #[serde(default = "default_fetch_replies_page_size")]
    pub page_size: u32,
    #[serde(default)]
    pub cookie: Option<String>,
}

#[derive(Deserialize)]
pub struct FetchPostsPagesQuery {
    pub euid: String,
    #[serde(default = "default_fetch_posts_max_pages")]
    pub max_pages: u32,
    #[serde(default)]
    pub cookie: Option<String>,
}

#[derive(Deserialize)]
pub struct FetchPostsProgressQuery {
    pub euid: String,
}

#[derive(Deserialize)]
pub struct FetchRepliesProgressQuery {
    pub euid: String,
}

#[derive(Deserialize)]
pub struct PostsQuery {
    pub euid: String,
    #[serde(default = "default_posts_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_fetch_replies_max_pages() -> u32 {
    0
}
fn default_fetch_replies_page_size() -> u32 {
    10
}
fn default_fetch_posts_max_pages() -> u32 {
    0
}
fn default_posts_limit() -> usize {
    100
}

#[derive(Serialize)]
pub struct StatsResponse {
    pub total_replies: usize,
    pub unique_replies: usize,
    pub repeated_replies: usize,
    pub repeat_rate: f64,
    pub topic_distribution: HashMap<String, usize>,
    pub time_distribution: std::collections::BTreeMap<String, usize>,
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
    pub deploy_mode: bool,
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

#[derive(Serialize)]
pub struct EuidEntry {
    pub euid: String,
    pub username: String,
}

#[derive(Deserialize)]
pub struct QaAskRequest {
    pub euid: String,
    pub question: String,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct HistoryEntry {
    pub question: String,
    pub answer: String,
}

// ── Monitor (舆论监控) types ──

#[derive(Deserialize)]
pub struct MonitorFetchQuery {
    pub topic_id: String,
    /// Number of days to fetch (1 = today, 7 = past week).
    #[serde(default = "default_monitor_fetch_days")]
    pub days: u32,
    #[serde(default = "default_monitor_replies_per_post")]
    pub replies_per_post: usize,
    #[serde(default)]
    pub cookie: Option<String>,
}

fn default_monitor_replies_per_post() -> usize {
    10
}

#[derive(Deserialize)]
pub struct MonitorTopicQuery {
    pub topic_id: String,
}

#[derive(Deserialize)]
pub struct MonitorPostsQuery {
    pub topic_id: String,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default = "default_monitor_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

#[derive(Deserialize)]
pub struct MonitorAnalyzeQuery {
    pub topic_id: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Deserialize)]
pub struct MonitorStatsQuery {
    pub topic_id: String,
    #[serde(default = "default_monitor_days")]
    pub days: i64,
}

fn default_monitor_limit() -> usize {
    50
}
fn default_monitor_fetch_days() -> u32 {
    1
}
fn default_monitor_days() -> i64 {
    7
}

