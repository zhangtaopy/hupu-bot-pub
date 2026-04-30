use std::collections::HashMap;
use std::collections::HashSet;

use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use chrono::{Datelike, Timelike};
use crate::replies::ReplyRow;
use crate::utils::strip_html;
static PUNCT_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^\w\s]").unwrap());

// ── Data structures ──

#[derive(Debug, Clone)]
pub enum GroupSimilarity {
    Exact,
    Similar(f64),
}

impl serde::Serialize for GroupSimilarity {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            GroupSimilarity::Exact => serializer.serialize_str("exact"),
            GroupSimilarity::Similar(v) => serializer.serialize_f64(*v),
        }
    }
}

impl<'de> serde::Deserialize<'de> for GroupSimilarity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        // Try f64 first, fall back to string
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Helper {
            Num(f64),
            Str(String),
        }
        match Helper::deserialize(deserializer)? {
            Helper::Str(s) if s == "exact" => Ok(GroupSimilarity::Exact),
            Helper::Num(v) => Ok(GroupSimilarity::Similar(v)),
            Helper::Str(s) => Err(D::Error::custom(format!("invalid similarity value: {}", s))),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ReplyGroup {
    pub group_id: usize,
    pub count: usize,
    pub similarity: GroupSimilarity,
    pub representative: String,
    pub replies: Vec<ReplyGroupItem>,
    pub topic_distribution: HashMap<String, usize>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ReplyGroupItem {
    pub pid: i64,
    pub tid: i64,
    pub content: String,
    pub title: String,
    pub topic_name: Option<String>,
    pub create_time: i64,
    pub light_count: i64,
    pub format_time: String,
    pub reply_url: String,
}

// ── Text processing ──

fn normalize(s: &str) -> String {
    let s = strip_html(s);
    let s = s.to_lowercase();
    let s = PUNCT_REGEX.replace_all(&s, "");
    s.split_whitespace().collect::<Vec<_>>().join("")
}

/// Tokenize: Chinese by overlapping bigram, English by word
fn tokenize(s: &str) -> HashSet<String> {
    let mut tokens = HashSet::new();
    let mut ascii_buf = String::new();
    let chars: Vec<char> = s.chars().collect();

    // Process ASCII words
    for &ch in &chars {
        if ch.is_ascii_alphanumeric() {
            ascii_buf.push(ch.to_ascii_lowercase());
        } else {
            if !ascii_buf.is_empty() {
                tokens.insert(ascii_buf.clone());
                ascii_buf.clear();
            }
        }
    }
    if !ascii_buf.is_empty() {
        tokens.insert(ascii_buf);
    }

    // Chinese overlapping bigrams (2-char sliding window)
    for pair in chars.windows(2) {
        if !pair[0].is_ascii() && !pair[1].is_ascii() {
            tokens.insert(pair.iter().collect::<String>());
        }
    }

    tokens
}

fn jaccard_similarity(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let intersection = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union == 0.0 {
        return 0.0;
    }
    intersection / union
}

// ── Union-Find ──

struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }

    fn union(&mut self, x: usize, y: usize) {
        let rx = self.find(x);
        let ry = self.find(y);
        if rx == ry {
            return;
        }
        if self.rank[rx] < self.rank[ry] {
            self.parent[rx] = ry;
        } else if self.rank[rx] > self.rank[ry] {
            self.parent[ry] = rx;
        } else {
            self.parent[ry] = rx;
            self.rank[rx] += 1;
        }
    }
}

// ── Clustering ──

/// Progress callback: current_step, total_steps, phase_name
pub type ProgressFn = Box<dyn Fn(usize, usize, &str) + Send>;

/// Convenience wrapper with no progress callback
pub fn cluster_replies(replies: &[ReplyRow], threshold: f64) -> Vec<ReplyGroup> {
    cluster_replies_with_progress(replies, threshold, None)
}

