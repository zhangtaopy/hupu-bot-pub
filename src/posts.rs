use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::client::HupuClient;
use crate::utils::strip_html;

#[derive(Deserialize, Debug, Clone, Default)]
#[allow(dead_code)]
struct ApiPostItem {
    #[serde(default)]
    tid: i64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    create_time: i64,
    #[serde(default)]
    lastpost_time: i64,
    #[serde(default)]
    replies: i64,
    #[serde(default)]
    visits: i64,
    #[serde(default)]
    lights: i64,
    #[serde(default)]
    recommend_num: i64,
    #[serde(default)]
    forum_name: String,
    #[serde(default)]
    topic_name: String,
    #[serde(default)]
    topic_id: i64,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    total_pics: i64,
    #[serde(default, alias = "type")]
    post_type: String,
    #[serde(default)]
    share_num: i64,
    #[serde(default)]
    nickname: Option<String>,
    #[serde(default)]
    puid: Option<i64>,
}

// ── Public data struct ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostRow {
    pub tid: i64,
    pub euid: String,
    pub username: String,
    pub title: String,
    pub summary: String,
    pub create_time: i64,
    pub lastpost_time: i64,
    pub replies: i64,
    pub visits: i64,
    pub lights: i64,
    pub recommend_num: i64,
    pub forum_name: String,
    pub topic_name: String,
    pub topic_id: i64,
    pub total_pics: i64,
    pub has_video: bool,
    pub share_num: i64,
    pub format_time: Option<String>,
}

impl PostRow {
    pub fn url(&self) -> String {
        format!("https://bbs.hupu.com/{}.html", self.tid)
    }
}

const API_BASE: &str = "https://my.hupu.com/pcmapi/pc/space/v1/getThreadList";
pub const PAGE_SIZE: u32 = 30;

#[derive(Deserialize, Debug)]
#[allow(non_snake_case)]
struct ApiListResponse {
    code: i64,
    #[allow(dead_code)]
    internalCode: Option<String>,
    #[allow(dead_code)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<Vec<ApiPostItem>>,
}

// ── Fetch posts from API ──

pub async fn fetch_posts_paginated(
    client: &HupuClient,
    euid: &str,
    max_pages: u32,
) -> Result<Vec<PostRow>> {
    let mut all_posts = Vec::new();

    for page in 1..=max_pages {
        let posts = fetch_posts_page(client, euid, page).await?;

        let count = posts.len();
        let total_fetched = all_posts.len() + count;
        println!("Page {}: fetched {} posts (total: {})", page, count, total_fetched);

        all_posts.extend(posts);

        if (count as u32) < PAGE_SIZE {
            break;
        }
    }

    Ok(all_posts)
}

pub async fn fetch_posts_page(
    client: &HupuClient,
    euid: &str,
    page: u32,
) -> Result<Vec<PostRow>> {
    let url = format!(
        "{}?euid={}&page={}&pageSize={}",
        API_BASE, euid, page, PAGE_SIZE
    );

    let resp = client
        .client
        .get(&url)
        .header("referer", format!("https://my.hupu.com/{}", euid))
        .send()
        .await?;

    let body: ApiListResponse = resp.json().await?;

    if body.code != 1 {
        anyhow::bail!("API returned error code: {}", body.code);
    }

    let items = body.data.unwrap_or_default();
    if items.is_empty() {
        return Ok(Vec::new());
    }

    // Use nickname from the first item as the username
    let username = items[0].nickname.as_deref().unwrap_or("未知用户").to_string();

    let posts: Vec<PostRow> = items
        .into_iter()
        .map(|item| {
            let fmt_time = chrono::DateTime::from_timestamp(item.create_time, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string());
            PostRow {
                tid: item.tid,
                euid: euid.to_string(),
                username: item.nickname.unwrap_or_else(|| username.clone()),
                title: item.title,
                summary: strip_html(&item.summary),
                create_time: item.create_time,
                lastpost_time: item.lastpost_time,
                replies: item.replies,
                visits: item.visits,
                lights: item.lights,
                recommend_num: item.recommend_num,
                forum_name: item.forum_name,
                topic_name: item.topic_name,
                topic_id: item.topic_id,
                total_pics: item.total_pics,
                has_video: item.post_type == "vt",
                share_num: item.share_num,
                format_time: fmt_time,
            }
        })
        .collect();

    Ok(posts)
}

