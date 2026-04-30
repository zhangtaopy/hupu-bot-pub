use anyhow::{bail, Result};
use chrono::{Duration, Local, NaiveDateTime};
use serde::Deserialize;

use crate::client::HupuClient;
use crate::utils::strip_html;

const API_URL: &str = "https://my.hupu.com/pcmapi/pc/space/v1/getMentionedRemindList";

/// API 返回结构
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
    #[serde(default, rename = "hisList")]
    his_list: Vec<MentionItem>,
    #[serde(default, rename = "pageStr")]
    page_str: Option<String>,
    #[serde(default, rename = "hasNextPage")]
    has_next_page: Option<bool>,
}

/// 单条消息
#[derive(Deserialize, Debug, Clone)]
#[allow(dead_code)]
pub struct MentionItem {
    pub id: Option<i64>,
    #[serde(rename = "msgType")]
    pub msg_type: Option<i32>,
    pub puid: Option<i64>,
    pub username: Option<String>,
    #[serde(rename = "headerUrl")]
    pub header_url: Option<String>,
    #[serde(rename = "postContent")]
    pub post_content: Option<String>,
    #[serde(rename = "threadTitle")]
    pub thread_title: Option<String>,
    pub tid: Option<i64>,
    pub pid: Option<i64>,
    #[serde(rename = "topicId")]
    pub topic_id: Option<i64>,
    #[serde(rename = "quoteContent")]
    pub quote_content: Option<String>,
    #[serde(rename = "publishTime")]
    pub publish_time: Option<String>,
    #[serde(rename = "updateTime")]
    pub update_time: Option<i64>,
}

/// 返回结果
pub struct FetchResult {
    pub items: Vec<MentionItem>,
    pub has_next_page: bool,
    pub page_str: Option<String>,
}

impl MentionItem {
    pub fn url(&self) -> String {
        format!("https://bbs.hupu.com/{}.html", self.tid.unwrap_or(0))
    }

    pub fn reply_url(&self) -> String {
        format!("https://bbs.hupu.com/{}.html?pid={}", self.tid.unwrap_or(0), self.pid.unwrap_or(0))
    }
}

/// Tab类型映射到 plate 参数
pub fn tab_to_plate(tab: &str) -> &'static str {
    match tab {
        "mentions" | "mention" | "m" => "1",
        "comments" | "comment" | "c" => "2",
        "likes" | "like" | "l" => "3",
        _ => "1",
    }
}

/// 解析时间过滤参数
pub fn parse_since(since: &str) -> Result<i64> {
    let now = Local::now();

    if since.ends_with('h') {
        let hours: i64 = since.trim_end_matches('h').parse()?;
        return Ok((now - Duration::hours(hours)).timestamp());
    }
    if since.ends_with('d') {
        let days: i64 = since.trim_end_matches('d').parse()?;
        return Ok((now - Duration::days(days)).timestamp());
    }

    let dt_str = format!("{} 00:00:00", since);
    if let Ok(ndt) = NaiveDateTime::parse_from_str(&dt_str, "%Y-%m-%d %H:%M:%S") {
        let dt = ndt.and_local_timezone(Local).single().ok_or_else(|| anyhow::anyhow!("无效日期"))?;
        return Ok(dt.timestamp());
    }

    bail!("无法解析时间参数: {}，支持格式: 24h, 48h, 7d, 或 2026-03-20", since)
}

/// 获取消息列表（单页）
pub async fn fetch_mentions(
    client: &HupuClient,
    plate: &str,
    page_str: Option<&str>,
) -> Result<FetchResult> {
    let url = if let Some(ps) = page_str {
        format!("{}?plate={}&pageStr={}", API_URL, plate, ps)
    } else {
        format!("{}?plate={}", API_URL, plate)
    };

    let resp = client
        .client
        .get(&url)
        .header("referer", "https://my.hupu.com/message?tabKey=1")
        .send()
        .await?;

    let api: ApiResponse = resp.json().await?;

    // code=0 或 code=1 都表示成功
    if api.code > 1 {
        bail!("API 错误 code={}: {:?}", api.code, api.msg);
    }

    let data = api.data.unwrap_or_default();
    Ok(FetchResult {
        items: data.his_list,
        has_next_page: data.has_next_page.unwrap_or(false),
        page_str: data.page_str,
    })
}