pub fn cluster_replies_with_progress(
    replies: &[ReplyRow],
    threshold: f64,
    progress: Option<ProgressFn>,
) -> Vec<ReplyGroup> {
    let n = replies.len();
    if n == 0 {
        return vec![];
    }

    let report = |current: usize, total: usize, phase: &str| {
        if let Some(ref cb) = progress {
            cb(current, total, phase);
        }
    };

    // Step 1: exact match grouping via HashMap
    let mut exact_map: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, r) in replies.iter().enumerate() {
        let key = normalize(&r.content);
        if key.is_empty() {
            continue;
        }
        exact_map.entry(key).or_default().push(i);
    }

    report(0, n, "预处理精确匹配");

    let mut uf = UnionFind::new(n);

    // Merge exact matches
    for indices in exact_map.values() {
        for w in indices.windows(2) {
            uf.union(w[0], w[1]);
        }
    }

    // Step 2: Jaccard similarity for similar (but not exact) matches
    // Pre-compute token sets for all replies
    let token_sets: Vec<HashSet<String>> = replies
        .iter()
        .map(|r| tokenize(&normalize(&r.content)))
        .collect();

    report(0, n, "计算相似度中");

    // Only compare pairs not already in the same group.
    // Total pairs = n*(n-1)/2, update progress every ~1%
    let total_pairs = n.saturating_sub(1) * n / 2;
    let progress_interval = std::cmp::max(1, total_pairs / 100);
    let mut pair_count = 0usize;

    for i in 0..n {
        for j in (i + 1)..n {
            pair_count += 1;
            if pair_count % progress_interval == 0 {
                report(pair_count.min(total_pairs), total_pairs, "计算相似度中");
            }
            if uf.find(i) == uf.find(j) {
                continue;
            }
            let sim = jaccard_similarity(&token_sets[i], &token_sets[j]);
            if sim >= threshold {
                uf.union(i, j);
            }
        }
    }

    report(total_pairs, total_pairs, "整理分组结果");

    // Step 3: collect groups from Union-Find
    let mut group_map: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut uf_tmp = uf;
    for i in 0..n {
        let root = uf_tmp.find(i);
        group_map.entry(root).or_default().push(i);
    }

    // Step 4: centroid-based validation to prevent transitive chaining
    // For each non-exact group, pick the element with most in-group connections
    // at or above threshold, then only keep elements similar to that centroid.
    let mut validated_groups: Vec<Vec<usize>> = Vec::new();
    for (_, indices) in group_map {
        if indices.len() < 2 {
            continue;
        }

        // Check if exact match
        let first_key = normalize(&replies[indices[0]].content);
        let is_exact = indices.iter().all(|&i| normalize(&replies[i].content) == first_key);

        if is_exact {
            validated_groups.push(indices);
            continue;
        }

        // For non-exact: compute in-group degree (how many members each element
        // has ≥threshold similarity with)
        let mut degrees: Vec<usize> = vec![0; indices.len()];
        for a in 0..indices.len() {
            for b in (a + 1)..indices.len() {
                let sim = jaccard_similarity(&token_sets[indices[a]], &token_sets[indices[b]]);
                if sim >= threshold {
                    degrees[a] += 1;
                    degrees[b] += 1;
                }
            }
        }

        // Pick centroid (element with highest degree)
        let centroid_idx = degrees
            .iter()
            .enumerate()
            .max_by_key(|&(_, d)| d)
            .map(|(i, _)| i)
            .unwrap_or(0);

        // Only keep elements with ≥threshold similarity to centroid
        let mut core: Vec<usize> = indices
            .iter()
            .enumerate()
            .filter(|&(i, _)| {
                i == centroid_idx || {
                    let sim = jaccard_similarity(
                        &token_sets[indices[centroid_idx]],
                        &token_sets[indices[i]],
                    );
                    sim >= threshold
                }
            })
            .map(|(_, &idx)| idx)
            .collect();

        if core.len() >= 2 {
            // Sort by original position for stability
            core.sort();
            validated_groups.push(core);
        }
    }

    // Determine group similarity type
    let mut groups: Vec<ReplyGroup> = Vec::new();
    let mut group_id = 0;
    for indices in &validated_groups {
        // Check if group is exact match or similar
        let is_exact = {
            let first_key = normalize(&replies[indices[0]].content);
            indices.iter().all(|&i| normalize(&replies[i].content) == first_key)
        };

        let similarity = if is_exact {
            GroupSimilarity::Exact
        } else {
            // Find centroid (element with most in-group connections at threshold)
            let mut deg: Vec<usize> = vec![0; indices.len()];
            for a in 0..indices.len() {
                for b in (a + 1)..indices.len() {
                    let sim = jaccard_similarity(&token_sets[indices[a]], &token_sets[indices[b]]);
                    if sim >= threshold {
                        deg[a] += 1;
                        deg[b] += 1;
                    }
                }
            }
            let centroid = deg
                .iter()
                .enumerate()
                .max_by_key(|&(_, d)| d)
                .map(|(i, _)| i)
                .unwrap_or(0);

            let min_sim = indices
                .iter()
                .enumerate()
                .filter(|&(i, _)| i != centroid)
                .map(|(_, &j)| jaccard_similarity(&token_sets[indices[centroid]], &token_sets[j]))
                .fold(1.0_f64, f64::min);
            if min_sim >= 0.99 {
                GroupSimilarity::Exact
            } else {
                GroupSimilarity::Similar(min_sim)
            }
        };

        let group_items: Vec<ReplyGroupItem> = indices
            .iter()
            .map(|&i| {
                let r = &replies[i];
                ReplyGroupItem {
                    pid: r.pid,
                    tid: r.tid,
                    content: strip_html(&r.content),
                    title: r.title.clone(),
                    topic_name: r.topic_name.clone(),
                    create_time: r.create_time,
                    light_count: r.light_count,
                    format_time: format_time(r),
                    reply_url: format!("https://bbs.hupu.com/{}.html?pid={}", r.tid, r.pid),
                }
            })
            .collect();

        let dist: HashMap<String, usize> = topic_distribution(&indices.iter().map(|&i| &replies[i]).collect::<Vec<_>>());
        let representative = strip_html(&replies[indices[0]].content);

        group_id += 1;
        groups.push(ReplyGroup {
            group_id,
            count: indices.len(),
            representative,
            replies: group_items,
            similarity,
            topic_distribution: dist,
        });
    }

    // Sort: exact groups first, then by similarity descending
    groups.sort_by(|a, b| {
        let a_val = match &a.similarity {
            GroupSimilarity::Exact => 1.0,
            GroupSimilarity::Similar(v) => *v,
        };
        let b_val = match &b.similarity {
            GroupSimilarity::Exact => 1.0,
            GroupSimilarity::Similar(v) => *v,
        };
        b_val.partial_cmp(&a_val).unwrap_or(std::cmp::Ordering::Equal)
    });

    // Re-assign group_id after sorting
    for (i, g) in groups.iter_mut().enumerate() {
        g.group_id = i + 1;
    }

    groups
}

