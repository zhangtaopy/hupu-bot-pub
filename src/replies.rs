use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::client::HupuClient;
use crate::utils::strip_html;

const API_URL: &str = "https://my.hupu.com/pcmapi/pc/space/v1/getReplyList";

// ── API response structs ──

#[derive(Deserialize, Debug)]
struct ApiResponse {
    code: i32,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<ApiData>,
}

#[derive(Deserialize, Debug, Default)]
struct ApiData {
    #[serde(default, rename = "replyWithQuoteDtoList")]
    reply_list: Vec<ApiReplyItem>,
    #[serde(default, rename = "nextPage")]
    next_page: Option<bool>,
    #[serde(default, rename = "maxTime")]
    max_time: Option<i64>,
}

#[derive(Deserialize, Debug, Clone, Default)]
struct ApiReplyItem {
    #[serde(default)]
    pid: i64,
    #[serde(default)]
    tid: i64,
    #[serde(default)]
    puid: Option<i64>,
    #[serde(default)]
    euid: Option<serde_json::Value>,
    #[serde(default)]
    username: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    quote: Option<i64>,
    #[serde(default, rename = "quoteInfo")]
    quote_info: Option<ApiQuoteInfo>,
    #[serde(default, rename = "createTime")]
    create_time: i64,
    #[serde(default, rename = "lightCount")]
    light_count: i64,
    #[serde(default, rename = "unlightCount")]
    unlight_count: i64,
    #[serde(default)]
    title: String,
    #[serde(default, rename = "topicId")]
    topic_id: Option<i64>,
    #[serde(default, rename = "topicName")]
    topic_name: Option<String>,
    #[serde(default, rename = "formatTime")]
    format_time: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct ApiQuoteInfo {
    pid: i64,
    tid: i64,
    puid: Option<i64>,
    euid: Option<serde_json::Value>,
    username: String,
    content: String,
    #[serde(rename = "createTime")]
    create_time: Option<i64>,
    #[serde(default, rename = "lightCount")]
    #[allow(dead_code)]
    light_count: i64,
    #[allow(dead_code)]
    title: Option<String>,
}

// ── Public data struct ──

#[derive(Debug, Clone, Serialize)]
pub struct ReplyRow {
    pub pid: i64,
    pub tid: i64,
    pub puid: Option<i64>,
    pub euid: Option<String>,
    pub username: String,
    pub content: String,
    pub quote: i64,
    pub quote_pid: Option<i64>,
    pub quote_tid: Option<i64>,
    pub quote_puid: Option<i64>,
    pub quote_euid: Option<String>,
    pub quote_username: Option<String>,
    pub quote_content: Option<String>,
    pub quote_create_time: Option<i64>,
    pub create_time: i64,
    pub light_count: i64,
    pub unlight_count: i64,
    pub title: String,
    pub topic_id: Option<i64>,
    pub topic_name: Option<String>,
    pub format_time: Option<String>,
}

impl From<ApiReplyItem> for ReplyRow {
    fn from(item: ApiReplyItem) -> Self {
        let (quote_pid, quote_tid, quote_puid, quote_euid, quote_username, quote_content, quote_create_time) =
            if let Some(qi) = &item.quote_info {
                (
                    Some(qi.pid),
                    Some(qi.tid),
                    qi.puid,
                    qi.euid.as_ref().map(|v| json_value_to_string(v)),
                    Some(qi.username.clone()),
                    Some(qi.content.clone()),
                    qi.create_time,
                )
            } else {
                (None, None, None, None, None, None, None)
            };

        ReplyRow {
            pid: item.pid,
            tid: item.tid,
            puid: item.puid,
            euid: item.euid.as_ref().map(json_value_to_string),
            username: item.username,
            content: item.content,
            quote: item.quote.unwrap_or(0),
            quote_pid,
            quote_tid,
            quote_puid,
            quote_euid,
            quote_username,
            quote_content,
            quote_create_time,
            create_time: item.create_time,
            light_count: item.light_count,
            unlight_count: item.unlight_count,
            title: item.title,
            topic_id: item.topic_id,
            topic_name: item.topic_name,
            format_time: item.format_time,
        }
    }
}

impl ReplyRow {
    pub fn url(&self) -> String {
        format!("https://bbs.hupu.com/{}.html", self.tid)
    }

