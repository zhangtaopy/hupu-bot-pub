use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::posts::PostRow;
use crate::replies::ReplyRow;
use crate::utils::strip_html;

const CHUNK_CHARS: usize = 15_000;
const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/chat/completions";
const OLLAMA_BASE_URL: &str = "https://ollama.com/v1/chat/completions";
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const OPENCODE_BASE_URL: &str = "https://opencode.ai/zen/go/v1/chat/completions";

pub const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek-v4-flash";
pub const DEFAULT_OLLAMA_MODEL: &str = "gpt-oss:120b";
pub const DEFAULT_OPENROUTER_MODEL: &str = "google/gemini-2.0-flash-001";
pub const DEFAULT_OPENCODE_MODEL: &str = "opencode/deepseek-v4-flash-free";

/// LLM provider configuration.
#[derive(Clone)]
pub enum AiProvider {
    DeepSeek { api_key: String, model: String },
    Ollama { api_key: String, model: String },
    OpenRouter { api_key: String, model: String },
    OpenCode { api_key: String, model: String },
}

impl AiProvider {
    fn base_url(&self) -> &str {
        match self {
            AiProvider::DeepSeek { .. } => DEEPSEEK_BASE_URL,
            AiProvider::Ollama { .. } => OLLAMA_BASE_URL,
            AiProvider::OpenRouter { .. } => OPENROUTER_BASE_URL,
            AiProvider::OpenCode { .. } => OPENCODE_BASE_URL,
        }
    }

    fn api_key(&self) -> &str {
        match self {
            AiProvider::DeepSeek { api_key, .. } => api_key,
            AiProvider::Ollama { api_key, .. } => api_key,
            AiProvider::OpenRouter { api_key, .. } => api_key,
            AiProvider::OpenCode { api_key, .. } => api_key,
        }
    }

    fn model(&self) -> &str {
        match self {
            AiProvider::DeepSeek { model, .. } => model,
            AiProvider::Ollama { model, .. } => model,
            AiProvider::OpenRouter { model, .. } => model,
            AiProvider::OpenCode { model, .. } => model,
        }
    }

    /// Whether to include `response_format: json_object` in requests.
    fn use_json_mode(&self) -> bool {
        matches!(self, AiProvider::DeepSeek { .. } | AiProvider::OpenRouter { .. } | AiProvider::OpenCode { .. })
    }

    /// Whether this provider supports tool calling.
    pub fn supports_tool_calling(&self) -> bool {
        matches!(self, AiProvider::DeepSeek { .. } | AiProvider::OpenRouter { .. } | AiProvider::OpenCode { .. })
    }
}

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

// ── Agent multi-round structures ──

fn default_sort() -> String { "relevance".into() }
fn default_max_results() -> usize { 50 }

const QA_ANSWER_PROMPT: &str = r#"你是一个虎扑论坛数据分析助手。根据用户概览、搜索结果和提问者的问题，给出自然友好的回答。

重要：你分析的对象是"用户概览"中的人，不是提问者。永远用第三人称（他/她/用户名）来描述分析对象，不要用"你"来称呼他。

要求：
1. 充分利用用户概览中的统计数据和AI分析结果
2. 结合搜索结果中的具体内容作为佐证
3. 基于数据实事求是地回答，不要编造信息
4. 如果数据中没有相关信息，明确说明"根据现有数据无法判断"
5. 回答风格自然、友好，像在和用户聊天"#;