// ── Format ──

fn format_time(row: &ReplyRow) -> String {
    if let Some(ft) = &row.format_time {
        if !ft.is_empty() {
            return ft.clone();
        }
    }
    chrono::DateTime::from_timestamp(row.create_time, 0)
        .map(|dt| dt.format("%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn topic_distribution<'a>(replies: &[&'a ReplyRow]) -> HashMap<String, usize> {
    let mut dist: HashMap<String, usize> = HashMap::new();
    for r in replies {
        if let Some(tn) = &r.topic_name {
            *dist.entry(tn.clone()).or_insert(0) += 1;
        }
    }
    dist
}

pub fn format_groups_simple(groups: &[ReplyGroup], total_replies: usize) {
    if groups.is_empty() {
        println!("没有发现重复或相似的回帖");
        return;
    }

    let grouped_count: usize = groups.iter().map(|g| g.count).sum();

    for g in groups {
        let sim_label = match &g.similarity {
            GroupSimilarity::Exact => "完全相同".to_string(),
            GroupSimilarity::Similar(v) => format!("{:.0}%", v * 100.0),
        };
        println!(
            "相似回帖组 #{} (出现 {} 次, 相似度: {}):",
            g.group_id, g.count, sim_label
        );
        for r in &g.replies {
            println!("  [{}] {} (pid:{})", r.format_time, r.content, r.pid);
        }
        let dist_str: Vec<String> = g.topic_distribution
            .iter()
            .map(|(k, v)| format!("{} x{}", k, v))
            .collect();
        println!("  板块分布: {}", dist_str.join(", "));
        println!();
    }

    let singleton_count = total_replies - grouped_count;
    if singleton_count > 0 {
        println!("独立回帖: 共 {} 条（仅出现1次）", singleton_count);
    }
    println!(
        "统计: 共 {} 条回帖, {} 个相似组, {} 条独立",
        total_replies, groups.len(), singleton_count
    );
}

pub fn format_groups_json(groups: &[ReplyGroup]) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&groups)?);
    Ok(())
}