    pub fn reply_url(&self) -> String {
        format!("https://bbs.hupu.com/{}.html?pid={}", self.tid, self.pid)
    }
}

fn json_value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

// ── Fetch ──

pub struct FetchResult {
    pub items: Vec<ReplyRow>,
    pub has_next_page: bool,
    pub max_time: Option<i64>,
}

pub async fn fetch_replies(
    client: &HupuClient,
    euid: &str,
    max_time: Option<i64>,
    page: u32,
    page_size: u32,
) -> Result<FetchResult> {
    let mut url = format!(
        "{}?euid={}&page={}&pageSize={}",
        API_URL, euid, page, page_size
    );
    if let Some(mt) = max_time {
        url.push_str(&format!("&maxTime={}", mt));
    }

    let referer = format!("https://my.hupu.com/{}?tabKey=2", euid);

    let resp = client
        .client
        .get(&url)
        .header("referer", &referer)
        .send()
        .await?;

    let api: ApiResponse = resp.json().await?;

    if api.code > 1 {
        bail!("API error code={}: {:?}", api.code, api.msg);
    }

    let data = api.data.unwrap_or_default();
    let items: Vec<ReplyRow> = data
        .reply_list
        .into_iter()
        .map(|item| {
            let mut row = ReplyRow::from(item);
            if row.euid.is_none() || row.euid.as_deref() == Some("") {
                row.euid = Some(euid.to_string());
            }
            row
        })
        .collect();

    Ok(FetchResult {
        items,
        has_next_page: data.next_page.unwrap_or(false),
        max_time: data.max_time,
    })
}

pub struct PaginatedResult {
    pub total_fetched: usize,
}

pub async fn fetch_replies_paginated(
    client: &HupuClient,
    euid: &str,
    max_pages: u32,
    page_size: u32,
    conn: &rusqlite::Connection,
) -> Result<PaginatedResult> {
    let mut total_fetched = 0usize;
    let now_ts = chrono::Local::now().timestamp();
    let mut max_time: Option<i64> = Some(now_ts);
    let mut pages_fetched = 0u32;

    loop {
        let page = pages_fetched + 1;
        let result = fetch_replies(client, euid, max_time, page, page_size).await?;

        let count = result.items.len();
        total_fetched += count;

        crate::db::upsert_replies(conn, &result.items)?;
        println!(
            "Page {}: fetched {} replies (total: {})",
            page, count, total_fetched
        );

        pages_fetched += 1;

        if !result.has_next_page || pages_fetched >= max_pages {
            break;
        }

        max_time = result.max_time;
    }

    Ok(PaginatedResult {
        total_fetched,
    })
}

// ── Format ──

fn format_time(row: &ReplyRow) -> String {
    if let Some(ft) = &row.format_time {
        if !ft.is_empty() {
            return ft.clone();
        }
    }
    // Fallback: format create_time timestamp as MM-DD HH:MM
    chrono::DateTime::from_timestamp(row.create_time, 0)
        .map(|dt| dt.format("%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn truncate(s: &str, max_len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_len {
        s.to_string()
    } else {
        chars[..max_len.saturating_sub(2)]
            .iter()
            .collect::<String>()
            + ".."
    }
}

pub fn format_table(replies: &[ReplyRow]) {
    if replies.is_empty() {
        println!("没有找到回帖");
        return;
    }

    let max_user = replies
        .iter()
        .map(|r| r.username.chars().count())
        .max()
        .unwrap_or(4)
        .min(15);
    let max_topic = replies
        .iter()
        .map(|r| r.topic_name.as_deref().unwrap_or("").chars().count())
        .max()
        .unwrap_or(4)
        .min(12);
    let max_title = 25;
    let max_content = 35;

    let w1 = max_user + 2;
    let w2 = max_topic + 2;
    let w3 = max_title + 2;
    let w4 = max_content + 2;

    let top_border = format!(
        "┌{}┬{}┬{}┬{}┬{}┬{}┐",
        "─".repeat(8),
        "─".repeat(w1),
        "─".repeat(w2),
        "─".repeat(w3),
        "─".repeat(w4),
        "─".repeat(6)
    );
    let header = format!(
        "│ 时间   │ {:^width1$} │ {:^width2$} │ {:^width3$} │ {:^width4$} │ 亮数 │",
        "用户",
        "板块",
        "标题",
        "回复内容",
        width1 = max_user,
        width2 = max_topic,
        width3 = max_title,
        width4 = max_content,
    );
    let mid_border = format!(
        "├{}┼{}┼{}┼{}┼{}┼{}┤",
        "─".repeat(8),
        "─".repeat(w1),
        "─".repeat(w2),
        "─".repeat(w3),
        "─".repeat(w4),
        "─".repeat(6)
    );
    let bottom_border = format!(
        "└{}┴{}┴{}┴{}┴{}┴{}┘",
        "─".repeat(8),
        "─".repeat(w1),
        "─".repeat(w2),
        "─".repeat(w3),
        "─".repeat(w4),
        "─".repeat(6)
    );

    println!("{}", top_border);
    println!("{}", header);
    println!("{}", mid_border);

    for r in replies {
        let time = format_time(r);
        let user = &r.username;
        let topic = r.topic_name.as_deref().unwrap_or("");
        let title = truncate(&r.title, max_title);
        let content = truncate(&strip_html(&r.content), max_content);

        println!(
            "│ {:6} │ {:width1$} │ {:width2$} │ {:width3$} │ {:width4$} │ {:4} │",
            truncate(&time, 8),
            truncate(user, max_user),
            truncate(topic, max_topic),
            title,
            content,
            r.light_count,
            width1 = max_user,
            width2 = max_topic,
            width3 = max_title,
            width4 = max_content,
        );
    }

    println!("{}", bottom_border);
    println!("共 {} 条", replies.len());
}

pub fn format_simple(replies: &[ReplyRow]) {
    if replies.is_empty() {
        println!("没有找到回帖");
        return;
    }

    for r in replies {
        let time = format_time(r);
        println!("[{}] {} 在 {}: {}", time, r.username, r.topic_name.as_deref().unwrap_or(""), truncate(&r.title, 50));
        println!("  {}", strip_html(&r.content));
        if r.quote > 0 {
            if let (Some(qu), Some(qc)) = (&r.quote_username, &r.quote_content) {
                println!("  引用 {}: {}", qu, truncate(&strip_html(qc), 60));
            }
        }
        println!("  亮数: {} | {}", r.light_count, r.reply_url());
        println!();
    }
    println!("共 {} 条", replies.len());
}

pub fn format_json(replies: &[ReplyRow]) -> Result<()> {
    let output: Vec<serde_json::Value> = replies
        .iter()
        .map(|r| {
            serde_json::json!({
                "pid": r.pid,
                "tid": r.tid,
                "puid": r.puid,
                "euid": r.euid,
                "username": r.username,
                "content": strip_html(&r.content),
                "quote": r.quote,
                "quote_username": r.quote_username,
                "quote_content": r.quote_content.as_ref().map(|s| strip_html(s)),
                "create_time": r.create_time,
                "light_count": r.light_count,
                "unlight_count": r.unlight_count,
                "title": r.title,
                "topic_id": r.topic_id,
                "topic_name": r.topic_name,
                "format_time": r.format_time,
                "url": r.url(),
                "reply_url": r.reply_url(),
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}