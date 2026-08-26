use std::sync::Arc;

use crate::server::types::{AppState, ProgressState};

pub async fn run_fetch_posts_background(
    state: Arc<AppState>,
    euid: String,
    max_pages: u32,
    cookie_override: Option<String>,
    incremental: bool,
) {
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
    set_progress(&state, if incremental { "增量模式：只获取新发帖" } else { "准备中" }, 0, total_hint, false, None);

    let client = match crate::resolver::create_hupu_client(cookie_override.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(e));
            return;
        }
    };

    let mut total_fetched = 0usize;

    let mut page = 1u32;
    loop {
        let phase = if incremental {
            format!("增量获取发帖: 第 {} 页", page)
        } else if max_pages > 0 {
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
        if count == 0 {
            break;
        }

        if incremental {
            // 增量模式：整页 tid 都已存在 → 后续页只会更旧 → 停止
            let tids: Vec<i64> = posts.iter().map(|p| p.tid).collect();
            let new_count = match crate::db::open_db(&state.db_path) {
                Ok(conn) => {
                    let existing = match crate::db::existing_post_tids(&conn, &tids) {
                        Ok(e) => e,
                        Err(e) => {
                            set_progress(&state, "error", cur, tot, true, Some(format!("数据库查询失败: {}", e)));
                            return;
                        }
                    };
                    tids.iter().filter(|t| !existing.contains(t)).count()
                }
                Err(e) => {
                    set_progress(&state, "error", 0, 0, true, Some(format!("数据库打开失败: {}", e)));
                    return;
                }
            };

            if new_count == 0 {
                set_progress(&state, &format!("完成（增量：第{}页起全部已存在，共{}条新发帖）", page, total_fetched), total_fetched, total_fetched, true, None);
                return;
            }

            if let Ok(conn) = crate::db::open_db(&state.db_path) {
                if let Err(e) = crate::db::upsert_posts(&conn, &posts) {
                    set_progress(&state, "error", total_fetched, tot, true, Some(format!("数据库写入失败: {}", e)));
                    return;
                }
            }
            total_fetched += new_count;
        } else {
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
        }
        page += 1;
        if max_pages > 0 && page > max_pages {
            break;
        }
    }

    set_progress(&state, "完成", total_fetched, total_fetched, true, None);
}

pub async fn run_fetch_replies_background(
    state: Arc<AppState>,
    euid: String,
    max_pages: u32,
    page_size: u32,
    cookie_override: Option<String>,
    incremental: bool,
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
    set_progress(&state, if incremental { "增量模式：只获取新回帖" } else { "准备中" }, 0, total_hint, false, None);

    let client = match crate::resolver::create_hupu_client(cookie_override.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(e));
            return;
        }
    };

    let mut all_items = Vec::new();
    let mut total_fetched = 0usize;
    let mut total_new = 0usize;
    let now_ts = chrono::Local::now().timestamp();
    let mut max_time: Option<i64> = Some(now_ts);

    let mut page = 1u32;
    loop {
        let phase = if incremental {
            format!("增量获取回帖: 第 {} 页", page)
        } else if max_pages > 0 {
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
                if count == 0 {
                    break;
                }

                if incremental {
                    // 增量模式：整页 pid 都已存在 → 后续页只会更旧 → 停止
                    let pids: Vec<i64> = result.items.iter().map(|r| r.pid).collect();
                    let existing = match crate::db::open_db(&state.db_path) {
                        Ok(conn) => match crate::db::existing_reply_pids(&conn, &pids) {
                            Ok(e) => e,
                            Err(e) => {
                                set_progress(&state, "error", cur, tot, true, Some(format!("数据库查询失败: {}", e)));
                                return;
                            }
                        },
                        Err(e) => {
                            set_progress(&state, "error", 0, 0, true, Some(format!("数据库打开失败: {}", e)));
                            return;
                        }
                    };
                    let new_count = pids.iter().filter(|p| !existing.contains(p)).count();

                    if new_count == 0 {
                        set_progress(
                            &state,
                            &format!("完成（增量：第{}页起全部已存在，共新增{}条回帖）", page, total_new),
                            total_new,
                            total_new,
                            true,
                            None,
                        );
                        return;
                    }

                    if let Ok(conn) = crate::db::open_db(&state.db_path) {
                        if let Err(e) = crate::db::upsert_replies(&conn, &result.items) {
                            set_progress(&state, "error", cur, tot, true, Some(format!("数据库写入失败: {}", e)));
                            return;
                        }
                    }
                    total_new += new_count;
                } else {
                    total_fetched += count;
                    all_items.extend(result.items);
                }

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

    // 非增量：最后统一写入
    if !incremental {
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
    }

    set_progress(&state, "完成", total_fetched, total_fetched, true, None);
}
