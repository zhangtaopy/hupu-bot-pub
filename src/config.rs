fn default_concurrency() -> usize { 3 }

use anyhow::{bail, Result};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::RwLock;

use crate::deepseek::AiProvider;

#[derive(Deserialize, Serialize, Debug, Clone)]
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
    /// Ollama Cloud API key（可选，用于替代 DeepSeek）
    #[serde(default)]
    pub ollama_api_key: String,
    /// Ollama Cloud 模型名（可选，默认 gpt-oss:120b）
    #[serde(default)]
    pub ollama_model: String,
    /// DeepSeek 模型名（可选，默认 deepseek-v4-flash）
    #[serde(default)]
    pub deepseek_model: String,
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
        // puid 不是启动 serve 的硬性条件，解析失败时保持空值
        if config.puid.is_empty() {
            config.puid = Self::parse_puid_from_cookie(&config.cookie)
                .unwrap_or_default();
        }

        Ok(config)
    }

    /// 从 cookie 中的 u 字段解析 puid
    /// cookie 中的 u 字段格式: u=113152029|TDJlcHNpbG9u|...
    pub fn parse_puid_from_cookie(cookie: &str) -> Result<String> {
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
    pub fn parse_smid_from_cookie(cookie: &str) -> Result<String> {
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

/// Try to get config instance, returns None if not initialized
pub fn try_get() -> Option<Config> {
    let guard = CONFIG.read().unwrap();
    guard.clone()
}

/// Check if config has been initialized (currently unused but kept for API completeness)
#[allow(dead_code)]
pub fn is_initialized() -> bool {
    let guard = CONFIG.read().unwrap();
    guard.is_some()
}

/// Initialize config without failing — used by the Serve command
/// so the web server can start even without a valid config
pub fn init_optional() {
    match Config::load() {
        Ok(config) => {
            let mut guard = CONFIG.write().unwrap();
            *guard = Some(config);
        }
        Err(_) => {
            // Leave CONFIG as None — server can still start
        }
    }
}

impl Config {
    /// 根据配置创建 AI provider。优先使用 Ollama Cloud，否则使用 DeepSeek。
    pub fn ai_provider(&self) -> AiProvider {
        if !self.ollama_api_key.is_empty() {
            AiProvider::Ollama {
                api_key: self.ollama_api_key.clone(),
                model: if self.ollama_model.is_empty() {
                    "gpt-oss:120b".to_string()
                } else {
                    self.ollama_model.clone()
                },
            }
        } else {
            AiProvider::DeepSeek {
                api_key: self.deepseek_api_key.clone(),
                model: if self.deepseek_model.is_empty() {
                    "deepseek-v4-flash".to_string()
                } else {
                    self.deepseek_model.clone()
                },
            }
        }
    }
}

/// Save config to disk and update the global instance
pub fn save(config: &Config) -> Result<()> {
    let path = std::path::Path::new("config.json");
    let json = serde_json::to_string_pretty(config)?;
    fs::write(path, json)?;
    let mut guard = CONFIG.write().unwrap();
    *guard = Some(config.clone());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn parse_puid_from_cookie_normal() {
        let cookie = "u=113152029|TDJlcHNpbG9u|...; other=value";
        let result = Config::parse_puid_from_cookie(cookie).unwrap();
        assert_eq!(result, "113152029");
    }

    #[test]
    fn parse_puid_from_cookie_no_pipe() {
        let cookie = "u=123456; other=value";
        let result = Config::parse_puid_from_cookie(cookie).unwrap();
        assert_eq!(result, "123456");
    }

    #[test]
    fn parse_puid_from_cookie_missing_u_field() {
        let cookie = "other=value; no=uid";
        let result = Config::parse_puid_from_cookie(cookie);
        assert!(result.is_err());
    }

    #[test]
    fn parse_puid_from_cookie_empty_cookie() {
        let result = Config::parse_puid_from_cookie("");
        assert!(result.is_err());
    }

    #[test]
    fn parse_smid_from_cookie_normal() {
        let cookie = "smidV2=abc123def456; other=value";
        let result = Config::parse_smid_from_cookie(cookie).unwrap();
        assert_eq!(result, "abc123def456");
    }

    #[test]
    fn parse_smid_from_cookie_empty_value() {
        let cookie = "smidV2=; other=value";
        let result = Config::parse_smid_from_cookie(cookie);
        assert!(result.is_err());
    }

    #[test]
    fn parse_smid_from_cookie_missing_field() {
        let cookie = "other=value; no=smid";
        let result = Config::parse_smid_from_cookie(cookie);
        assert!(result.is_err());
    }

    #[test]
    fn parse_smid_from_cookie_empty_cookie() {
        let result = Config::parse_smid_from_cookie("");
        assert!(result.is_err());
    }
}