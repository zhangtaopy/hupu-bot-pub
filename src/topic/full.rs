//! Read the chronological floor list and bind front-page hot replies by PID.
use super::{fetch_post_page, Author, HupuClient, Thread};
use anyhow::{ensure, Context, Result};
use futures_util::{stream, StreamExt, TryStreamExt};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Deserialize)]
struct FullDetail {
    thread: FullPost,
    replies: FloorPage,
    #[serde(default)]
    lights: Vec<Floor>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FullPost {
    #[serde(flatten)]
    post: Thread,
    video: Option<serde_json::Value>,
    #[serde(default)]
    has_video: serde_json::Value,
}
#[derive(Deserialize)]
struct FloorPage {
    current: usize,
    total: usize,
    count: usize,
    list: Vec<Floor>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Floor {
    pid: String,
    author: Option<Author>,
    content: Option<String>,
    all_light_count: Option<i32>,
    quote: Option<Box<Floor>>,
    #[serde(default)]
    is_hidden: bool,
    #[serde(default)]
    is_delete: bool,
    #[serde(default)]
    is_self_delete: bool,
    #[serde(default)]
    is_starter: bool,
}

#[derive(Serialize, Default)]
pub struct Body {
    pub text: String,
    pub media: Vec<Attachment>,
}
#[derive(Serialize)]
pub struct Attachment {
    kind: &'static str,
    url: String,
    caption: String,
}
#[derive(Serialize)]
pub struct Quote {
    pub id: String,
    pub author: String,
    #[serde(flatten)]
    pub body: Body,
}
#[derive(Serialize)]
pub struct Source {
    pub id: String,
    pub author: String,
    pub by_op: bool,
    pub lights: Option<i32>,
    pub url: String,
    pub page: usize,
    pub hidden: bool,
    #[serde(flatten)]
    pub body: Body,
    pub quotes: Vec<Quote>,
}
#[derive(Serialize)]
pub struct Conversation {
    pub title: String,
    pub total_replies: usize,
    pub pages: usize,
    pub floor_count: usize,
    pub hot_ids: Vec<String>,
    pub sources: Vec<Source>,
}

static TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<[^>]+>").unwrap());
static ATTRIBUTE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)\s([\w:-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).unwrap());

fn web_url(raw: &str) -> Option<String> {
    let raw = html_escape::decode_html_entities(raw);
    let url = if raw.starts_with("//") {
        format!("https:{raw}")
    } else {
        raw.into_owned()
    };
    let parsed = reqwest::Url::parse(&url).ok()?;
    matches!(parsed.scheme(), "http" | "https").then_some(url)
}

fn body(html: &str) -> Body {
    let html = crate::utils::decode_json_escapes(html);
    let mut media = Vec::new();
    let plain = TAG.replace_all(&html, |caps: &regex::Captures| {
        let tag = &caps[0];
        let name = tag
            .trim_start_matches('<')
            .split([' ', '\t', '\n', '\r', '>'])
            .next()
            .unwrap_or("")
            .trim_end_matches('/')
            .to_ascii_lowercase();
        if matches!(name.as_str(), "br" | "/p" | "/div" | "/li" | "/blockquote") {
            return "\n".to_owned();
        }
        let kind = match name.as_str() {
            "a" => "链接",
            "img" => "图片",
            "video" | "source" => "视频",
            _ => return String::new(),
        };
        let mut url = None;
        let mut caption = String::new();
        for attr in ATTRIBUTE.captures_iter(tag) {
            let value = (2..=4).find_map(|n| attr.get(n)).unwrap().as_str();
            match attr[1].to_ascii_lowercase().as_str() {
                "src" | "data-src" | "data-original" | "href" if url.is_none() => {
                    url = web_url(value)
                }
                "alt" | "title" if caption.is_empty() => {
                    caption = html_escape::decode_html_entities(value).into_owned()
                }
                _ => (),
            }
        }
        if let Some(url) = url {
            media.push(Attachment {
                kind,
                url,
                caption: caption.clone(),
            });
        }
        if kind == "链接" {
            return String::new();
        }
        if caption.is_empty() {
            format!("[{kind}]")
        } else {
            format!("[{kind}：{caption}]")
        }
    });
    let text = html_escape::decode_html_entities(&plain)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Body { text, media }
}

impl Floor {
    fn hidden(&self) -> bool {
        self.is_hidden || self.is_delete || self.is_self_delete
    }
    fn author(&self) -> String {
        self.author
            .as_ref()
            .and_then(|a| a.puname.clone())
            .unwrap_or_default()
    }
    fn readable_body(&self) -> Body {
        if self.hidden() {
            Body::default()
        } else {
            body(self.content.as_deref().unwrap_or_default())
        }
    }
    fn source(self, tid: &str, page: usize) -> Source {
        let mut result = Source {
            url: format!("https://bbs.hupu.com/{tid}-{page}.html?pid={}", self.pid),
            id: self.pid.clone(),
            author: self.author(),
            by_op: self.is_starter,
            page,
            lights: self.all_light_count,
            hidden: self.hidden(),
            body: self.readable_body(),
            quotes: Vec::new(),
        };
        if !result.hidden {
            let mut quoted = self.quote;
            while let Some(q) = quoted {
                result.quotes.push(Quote {
                    id: q.pid.clone(),
                    author: q.author(),
                    body: q.readable_body(),
                });
                if q.hidden() {
                    break;
                }
                quoted = q.quote;
            }
        }
        result
    }
}

async fn page(client: &HupuClient, tid: &str, n: usize) -> Result<FullDetail> {
    let props = fetch_post_page::<FullDetail>(client, tid, n)
        .await?
        .props
        .page_props;
    if let Some(err) = props.detail_error_info {
        ensure!(err.code == 200, "帖子访问失败：{}", err.message);
    }
    let detail = props.detail.context("帖子暂不可读")?;
    ensure!(
        detail.thread.post.tid == tid && detail.replies.current == n,
        "虎扑返回了其他帖子或页码"
    );
    Ok(detail)
}

pub async fn fetch(client: &HupuClient, tid: &str) -> Result<Conversation> {
    let first = page(client, tid, 1).await.context("读取第 1 页失败")?;
    let total_replies = first.replies.count;
    let pages = first.replies.total.max(1);
    let thread = first.thread.post;
    let mut op = body(&thread.content);
    let video = first
        .thread
        .video
        .as_ref()
        .and_then(|v| v.as_str().or_else(|| v.get("url")?.as_str()))
        .and_then(web_url);
    if video.is_some()
        || matches!(first.thread.has_video, serde_json::Value::Bool(true))
        || first.thread.has_video.as_u64().unwrap_or(0) > 0
    {
        if !op.text.contains("[视频") {
            op.text.push_str("\n[视频]");
        }
        if let Some(url) = video {
            if !op.media.iter().any(|m| m.url == url) {
                op.media.push(Attachment {
                    kind: "视频",
                    url,
                    caption: String::new(),
                });
            }
        }
    }
    let mut sources = vec![Source {
        id: "op".into(),
        author: thread.author.and_then(|a| a.puname).unwrap_or_default(),
        by_op: true,
        page: 1,
        url: format!("https://bbs.hupu.com/{tid}.html"),
        lights: thread.lights,
        hidden: false,
        body: op,
        quotes: vec![],
    }];
    let mut seen = HashSet::new();
    let mut append = |n, rows: Vec<Floor>| -> Result<()> {
        let before = seen.len();
        for row in rows {
            if seen.insert(row.pid.clone()) {
                sources.push(row.source(tid, n));
            }
        }
        ensure!(
            total_replies == 0 || before < seen.len(),
            "第 {n} 页没有返回新楼层，无法确认读完整帖，请重试"
        );
        Ok(())
    };
    append(1, first.replies.list)?;
    let mut next = stream::iter(2..=pages)
        .map(|n| async move {
            let page = page(client, tid, n)
                .await
                .with_context(|| format!("读取第 {n} 页失败"))?
                .replies;
            ensure!(
                page.total == pages && page.count == total_replies,
                "抓取中楼层数发生变化，请重新读取"
            );
            Ok::<_, anyhow::Error>((n, page.list))
        })
        .buffered(3);
    while let Some((n, rows)) = next.try_next().await? {
        append(n, rows)?;
    }
    let floor_count = sources.len() - 1;
    let mut hot_ids = Vec::new();
    for floor in first.lights {
        if floor.hidden() || hot_ids.contains(&floor.pid) {
            continue;
        }
        hot_ids.push(floor.pid.clone());
        if seen.insert(floor.pid.clone()) {
            sources.push(floor.source(tid, 1));
        }
    }
    Ok(Conversation {
        title: thread.title,
        total_replies,
        floor_count,
        hot_ids,
        pages,
        sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn body_keeps_paragraphs_entities_and_media_captions() {
        let b = body(
            r#"<p>第一段 &amp; 第二段</p><p><img data-src='https://example.org/img' alt='记者原话'></p>"#,
        );
        assert_eq!(b.text, "第一段 & 第二段\n[图片：记者原话]");
        assert_eq!(b.media.len(), 1);
    }
}