/// 获取消息列表（多页聚合）
pub async fn fetch_mentions_paginated(
    client: &HupuClient,
    plate: &str,
    since_ts: Option<i64>,
    limit: u32,
    max_pages: u32,
) -> Result<Vec<MentionItem>> {
    let mut all_items = Vec::new();
    let mut page_str: Option<String> = None;
    let mut page_count = 0;

    loop {
        let result = fetch_mentions(client, plate, page_str.as_deref()).await?;

        // 按时间过滤
        let mut items = result.items;
        if let Some(ts) = since_ts {
            items.retain(|item| item.update_time.unwrap_or(0) >= ts);
        }

        all_items.extend(items);
        page_count += 1;

        // 退出条件：没有下一页 或 达到最大页数
        if !result.has_next_page || page_count >= max_pages {
            break;
        }

        page_str = result.page_str;
    }

    // 最后截断到限制数量
    all_items.truncate(limit as usize);
    Ok(all_items)
}

/// 格式化输出 - 表格
pub fn format_table(items: &[MentionItem]) {
    if items.is_empty() {
        println!("没有找到消息");
        return;
    }

    let max_user = items.iter()
        .map(|i| i.username.as_deref().unwrap_or("").chars().count())
        .max().unwrap_or(4).min(15);
    let max_title = items.iter()
        .map(|i| i.thread_title.as_deref().unwrap_or("").chars().count())
        .max().unwrap_or(4).min(30);
    let max_content = 40;

    let w1 = max_user + 2;
    let w2 = max_title + 2;
    let w3 = max_content + 2;

    let top_border = format!("┌{}┬{}┬{}┬{}┐", "─".repeat(8), "─".repeat(w1), "─".repeat(w2), "─".repeat(w3));
    let header = format!("│ 时间   │ {:^width1$} │ {:^width2$} │ {:^width3$} │",
        "用户", "帖子", "回复内容",
        width1 = max_user, width2 = max_title, width3 = max_content
    );
    let mid_border = format!("├{}┼{}┼{}┼{}┤", "─".repeat(8), "─".repeat(w1), "─".repeat(w2), "─".repeat(w3));
    let bottom_border = format!("└{}┴{}┴{}┴{}┘", "─".repeat(8), "─".repeat(w1), "─".repeat(w2), "─".repeat(w3));

    println!("{}", top_border);
    println!("{}", header);
    println!("{}", mid_border);

    for item in items {
        let time = item.publish_time.as_deref().unwrap_or("");
        let user = item.username.as_deref().unwrap_or("");
        let title = item.thread_title.as_deref().unwrap_or("");
        let content = item.post_content.as_deref().unwrap_or("");

        println!("│ {:6} │ {:width1$} │ {:width2$} │ {:width3$} │",
            truncate(time, 8),
            truncate(user, max_user),
            truncate(title, max_title),
            truncate(&strip_html(content), max_content),
            width1 = max_user, width2 = max_title, width3 = max_content
        );
    }

    println!("{}", bottom_border);
    println!("共 {} 条", items.len());
}

/// 格式化输出 - 简洁
pub fn format_simple(items: &[MentionItem]) {
    if items.is_empty() {
        println!("没有找到消息");
        return;
    }

    for item in items {
        let time = item.publish_time.as_deref().unwrap_or("");
        let user = item.username.as_deref().unwrap_or("");
        let content = item.post_content.as_deref().unwrap_or("");
        let title = item.thread_title.as_deref().unwrap_or("");
        let quote = item.quote_content.as_deref().unwrap_or("");

        println!("[{}] {} 回复了你:", time, user);
        if !quote.is_empty() {
            println!("  你说: {}", truncate(&strip_html(quote), 60));
        }
        println!("  回复: {}", strip_html(content));
        println!("  帖子: {}", title);
        println!("  链接: {}", item.reply_url());
        println!();
    }
    println!("共 {} 条", items.len());
}

/// 格式化输出 - JSON
pub fn format_json(items: &[MentionItem]) -> Result<()> {
    let output: Vec<serde_json::Value> = items.iter().map(|item| {
        serde_json::json!({
            "username": item.username,
            "thread_title": item.thread_title,
            "post_content": item.post_content.as_deref().map(|s| strip_html(s)),
            "quote_content": item.quote_content.as_deref().map(|s| strip_html(s)),
            "tid": item.tid,
            "pid": item.pid,
            "topic_id": item.topic_id,
            "update_time": item.update_time,
            "url": item.url(),
            "reply_url": item.reply_url(),
        })
    }).collect();
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_len {
        s.to_string()
    } else {
        chars[..max_len.saturating_sub(2)].iter().collect::<String>() + ".."
    }
}

