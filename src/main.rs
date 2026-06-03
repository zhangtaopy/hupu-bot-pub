mod analyze;
mod api;
mod client;
mod config;
mod cookie_extract;
mod db;
mod deepseek;
mod mentions;
mod posts;
mod replies;
mod resolver;
mod search;
mod server;
mod services;
mod topic;
mod utils;

use anyhow::Result;
use clap::{Parser, Subcommand};
use client::HupuClient;

#[derive(Parser)]
#[command(name = "hupu-bot", about = "虎扑论坛命令行工具")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 点赞/取消点赞回复
    Like {
        /// 帖子 ID，如 638074156
        #[arg(short = 'p', long)]
        tid: String,

        /// 回复 ID
        #[arg(short = 'c', long)]
        pid: String,

        /// 板块 ID，如 278（汽车区）
        #[arg(short = 'f', long)]
        fid: String,

        /// 取消点赞
        #[arg(short = 'u', long)]
        undo: bool,
    },

    /// 回复帖子
    Reply {
        /// 板块 ID，如 278
        #[arg(short = 't', long)]
        topic_id: String,

        /// 帖子 ID，如 638074156
        #[arg(short = 'p', long)]
        tid: String,

        /// 回复内容
        #[arg(short = 'c', long)]
        content: String,

        /// 引用回复ID，回复指定发言时填写（被引用发言的 pid）
        #[arg(short = 'q', long)]
        quote_id: Option<String>,
    },

    /// 获取被回复/提到的消息
    Mentions {
        /// 消息类型: mentions(提到我的), comments(评论), likes(亮了/推荐)
        #[arg(short, long, default_value = "mentions")]
        tab: String,

        /// 时间过滤: 24h, 48h, 7d, 或具体日期 "2026-03-20"
        #[arg(short, long)]
        since: Option<String>,

        /// 限制条数
        #[arg(short, long, default_value = "20")]
        limit: u32,

        /// 最大页数
        #[arg(short, long, default_value = "5")]
        pages: u32,

        /// 输出格式: table, json, simple
        #[arg(short, long, default_value = "table")]
        format: String,
    },

    /// 搜索虎扑帖子
    Search {
        /// 搜索关键词
        #[arg(short = 'k', long)]
        keyword: String,

        /// 页码
        #[arg(short = 'p', long, default_value = "1")]
        page: u32,

        /// 限制条数
        #[arg(short = 'l', long, default_value = "20")]
        limit: usize,

        /// 输出格式: table, json, simple
        #[arg(short = 'f', long, default_value = "table")]
        format: String,

        /// 指定板块 ID 过滤
        #[arg(long)]
        forum: Option<String>,

        /// 排序方式: general(综合), createtime(最新), createtimeasc(最早), replytime(回复时间), light(亮回复数), reply(回复数)
        #[arg(short = 's', long, default_value = "general")]
        sort: Option<String>,
    },

    /// 启动 Web 可视化分析服务
    Serve {
        /// 端口号
        #[arg(short = 'p', long, default_value = "3000")]
        port: u16,

        /// 部署模式：跳过首次配置引导，用户自行在页面填写 Cookie 和 API Key
        #[arg(long, default_value = "false")]
        deploy: bool,
    },

    /// 分析用户回帖的相似度，找出重复/近似内容
    Analyze {
        /// 用户加密 UID
        #[arg(short = 'e', long)]
        euid: String,

        /// Jaccard 相似度阈值 (0~1)，越大越严格
        #[arg(short = 't', long, default_value = "0.5")]
        threshold: f64,

        /// 输出格式: table, json, simple
        #[arg(short = 'f', long, default_value = "simple")]
        format: String,
    },

    /// 获取用户回帖列表并存储到数据库
    Replies {
        /// 用户加密 UID（从个人主页 URL 获取）
        #[arg(short = 'e', long)]
        euid: String,

        /// 最大获取页数（0=自动获取全部）
        #[arg(short = 'p', long, default_value = "0")]
        max_pages: u32,

        /// 每页条数
        #[arg(short = 's', long, default_value = "10")]
        page_size: u32,

        /// 输出格式: table, json, simple
        #[arg(short = 'f', long, default_value = "table")]
        format: String,
    },

    /// 获取用户发帖数据（需要 cookie 登录）
    Posts {
        /// 用户加密 UID（从个人主页 URL 获取）
        #[arg(short = 'e', long)]
        euid: String,

        /// 最大获取页数（0=自动获取全部）
        #[arg(short = 'p', long, default_value = "0")]
        max_pages: u32,

        /// 输出格式: table, json, simple
        #[arg(short = 'f', long, default_value = "table")]
        format: String,
    },

    /// 从 Chrome/Edge 浏览器自动提取虎扑 Cookie 并保存到 config.json
    ExtractCookies,

    /// 获取板块帖子列表 / 帖子详情 / 热门回复
    Topic {
        /// 板块 ID（如 278 汽车区）或帖子 ID（9位数字）
        #[arg(short, long)]
        id: String,

        /// 页码（获取帖子列表时）
        #[arg(short, long, default_value = "1")]
        page: u32,

        /// 限制条数
        #[arg(short, long, default_value = "20")]
        limit: usize,

        /// 输出格式: table, json, simple
        #[arg(short, long, default_value = "table")]
        format: String,

        /// 获取帖子详情和热门回复
        #[arg(short, long)]
        detail: bool,

        /// 热门回复条数（配合 --detail 使用）
        #[arg(short, long, default_value = "10")]
        replies: usize,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // CLI 命令需要配置才能运行，Serve 命令允许无配置启动（由前端引导配置）
    match &cli.command {
        Commands::Serve { .. } => {
            config::init_optional();
        }
        Commands::ExtractCookies => {
        }
        _ => {
            config::init()?;
        }
    }

    match cli.command {
        Commands::Like { tid, pid, fid, undo } => {
            let cfg = config::get();
            let client = HupuClient::new(&cfg.cookie)?;
            if undo {
                api::unlight(&client, &tid, &pid, &cfg.puid, &fid, &cfg.shumei_id).await?;
            } else {
                api::like(&client, &tid, &pid, &cfg.puid, &fid, &cfg.shumei_id).await?;
            }
        }
        Commands::Reply { topic_id, tid, content, quote_id } => {
            let cfg = config::get();
            let client = HupuClient::new(&cfg.cookie)?;
            api::reply(&client, &topic_id, &tid, &content, &cfg.shumei_id, quote_id.as_deref()).await?;
        }
        Commands::Mentions { tab, since, limit, pages, format } => {
            let cfg = config::get();
            let client = HupuClient::new(&cfg.cookie)?;
            let plate = mentions::tab_to_plate(&tab);
            let since_ts = match since {
                Some(ref s) => Some(mentions::parse_since(s)?),
                None => None,
            };

            let items = mentions::fetch_mentions_paginated(&client, plate, since_ts, limit, pages).await?;

            match format.as_str() {
                "json" => mentions::format_json(&items)?,
                "simple" => mentions::format_simple(&items),
                _ => mentions::format_table(&items),
            }
        }
        Commands::Search { keyword, page, limit, format, forum, sort } => {
            let cfg = config::get();
            let client = HupuClient::new(&cfg.cookie)?;
            let response = search::search_posts(
                &client,
                &keyword,
                page,
                limit,
                forum.as_deref(),
                sort.as_deref(),
            ).await?;

            match format.as_str() {
                "json" => search::format_search_json(&response)?,
                "simple" => search::format_search_simple(&response),
                _ => search::format_search_table(&response),
            }
        }
        Commands::Serve { port, deploy } => {
            server::start_server(port, deploy).await?;
        }
        Commands::Analyze { euid, threshold, format } => {
            config::get(); // ensure config is initialized for CLI
            let db_path = std::path::Path::new("hupu.db");
            let conn = db::open_db(db_path)?;

            let total = db::count_replies(&conn, Some(&euid))?;
            let all_replies = db::query_replies(&conn, Some(&euid), total as usize, 0)?;

            let groups = analyze::cluster_replies(&all_replies, threshold);

            match format.as_str() {
                "json" => analyze::format_groups_json(&groups)?,
                "table" => analyze::format_groups_table(&groups, all_replies.len()),
                _ => analyze::format_groups_simple(&groups, all_replies.len()),
            }
        }
        Commands::Replies { euid, max_pages, page_size, format } => {
            let cfg = config::get();
            let client = HupuClient::new(&cfg.cookie)?;
            let db_path = std::path::Path::new("hupu.db");
            let conn = db::open_db(db_path)?;

            let result = replies::fetch_replies_paginated(
                &client, &euid, max_pages, page_size, &conn,
            ).await?;

            let stored = db::query_replies(&conn, Some(&euid), result.total_fetched, 0)?;

            match format.as_str() {
                "json" => replies::format_json(&stored)?,
                "simple" => replies::format_simple(&stored),
                _ => replies::format_table(&stored),
            }
        }
        Commands::Posts { euid, max_pages, format } => {
            let cfg = config::get();
            let client = HupuClient::new(&cfg.cookie)?;
            let db_path = std::path::Path::new("hupu.db");
            let conn = db::open_db(db_path)?;

            let result = posts::fetch_posts_paginated(
                &client, &euid, max_pages,
            ).await?;

            let count = result.len();
            db::upsert_posts(&conn, &result)?;
            let stored = db::query_posts(&conn, Some(&euid), count, 0)?;

            match format.as_str() {
                "json" => posts::format_json(&stored)?,
                "simple" => posts::format_simple(&stored),
                _ => posts::format_table(&stored),
            }
        }
        Commands::ExtractCookies => {
            cookie_extract::run().await?;
        }
        Commands::Topic { id, page, limit, format, detail, replies } => {
            let cfg = config::get();
            let client = HupuClient::new(&cfg.cookie)?;
            // 判断是帖子ID（9位数字）还是板块ID
            let is_post_id = id.len() == 9 && id.chars().all(|c| c.is_ascii_digit());

            if is_post_id {
                // 获取帖子详情
                let post_detail = topic::fetch_post_detail(&client, &id).await?;
                let hot_replies = if detail || replies > 0 {
                    Some(topic::fetch_post_replies(&client, &id, replies).await?)
                } else {
                    None
                };

                if format == "json" {
                    let output = serde_json::json!({
                        "detail": post_detail,
                        "replies": hot_replies,
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    topic::format_post_detail(&post_detail, hot_replies.as_deref());
                }
            } else {
                // 获取板块帖子列表
                let posts = topic::fetch_topic_posts(&client, &id, page, limit).await?;

                match format.as_str() {
                    "json" => topic::format_post_json(&posts)?,
                    "simple" => topic::format_post_simple(&posts),
                    _ => topic::format_post_table(&posts),
                }
            }
        }
    }

    Ok(())
}