#[derive(Debug, Clone, Serialize)]
pub struct AgentTrace {
    pub round: usize,
    pub action: String,
    pub reasoning: String,
    pub keywords: Vec<String>,
    pub search_tables: Vec<String>,
    pub reply_count: usize,
    pub post_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallTrace>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallTrace {
    pub tool_name: String,
    pub args_summary: String,
    pub result_summary: String,
}

impl AgentTrace {
    pub fn format_md(&self) -> String {
        if let Some(ref tc) = self.tool_calls {
            let tool_details: Vec<String> = tc.iter()
                .map(|t| format!("🔧 {}({}) → {}", t.tool_name, t.args_summary, t.result_summary))
                .collect();
            format!(
                "<details>\n<summary>💬 第{}轮：调用 {} 个工具</summary>\n\n{}\n\n- 累计回帖: {} 条\n- 累计发帖: {} 条\n</details>",
                self.round,
                tc.len(),
                tool_details.join("\n\n"),
                self.reply_count,
                self.post_count,
            )
        } else if self.keywords.is_empty() {
            format!(
                "<details>\n<summary>💬 第{}轮：{}</summary>\n\n- 累计回帖: {} 条\n- 累计发帖: {} 条\n</details>",
                self.round,
                self.action,
                self.reply_count,
                self.post_count,
            )
        } else {
            let tables = if self.search_tables.is_empty() { "全部".into() } else { self.search_tables.join("、") };
            format!(
                "<details>\n<summary>🔍 第{}轮：{} | 搜索表: {} | 关键词: {}</summary>\n\n> {}\n\n- 本轮回帖: {} 条\n- 本轮发帖: {} 条\n</details>",
                self.round,
                self.action,
                tables,
                self.keywords.join("、"),
                self.reasoning,
                self.reply_count,
                self.post_count,
            )
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct AgentAction {
    pub action: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub search_tables: Vec<String>,
    #[serde(default)]
    pub topic_filter: Vec<String>,
    #[serde(default = "default_sort")]
    pub sort_by: String,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub answer: String,
}

// ── Tool Calling data structures ──

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub r#type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
pub struct ToolCallResponse {
    #[serde(rename = "type")]
    pub r#type: String,
    pub function: ToolCallFunctionResponse,
}

#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
pub struct ToolCallFunctionResponse {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ChatMessage {
    System { content: String },
    User { content: String },
    Assistant { content: String },
    AssistantWithToolCalls { content: Option<String>, tool_calls: Vec<ToolCall>, reasoning_content: Option<String> },
    ToolResult { tool_call_id: String, content: String },
}

impl ChatMessage {
    fn to_json(&self) -> serde_json::Value {
        match self {
            ChatMessage::System { content } => serde_json::json!({
                "role": "system",
                "content": content,
            }),
            ChatMessage::User { content } => serde_json::json!({
                "role": "user",
                "content": content,
            }),
            ChatMessage::Assistant { content } => serde_json::json!({
                "role": "assistant",
                "content": content,
            }),
            ChatMessage::AssistantWithToolCalls { content, tool_calls, reasoning_content } => {
                let calls: Vec<serde_json::Value> = tool_calls.iter().map(|tc| {
                    serde_json::json!({
                        "id": tc.id,
                        "type": tc.r#type,
                        "function": {
                            "name": tc.function.name,
                            "arguments": tc.function.arguments,
                        }
                    })
                }).collect();
                let mut map = serde_json::json!({
                    "role": "assistant",
                    "tool_calls": calls,
                });
                if let Some(c) = content {
                    map["content"] = serde_json::json!(c);
                }
                if let Some(rc) = reasoning_content {
                    map["reasoning_content"] = serde_json::json!(rc);
                }
                map
            }
            ChatMessage::ToolResult { tool_call_id, content } => serde_json::json!({
                "role": "tool",
                "tool_call_id": tool_call_id,
                "content": content,
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmToolResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub token_usage: TokenUsage,
    pub reasoning_content: Option<String>,
}

pub const QA_TOOL_SYSTEM_PROMPT: &str = r#"你是一个虎扑论坛数据分析助手。你可以使用以下工具来查询数据库中的用户信息，然后基于查询结果回答提问者的问题。

## 关键规则

你分析的对象是"用户基本信息"中的那个用户，不是提问者。在回答中：
- 永远用第三人称（他/她/用户名）来描述分析对象
- 禁止用"你"来称呼分析对象，因为提问者和分析对象是不同的人

## 可用工具

1. **search_by_keywords** - 按关键词搜索回帖/发帖，适合查找特定内容
2. **search_by_time_range** - 按时间范围搜索，适合时间趋势分析
3. **get_topic_stats** - 获取板块分布统计，适合了解用户活跃板块
4. **get_hot_content** - 获取热门内容，适合了解用户最有影响力的帖子
5. **get_user_stats** - 获取用户综合统计，适合总体概览
6. **get_ai_profile** - 获取AI分析画像，适合深入了解用户特征

## 工作流程

1. 先理解提问者的问题
2. 选择合适的工具查询数据（可以多次调用不同工具）
3. 基于查询结果，直接给出自然友好的回答

## 注意事项

- 同一个工具不要重复调用相同参数
- 如果已经获得了足够的信息，直接回答，不要再调用工具
- 回答要具体，引用数据作为支撑
- 回答风格自然友好，像在聊天"#;

pub fn build_qa_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "search_by_keywords".to_string(),
                description: "按关键词搜索用户的回帖和发帖。适合查找特定话题、观点或内容。".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "keywords": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "搜索关键词列表，如[\"篮球\", \"NBA\"]"
                        },
                        "tables": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "搜索哪些表，可选值：\"replies\"、\"posts\"，或同时包含两者。默认搜索全部"
                        },
                        "topic_filter": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "板块过滤，如[\"步行街\"]。空数组表示不过滤"
                        },
                        "sort_by": {
                            "type": "string",
                            "description": "排序方式：\"relevance\"（默认）、\"create_time\"、\"light_count\""
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "最大返回条数，默认50"
                        }
                    },
                    "required": ["keywords"]
                }),
            },
        },
        ToolDefinition {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "search_by_time_range".to_string(),
                description: "按时间范围搜索用户的回帖和发帖，用于分析用户在不同时段的活动情况。时间格式为YYYY-MM。".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "start_date": {
                            "type": "string",
                            "description": "开始月份，格式YYYY-MM，如\"2024-01\""
                        },
                        "end_date": {
                            "type": "string",
                            "description": "结束月份，格式YYYY-MM，如\"2024-12\""
                        },
                        "tables": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "搜索哪些表，可选值：\"replies\"、\"posts\"，或同时包含两者"
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "最大返回条数，默认50"
                        }
                    },
                    "required": ["start_date", "end_date"]
                }),
            },
        },
        ToolDefinition {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "get_topic_stats".to_string(),
                description: "获取用户在各板块的回帖分布统计，了解用户主要在哪些板块活跃。".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
        },
        ToolDefinition {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "get_hot_content".to_string(),
                description: "获取用户最热门的回帖或发帖，按点赞数、回复数或浏览数排序。".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "table": {
                            "type": "string",
                            "description": "\"replies\"或\"posts\""
                        },
                        "sort_by": {
                            "type": "string",
                            "description": "排序方式：\"light_count\"/\"lights\"（点赞）、\"replies\"（回复）、\"visits\"（浏览）。默认lights"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "返回条数，默认10"
                        }
                    },
                    "required": ["table"]
                }),
            },
        },
        ToolDefinition {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "get_user_stats".to_string(),
                description: "获取用户综合统计数据，包括总回帖数、总发帖数、活跃时间范围、板块分布等。".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
        },
        ToolDefinition {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "get_ai_profile".to_string(),
                description: "获取AI对该用户的分析画像，包括观点倾向、关注话题、个人信息推断等。".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
        },
    ]
}

