use anyhow::{bail, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::client::HupuClient;
use crate::utils::{decode_json_escapes, find_matching_brace, strip_html};

/// 帖子信息
#[derive(Debug, Clone, Serialize)]
pub struct Post {
    pub tid: String,
    pub title: String,
    pub author: Option<String>,
    pub reply_count: Option<i32>,
    pub light_count: Option<i32>,
    pub create_time: Option<String>,
}

impl Post {
    pub fn url(&self) -> String {
        format!("https://bbs.hupu.com/{}.html", self.tid)
    }
}

/// 帖子详情（正文）
#[derive(Debug, Serialize)]
pub struct PostDetail {
    pub tid: String,
    pub title: String,
    pub author: String,
    pub content: String,
    pub create_time: Option<String>,
    pub reply_count: Option<i32>,
    pub light_count: Option<i32>,
}

/// 帖子回复
#[derive(Debug, Clone, Serialize)]
pub struct PostReply {
    pub pid: String,
    pub username: String,
    pub content: String,
    pub light_count: i32,
    pub create_time: Option<String>,
}

// ============ JSON 解析结构 ============

#[derive(Debug, Deserialize)]
struct NextData {
    props: Props,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Props {
    page_props: PageProps,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageProps {
    detail: Option<Detail>,
    detail_error_info: Option<DetailError>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailError {
    code: i32,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Detail {
    thread: Option<Thread>,
    lights: Option<Vec<ReplyData>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Thread {
    tid: String,
    title: String,
    content: String,
    #[serde(default)]
    create_time: Option<String>,
    lights: Option<i32>,
    replies: Option<i32>,
    author: Option<Author>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Author {
    puname: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplyData {
    pid: String,
    author: Option<Author>,
    content: String,
    all_light_count: Option<i32>,
    created_at_format: Option<String>,
}

// ============ HTML 解析正则 ============

// 帖子列表正则（HTML 元素，无法用 JSON）
static POST_LINK_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"<a[^>]*href="/(\d{9})\.html"[^>]*class="p-title"[^>]*>([^<]+)</a>"#).unwrap()
});
static POST_LINK_REGEX_FALLBACK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"<a[^>]*href="/(\d{9})\.html"[^>]*>([^<]+)</a>"#).unwrap()
});

/// Check if the response is a WAF challenge page (Aliyun, Cloudflare, etc.)
fn check_waf_block(html: &str) -> Result<()> {
    // Aliyun WAF challenge
    if html.contains("aliyun_waf_aa") || html.contains("aliyun_waf_bb") {
        bail!("被反爬系统拦截（阿里云 WAF），请更新 Cookie 或更换 IP");
    }
    // Cloudflare / generic JS challenge
    if html.len() < 500 {
        let s = html.to_lowercase();
        if s.contains("challenge") && (s.contains("javascript") || s.contains("cf-")) {
            bail!("被反爬系统拦截（JS 挑战页面），请更新 Cookie 或更换 IP");
        }
    }
    Ok(())
}

/// 获取指定 topic 的帖子列表
pub async fn fetch_topic_posts(
    client: &HupuClient,
    topic_id: &str,
    page: u32,
    limit: usize,
) -> Result<Vec<Post>> {
    let url = if page == 1 {
        format!("https://bbs.hupu.com/{}", topic_id)
    } else {
        format!("https://bbs.hupu.com/{}-{}", topic_id, page)
    };

    let text = client
        .client
        .get(&url)
        .send()
        .await?
        .text()
        .await?;

    check_waf_block(&text)?;
    let posts = parse_post_list(&text, limit);
    Ok(posts)
}

/// 解析帖子列表（从 HTML 中提取 TID + 标题；其他字段由 detail 接口获取）
fn parse_post_list(html: &str, limit: usize) -> Vec<Post> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut posts = Vec::new();

    for regex in [&POST_LINK_REGEX as &Regex, &POST_LINK_REGEX_FALLBACK] {
        for cap in regex.captures_iter(html) {
            if posts.len() >= limit {
                break;
            }
            let tid = cap[1].to_string();
            if seen.contains(&tid) {
                continue;
            }
            seen.insert(tid.clone());
            let title = html_escape::decode_html_entities(&cap[2]).trim().to_string();
            if !title.is_empty() {
                posts.push(Post {
                    tid,
                    title,
                    author: None,
                    reply_count: None,
                    light_count: None,
                    create_time: None,
                });
            }
        }
        if !posts.is_empty() {
            break;
        }
    }

    posts
}

/// 一次性获取帖子详情 + 热门回复（合并请求，避免重复 HTTP 调用）
pub async fn fetch_post_combined(client: &HupuClient, tid: &str, reply_limit: usize) -> Result<(PostDetail, Vec<PostReply>)> {
    let data = fetch_post_data(client, tid).await?;

    if let Some(err) = &data.props.page_props.detail_error_info {
        if err.code != 200 {
            anyhow::bail!("帖子访问失败: {} (code: {})", err.message, err.code);
        }
    }

    let detail_info = data
        .props
        .page_props
        .detail
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("帖子不存在或无法访问"))?;

    let thread = detail_info.thread.as_ref()
        .ok_or_else(|| anyhow::anyhow!("帖子不存在或已被删除"))?;

    let author_name = thread
        .author
        .as_ref()
        .and_then(|a| a.puname.as_ref())
        .cloned()
        .unwrap_or_default();

    let create_time = thread.create_time.clone(); // "2025-06-02T20:30:00.000+08:00" or similar

    let detail = PostDetail {
        tid: thread.tid.clone(),
        title: thread.title.clone(),
        author: author_name,
        content: strip_html(&decode_json_escapes(&thread.content)),
        create_time,
        reply_count: thread.replies,
        light_count: thread.lights,
    };

    let replies: Vec<PostReply> = detail_info
        .lights
        .as_ref()
        .map(|lights| {
            lights.iter().take(reply_limit).map(|r| PostReply {
                pid: r.pid.clone(),
                username: r.author.as_ref().and_then(|a| a.puname.clone()).unwrap_or_default(),
                content: strip_html(&decode_json_escapes(&r.content)),
                light_count: r.all_light_count.unwrap_or(0),
                create_time: r.created_at_format.clone(),
            }).collect()
        })
        .unwrap_or_default();

    Ok((detail, replies))
}

/// 获取帖子详情和热门回复
pub async fn fetch_post_detail(client: &HupuClient, tid: &str) -> Result<PostDetail> {
    let data = fetch_post_data(client, tid).await?;

    // 检查错误信息（code != 200 表示错误）
    if let Some(err) = &data.props.page_props.detail_error_info {
        if err.code != 200 {
            bail!("帖子访问失败: {} (code: {})", err.message, err.code);
        }
    }

    let thread = data
        .props
        .page_props
        .detail
        .as_ref()
        .and_then(|d| d.thread.as_ref())
        .ok_or_else(|| anyhow::anyhow!("帖子不存在或已被删除"))?;

    let author_name = thread
        .author
        .as_ref()
        .and_then(|a| a.puname.as_ref())
        .cloned()
        .unwrap_or_default();

    Ok(PostDetail {
        tid: thread.tid.clone(),
        title: thread.title.clone(),
        author: author_name,
        content: strip_html(&decode_json_escapes(&thread.content)),
        create_time: None,
        reply_count: thread.replies,
        light_count: thread.lights,
    })
}

/// 获取帖子热门回复
pub async fn fetch_post_replies(
    client: &HupuClient,
    tid: &str,
    limit: usize,
) -> Result<Vec<PostReply>> {
    let data = fetch_post_data(client, tid).await?;

    // 检查错误信息（code != 200 表示错误）
    if let Some(err) = &data.props.page_props.detail_error_info {
        if err.code != 200 {
            bail!("帖子访问失败: {} (code: {})", err.message, err.code);
        }
    }

    let detail = data
        .props
        .page_props
        .detail
        .ok_or_else(|| anyhow::anyhow!("帖子不存在或无法访问"))?;

    let replies: Vec<PostReply> = detail
        .lights
        .unwrap_or_default()
        .into_iter()
        .take(limit)
        .map(|r| PostReply {
            pid: r.pid,
            username: r.author.and_then(|a| a.puname).unwrap_or_default(),
            content: strip_html(&decode_json_escapes(&r.content)),
            light_count: r.all_light_count.unwrap_or(0),
            create_time: r.created_at_format,
        })
        .collect();

    Ok(replies)
}

/// 获取帖子页面 JSON 数据
async fn fetch_post_data(client: &HupuClient, tid: &str) -> Result<NextData> {
    let url = format!("https://bbs.hupu.com/{}.html", tid);

    let text = client
        .client
        .get(&url)
        .send()
        .await?
        .text()
        .await?;

    check_waf_block(&text)?;
    // 查找 __NEXT_DATA__ 或 {"props":{"pageProps" 格式的 JSON
    let json_str = extract_next_data(&text)?;

    let data: NextData = serde_json::from_str(json_str)?;
    Ok(data)
}

/// 从 HTML 中提取 __NEXT_DATA__ JSON
fn extract_next_data(html: &str) -> Result<&str> {
    // 方式1: 查找 <script id="__NEXT_DATA__">
    if let Some(start) = html.find(r#"<script id="__NEXT_DATA__""#) {
        if let Some(content_start) = html[start..].find('>') {
            let json_start = start + content_start + 1;
            if let Some(end) = html[json_start..].find("</script>") {
                let json_str = &html[json_start..json_start + end];
                return Ok(json_str);
            }
        }
    }

    // 方式2: 查找 {"props":{"pageProps"
    let marker = r#"{"props":{"pageProps""#;
    if let Some(start) = html.find(marker) {
        // 找到匹配的 }
        let end = find_matching_brace(html, start)
            .ok_or_else(|| anyhow::anyhow!("无法找到 JSON 结束位置"))?;
        return Ok(&html[start..=end]);
    }

    bail!("未找到帖子数据")
}


/// 格式化输出 - 帖子列表表格
pub fn format_post_table(posts: &[Post]) {
    if posts.is_empty() {
        println!("没有找到帖子");
        return;
    }

    let max_title = posts
        .iter()
        .map(|p| p.title.chars().count())
        .max()
        .unwrap_or(4)
        .min(50);

    let w1 = 12;
    let w2 = max_title + 2;
    let w3 = 8;

    println!("┌{}┬{}┬{}┐", "─".repeat(w1), "─".repeat(w2), "─".repeat(w3));
    println!(
        "│ {:^width1$} │ {:^width2$} │ {:^width3$} │",
        "帖子ID",
        "标题",
        "",
        width1 = w1 - 2,
        width2 = w2 - 2,
        width3 = w3 - 2
    );
    println!("├{}┼{}┼{}┤", "─".repeat(w1), "─".repeat(w2), "─".repeat(w3));

    for post in posts {
        let tid = &post.tid;
        let title = truncate(&post.title, max_title);

        println!(
            "│ {:10} │ {:width2$} │ {:6} │",
            tid,
            title,
            "",
            width2 = max_title
        );
    }

    println!("└{}┴{}┴{}┘", "─".repeat(w1), "─".repeat(w2), "─".repeat(w3));
    println!("共 {} 条", posts.len());
}

/// 格式化输出 - 帖子详情
pub fn format_post_detail(detail: &PostDetail, replies: Option<&[PostReply]>) {
    let sep1 = "═".repeat(60);
    let sep2 = "─".repeat(60);

    println!("{}", sep1);
    println!("标题: {}", detail.title);
    println!("作者: {} | 帖子ID: {}", detail.author, detail.tid);
    if let Some(ref count) = detail.reply_count {
        println!("回复数: {}", count);
    }
    if let Some(ref lights) = detail.light_count {
        println!("亮了数: {}", lights);
    }
    println!("链接: https://bbs.hupu.com/{}.html", detail.tid);
    println!("{}", sep2);
    println!("正文:");
    println!("{}", detail.content);
    println!("{}", sep2);

    if let Some(replies) = replies {
        if !replies.is_empty() {
            println!("热评 (按点赞数排序):");
            for (i, reply) in replies.iter().enumerate() {
                println!(
                    "  [{}] [{}赞] {}: {}",
                    i + 1,
                    reply.light_count,
                    reply.username.as_str(),
                    truncate(&reply.content, 60)
                );
            }
        } else {
            println!("暂无热门回复");
        }
    }

    println!("{}", sep1);
}

/// 格式化输出 - 简洁列表
pub fn format_post_simple(posts: &[Post]) {
    if posts.is_empty() {
        println!("没有找到帖子");
        return;
    }

    for post in posts {
        println!("[{}] {}", post.tid, post.title);
        println!("    链接: {}", post.url());
        println!();
    }
    println!("共 {} 条", posts.len());
}

/// 格式化输出 - JSON
pub fn format_post_json(posts: &[Post]) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(posts)?);
    Ok(())
}

