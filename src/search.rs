use anyhow::Result;
use once_cell::sync::Lazy;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::client::HupuClient;
use crate::utils::find_matching_brace;

/// 搜索结果项
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub content: Option<String>,
    pub username: Option<String>,
    #[serde(rename = "addTimeDisplay")]
    pub add_time: Option<String>,
    pub replies: Option<String>,
    pub lights: Option<String>,
    #[serde(rename = "recNum")]
    pub rec_num: Option<String>,
    #[serde(rename = "forum_name")]
    pub forum_name: Option<String>,
    pub fid: Option<String>,
}

impl SearchResult {
    pub fn url(&self) -> String {
        format!("https://bbs.hupu.com/{}.html", self.id)
    }
}

/// 搜索响应
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub total: u32,
    pub total_page: u32,
    pub results: Vec<SearchResult>,
}

/// JSON 根结构
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchData {
    search_res: SearchRes,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchRes {
    count: u32,
    total_page: u32,
    data: Vec<SearchResult>,
}

/// 清理标题中的 HTML 标签
static HTML_TAG_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"<[^>]+>").unwrap()
});

/// 搜索虎扑帖子
///
/// # 参数
/// - `client`  : HupuClient
/// - `keyword` : 搜索关键词
/// - `page`    : 页码（从1开始）
/// - `limit`   : 限制返回条数
/// - `forum`   : 可选，板块 ID 过滤
/// - `sort`    : 可选，排序方式 (general/createtime/createtimeasc/replytime/light/reply)
pub async fn search_posts(
    client: &HupuClient,
    keyword: &str,
    page: u32,
    limit: usize,
    forum: Option<&str>,
    sort: Option<&str>,
) -> Result<SearchResponse> {
    let encoded_keyword = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let mut url = format!(
        "https://bbs.hupu.com/search?q={}&page={}",
        encoded_keyword, page
    );

    // 添加板块过滤（虎扑搜索用 topicId 参数）
    if let Some(fid) = forum {
        url.push_str(&format!("&topicId={}", fid));
    }

    // 添加排序（虎扑搜索用 sortby 参数）
    // general=综合, createtime=最新, createtimeasc=最早, replytime=回复时间, light=亮回复数, reply=回复数
    if let Some(s) = sort {
        url.push_str(&format!("&sortby={}", s));
    }

    let text = client
        .client
        .get(&url)
        .send()
        .await?
        .text()
        .await?;

    parse_search_results(&text, limit)
}

/// 从 HTML 中解析搜索结果
fn parse_search_results(html: &str, limit: usize) -> Result<SearchResponse> {
    // 找到 window.$$data= 的位置
    let marker = "window.$$data=";
    let start = html
        .find(marker)
        .ok_or_else(|| anyhow::anyhow!("未找到搜索数据 (window.$$data)"))?;

    // JSON 从 = 后面开始
    let json_start = start + marker.len();

    // 找到匹配的 }（JSON 对象结束）
    let json_end = find_matching_brace(html, json_start)
        .ok_or_else(|| anyhow::anyhow!("无法解析 JSON 数据"))?;

    let json_str = &html[json_start..=json_end];

    // 用 serde_json 解析
    let data: SearchData = serde_json::from_str(json_str)?;

    // 清理 HTML 标签并限制数量
    let mut results: Vec<SearchResult> = data.search_res.data.into_iter().map(|mut r| {
        r.title = HTML_TAG_REGEX.replace_all(&r.title, "").to_string();
        r.title = html_escape::decode_html_entities(&r.title).to_string();
        if let Some(ref content) = r.content {
            r.content = Some(
                html_escape::decode_html_entities(
                    &HTML_TAG_REGEX.replace_all(content, "")
                ).to_string()
            );
        }
        r
    }).collect();

    results.truncate(limit);

    Ok(SearchResponse {
        total: data.search_res.count,
        total_page: data.search_res.total_page,
        results,
    })
}

/// 格式化输出 - 表格
pub fn format_search_table(response: &SearchResponse) {
    if response.results.is_empty() {
        println!("没有找到相关帖子（共 {} 条结果）", response.total);
        return;
    }

    println!(
        "搜索结果：共 {} 条，{} 页（当前第 1 页显示 {} 条）",
        response.total,
        response.total_page,
        response.results.len()
    );
    println!();

    for (i, item) in response.results.iter().enumerate() {
        let forum = item.forum_name.as_deref().unwrap_or("未知板块");
        let time = item.add_time.as_deref().unwrap_or("未知时间");
        let replies = item.replies.as_deref().unwrap_or("0");
        let lights = item.lights.as_deref().unwrap_or("0");
        let author = item.username.as_deref().unwrap_or("匿名");

        println!("[{}] {}", i + 1, item.title);
        println!(
            "    📂 {} | 👤 {} | 📅 {} | 💬 {}回复 | 💡 {}亮",
            forum, author, time, replies, lights
        );
        println!("    🔗 {}", item.url());

        if let Some(ref content) = item.content {
            let preview = truncate(content, 80);
            if !preview.is_empty() {
                println!("    📄 {}", preview);
            }
        }
        println!();
    }
}

/// 格式化输出 - 简洁列表
pub fn format_search_simple(response: &SearchResponse) {
    if response.results.is_empty() {
        println!("没有找到相关帖子（共 {} 条结果）", response.total);
        return;
    }

    println!("共 {} 条结果，{} 页", response.total, response.total_page);
    println!();

    for (i, item) in response.results.iter().enumerate() {
        let forum = item.forum_name.as_deref().unwrap_or("");
        println!("[{}] {} ({})", i + 1, item.title, forum);
        println!("    {}", item.url());
        println!();
    }
}

/// 格式化输出 - JSON
pub fn format_search_json(response: &SearchResponse) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(response)?);
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
