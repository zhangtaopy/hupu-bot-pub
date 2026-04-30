use anyhow::{bail, Result};
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};

use crate::client::HupuClient;

const BASE_URL: &str = "https://bbs.hupu.com";

/// 点赞请求体
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LikeBody<'a> {
    tid: &'a str,
    pid: &'a str,
    puid: &'a str,
    fid: &'a str,
    shumei_id: &'a str,
    deviceid: &'a str,
}

/// 回复帖子的请求体（字段来自抓包）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateReplyBody<'a> {
    /// 板块 ID（抓包中为 "278"）
    topic_id: &'a str,
    /// 回复正文，虎扑使用 HTML 格式，纯文本用 <p>内容</p> 包裹即可
    content: String,
    /// 数美设备指纹，从 cookie .thumbcache_* 的值获得，固定值
    shumei_id: &'a str,
    /// 同上，与 shumei_id 相同
    deviceid: &'a str,
    /// 帖子 ID（URL 中的数字，如 638074156）
    tid: &'a str,
    /// 引用回复ID（被引用发言的 pid），可选
    #[serde(skip_serializing_if = "Option::is_none")]
    quote_id: Option<&'a str>,
}

#[derive(Deserialize, Debug)]
struct ApiResponse {
    code: Option<i64>,
    #[serde(alias = "msg", alias = "message")]
    msg: Option<String>,
}

/// 点赞回复
///
/// # 参数
/// - `client`    : 已构建的 HupuClient
/// - `tid`       : 帖子 ID，如 "638074156"
/// - `pid`       : 回复 ID
/// - `puid`      : 用户 ID（从 cookie 中的 u 字段解析）
/// - `fid`       : 板块 ID，如 "278"（汽车区）
/// - `shumei_id` : 数美设备指纹
pub async fn like(
    client: &HupuClient,
    tid: &str,
    pid: &str,
    puid: &str,
    fid: &str,
    shumei_id: &str,
) -> Result<()> {
    let url = format!("{}/pcmapi/pc/bbs/v1/reply/light", BASE_URL);
    let referer = format!("{}/{}-1.html", BASE_URL, tid);

    let body = LikeBody {
        tid,
        pid,
        puid,
        fid,
        shumei_id,
        deviceid: shumei_id,
    };

    let resp = client
        .client
        .post(&url)
        .header("referer", HeaderValue::from_str(&referer)?)
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let resp_text = resp.text().await?;

    let api: ApiResponse = match serde_json::from_str(&resp_text) {
        Ok(a) => a,
        Err(e) => bail!("JSON解析失败: {} | 原始响应: {}", e, resp_text),
    };

    if !status.is_success() {
        bail!("HTTP {}: {:?}", status, api.msg);
    }

    match api.code {
        Some(1) => println!("✅ 点赞成功"),
        Some(0) => {
            match api.msg.as_deref() {
                Some("你已经点亮过这个回帖了") => println!("⚠️ 已经点赞过了"),
                Some(msg) => println!("⚠️ {}", msg),
                None => println!("⚠️ 操作未执行"),
            }
        }
        Some(code) => bail!("接口错误 code={}: {:?}", code, api.msg),
        None => bail!("响应无 code 字段: {:?}", api.msg),
    }

    Ok(())
}

/// 取消点赞
pub async fn unlight(
    client: &HupuClient,
    tid: &str,
    pid: &str,
    puid: &str,
    fid: &str,
    shumei_id: &str,
) -> Result<()> {
    let url = format!("{}/pcmapi/pc/bbs/v1/reply/cancelLight", BASE_URL);
    let referer = format!("{}/{}-1.html", BASE_URL, tid);

    let body = LikeBody {
        tid,
        pid,
        puid,
        fid,
        shumei_id,
        deviceid: shumei_id,
    };

    let resp = client
        .client
        .post(&url)
        .header("referer", HeaderValue::from_str(&referer)?)
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let resp_text = resp.text().await?;

    let api: ApiResponse = match serde_json::from_str(&resp_text) {
        Ok(a) => a,
        Err(e) => bail!("JSON解析失败: {} | 原始响应: {}", e, resp_text),
    };

    if !status.is_success() {
        bail!("HTTP {}: {:?}", status, api.msg);
    }

    match api.code {
        Some(1) => println!("✅ 取消点赞成功"),
        Some(0) => {
            match api.msg.as_deref() {
                Some("你还没有点亮过这个回帖") => println!("⚠️ 你还没点赞过"),
                Some(msg) => println!("⚠️ {}", msg),
                None => println!("⚠️ 操作未执行"),
            }
        }
        Some(code) => bail!("接口错误 code={}: {:?}", code, api.msg),
        None => bail!("响应无 code 字段: {:?}", api.msg),
    }

    Ok(())
}

/// 回复帖子
///
/// # 参数
/// - `client`    : 已构建的 HupuClient
/// - `topic_id`  : 板块 ID，如 "278"
/// - `tid`       : 帖子 ID，如 "638074156"
/// - `text`      : 纯文本内容（函数内部会包装为 HTML）
/// - `shumei_id` : 数美设备指纹（从你的 cookie thumbcache 值复制）
/// - `quote_id`  : 引用回复ID（被@消息的 pid），可选
pub async fn reply(
    client: &HupuClient,
    topic_id: &str,
    tid: &str,
    text: &str,
    shumei_id: &str,
    quote_id: Option<&str>,
) -> Result<()> {
    let url = format!("{}/pcmapi/pc/bbs/v1/createReply", BASE_URL);
    let referer = format!("{}/{}-1.html", BASE_URL, tid);

    // 虎扑内容用 <p> 包裹
    let content = format!("<p>{}</p>", text);

    let body = CreateReplyBody {
        topic_id,
        content,
        shumei_id,
        deviceid: shumei_id, // 两个字段值相同
        tid,
        quote_id,
    };

    let resp = client
        .client
        .post(&url)
        .header("referer", HeaderValue::from_str(&referer)?)
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let resp_text = resp.text().await?;

    let api: ApiResponse = match serde_json::from_str(&resp_text) {
        Ok(a) => a,
        Err(e) => bail!("JSON解析失败: {} | 原始响应: {}", e, resp_text),
    };

    if !status.is_success() {
        bail!("HTTP {}: {:?}", status, api.msg);
    }

    match api.code {
        Some(0) | Some(1) => println!("✅ 回复成功"),
        Some(code) => bail!("接口错误 code={}: {:?}", code, api.msg),
        None => bail!("响应无 code 字段: {:?}", api.msg),
    }

    Ok(())
}