const QA_AGENT_PROMPT: &str = r#"你是一个虎扑论坛数据分析助手。你的任务是通过多轮数据库搜索，逐步收集信息，最终回答提问者关于某个用户的的问题。

## 关键规则

你分析的对象是"用户基本信息"中的那个用户，不是提问者。在 final_answer 中：
- 永远用第三人称（他/她/用户名）来描述分析对象
- 禁止用"你"来称呼分析对象，因为提问者和分析对象是不同的人

## 工作方式

你会收到分析对象的基本信息（统计数据、AI画像等）和提问者的问题。你可以进行多轮搜索，每轮用不同的关键词从不同角度查找信息。

每次你必须输出一个 JSON 对象，包含以下字段之一：

### 搜索操作
当需要更多信息时输出：
{"action":"search","keywords":["关键词1","关键词2"],"search_tables":["replies"],"topic_filter":[],"sort_by":"relevance","max_results":50,"reasoning":"为什么需要这轮搜索的简短说明"}

### 最终回答
当已收集足够信息或无法获取更多时输出：
{"action":"final_answer","answer":"你的完整回答（用第三人称描述分析对象）"}

## 数据库说明

- replies 表: content(回帖内容), title(帖子标题), topic_name(板块名), create_time(时间戳), light_count(点亮数)
- posts 表: title(帖子标题), summary(帖子摘要), topic_name(板块名), forum_name(分区名), create_time(时间戳), replies(回复数), visits(浏览数), lights(点亮数)

