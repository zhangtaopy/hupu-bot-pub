use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::posts::PostRow;
use crate::replies::ReplyRow;
use crate::utils::strip_html;

const CHUNK_CHARS: usize = 15_000;
const MODEL: &str = "deepseek-v4-flash";

/// Truncate `s` to at most `max_chars` characters at a valid UTF-8 boundary.
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((pos, _)) => &s[..pos],
        None => s,
    }
}

// ── Prompt templates ──

const BATCH_SYSTEM_PROMPT: &str = r#"你是一个虎扑论坛用户分析专家。分析以下用户的回帖和主题帖（发帖）内容，提取其观点倾向、关注话题、情感立场和个人信息线索。

要求：
1. 从回帖和发帖中识别出用户反复表达的观点（viewpoint_tendencies）
2. 归纳用户关注的主要话题领域（topics_of_interest）
3. 判断用户对各话题的情感倾向（emotional_stances），包括正面/负面/中立
4. 提取用户反复使用的关键论点或论证模式（key_arguments）
5. 从回帖和发帖中挖掘用户的个人信息线索（personal_info_clues），包括：年龄段、性别、身高体重、感情状况、籍贯/老家、现居城市、教育背景（学校/学历/专业）、留学经历、职业行业、收入水平、车辆、房产、支持的主队、兴趣爱好、玩的游戏、性格特征、政治立场倾向，以及其他任何有价值的个人线索。每条线索标注是从哪条回帖或发帖中推断的。

输出严格JSON格式，不要包含其他文字。"#;

const SYNTHESIS_SYSTEM_PROMPT: &str = r#"你是一个虎扑论坛用户画像专家。以下是该用户多批回帖的AI分析结果，请综合所有信息，为用户生成完整画像。

请严格按照以下JSON结构输出，不要包含其他文字：

{
  "user_portrait": {
    "overall_impression": "简练的整体印象标签（不超过20字）",
    "main_interests": ["关注领域1", "关注领域2"],
    "stance_profile": [
      { "topic": "话题", "stance": "positive/negative/neutral", "intensity": 0.8 }
    ]
  },
  "viewpoint_summary": {
    "core_viewpoints": ["核心观点1", "核心观点2"],
    "controversial_topics": ["争议话题1"],
    "unique_perspectives": ["独特视角1"]
  },
  "behavioral_patterns": {
    "reply_style": "回帖风格（如：理性讨论型/情绪表达型/幽默调侃型/专业知识型）",
    "interaction_preference": "互动偏好",
    "depth_level": "讨论深度层次"
  },
  "personal_info": {
    "age_range": "从发言推断出的年龄段，如25-30岁，无法推断则留空字符串",
    "gender": "性别，如男/女，无法推断则留空字符串",
    "height_weight": "身高体重线索，如180cm/75kg，无法推断则留空字符串",
    "relationship": "感情状况，如单身/已婚/有女友，无法推断则留空字符串",
    "hometown": "籍贯/老家，如江西南昌，无法推断则留空字符串",
    "current_city": "现居城市，如上海浦东，无法推断则留空字符串",
    "education": "教育背景（学校+学历+专业），如浙江大学本科计算机，无法推断则留空字符串",
    "study_abroad": "留学经历，如英国曼彻斯特大学硕士，无法推断则留空字符串",
    "profession": "行业+职业，如互联网/后端开发，无法推断则留空字符串",
    "income_hint": "收入线索，如年薪30w左右，无法推断则留空字符串",
    "car": "车辆信息，如特斯拉Model 3，无法推断则留空字符串",
    "housing": "房产状况，如已买房/租房/和父母住，无法推断则留空字符串",
    "sports_teams": ["支持的主队1（从发言中推断）", "支持的主队2"],
    "hobbies": ["兴趣爱好1（从发言中推断）", "兴趣爱好2"],
    "games": ["玩的游戏1（从发言中推断）", "玩的游戏2"],
    "personality_traits": "性格特征描述，无法推断则留空字符串",
    "political_stance": "政治立场倾向描述，无法推断则留空字符串",
    "other_clues": ["其他有价值的个人线索1", "其他有价值的个人线索2"],
    "confidence_note": "简短说明推断依据和置信度，如'大部分信息来自用户自述，年龄为推测'"
  },
  "summary": "200-300字的综合评语,评论可以激烈一点，可以不留情面，但要基于分析结果，不能无中生有。"
}"#;

// ── Post analysis prompts ──

const POST_BATCH_SYSTEM_PROMPT: &str = r#"你是一个虎扑论坛用户发帖分析专家。分析以下用户的发帖内容，提取其发帖风格、关注话题和内容特点。

