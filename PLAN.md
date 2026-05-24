# Q&A Agent: 从 Prompt JSON 迁移到原生 Tool Calling

## Context

当前 Q&A Agent 通过 prompt 工程（`QA_AGENT_PROMPT`）让 LLM 输出 `{"action":"search","keywords":[...]}` JSON，Rust 代码解析后执行 SQL `LIKE` 查询。这种方式只有一个搜索维度（关键词模糊匹配），agent 无法按时间、热度、板块统计等维度查询，导致分析不够全面。

目标：迁移到 OpenAI 兼容的 `tools` 参数，提供 6 个专用工具让 LLM 自主选择调用，获得更全面的数据库信息。

## 修改文件

1. **`src/db.rs`** — 新增 4 个查询函数
2. **`src/deepseek.rs`** — 新增 tool calling 数据结构、API 函数、工具定义、新 system prompt
3. **`src/services/qa.rs`** — 重写 agent_loop、新增工具执行函数、修改 SSE 事件格式
4. **`web/index.html`** — 更新前端渲染工具调用结果

## 实施步骤

### 1. db.rs: 新增查询函数

新增 4 个函数：

- **`search_replies_by_time(conn, euid, start_date, end_date, max_results)`** — 按 YYYY-MM 时间范围搜回帖
- **`search_posts_by_time(conn, euid, start_date, end_date, max_results)`** — 按 YYYY-MM 时间范围搜发帖
- **`get_hot_replies(conn, euid, sort_by, limit)`** — 按 light_count 排序获取热门回帖
- **`get_hot_posts(conn, euid, sort_by, limit)`** — 按 lights/replies/visits 排序获取热门发帖

保留现有 `search_replies/search_posts` 不变，`search_by_keywords` 工具直接复用。

### 2. deepseek.rs: Tool Calling 基础设施

#### 2a. 新增数据结构

```rust
// 工具定义（发送给 LLM）
ToolDefinition { r#type, function: ToolFunction { name, description, parameters } }
ToolCall { id, r#type, function: ToolCallFunction { name, arguments } }
ToolCallResponse { id, r#type, function: ToolCallFunctionResponse { name, arguments } }

// 对话消息（支持 tool 角色）
enum ChatMessage {
    Simple { role, content },
    AssistantWithToolCalls { role, content, tool_calls },
    ToolResult { role: "tool", content, tool_call_id },
}
```

#### 2b. 新增 `call_llm_with_tools` 函数

- 发送 `messages` + `tools` 到 LLM API
- 解析响应中的 `tool_calls` 字段
- 返回 `LlmToolResponse { content, tool_calls, token_usage }`
- 当有 `tools` 时不使用 `response_format: json_object`

#### 2c. 新增 `build_qa_tools()` 函数

定义 6 个工具：

| 工具名 | 用途 | 关键参数 |
|--------|------|----------|
| `search_by_keywords` | 关键词搜索回帖/发帖 | keywords, tables, topic_filter, sort_by, max_results |
| `search_by_time_range` | 按时间范围搜索 | start_date, end_date, tables, max_results |
| `get_topic_stats` | 板块分布统计 | （无参数） |
| `get_hot_content` | 热门内容排行 | table, sort_by, limit |
| `get_user_stats` | 用户综合统计 | （无参数） |
| `get_ai_profile` | AI 分析画像 | （无参数） |

#### 2d. 新增 `QA_TOOL_SYSTEM_PROMPT`

替代旧的 `QA_AGENT_PROMPT`，不再要求 LLM 输出 JSON，而是指导其如何使用工具。

#### 2e. 扩展 `AgentTrace`

增加 `tool_calls: Option<Vec<ToolCallTrace>>` 字段，保留原字段向后兼容。

### 3. qa.rs: 重写 Agent Loop

#### 3a. 新 `agent_loop` 逻辑

```
1. 构建 messages（system + user）
2. loop (max 15 turns):
   a. 调用 call_llm_with_tools(messages, tools)
   b. 如果无 tool_calls → 提取文本作为最终回答，break
   c. 有 tool_calls → 将 assistant 消息加入 messages
   d. 逐一执行每个 tool_call，将 tool result 加入 messages
   e. 发送 SSE 事件（tool_call 类型）
   f. 记录 AgentTrace
3. 发送最终回答 SSE 事件
```

#### 3b. 6 个工具执行函数

每个解析 `serde_json::Value` 参数，调用对应 db 函数，返回 `ToolExecResult { content, summary, reply_count, post_count }`。

- `execute_search_by_keywords` → 复用现有 `db::search_replies/search_posts`
- `execute_search_by_time_range` → 调用新 db 函数
- `execute_get_topic_stats` → 从 UserContext 获取板块分布
- `execute_get_hot_content` → 调用新 db 函数
- `execute_get_user_stats` → 从 UserContext + db::query_time_distribution 获取统计
- `execute_get_ai_profile` → 从 UserContext 获取 AI 画像

#### 3c. 保留旧版作为 fallback

将旧 `agent_loop` 重命名为 `agent_loop_legacy`，在 provider 不支持 tool calling 时回退。

#### 3d. SSE 事件格式

保留 `{type: "round"}` 事件的向后兼容，增加 `tool_calls` 字段。
新增 `{type: "tool_call", round, tool_name, args_summary, result_summary}` 事件。

### 4. web/index.html: 前端更新

- SSE 事件处理：增加 `tool_call` 类型解析
- round 对象增加 `tool_calls` 字段
- 模板中增加 tool_calls 渲染（工具名 + 参数摘要 + 结果摘要）

### 5. Token 预算管理

- 工具结果截断：`content` 最多 4000 字符
- 累计 prompt_tokens 监控：超过 60000 时强制生成最终回答
- `UserContext` 增加 `activity_period` 字段

## 验证

1. `cargo build` 编译通过
2. `cargo test` 现有测试通过
3. 新增 db 函数的单元测试
4. 手动测试：启动服务器，在 Q&A 页面提问，验证：
   - agent 正确调用多个工具
   - 前端正确显示每轮工具调用信息
   - Ollama 等不支持 tool calling 的 provider 回退到 legacy 模式
   - token 预算管理正常截断