## 搜索策略

1. 第一轮：根据已知信息和问题生成精准的关键词，广泛搜索
2. 后续轮次：如果已有结果不够或方向不对，换角度、换关键词
3. **禁止使用前几轮已经用过的关键词**，每次搜索必须是新的关键词。重复关键词只会浪费一轮搜索
4. 如果连续搜索都找不到相关信息，承认"根据现有数据无法判断"并结束
4. search_tables 可选: ["replies"]、["posts"] 或 ["replies","posts"]
5. sort_by: "relevance" 用于相关性，"create_time" 用于时间顺序，"light_count" 用于热度
6. max_results: 通常 50，特别需要大量样本时用 80-100
7. topic_filter: 空数组表示不过滤板块，否则填入板块名

## 核心原则

- 每个 reasoning 要简短说明为什么选这些关键词
- 如果已有结果已经能充分回答问题，立即输出 final_answer
- 不要超过 5 轮搜索
- 回答要基于数据，引用具体内容作为佐证
- 回答风格自然友好，像在聊天
- 在 final_answer 的 answer 字段中引用原文或描述时，使用中文引号""而不是英文引号""，以确保JSON格式正确"#;

pub async fn agent_decide(
    client: &reqwest::Client,
    provider: &AiProvider,
    question: &str,
    user_ctx: &str,
    history_ctx: &str,
    previous_rounds: &str,
    round: usize,
) -> Result<(AgentAction, TokenUsage)> {
    let history_block = if history_ctx.is_empty() {
        String::new()
    } else {
        format!("对话历史：\n\n{}\n\n---\n\n", history_ctx)
    };

    let previous_block = if previous_rounds.is_empty() {
        String::new()
    } else {
        format!("前几轮搜索结果：\n\n{}\n\n---\n\n", previous_rounds)
    };

    let user_prompt = format!(
        "{}以下是你要分析的用户的基本信息（注意：这是分析对象，不是提问者）：\n\n{}\n\n---\n\n{}提问者的问题是：{}\n\n---\n\n当前是第 {} 轮。请决定下一步操作。",
        history_block, user_ctx, previous_block, question, round
    );

    let result = call_llm(client, provider, QA_AGENT_PROMPT, &user_prompt).await?;
    let action: AgentAction = serde_json::from_value(result.value)
        .map_err(|e| anyhow::anyhow!("解析Agent决策失败: {}，原始: {}", e, result.raw_content))?;
    Ok((action, result.token_usage))
}

