fn default_concurrency() -> usize { 3 }

use anyhow::{bail, Result};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::fs;
use std::sync::RwLock;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub cookie: String,
    /// 数美设备指纹，从 cookie 中的 smidV2 字段自动解析
    #[serde(skip)]
    pub shumei_id: String,
    /// 用户 ID，可从 cookie 中的 u 字段自动解析
    #[serde(default)]
    pub puid: String,
    /// DeepSeek API key，用于 AI 分析（可选）
    #[serde(default)]
    pub deepseek_api_key: String,
    /// DeepSeek API 最大并发数
    #[serde(default = "default_concurrency")]
    pub deepseek_max_concurrency: usize,
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = std::path::Path::new("config.json");
        if !path.exists() {
            bail!(
                "config.json not found!\n\
                Please copy config.example.json to config.json and fill in your credentials:\n\
                cp config.example.json config.json"
            );
        }

        let content = fs::read_to_string(path)?;
        let mut config: Config = serde_json::from_str(&content)?;

        if config.cookie.is_empty() {
            bail!("cookie is empty in config.json");
        }

        // 从 cookie 中的 smidV2 字段解析 shumei_id
        config.shumei_id = Self::parse_smid_from_cookie(&config.cookie)?;

        // 如果 puid 为空，从 cookie 中自动解析
        if config.puid.is_empty() {
            config.puid = Self::parse_puid_from_cookie(&config.cookie)?;
        }

        Ok(config)
    }

    /// 从 cookie 中的 u 字段解析 puid
    /// cookie 中的 u 字段格式: u=113152029|TDJlcHNpbG9u|...
    fn parse_puid_from_cookie(cookie: &str) -> Result<String> {
        for part in cookie.split(';') {
            let part = part.trim();
            if part.starts_with("u=") {
                let value = &part[2..];
                // 取第一个 | 之前的部分作为 puid
                if let Some(idx) = value.find('|') {
                    return Ok(value[..idx].to_string());
                }
                return Ok(value.to_string());
            }
        }
        bail!("无法从 cookie 中解析 puid，请手动在 config.json 中添加 puid 字段");
    }

    /// 从 cookie 中的 smidV2 字段解析数美设备指纹
    fn parse_smid_from_cookie(cookie: &str) -> Result<String> {
        for part in cookie.split(';') {
            let part = part.trim();
            if part.starts_with("smidV2=") {
                let value = &part[7..];
                if !value.is_empty() {
                    return Ok(value.to_string());
                }
            }
        }
        bail!("无法从 cookie 中解析 smidV2（数美设备指纹）");
    }
}

// Global config instance
static CONFIG: Lazy<RwLock<Option<Config>>> = Lazy::new(|| RwLock::new(None));

/// Initialize and load config
pub fn init() -> Result<()> {
    let config = Config::load()?;
    let mut guard = CONFIG.write().unwrap();
    *guard = Some(config);
    Ok(())
}

/// Get config instance
pub fn get() -> Config {
    let guard = CONFIG.read().unwrap();
    guard.as_ref().expect("Config not initialized").clone()
}