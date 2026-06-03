use anyhow::Result;
use reqwest::{
    header::{self, HeaderMap, HeaderValue},
    Client,
};

pub struct HupuClient {
    pub client: Client,
}

impl HupuClient {
    pub fn new(cookie: &str) -> Result<Self> {
        let mut headers = HeaderMap::new();

        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(cookie)?,
        );
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("*/*"),
        );
        headers.insert(
            header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("zh-CN,zh;q=0.9,en;q=0.8"),
        );
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://bbs.hupu.com"),
        );
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                 AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/145.0.0.0 Safari/537.36 Edg/145.0.0.0",
            ),
        );
        // sec-* headers
        headers.insert("sec-ch-ua", HeaderValue::from_static(
            r#""Not:A-Brand";v="99", "Microsoft Edge";v="145", "Chromium";v="145""#,
        ));
        headers.insert("sec-ch-ua-mobile", HeaderValue::from_static("?0"));
        headers.insert("sec-ch-ua-platform", HeaderValue::from_static("\"Windows\""));
        headers.insert("sec-fetch-dest", HeaderValue::from_static("empty"));
        headers.insert("sec-fetch-mode", HeaderValue::from_static("cors"));
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        headers.insert("priority", HeaderValue::from_static("u=1, i"));

        let client = Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()?;

        Ok(Self { client })
    }
}