pub fn format_search_results_summary(replies: &[ReplyRow], posts: &[PostRow]) -> String {
    let mut s = String::new();
    if !posts.is_empty() {
        s.push_str(&format!("发帖命中 {} 条:\n", posts.len()));
        for (i, p) in posts.iter().enumerate() {
            s.push_str(&format!("  {}. [{}] {} ({}回复/{}亮)\n",
                i + 1, p.topic_name, p.title, p.replies, p.lights));
            if !p.summary.is_empty() {
                let brief: String = p.summary.chars().take(120).collect();
                s.push_str(&format!("     {}\n", brief));
            }
        }
    }
    if !replies.is_empty() {
        s.push_str(&format!("回帖命中 {} 条:\n", replies.len()));
        for (i, r) in replies.iter().enumerate() {
            let brief: String = r.content.chars().take(150).collect();
            s.push_str(&format!("  {}. [{}] {} ({})\n",
                i + 1,
                r.topic_name.as_deref().unwrap_or("未知"),
                r.title,
                brief,
            ));
        }
    }
    if s.is_empty() {
        s = String::from("(无结果)");
    }
    s
}

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
    usage: Option<TokenUsage>,
    error: Option<ApiError>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCall>>,
    #[serde(default)]
    reasoning_content: Option<String>,
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

/// Result of a LLM API call: parsed JSON value, the raw content string, and token usage.
pub struct DeepSeekResult {
    pub value: serde_json::Value,
    pub raw_content: String,
    pub token_usage: TokenUsage,
}

#[derive(Serialize)]
struct LlmRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

