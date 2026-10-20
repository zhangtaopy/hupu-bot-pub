use once_cell::sync::Lazy;
use regex::Regex;

static HTML_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());

/// 去掉 HTML 标签，清理虎扑特有的内容标记
pub fn strip_html(s: &str) -> String {
    let s = s.replace("[图片]", "").replace("[视频]", "");
    HTML_REGEX.replace_all(&s, "").trim().to_string()
}

/// 解码 JSON 中的 unicode 转义和换行转义（topic 模块解析原始 HTML 时使用）
pub fn decode_json_escapes(s: &str) -> String {
    s.replace("\\u003c", "<")
        .replace("\\u003e", ">")
        .replace("\\n", "\n")
        .replace("\\r", "")
}

/// 找到匹配的花括号（用于 JSON 对象）
pub fn find_matching_brace(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    if start >= bytes.len() || bytes[start] != b'{' {
        return None;
    }

    let mut depth = 0;
    let mut in_string = false;
    let mut escape = false;

    for i in start..bytes.len() {
        let b = bytes[i];

        if escape {
            escape = false;
            continue;
        }

        if b == b'\\' && in_string {
            escape = true;
            continue;
        }

        if b == b'"' {
            in_string = !in_string;
            continue;
        }

        if in_string {
            continue;
        }

        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }

    None
}

/// Fix unescaped control characters inside JSON string values.
/// LLMs sometimes emit literal \n \r \t inside strings instead of \\n \\r \\t.
pub fn sanitize_json(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(s.len());
    let mut in_string = false;
    let mut escape = false;

    let mut i = 0;
    while i < len {
        let ch = chars[i];
        if escape {
            escape = false;
            result.push(ch);
            i += 1;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            result.push('\\');
            i += 1;
            continue;
        }
        if ch == '"' {
            if !in_string {
                in_string = true;
                result.push('"');
            } else {
                // Peek at the next non-whitespace char to decide if this
                // quote is a structural delimiter (key/value end) or an
                // unescaped embedded quote that needs escaping.
                let mut j = i + 1;
                while j < len && chars[j].is_ascii_whitespace() {
                    j += 1;
                }
                let next_ch = if j < len { chars[j] } else { '\0' };
                if matches!(next_ch, ':' | ',' | '}' | ']' | '\0') {
                    in_string = false;
                    result.push('"');
                } else {
                    result.push_str("\\\"");
                }
            }
            i += 1;
            continue;
        }
        if in_string {
            match ch {
                '\n' => result.push_str("\\n"),
                '\r' => result.push_str("\\r"),
                '\t' => result.push_str("\\t"),
                c if (c as u32) < 0x20 => result.push_str(&format!("\\u{:04x}", c as u32)),
                _ => result.push(ch),
            }
        } else {
            result.push(ch);
        }
        i += 1;
    }

    result
}

/// Try to close a truncated JSON string by counting unclosed brackets/braces
/// and appending closing delimiters. Also closes any unterminated string.
pub fn repair_truncated_json(s: &str) -> String {
    let mut result = s.to_string();
    let bytes = result.as_bytes();
    let mut in_string = false;
    let mut escape = false;
    let mut brace_stack: Vec<u8> = Vec::new();

    for &b in bytes {
        if escape {
            escape = false;
            continue;
        }
        if b == b'\\' && in_string {
            escape = true;
            continue;
        }
        if b == b'"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match b {
            b'{' | b'[' => brace_stack.push(b),
            b'}' => { if brace_stack.last() == Some(&b'{') { brace_stack.pop(); } }
            b']' => { if brace_stack.last() == Some(&b'[') { brace_stack.pop(); } }
            _ => {}
        }
    }

    if in_string {
        result.push('"');
    }

    while let Some(&b) = brace_stack.last() {
        result.push(if b == b'{' { '}' } else { ']' });
        brace_stack.pop();
    }

    result
}