pub fn format_groups_table(groups: &[ReplyGroup], total_replies: usize) {
    if groups.is_empty() {
        println!("没有发现重复或相似的回帖");
        return;
    }

    let grouped_count: usize = groups.iter().map(|g| g.count).sum();

    let w_id = 4;
    let w_count = 6;
    let w_sim = 8;
    let w_content = 40;
    let w_topics = 20;

    let top_border = format!(
        "┌{}┬{}┬{}┬{}┬{}┐",
        "─".repeat(w_id),
        "─".repeat(w_count),
        "─".repeat(w_sim),
        "─".repeat(w_content),
        "─".repeat(w_topics)
    );
    let header = format!(
        "│ {:2} │ {:4} │ {:6} │ {:width1$} │ {:width2$} │",
        "#",
        "次数",
        "相似度",
        "代表性内容",
        "板块分布",
        width1 = w_content - 2,
        width2 = w_topics - 2,
    );
    let mid_border = format!(
        "├{}┼{}┼{}┼{}┼{}┤",
        "─".repeat(w_id),
        "─".repeat(w_count),
        "─".repeat(w_sim),
        "─".repeat(w_content),
        "─".repeat(w_topics)
    );
    let bottom_border = format!(
        "└{}┴{}┴{}┴{}┴{}┘",
        "─".repeat(w_id),
        "─".repeat(w_count),
        "─".repeat(w_sim),
        "─".repeat(w_content),
        "─".repeat(w_topics)
    );

    println!("{}", top_border);
    println!("{}", header);
    println!("{}", mid_border);

    for g in groups {
        let sim_label = match &g.similarity {
            GroupSimilarity::Exact => "相同".to_string(),
            GroupSimilarity::Similar(v) => format!("{:.0}%", v * 100.0),
        };
        let content = truncate(&g.representative, w_content - 2);
        let topics_str = truncate(
            &g.topic_distribution
                .iter()
                .map(|(k, v)| format!("{}x{}", k, v))
                .collect::<Vec<_>>()
                .join(" "),
            w_topics - 2,
        );

        println!(
            "│ {:2} │ {:4} │ {:6} │ {:width1$} │ {:width2$} │",
            g.group_id,
            g.count,
            sim_label,
            content,
            topics_str,
            width1 = w_content - 2,
            width2 = w_topics - 2,
        );
    }

    println!("{}", bottom_border);
    let singleton_count = total_replies - grouped_count;
    println!(
        "统计: 共 {} 条, {} 个相似组, {} 条独立",
        total_replies, groups.len(), singleton_count
    );
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

// ── Word Cloud (jieba-based) ──

static STOP_WORDS: &[&str] = &[
    "的", "了", "在", "是", "我", "有", "和", "就", "不", "人", "都", "一", "一个",
    "上", "也", "很", "到", "说", "要", "去", "你", "会", "着", "没有", "看", "好",
    "自己", "这", "他", "她", "它", "们", "那", "这个", "那个", "什么", "怎么", "因为",
    "所以", "但是", "如果", "虽然", "而且", "或者", "还是", "已经", "可以", "时候",
    "因为", "所以", "但是", "然后", "就是", "不是", "不会", "不能", "不要", "没",
    "吧", "吗", "啊", "呢", "嗯", "哈", "呀", "嘛", "哦", "啦", "呵", "哇",
    "之", "与", "而", "及", "或", "被", "把", "对", "从", "以", "为",
    "中", "将", "能", "做", "让", "用", "其", "最", "更", "多", "少",
    "大", "小", "新", "老", "真", "太", "很", "非常", "比较", "相当",
    "还", "也", "又", "再", "才", "刚", "已", "曾", "都", "只",
    "啊", "哈", "啦", "哟", "噢", "嗯", "哦", "呀", "吧",
    "来", "去", "上", "下", "前", "后", "里", "外", "这", "那",
    "你", "我", "他", "她", "它", "们", "大家", "自己", "什么",
    "如何", "为何", "哪个", "哪些", "谁", "哪", "怎么", "怎样",
    "没", "无", "未", "别", "勿", "看", "让", "叫",
    "年", "月", "日", "时", "分", "秒", "天", "周",
    "块", "个", "只", "条", "种", "次", "下", "点", "些", "张",
    "位", "篇", "件", "台", "辆", "把", "面", "口", "份",
    "一", "二", "三", "四", "五", "六", "七", "八", "九", "十",
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
];

#[derive(Serialize, Clone)]
pub struct WordCloudItem {
    pub text: String,
    pub count: usize,
}

pub fn word_frequency(replies: &[ReplyRow]) -> Vec<WordCloudItem> {
    use jieba_rs::Jieba;
    use std::collections::HashMap;

    let jieba = Jieba::new();
    let mut freq: HashMap<String, usize> = HashMap::new();

    for reply in replies {
        let clean = strip_html(&reply.content);
        let words = jieba.cut(&clean, true);
        for w in words {
            let w = w.trim();
            if w.chars().count() < 2
                || STOP_WORDS.contains(&w)
                || !w.chars().any(|c| c.is_alphabetic())
            {
                continue;
            }
            *freq.entry(w.to_string()).or_insert(0) += 1;
        }
    }

    let mut words: Vec<WordCloudItem> = freq
        .into_iter()
        .map(|(text, count)| WordCloudItem { text, count })
        .collect();
    words.sort_by(|a, b| b.count.cmp(&a.count));
    words.truncate(80);
    words
}

// ── Detailed Analysis ──

#[derive(Serialize)]
pub struct DetailedAnalysis {
    pub length_stats: LengthStats,
    pub hour_distribution: Vec<usize>,
    pub weekday_distribution: Vec<usize>,
    pub light_stats: LightStats,
    pub top_replied_posts: Vec<TopPost>,
    pub quote_rate: f64,
    pub total_quotes: usize,
    pub reply_length_buckets: Vec<usize>,
}

#[derive(Serialize)]
pub struct LengthStats {
    pub avg: f64,
    pub min: usize,
    pub max: usize,
    pub median: usize,
}

#[derive(Serialize)]
pub struct LightStats {
    pub total_lights: i64,
    pub avg_lights: f64,
    pub max_lights: i64,
    pub replied_count: i64,
}

#[derive(Serialize)]
pub struct TopPost {
    pub tid: i64,
    pub title: String,
    pub count: usize,
}

pub fn detailed_analysis(replies: &[ReplyRow]) -> DetailedAnalysis {
    use std::collections::HashMap;
    let n = replies.len();
    if n == 0 {
        return DetailedAnalysis {
            length_stats: LengthStats { avg: 0.0, min: 0, max: 0, median: 0 },
            hour_distribution: vec![0; 24],
            weekday_distribution: vec![0; 7],
            light_stats: LightStats { total_lights: 0, avg_lights: 0.0, max_lights: 0, replied_count: 0 },
            top_replied_posts: vec![],
            quote_rate: 0.0,
            total_quotes: 0,
            reply_length_buckets: vec![0; 5],
        };
    }

    // Length stats
    let mut lengths: Vec<usize> = replies
        .iter()
        .map(|r| strip_html(&r.content).chars().count())
        .collect();
    lengths.sort();
    let total_len: usize = lengths.iter().sum();
    let avg = total_len as f64 / n as f64;
    let min = lengths[0];
    let max = lengths[n - 1];
    let median = lengths[n / 2];

    // Length buckets: 0-10, 11-50, 51-100, 101-200, 200+
    let mut buckets = vec![0usize; 5];
    for &l in &lengths {
        if l <= 10 { buckets[0] += 1; }
        else if l <= 50 { buckets[1] += 1; }
        else if l <= 100 { buckets[2] += 1; }
        else if l <= 200 { buckets[3] += 1; }
        else { buckets[4] += 1; }
    }

    // Time distribution
    let mut hours = vec![0usize; 24];
    let mut weekdays = vec![0usize; 7];
    for r in replies {
        if let Some(dt) = chrono::DateTime::from_timestamp(r.create_time, 0) {
            hours[dt.hour() as usize] += 1;
            // chrono::Weekday: Mon=0, Sun=6; we want Mon=1, Sun=7
            let wd = dt.weekday().num_days_from_monday() as usize;
            weekdays[wd] += 1;
        }
    }

    // Light stats
    let total_lights: i64 = replies.iter().map(|r| r.light_count).sum();
    let avg_lights = total_lights as f64 / n as f64;
    let max_lights = replies.iter().map(|r| r.light_count).max().unwrap_or(0);
    let replied_count = replies.iter().filter(|r| r.light_count > 0).count() as i64;

    // Top replied posts
    let mut post_count: HashMap<i64, (String, usize)> = HashMap::new();
    for r in replies {
        let entry = post_count.entry(r.tid).or_insert_with(|| (r.title.clone(), 0));
        entry.1 += 1;
    }
    let mut top_posts: Vec<TopPost> = post_count
        .into_iter()
        .map(|(tid, (title, count))| TopPost { tid, title, count })
        .collect();
    top_posts.sort_by(|a, b| b.count.cmp(&a.count));
    top_posts.truncate(10);

    // Quote rate
    let total_quotes = replies.iter().filter(|r| r.quote > 0).count();
    let quote_rate = total_quotes as f64 / n as f64;

    DetailedAnalysis {
        length_stats: LengthStats { avg, min, max, median },
        hour_distribution: hours,
        weekday_distribution: weekdays,
        light_stats: LightStats { total_lights, avg_lights, max_lights, replied_count },
        top_replied_posts: top_posts,
        quote_rate,
        total_quotes,
        reply_length_buckets: buckets,
    }
}