async fn call_llm(
    client: &reqwest::Client,
    provider: &AiProvider,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<DeepSeekResult> {
    let response_format = if provider.use_json_mode() {
        Some(ResponseFormat { type_: "json_object".to_string() })
    } else {
        None
    };

    let request = LlmRequest {
        model: provider.model().to_string(),
        messages: vec![
            Message { role: "system".to_string(), content: system_prompt.to_string() },
            Message { role: "user".to_string(), content: user_prompt.to_string() },
        ],
        max_tokens: 8192,
        response_format,
    };

    let resp = client
        .post(provider.base_url())
        .header("Authorization", format!("Bearer {}", provider.api_key()))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?;

    let status = resp.status();
    let raw_text = resp.text().await?;

    let body: DeepSeekResponse = serde_json::from_str(&raw_text)
        .map_err(|e| anyhow::anyhow!("解码响应失败: {}，原始响应: {}", e, safe_truncate(&raw_text, 2000)))?;

    if let Some(err) = body.error {
        bail!("LLM API error: {} ({:?})，原始响应: {}", err.message, err.type_, safe_truncate(&raw_text, 2000));
    }

    if !status.is_success() {
        bail!("LLM API HTTP {}，原始响应: {}", status, safe_truncate(&raw_text, 2000));
    }

    let content = body.choices
        .first()
        .ok_or_else(|| anyhow::anyhow!("No choices in LLM response，原始响应: {}", safe_truncate(&raw_text, 2000)))?
        .message
        .content
        .clone()
        .unwrap_or_default();

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

    Ok(DeepSeekResult { value, raw_content: content, token_usage: body.usage.unwrap_or_default() })
}

async fn call_llm_text(
    client: &reqwest::Client,
    provider: &AiProvider,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<(String, TokenUsage)> {
    #[derive(Serialize)]
    struct TextRequest {
        model: String,
        messages: Vec<Message>,
        max_tokens: u32,
    }

    let request = TextRequest {
        model: provider.model().to_string(),
        messages: vec![
            Message { role: "system".to_string(), content: system_prompt.to_string() },
            Message { role: "user".to_string(), content: user_prompt.to_string() },
        ],
        max_tokens: 4096,
    };

    let resp = client
        .post(provider.base_url())
        .header("Authorization", format!("Bearer {}", provider.api_key()))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?;

    let status = resp.status();
    let raw_text = resp.text().await?;

    let body: DeepSeekResponse = serde_json::from_str(&raw_text)
        .map_err(|e| anyhow::anyhow!("解码响应失败: {}，原始响应: {}", e, safe_truncate(&raw_text, 2000)))?;

    if let Some(err) = body.error {
        bail!("LLM API error: {} ({:?})，原始响应: {}", err.message, err.type_, safe_truncate(&raw_text, 2000));
    }

    if !status.is_success() {
        bail!("LLM API HTTP {}，原始响应: {}", status, safe_truncate(&raw_text, 2000));
    }

    let content = body.choices
        .first()
        .ok_or_else(|| anyhow::anyhow!("No choices in LLM response，原始响应: {}", safe_truncate(&raw_text, 2000)))?
        .message
        .content
        .clone()
        .unwrap_or_default();

    Ok((content, body.usage.unwrap_or_default()))
}

/// Call LLM with optional tool calling support. Returns content (if any) and tool_calls.
/// Tools should be provided every round, otherwise the API won't return tool_calls.
pub async fn call_llm_with_tools(
    client: &reqwest::Client,
    provider: &AiProvider,
    messages: &[ChatMessage],
    tools: Option<&[ToolDefinition]>,
) -> Result<LlmToolResponse> {
    let messages_json: Vec<serde_json::Value> = messages.iter().map(|m| m.to_json()).collect();

    let mut request_body = serde_json::json!({
        "model": provider.model(),
        "messages": messages_json,
        "max_tokens": 8192,
    });
    if let Some(t) = tools {
        let tools_json: Vec<serde_json::Value> = t.iter().map(|def| serde_json::to_value(def).unwrap()).collect();
        request_body["tools"] = serde_json::json!(tools_json);
    }

    let resp = client
        .post(provider.base_url())
        .header("Authorization", format!("Bearer {}", provider.api_key()))
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await?;

    let status = resp.status();
    let raw_text = resp.text().await?;

    let body: DeepSeekResponse = serde_json::from_str(&raw_text)
        .map_err(|e| anyhow::anyhow!("Tool calling 解码响应失败: {}，原始响应: {}", e, safe_truncate(&raw_text, 2000)))?;

    if let Some(err) = body.error {
        bail!("Tool calling LLM API error: {} ({:?})", err.message, err.type_);
    }

    if !status.is_success() {
        bail!("Tool calling LLM API HTTP {}，原始响应: {}", status, safe_truncate(&raw_text, 2000));
    }

    let choice = body.choices
        .first()
        .ok_or_else(|| anyhow::anyhow!("No choices in tool calling response"))?;

    let content = choice.message.content.clone();
    let tool_calls = choice.message.tool_calls.clone().unwrap_or_default();
    let reasoning_content = choice.message.reasoning_content.clone();

    Ok(LlmToolResponse {
        content,
        tool_calls,
        token_usage: body.usage.unwrap_or_default(),
        reasoning_content,
    })
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
    provider: &AiProvider,
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

    let result = call_llm(client, provider, BATCH_SYSTEM_PROMPT, &user_prompt).await?;
    Ok((result.value, result.raw_content))
}

/// Call `analyze_batch` with up to `max_retries` retries on failure.
/// Uses exponential backoff (1s, 2s, 4s…) between retries.
/// Only saves error to DB if ALL retries fail.
pub async fn analyze_batch_with_retry(
    client: &reqwest::Client,
    provider: &AiProvider,
    replies: &[ReplyRow],
    posts_context: Option<&str>,
    max_retries: usize,
) -> Result<(serde_json::Value, String)> {
    let mut last_err = String::new();
    for attempt in 0..=max_retries {
        if attempt > 0 {
            let base_ms = 1000u64 << (attempt - 1);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let delay = std::time::Duration::from_millis(base_ms + (nanos % 500) as u64);
            tokio::time::sleep(delay).await;
        }
        match analyze_batch(client, provider, replies, posts_context).await {
            Ok(v) => return Ok(v),
            Err(e) => last_err = e.to_string(),
        }
    }
    anyhow::bail!("批次分析重试{}次均已失败: {}", max_retries, last_err)
}

/// Synthesize all batch results into a final user portrait.
pub async fn synthesize_results(
    client: &reqwest::Client,
    provider: &AiProvider,
    batch_results: &[serde_json::Value],
) -> Result<AiAnalysisResult> {
    let batch_results_json = serde_json::to_string_pretty(batch_results)?;
    let user_prompt = format!("以下为该用户所有批次的AI分析结果，请综合生成完整用户画像：\n\n{}", batch_results_json);

    let result = call_llm(client, provider, SYNTHESIS_SYSTEM_PROMPT, &user_prompt).await?;
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
    provider: &AiProvider,
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

    let result = call_llm(client, provider, POST_BATCH_SYSTEM_PROMPT, &posts_text).await?;
    Ok((result.value, result.raw_content))
}

/// Call `analyze_post_batch` with up to `max_retries` retries on failure.
/// Uses exponential backoff (1s, 2s, 4s…) between retries.
pub async fn analyze_post_batch_with_retry(
    client: &reqwest::Client,
    provider: &AiProvider,
    posts: &[PostRow],
    max_retries: usize,
) -> Result<(serde_json::Value, String)> {
    let mut last_err = String::new();
    for attempt in 0..=max_retries {
        if attempt > 0 {
            let base_ms = 1000u64 << (attempt - 1);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let delay = std::time::Duration::from_millis(base_ms + (nanos % 500) as u64);
            tokio::time::sleep(delay).await;
        }
        match analyze_post_batch(client, provider, posts).await {
            Ok(v) => return Ok(v),
            Err(e) => last_err = e.to_string(),
        }
    }
    anyhow::bail!("批次分析重试{}次均已失败: {}", max_retries, last_err)
}

/// Synthesize all post batch results into a final analysis.
pub async fn synthesize_post_results(
    client: &reqwest::Client,
    provider: &AiProvider,
    batch_results: &[serde_json::Value],
) -> Result<AiPostAnalysisResult> {
    let batch_results_json = serde_json::to_string_pretty(batch_results)?;
    let user_prompt = format!("以下为该用户所有批次的发帖AI分析结果，请综合生成完整发帖画像：\n\n{}", batch_results_json);

    let result = call_llm(client, provider, POST_SYNTHESIS_SYSTEM_PROMPT, &user_prompt).await?;
    let analysis: AiPostAnalysisResult = serde_json::from_value(result.value.clone())
        .map_err(|e| anyhow::anyhow!("解析AI发帖分析结果失败: {}，原始响应: {}", e, serde_json::to_string(&result.value).unwrap_or_default()))?;
    Ok(analysis)
}

/// Format query results into a context string for the AI.
pub fn format_query_results(replies: &[ReplyRow], posts: &[PostRow]) -> String {
    let mut ctx = String::new();

    if !posts.is_empty() {
        ctx.push_str(&format!("=== 相关发帖（共{}条）===\n", posts.len()));
        for (i, p) in posts.iter().enumerate() {
            ctx.push_str(&format!(
                "{}. [{}] {} ({}回复/{}亮/{}浏览)\n",
                i + 1, p.topic_name, p.title, p.replies, p.lights, p.visits
            ));
            if !p.summary.is_empty() {
                ctx.push_str(&format!("   摘要: {}\n", p.summary));
            }
        }
        ctx.push('\n');
    }

    if !replies.is_empty() {
        ctx.push_str(&format!("=== 相关回帖（共{}条）===\n", replies.len()));
        for (i, r) in replies.iter().enumerate() {
            ctx.push_str(&format!(
                "{}. [{}] {} | {}亮\n   {}\n",
                i + 1,
                r.topic_name.as_deref().unwrap_or("未知"),
                r.title,
                r.light_count,
                r.content,
            ));
        }
    }

    ctx
}

// ── User overview data for Q&A context ──

pub struct UserOverview {
    pub total_replies: i64,
    pub total_posts: i64,
    pub topic_distribution: Vec<(String, usize)>, // top N topics
    pub reply_time_distribution: Vec<(String, usize)>, // monthly reply counts
    pub activity_period: Option<String>,          // "2023-01 ~ 2026-05"
    pub ai_reply_analysis_summary: Option<String>,  // summary field from ai_analysis
    pub ai_post_analysis_summary: Option<String>,   // summary field from ai_post_analysis
    pub ai_reply_personal_info: Option<String>,     // personal_info JSON from ai_analysis
}

impl UserOverview {
    pub fn format(&self) -> String {
        let mut s = String::new();

        s.push_str(&format!("总回帖数: {}\n", self.total_replies));
        s.push_str(&format!("总发帖数: {}\n", self.total_posts));

        if let Some(ref period) = self.activity_period {
            s.push_str(&format!("活跃时间范围: {}\n", period));
        }

        if !self.topic_distribution.is_empty() {
            s.push_str("\n活跃板块分布（前10）:\n");
            for (i, (topic, count)) in self.topic_distribution.iter().enumerate() {
                s.push_str(&format!("  {}. {} - {}条\n", i + 1, topic, count));
            }
        }

        if !self.reply_time_distribution.is_empty() {
            s.push_str("\n月度回帖分布:\n");
            for (month, count) in &self.reply_time_distribution {
                s.push_str(&format!("  {}: {}条\n", month, count));
            }
        }

        if let Some(ref info) = self.ai_reply_personal_info {
            s.push_str(&format!("\n已推断的个人信息:\n{}\n", info));
        }

        if let Some(ref summary) = self.ai_reply_analysis_summary {
            s.push_str(&format!("\nAI回帖分析评语:\n{}\n", summary));
        }

        if let Some(ref summary) = self.ai_post_analysis_summary {
            s.push_str(&format!("\nAI发帖分析评语:\n{}\n", summary));
        }

        s
    }
}

/// Step 3: Generate the final answer based on query results and user overview.
pub async fn generate_answer(
    client: &reqwest::Client,
    provider: &AiProvider,
    question: &str,
    _username: &str,
    overview: &UserOverview,
    replies: &[ReplyRow],
    posts: &[PostRow],
    history_ctx: &str,
) -> Result<(String, TokenUsage)> {
    let overview_text = overview.format();
    let context = format_query_results(replies, posts);

    // Truncate raw context if too large (keep overview intact)
    let max_context = 20_000;
    let context = if context.chars().count() > max_context {
        format!("{}...(结果过多，已截断)", safe_truncate(&context, max_context))
    } else {
        context
    };

    let history_block = if history_ctx.is_empty() {
        String::new()
    } else {
        format!("对话历史：\n\n{}\n\n---\n\n", history_ctx)
    };

    let user_prompt = format!(
        "{}以下是分析对象的概览数据：\n\n{}\n\n---\n\n相关数据：\n\n{}\n\n---\n\n提问者的问题是：{}\n\n请基于以上信息回答（用第三人称描述分析对象）。",
        history_block, overview_text, context, question
    );

    call_llm_text(client, provider, QA_ANSWER_PROMPT, &user_prompt).await
}

/// Format conversation history entries into a context string.
pub fn format_history(history: &[crate::server::types::HistoryEntry]) -> String {
    if history.is_empty() {
        return String::new();
    }
    let mut s = String::new();
    for (i, h) in history.iter().enumerate() {
        let brief: String = h.answer.chars().take(80).collect();
        let ellipsis = if h.answer.chars().count() > 80 { "..." } else { "" };
        s.push_str(&format!("Q{}: {}\nA{}: {}{}\n\n", i + 1, h.question, i + 1, brief, ellipsis));
    }
    s
}
