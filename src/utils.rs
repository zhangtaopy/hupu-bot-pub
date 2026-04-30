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
    let mut result = String::with_capacity(s.len());
    let mut in_string = false;
    let mut escape = false;

    for ch in s.chars() {
        if escape {
            escape = false;
            result.push(ch);
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            result.push('\\');
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            result.push('"');
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
}