要求：
1. 从发帖中识别用户关注的领域和话题（topics_of_interest）
2. 分析用户的内容创作类型（图片、视频、文字等）
3. 判断用户的发帖情感倾向和观点立场
4. 评估帖子质量、互动表现和受众反馈

输出严格JSON格式，不要包含其他文字。"#;

const POST_SYNTHESIS_SYSTEM_PROMPT: &str = r#"你是一个虎扑论坛用户画像专家。以下是该用户多批发帖的AI分析结果，请综合所有信息，为用户生成完整发帖画像。

请严格按照以下JSON结构输出，不要包含其他文字：

{
  "user_portrait": {
    "overall_impression": "简练的整体印象标签（不超过20字）",
    "main_interests": ["关注领域1", "关注领域2"],
    "content_style": "内容创作风格描述"
  },
  "post_analysis": {
    "topic_focus": ["主要发帖话题1", "主要发帖话题2"],
    "content_types": {
      "text_only": "纯文字帖比例描述",
      "image_posts": "图文帖比例描述",
      "video_posts": "视频帖比例描述"
    },
    "quality_assessment": "内容质量评估"
  },
  "viewpoint_summary": {
    "core_viewpoints": ["核心观点1", "核心观点2"],
    "controversial_topics": ["争议话题1"],
    "unique_perspectives": ["独特视角1"]
  },
  "engagement_analysis": {
    "average_replies": "平均回复互动量评估",
    "audience_response": "受众反馈特点",
    "most_engaging_content": "最受欢迎的帖子类型"
  },
  "summary": "200-300字的综合评语，分析用户的发帖特点、内容质量和影响力。"
}"#;

// ── Data structures for API communication ──

#[derive(Serialize)]
struct DeepSeekRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
    #[serde(rename = "response_format")]
    response_format: ResponseFormat,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    type_: String,
}

#[derive(Deserialize)]
struct DeepSeekResponse {
    choices: Vec<Choice>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
    #[serde(rename = "type")]
    type_: Option<String>,
}

// ── Public data structures for analysis results ──