/// Try to convert a parsed time string to days-old. Returns None if unparseable.
/// Handles: "2024-12-20", "12-20 14:30", "3小时前", "昨天", etc.
#[allow(dead_code)]
pub fn time_str_days_old(s: &str) -> Option<i64> {
    let now = chrono::Utc::now();

    // Absolute date: 2024-12-20 or 2024-12-20 14:30
    if let Some(date_part) = s.split_whitespace().next() {
        if date_part.len() >= 10 && date_part.chars().filter(|c| *c == '-').count() == 2 {
            if let Ok(d) = chrono::NaiveDate::parse_from_str(&date_part[..10], "%Y-%m-%d") {
                let dt = d.and_hms_opt(0, 0, 0).unwrap();
                let days = (now.naive_utc() - dt).num_hours() / 24;
                return Some(days);
            }
        }
    }

    // MM-DD HH:MM → assume current year
    if s.len() >= 5 && &s[2..3] == "-" {
        let today = now.date_naive();
        let year = today.format("%Y").to_string();
        let full = format!("{}-{}", year, &s[..5]);
        if let Ok(d) = chrono::NaiveDate::parse_from_str(&full, "%Y-%m-%d") {
            let dt = d.and_hms_opt(0, 0, 0).unwrap();
            let days = (now.naive_utc() - dt).num_hours() / 24;
            return Some(days);
        }
    }

    // "N小时前"
    if let Some(hours) = s.strip_suffix("小时前").and_then(|h| h.parse::<i64>().ok()) {
        return Some(if hours < 24 { 0 } else { hours / 24 });
    }

    // "N分钟前"
    if s.contains("分钟前") {
        return Some(0);
    }

    // "昨天"
    if s.starts_with("昨天") {
        return Some(1);
    }

    None
}

fn truncate(s: &str, max_len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_len {
        s.to_string()
    } else {
        chars[..max_len.saturating_sub(2)].iter().collect::<String>() + ".."
    }
}