/// 把虎扑接口的 `internalCode` 翻译成人话。
///
/// 虎扑接口失败时 `code` 恒为 0，真正的错误语义在 `internalCode` 里。
/// 只译已知的、对排查有用的码；未知码原样返回 None，由调用方展示原始值。
pub fn describe_internal_code(internal_code: &str) -> Option<&'static str> {
    match internal_code {
        "PC022003" => Some("登录态失效（Cookie 过期或缺少该接口要求的登录凭证），请重新登录后更新 Cookie"),
        "PC000000" => Some("成功"),
        "AS021999" => Some("内容数据异常（多为 referer 不匹配或请求参数不被接受）"),
        _ => None,
    }
}

/// 统一格式化虎扑接口的业务错误，保证 `code` / `internalCode` / `msg` 三个字段都不会丢。
///
/// 排查时最常见的情况是 `code=0` 但 `msg` 已写明「用户未登录」，只报 code 等于没报。
pub fn format_api_error(
    label: &str,
    code: impl std::fmt::Display,
    internal_code: Option<&str>,
    msg: Option<&str>,
) -> String {
    let mut out = format!("{}失败: code={}", label, code);
    if let Some(ic) = internal_code.filter(|s| !s.is_empty()) {
        out.push_str(&format!(", internalCode={}", ic));
        if let Some(hint) = describe_internal_code(ic) {
            out.push_str(&format!("（{}）", hint));
        }
    }
    if let Some(m) = msg.filter(|s| !s.is_empty()) {
        out.push_str(&format!(", msg={}", m));
    }
    out
}

/// 当接口返回的不是 JSON（WAF 挑战页、登录跳转页、网关错误页）时，给出可排查的摘要。
///
/// 直接跑 `resp.json()` 只会得到 "expected value at line 1 column 1"，
/// 看不出到底是没登录、被 WAF 拦了还是网关挂了。
pub fn summarize_unparsable_response(
    label: &str,
    http_status: u16,
    body: &str,
    parse_error: &str,
) -> String {
    let head_lower = body.chars().take(2048).collect::<String>().to_lowercase();

    let cause = if head_lower.contains("aliyun_waf") || head_lower.contains("acw_sc__v") {
        "疑似阿里云 WAF 拦截页"
    } else if head_lower.contains("cf-chl") || body.contains("Just a moment") {
        "疑似 Cloudflare 人机校验页"
    } else if http_status == 401 || http_status == 403 {
        "HTTP 401/403，登录态或权限不足"
    } else if head_lower.trim_start().starts_with("<!doctype") || head_lower.trim_start().starts_with("<html") {
        "返回的是 HTML 页面而非 JSON，通常是登录跳转或被反爬拦截"
    } else if body.trim().is_empty() {
        "响应体为空"
    } else {
        "响应不是预期的 JSON 结构，接口可能已变更"
    };

    let snippet: String = body.trim().chars().take(200).collect();
    format!(
        "{}失败: {} (HTTP {}, 解析错误: {}) | 响应前 200 字符: {}",
        label, cause, http_status, parse_error, snippet
    )
}

