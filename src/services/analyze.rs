use std::sync::Arc;

use crate::server::types::{AnalyzeQuery, AnalyzeResponse, AppState, ProgressState};

pub async fn run_analysis_background(state: Arc<AppState>, params: AnalyzeQuery) {
    let key = format!("{}:{}", params.euid, params.threshold);

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

    set_progress(&state, "读取数据中", 0, 0, false, None);

    let conn = match crate::db::open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("数据库打开失败: {}", e)));
            return;
        }
    };

    let total = match crate::db::count_replies(&conn, Some(&params.euid)) {
        Ok(t) => t,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("查询失败: {}", e)));
            return;
        }
    };

    let all_replies =
        match crate::db::query_replies(&conn, Some(&params.euid), total as usize, 0) {
            Ok(r) => r,
            Err(e) => {
                set_progress(&state, "error", 0, 0, true, Some(format!("读取数据失败: {}", e)));
                return;
            }
        };

    set_progress(&state, "计算相似度中", 0, 0, false, None);

    let cb_state = state.clone();
    let cb_key = key.clone();
    let threshold = params.threshold;
    let all_replies_clone = all_replies.clone();

    let groups = tokio::task::spawn_blocking(move || {
        let cb: crate::analyze::ProgressFn = Box::new(move |current, total, phase| {
            let _ = cb_state.progress.lock().map(|mut p| {
                p.insert(
                    cb_key.clone(),
                    ProgressState {
                        phase: phase.into(),
                        current,
                        total,
                        done: false,
                        error: None,
                    },
                );
            });
        });
        crate::analyze::cluster_replies_with_progress(&all_replies_clone, threshold, Some(cb))
    })
    .await
    .unwrap_or_default();

    let response = AnalyzeResponse {
        total_replies: total as usize,
        groups,
    };

    if let Ok(mut results) = state.results.lock() {
        results.insert(key.clone(), response.clone());
    }

    if let Ok(conn) = crate::db::open_db(&state.db_path) {
        if let Ok(result_json) = serde_json::to_string(&response) {
            let _ = crate::db::save_similarity_analysis(
                &conn,
                &params.euid,
                params.threshold,
                &result_json,
            );
        }
    }

    set_progress(&state, "完成", 1, 1, true, None);
}