/// Deserialize a `Vec<String>` that might be a single string from the AI.
fn deserialize_string_or_seq<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrSeq {
        String(String),
        Seq(Vec<String>),
    }
    match StringOrSeq::deserialize(deserializer)? {
        StringOrSeq::String(s) if s.is_empty() => Ok(vec![]),
        StringOrSeq::String(s) => Ok(vec![s]),
        StringOrSeq::Seq(v) => Ok(v),
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AiAnalysisResult {
    #[serde(default)]
    pub user_portrait: UserPortrait,
    #[serde(default)]
    pub viewpoint_summary: ViewpointSummary,
    #[serde(default)]
    pub behavioral_patterns: BehavioralPatterns,
    #[serde(default)]
    pub personal_info: PersonalInfo,
    #[serde(default)]
    pub summary: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct UserPortrait {
    #[serde(default)]
    pub overall_impression: String,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    pub main_interests: Vec<String>,
    #[serde(default)]
    pub stance_profile: Vec<StanceItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StanceItem {
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub stance: String,
    #[serde(default)]
    pub intensity: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ViewpointSummary {
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    pub core_viewpoints: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    pub controversial_topics: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    pub unique_perspectives: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct BehavioralPatterns {
    #[serde(default)]
    pub reply_style: String,
    #[serde(default)]
    pub interaction_preference: String,
    #[serde(default)]
    pub depth_level: String,
}

// ── Personal info inferred from posts ──

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PersonalInfo {
    #[serde(default)]
    pub age_range: String,
    #[serde(default)]
    pub gender: String,
    #[serde(default)]
    pub height_weight: String,
    #[serde(default)]
    pub relationship: String,
    #[serde(default)]
    pub hometown: String,
    #[serde(default)]
    pub current_city: String,
    #[serde(default)]
    pub education: String,
    #[serde(default)]
    pub study_abroad: String,
    #[serde(default)]
    pub profession: String,
    #[serde(default)]
    pub income_hint: String,
    #[serde(default)]
    pub car: String,
    #[serde(default)]
    pub housing: String,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    pub sports_teams: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    pub hobbies: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    pub games: Vec<String>,
    #[serde(default)]
    pub personality_traits: String,
    #[serde(default)]
    pub political_stance: String,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    pub other_clues: Vec<String>,
    #[serde(default)]
    pub confidence_note: String,
}

// ── Post analysis data structures ──

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AiPostAnalysisResult {
    #[serde(default)]
    pub user_portrait: PostUserPortrait,
    #[serde(default)]
    pub post_analysis: PostAnalysis,
    #[serde(default)]
    pub viewpoint_summary: ViewpointSummary,
    #[serde(default)]
    pub engagement_analysis: EngagementAnalysis,
    #[serde(default)]
    pub summary: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PostUserPortrait {
    #[serde(default)]
    pub overall_impression: String,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    pub main_interests: Vec<String>,
    #[serde(default)]
    pub content_style: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PostAnalysis {
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    pub topic_focus: Vec<String>,
    #[serde(default)]
    pub content_types: ContentTypes,
    #[serde(default)]
    pub quality_assessment: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ContentTypes {
    #[serde(default)]
    pub text_only: String,
    #[serde(default)]
    pub image_posts: String,
    #[serde(default)]
    pub video_posts: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct EngagementAnalysis {
    #[serde(default)]
    pub average_replies: String,
    #[serde(default)]
    pub audience_response: String,
    #[serde(default)]
    pub most_engaging_content: String,
}

// ── Batching ──

/// Split replies into chunks by character budget.
pub fn chunk_replies(replies: &[ReplyRow]) -> Vec<Vec<ReplyRow>> {
    let mut chunks: Vec<Vec<ReplyRow>> = Vec::new();
    let mut current_chunk: Vec<ReplyRow> = Vec::new();
    let mut current_size: usize = 0;

    for reply in replies {
        let clean = strip_html(&reply.content);
        // prefix "N. " + content + "\n"
        let estimated = 4 + clean.chars().count();

        if current_size + estimated > CHUNK_CHARS && !current_chunk.is_empty() {
            chunks.push(std::mem::take(&mut current_chunk));
            current_size = 0;
        }
        current_chunk.push(ReplyRow {
            content: clean,
            ..reply.clone()
        });
        current_size += estimated;
    }

    if !current_chunk.is_empty() {
        chunks.push(current_chunk);
    }

    chunks
}

// ── DeepSeek API call ──

/// Result of a DeepSeek API call: parsed JSON value and the raw content string.
pub struct DeepSeekResult {
    pub value: serde_json::Value,
    pub raw_content: String,
}

async fn call_deepseek(client: &reqwest::Client, api_key: &str, system_prompt: &str, user_prompt: &str) -> Result<DeepSeekResult> {
    let request = DeepSeekRequest {
        model: MODEL.to_string(),
        messages: vec![
            Message { role: "system".to_string(), content: system_prompt.to_string() },
            Message { role: "user".to_string(), content: user_prompt.to_string() },
        ],
        max_tokens: 8192,
        response_format: ResponseFormat { type_: "json_object".to_string() },
    };

    let resp = client
        .post("https://api.deepseek.com/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?;

    let status = resp.status();
    let raw_text = resp.text().await?;

    // Try to deserialize the raw text; on failure, include the raw response for debugging
    let body: DeepSeekResponse = serde_json::from_str(&raw_text)
        .map_err(|e| anyhow::anyhow!("解码响应失败: {}，原始响应: {}", e, safe_truncate(&raw_text, 2000)))?;

    if let Some(err) = body.error {
        bail!("DeepSeek API error: {} ({:?})，原始响应: {}", err.message, err.type_, safe_truncate(&raw_text, 2000));
    }

    if !status.is_success() {
        bail!("DeepSeek API HTTP {}，原始响应: {}", status, safe_truncate(&raw_text, 2000));
    }

    let content = body.choices
        .first()
        .ok_or_else(|| anyhow::anyhow!("No choices in DeepSeek response，原始响应: {}", safe_truncate(&raw_text, 2000)))?
        .message
        .content
        .clone();

    // json_object mode should return valid JSON, but LLMs may still emit
    // unescaped control characters or truncate on token limits.
    let value = if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
        v
    } else {
        let sanitized = crate::utils::sanitize_json(&content);
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&sanitized) {
            v
        } else {
            let repaired = crate::utils::repair_truncated_json(&sanitized);
            serde_json::from_str::<serde_json::Value>(&repaired)
                .map_err(|e| anyhow::anyhow!("JSON解析失败: {}，原始响应前500字: {}", e, safe_truncate(&content, 500)))?
        }
    };

    Ok(DeepSeekResult { value, raw_content: content })
}

// ── Public entry points ──

/// Format posts into a context string for inclusion in identity analysis.
pub fn format_posts_context(posts: &[PostRow]) -> String {
    if posts.is_empty() {
        return String::new();
    }
    let mut lines = vec![format!("=== 该用户的主题帖（共{}条）===", posts.len())];
    for (i, p) in posts.iter().enumerate() {
        let media_flag = if p.has_video { "[视频] " } else if p.total_pics > 0 { "[图片] " } else { "" };
        lines.push(format!(
            "{}. {}{} (板块: {}, 回复: {}, 亮: {}, 浏览: {})",
            i + 1, media_flag, p.title, p.topic_name, p.replies, p.lights, p.visits
        ));
        if !p.summary.is_empty() {
            lines.push(format!("   {}", p.summary));
        }
    }
    lines.join("\n")
}

/// Analyze one batch of replies via DeepSeek, with optional posts context.
/// Returns (parsed JSON, raw AI content) so callers can log failures.
pub async fn analyze_batch(
    client: &reqwest::Client,
    api_key: &str,
    replies: &[ReplyRow],
    posts_context: Option<&str>,
) -> Result<(serde_json::Value, String)> {
    let replies_text: String = replies.iter()
        .enumerate()
        .map(|(i, r)| format!("{}. {}", i + 1, r.content))
        .collect::<Vec<_>>()
        .join("\n");

    let user_prompt = match posts_context {
        Some(ctx) if !ctx.is_empty() => {
            format!("{}\n\n=== 以下是该用户的回帖内容 ===\n\n{}", ctx, replies_text)
        }
        _ => replies_text,
    };

    let result = call_deepseek(client, api_key, BATCH_SYSTEM_PROMPT, &user_prompt).await?;
    Ok((result.value, result.raw_content))
}

/// Synthesize all batch results into a final user portrait.
pub async fn synthesize_results(
    client: &reqwest::Client,
    api_key: &str,
    batch_results: &[serde_json::Value],
) -> Result<AiAnalysisResult> {
    let batch_results_json = serde_json::to_string_pretty(batch_results)?;
    let user_prompt = format!("以下为该用户所有批次的AI分析结果，请综合生成完整用户画像：\n\n{}", batch_results_json);

    let result = call_deepseek(client, api_key, SYNTHESIS_SYSTEM_PROMPT, &user_prompt).await?;
    let analysis: AiAnalysisResult = serde_json::from_value(result.value.clone())
        .map_err(|e| anyhow::anyhow!("解析AI结果失败: {}，原始响应: {}", e, serde_json::to_string(&result.value).unwrap_or_default()))?;
    Ok(analysis)
}

// ── Post analysis functions ──

/// Split posts into chunks by character budget.
pub fn chunk_posts(posts: &[PostRow]) -> Vec<Vec<PostRow>> {
    let mut chunks: Vec<Vec<PostRow>> = Vec::new();
    let mut current_chunk: Vec<PostRow> = Vec::new();
    let mut current_size: usize = 0;

    for post in posts {
        let clean = strip_html(&post.summary);
        // title + "|" + summary + "\n"
        let estimated = 8 + post.title.chars().count() + clean.chars().count();

        if current_size + estimated > CHUNK_CHARS && !current_chunk.is_empty() {
            chunks.push(std::mem::take(&mut current_chunk));
            current_size = 0;
        }
        current_chunk.push(PostRow {
            summary: clean,
            ..post.clone()
        });
        current_size += estimated;
    }

    if !current_chunk.is_empty() {
        chunks.push(current_chunk);
    }

    chunks
}

/// Analyze one batch of posts via DeepSeek.
/// Returns (parsed JSON, raw AI content) so callers can log failures.
pub async fn analyze_post_batch(
    client: &reqwest::Client,
    api_key: &str,
    posts: &[PostRow],
) -> Result<(serde_json::Value, String)> {
    let posts_text: String = posts.iter()
        .enumerate()
        .map(|(i, p)| {
            let media_flag = if p.has_video { "[视频] " } else if p.total_pics > 0 { "[图片] " } else { "" };
            format!("{}. {} {} (板块: {}, 回复: {}, 亮: {}, 浏览: {})\n  {}",
                i + 1, media_flag, p.title, p.topic_name, p.replies, p.lights, p.visits, p.summary)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let result = call_deepseek(client, api_key, POST_BATCH_SYSTEM_PROMPT, &posts_text).await?;
    Ok((result.value, result.raw_content))
}

/// Synthesize all post batch results into a final analysis.
pub async fn synthesize_post_results(
    client: &reqwest::Client,
    api_key: &str,
    batch_results: &[serde_json::Value],
) -> Result<AiPostAnalysisResult> {
    let batch_results_json = serde_json::to_string_pretty(batch_results)?;
    let user_prompt = format!("以下为该用户所有批次的发帖AI分析结果，请综合生成完整发帖画像：\n\n{}", batch_results_json);

    let result = call_deepseek(client, api_key, POST_SYNTHESIS_SYSTEM_PROMPT, &user_prompt).await?;
    let analysis: AiPostAnalysisResult = serde_json::from_value(result.value.clone())
        .map_err(|e| anyhow::anyhow!("解析AI发帖分析结果失败: {}，原始响应: {}", e, serde_json::to_string(&result.value).unwrap_or_default()))?;
    Ok(analysis)
}