/// Safe string slicing — takes the first `max_chars` characters (not bytes).
/// Returns the original string if it's shorter. Never panics on UTF-8.
#[allow(dead_code)]
pub fn safe_take_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((pos, _)) => &s[..pos],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_removes_tags() {
        assert_eq!(strip_html("<p>Hello</p>"), "Hello");
        assert_eq!(strip_html("<div><span>text</span></div>"), "text");
        assert_eq!(strip_html("<a href='x'>click</a>"), "click");
    }

    #[test]
    fn strip_html_removes_hupu_markers() {
        assert_eq!(strip_html("hello[图片]"), "hello");
        assert_eq!(strip_html("[视频]world"), "world");
        assert_eq!(strip_html("[图片]mid[视频]"), "mid");
    }

    #[test]
    fn strip_html_trims_whitespace() {
        assert_eq!(strip_html("  hello  "), "hello");
        assert_eq!(strip_html("<p>  hello  </p>"), "hello");
    }

    #[test]
    fn strip_html_handles_empty() {
        assert_eq!(strip_html(""), "");
        assert_eq!(strip_html("[图片][视频]"), "");
    }

    #[test]
    fn strip_html_preserves_normal_text() {
        assert_eq!(strip_html("hello world"), "hello world");
    }

    #[test]
    fn decode_unicode_escapes() {
        assert_eq!(decode_json_escapes("\\u003cdiv\\u003e"), "<div>");
    }

    #[test]
    fn decode_newlines_and_cr() {
        assert_eq!(decode_json_escapes("a\\nb\\rc"), "a\nbc");
    }

    #[test]
    fn decode_no_escapes_passthrough() {
        assert_eq!(decode_json_escapes("plain text"), "plain text");
    }

    #[test]
    fn decode_empty() {
        assert_eq!(decode_json_escapes(""), "");
    }

    #[test]
    fn find_brace_simple_object() {
        assert_eq!(find_matching_brace("{}", 0), Some(1));
    }

    #[test]
    fn find_brace_nested() {
        let json = r#"{"a":{"b":1}}"#;
        assert_eq!(find_matching_brace(json, 0), Some(json.len() - 1));
    }

    #[test]
    fn find_brace_with_strings_containing_braces() {
        let json = r#"{"key":"value with { and } inside"}"#;
        assert_eq!(find_matching_brace(json, 0), Some(json.len() - 1));
    }

    #[test]
    fn find_brace_with_escaped_quotes() {
        let json = r#"{"key":"value with \" escaped"}"#;
        assert_eq!(find_matching_brace(json, 0), Some(json.len() - 1));
    }

    #[test]
    fn find_brace_substring_start() {
        let json = r#"prefix {"inner":1} suffix"#;
        assert_eq!(find_matching_brace(json, 7), Some(17));
    }

    #[test]
    fn find_brace_bad_start() {
        assert_eq!(find_matching_brace("abc", 0), None);
    }

    #[test]
    fn find_brace_unclosed_returns_none() {
        assert_eq!(find_matching_brace("{", 0), None);
    }

    #[test]
    fn find_brace_array_inside_object() {
        let json = r#"{"arr":[1,2,3]}"#;
        assert_eq!(find_matching_brace(json, 0), Some(json.len() - 1));
    }

    #[test]
    fn sanitize_escapes_newlines_in_strings() {
        let input = "{\"key\":\"line1\nline2\"}";
        let output = sanitize_json(input);
        assert!(output.contains("\\n"));
        assert!(!output.contains('\n'));
    }

    #[test]
    fn sanitize_escapes_tabs_in_strings() {
        let input = "{\"key\":\"col1\tcol2\"}";
        let output = sanitize_json(input);
        assert!(output.contains("\\t"));
        assert!(!output.contains('\t'));
    }

    #[test]
    fn sanitize_escapes_carriage_return_in_strings() {
        let input = "{\"key\":\"before\rafter\"}";
        let output = sanitize_json(input);
        assert!(output.contains("\\r"));
        assert!(!output.contains('\r'));
    }

    #[test]
    fn sanitize_control_chars_to_unicode() {
        let input = "{\"key\":\"\x01\"}";
        let output = sanitize_json(input);
        assert!(output.contains("\\u0001"));
    }

    #[test]
    fn sanitize_keeps_normally_escaped_newlines() {
        let input = r#"{"key":"line1\\nline2"}"#;
        let output = sanitize_json(input);
        assert!(output.contains(r"\\n"));
    }

    #[test]
    fn sanitize_preserves_valid_json() {
        let input = r#"{"key":"value","num":42}"#;
        assert_eq!(sanitize_json(input), input);
    }

    #[test]
    fn sanitize_does_not_escape_outside_strings() {
        let input = "{\n  \"key\": \"value\"\n}";
        let output = sanitize_json(input);
        assert!(output.contains('\n'));
    }

    #[test]
    fn sanitize_empty() {
        assert_eq!(sanitize_json(""), "");
    }

    #[test]
    fn repair_closes_unterminated_string() {
        let input = r#"{"key":"value"#;
        let output = repair_truncated_json(input);
        assert_eq!(output, r#"{"key":"value"}"#);
    }

    #[test]
    fn repair_closes_unclosed_brace() {
        let input = r#"{"key":"value""#;
        let output = repair_truncated_json(input);
        assert!(output.ends_with('}'));
    }

    #[test]
    fn repair_closes_unclosed_bracket() {
        let input = r#"["a","b""#;
        let output = repair_truncated_json(input);
        assert!(output.ends_with(']'));
    }

    #[test]
    fn repair_closes_nested_unclosed() {
        let input = r#"{"a":{"b":"c"}"#;
        let output = repair_truncated_json(input);
        assert_eq!(output, r#"{"a":{"b":"c"}}"#);
    }

    #[test]
    fn repair_closes_string_then_brace() {
        let input = r#"{"key":"incomplete"#;
        let output = repair_truncated_json(input);
        assert!(output.ends_with('}'));
        assert!(output.contains(r#""incomplete""#));
    }

    #[test]
    fn repair_leaves_complete_json_unchanged() {
        let input = r#"{"key":"value"}"#;
        assert_eq!(repair_truncated_json(input), input);
    }

    #[test]
    fn repair_handles_array_nested_in_object() {
        let input = r#"{"arr":["a","b"#;
        let output = repair_truncated_json(input);
        assert!(output.ends_with('}'));
    }

    #[test]
    fn repair_empty() {
        assert_eq!(repair_truncated_json(""), "");
    }

    #[test]
    fn describe_known_internal_codes() {
        assert!(describe_internal_code("PC022003").unwrap().contains("登录"));
        assert!(describe_internal_code("PC000000").is_some());
        assert!(describe_internal_code("AS021999").is_some());
        assert!(describe_internal_code("XX_UNKNOWN").is_none());
    }

    #[test]
    fn api_error_keeps_all_fields() {
        let e = format_api_error("获取发帖", 0, Some("PC022003"), Some("用户未登录"));
        assert!(e.contains("code=0"), "{}", e);
        assert!(e.contains("internalCode=PC022003"), "{}", e);
        assert!(e.contains("msg=用户未登录"), "{}", e);
        assert!(e.contains("登录态失效"), "{}", e);
    }

    #[test]
    fn api_error_handles_missing_fields() {
        let e = format_api_error("获取发帖", 0, None, None);
        assert_eq!(e, "获取发帖失败: code=0");

        let e = format_api_error("获取发帖", 0, Some(""), Some(""));
        assert_eq!(e, "获取发帖失败: code=0");
    }

    #[test]
    fn api_error_unknown_code_has_no_hint() {
        let e = format_api_error("获取发帖", 0, Some("XX_UNKNOWN"), None);
        assert_eq!(e, "获取发帖失败: code=0, internalCode=XX_UNKNOWN");
    }

    #[test]
    fn unparsable_html_mentions_reason_and_status() {
        let e = summarize_unparsable_response(
            "获取发帖",
            200,
            "<!DOCTYPE html><html><body>login</body></html>",
            "expected value at line 1 column 1",
        );
        assert!(e.contains("获取发帖失败"), "{}", e);
        assert!(e.contains("HTML"), "{}", e);
        assert!(e.contains("HTTP 200"), "{}", e);
        assert!(e.contains("expected value"), "{}", e);
        assert!(e.contains("<!DOCTYPE"), "{}", e);
    }

    #[test]
    fn unparsable_detects_waf_and_forbidden() {
        let e = summarize_unparsable_response("获取发帖", 200, "<script>aliyun_waf_aa</script>", "err");
        assert!(e.contains("WAF"), "{}", e);

        let e = summarize_unparsable_response("获取发帖", 403, "{}", "err");
        assert!(e.contains("403"), "{}", e);
    }

    #[test]
    fn unparsable_handles_empty_body() {
        let e = summarize_unparsable_response("获取发帖", 502, "", "EOF");
        assert!(e.contains("响应体为空"), "{}", e);
        assert!(e.contains("HTTP 502"), "{}", e);
    }
}