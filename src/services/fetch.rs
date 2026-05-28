use std::sync::Arc;

use crate::server::types::{AppState, ProgressState};

pub async fn run_fetch_posts_background(state: Arc<AppState>, euid: String, max_pages: u32, cookie_override: Option<String>) {
    let key = format!("fetch_posts:{}", euid);

    let set_progress = |s: &AppState, phase: &str, current: usize, total: usize, done: bool, error: Option<String>| {
        if let Ok(mut p) = s.progress.lock() {
            p.insert(
                key.clone(),
                ProgressState {
                    phase: phase.into(),
                    current,
                    total,
                    done,
                    error,
                },
            );
        }
    };

    let total_hint = if max_pages > 0 { max_pages as usize } else { 0 };
    set_progress(&state, "准备中", 0, total_hint, false, None);

    let cookie_str = match cookie_override
        .filter(|c| !c.is_empty())
        .or_else(|| crate::config::try_get().and_then(|c| if !c.cookie.is_empty() { Some(c.cookie) } else { None }))
    {
        Some(c) => c,
        None => {
            set_progress(&state, "error", 0, 0, true, Some("请先配置 Cookie（在 config.json 中或页面上填写）".into()));
            return;
        }
    };
    let client = match crate::client::HupuClient::new(&cookie_str) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("创建客户端失败: {}", e)));
            return;
        }
    };

    let mut total_fetched = 0usize;

    let mut page = 1u32;
    loop {
        let phase = if max_pages > 0 {
            format!("获取第 {} / {} 页", page, max_pages)
        } else {
            format!("获取第 {} 页", page)
        };
        let cur = if max_pages > 0 { page as usize } else { total_fetched };
        let tot = if max_pages > 0 { max_pages as usize } else { 0 };
        set_progress(&state, &phase, cur, tot, false, None);

        let posts = match crate::posts::fetch_posts_page(&client, &euid, page).await {
            Ok(p) => p,
            Err(e) => {
                set_progress(
                    &state,
                    "error",
                    cur,
                    tot,
                    true,
                    Some(format!("获取第{}页失败: {}", page, e)),
                );
                return;
            }
        };

        let count = posts.len();
        total_fetched += count;

        if !posts.is_empty() {
            match crate::db::open_db(&state.db_path) {
                Ok(conn) => {
                    if let Err(e) = crate::db::upsert_posts(&conn, &posts) {
                        set_progress(
                            &state,
                            "error",
                            total_fetched,
                            tot,
                            true,
                            Some(format!("数据库写入失败: {}", e)),
                        );
                        return;
                    }
                }
                Err(e) => {
                    set_progress(
                        &state,
                        "error",
                        total_fetched,
                        tot,
                        true,
                        Some(format!("数据库打开失败: {}", e)),
                    );
                    return;
                }
            }
        }

        if (count as u32) < crate::posts::PAGE_SIZE || (max_pages > 0 && page >= max_pages) {
            break;
        }
        page += 1;
    }

    set_progress(&state, "完成", total_fetched, total_fetched, true, None);
}

pub async fn run_fetch_replies_background(
    state: Arc<AppState>,
    euid: String,
    max_pages: u32,
    page_size: u32,
    cookie_override: Option<String>,
) {
    let key = format!("fetch_replies:{}", euid);

    let set_progress = |s: &AppState, phase: &str, current: usize, total: usize, done: bool, error: Option<String>| {
        if let Ok(mut p) = s.progress.lock() {
            p.insert(
                key.clone(),
                ProgressState {
                    phase: phase.into(),
                    current,
                    total,
                    done,
                    error,
                },
            );
        }
    };

    let total_hint = if max_pages > 0 { max_pages as usize } else { 0 };
    set_progress(&state, "准备中", 0, total_hint, false, None);

    let cookie_str = match cookie_override
        .filter(|c| !c.is_empty())
        .or_else(|| crate::config::try_get().and_then(|c| if !c.cookie.is_empty() { Some(c.cookie) } else { None }))
    {
        Some(c) => c,
        None => {
            set_progress(&state, "error", 0, 0, true, Some("请先配置 Cookie（在 config.json 中或页面上填写）".into()));
            return;
        }
    };
    let client = match crate::client::HupuClient::new(&cookie_str) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("创建客户端失败: {}", e)));
            return;
        }
    };

    let mut all_items = Vec::new();
    let mut total_fetched = 0usize;
    let now_ts = chrono::Local::now().timestamp();
    let mut max_time: Option<i64> = Some(now_ts);

    let mut page = 1u32;
    loop {
        let phase = if max_pages > 0 {
            format!("获取第 {} / {} 页", page, max_pages)
        } else {
            format!("获取第 {} 页", page)
        };
        let cur = if max_pages > 0 { page as usize } else { total_fetched };
        let tot = if max_pages > 0 { max_pages as usize } else { 0 };
        set_progress(
            &state,
            &phase,
            cur,
            tot,
            false,
            None,
        );

        match crate::replies::fetch_replies(&client, &euid, max_time, page, page_size).await {
            Ok(result) => {
                let count = result.items.len();
                total_fetched += count;
                all_items.extend(result.items);

                if !result.has_next_page || (max_pages > 0 && page >= max_pages) {
                    break;
                }
                max_time = result.max_time;
            }
            Err(e) => {
                set_progress(
                    &state,
                    "error",
                    cur,
                    tot,
                    true,
                    Some(format!("获取第{}页失败: {}", page, e)),
                );
                return;
            }
        }
        page += 1;
    }

    set_progress(&state, "写入数据库", total_fetched, total_fetched, false, None);

    let conn = match crate::db::open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("数据库打开失败: {}", e)));
            return;
        }
    };

    if let Err(e) = crate::db::upsert_replies(&conn, &all_items) {
        set_progress(
            &state,
            "error",
            0,
            0,
            true,
            Some(format!("数据库写入失败: {}", e)),
        );
        return;
    }

    set_progress(&state, "完成", total_fetched, total_fetched, true, None);
}