// ── Format ──

fn format_time(post: &PostRow) -> String {
    if let Some(ft) = &post.format_time {
        if !ft.is_empty() {
            return ft.clone();
        }
    }
    chrono::DateTime::from_timestamp(post.create_time, 0)
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

pub fn format_table(posts: &[PostRow]) {
    if posts.is_empty() {
        println!("没有找到发帖");
        return;
    }

    let max_topic = posts
        .iter()
        .map(|r| r.topic_name.chars().count())
        .max()
        .unwrap_or(4)
        .min(12);
    let max_title = 30;

    let w1 = max_topic + 2;
    let w2 = max_title + 2;

    println!(
        "┌{}┬{}┬{}┬{}┬{}┬{}┐",
        "─".repeat(8),
        "─".repeat(w1),
        "─".repeat(w2),
        "─".repeat(8),
        "─".repeat(6),
        "─".repeat(6)
    );
    println!(
        "│ {:6} │ {:width1$} │ {:width2$} │ {:6} │ {:4} │ {:4} │",
        "时间",
        "板块",
        "标题",
        "回复",
        "点亮",
        "浏览",
        width1 = max_topic,
        width2 = max_title,
    );
    println!(
        "├{}┼{}┼{}┼{}┼{}┼{}┤",
        "─".repeat(8),
        "─".repeat(w1),
        "─".repeat(w2),
        "─".repeat(8),
        "─".repeat(6),
        "─".repeat(6)
    );

    for p in posts {
        println!(
            "│ {:6} │ {:width1$} │ {:width2$} │ {:6} │ {:4} │ {:4} │",
            truncate(&format_time(p), 8),
            truncate(&p.topic_name, max_topic),
            truncate(&p.title, max_title),
            p.replies,
            p.lights,
            p.visits,
            width1 = max_topic,
            width2 = max_title,
        );
    }

    println!(
        "└{}┴{}┴{}┴{}┴{}┴{}┘",
        "─".repeat(8),
        "─".repeat(w1),
        "─".repeat(w2),
        "─".repeat(8),
        "─".repeat(6),
        "─".repeat(6)
    );
    println!("共 {} 条发帖", posts.len());
}

pub fn format_simple(posts: &[PostRow]) {
    if posts.is_empty() {
        println!("没有找到发帖");
        return;
    }

    for p in posts {
        println!(
            "[{}] [{}] {}",
            format_time(p),
            p.topic_name,
            p.title
        );
        if !p.summary.is_empty() {
            println!("  {}", truncate(&p.summary, 80));
        }
        println!(
            "  回复:{} 亮:{} 浏览:{} {}",
            p.replies, p.lights, p.visits, p.url()
        );
        println!();
    }
    println!("共 {} 条", posts.len());
}

pub fn format_json(posts: &[PostRow]) -> Result<()> {
    let output: Vec<serde_json::Value> = posts
        .iter()
        .map(|p| {
            serde_json::json!({
                "tid": p.tid,
                "title": p.title,
                "summary": p.summary,
                "create_time": p.create_time,
                "lastpost_time": p.lastpost_time,
                "replies": p.replies,
                "visits": p.visits,
                "lights": p.lights,
                "recommend_num": p.recommend_num,
                "forum_name": p.forum_name,
                "topic_name": p.topic_name,
                "topic_id": p.topic_id,
                "total_pics": p.total_pics,
                "has_video": p.has_video,
                "share_num": p.share_num,
                "format_time": p.format_time,
                "url": p.url(),
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_row_url() {
        let row = PostRow {
            tid: 12345,
            euid: "u1".into(),
            username: "test".into(),
            title: "标题".into(),
            summary: "摘要".into(),
            create_time: 1700000000,
            lastpost_time: 1700000100,
            replies: 10,
            visits: 100,
            lights: 5,
            recommend_num: 1,
            forum_name: "步行街".into(),
            topic_name: "步行街".into(),
            topic_id: 1,
            total_pics: 0,
            has_video: false,
            share_num: 0,
            format_time: Some("2024-01-01".into()),
        };
        assert_eq!(row.url(), "https://bbs.hupu.com/12345.html");
    }
}