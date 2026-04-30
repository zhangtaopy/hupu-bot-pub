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