use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::time::Duration;
use tokio::time::sleep;

const CDP_HTTP: &str = "http://localhost:9222";
const DEBUG_PORT: u16 = 9222;

/// Hardcoded fallback paths for Windows (used when registry lookup fails)
#[cfg(windows)]
const WINDOWS_BROWSER_PATHS: &[(&str, &[&str])] = &[
    ("Chrome", &[
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ]),
    ("Edge", &[
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ]),
];

/// Standard browser paths on macOS
#[cfg(target_os = "macos")]
const MACOS_BROWSER_PATHS: &[(&str, &[&str])] = &[
    ("Chrome", &[
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ]),
    ("Edge", &[
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ]),
];

pub async fn run() -> Result<()> {
    // Step 1: try already-running debug browser
    if let Ok(cookie) = try_extract_cookies().await {
        save_config(&cookie)?;
        return Ok(());
    }

    // Step 2: find and launch browser with debug port
    println!("未检测到浏览器调试端口，正在查找浏览器...");
    let (browser_name, browser_path) = find_browser()?;

    // Kill background browser processes so the debug port can open
    #[cfg(any(windows, target_os = "macos"))]
    kill_browser_processes(browser_name);

    println!("正在启动 {} 并打开虎扑，请登录后等待自动提取...", browser_name);
    launch_browser(&browser_path)?;

    // Step 3: wait for CDP to become available
    println!("等待浏览器启动...");
    wait_for_cdp(Duration::from_secs(120)).await?;

    // Step 4: wait for user to login (smidV2 appears in cookies)
    println!("等待登录完成...");
    let cookie = wait_for_login(Duration::from_secs(300)).await?;

    save_config(&cookie)?;
    println!("Cookie 已自动保存到 config.json");
    Ok(())
}

async fn try_extract_cookies() -> Result<String> {
    let targets = get_cdp_targets().await?;
    let ws_url = find_ws_url(&targets)?;
    get_cookies_via_cdp(&ws_url).await
}

async fn get_cdp_targets() -> Result<Vec<Value>> {
    let resp = reqwest::get(format!("{}/json", CDP_HTTP))
        .await
        .context("无法连接到浏览器调试端口")?;
    let targets: Vec<Value> = resp.json().await?;
    Ok(targets)
}

fn find_ws_url(targets: &[Value]) -> Result<String> {
    // Prefer a hupu page
    for t in targets {
        if let Some(url) = t["url"].as_str() {
            if url.contains("hupu") {
                if let Some(ws) = t["webSocketDebuggerUrl"].as_str() {
                    return Ok(ws.to_string());
                }
            }
        }
    }
    // Fallback to first available target
    targets
        .first()
        .and_then(|t| t["webSocketDebuggerUrl"].as_str())
        .map(|s| s.to_string())
        .context("未找到可用的调试目标")
}

async fn get_cookies_via_cdp(ws_url: &str) -> Result<String> {
    let (ws, _) = tokio_tungstenite::connect_async(ws_url).await?;
    let (mut write, mut read) = ws.split();

    let cmd = serde_json::json!({
        "id": 1,
        "method": "Network.getCookies",
        "params": {
            "urls": ["https://bbs.hupu.com", "https://hupu.com"]
        }
    });

    write
        .send(tokio_tungstenite::tungstenite::Message::Text(cmd.to_string().into()))
        .await?;

    while let Some(msg) = read.next().await {
        let msg = msg?;
        if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
            let resp: Value = serde_json::from_str(&text)?;
            if resp["id"] == 1 {
                let cookies = resp["result"]["cookies"]
                    .as_array()
                    .context("CDP 返回格式异常")?;
                if cookies.is_empty() {
                    bail!("浏览器中未找到虎扑 Cookie，请先在浏览器中登录虎扑");
                }
                return Ok(format_cookies(cookies));
            }
        }
    }

    bail!("CDP 连接已关闭，未收到响应")
}

fn format_cookies(cookies: &[Value]) -> String {
    cookies
        .iter()
        .map(|c| {
            let name = c["name"].as_str().unwrap_or("");
            let value = c["value"].as_str().unwrap_or("");
            format!("{}={}", name, value)
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn find_browser() -> Result<(&'static str, String)> {
    // Windows: try registry first, then fall back to hardcoded paths
    #[cfg(windows)]
    if let Some(result) = find_browser_via_registry() {
        return Ok(result);
    }

    // macOS: use standard paths
    #[cfg(target_os = "macos")]
    for (name, paths) in MACOS_BROWSER_PATHS {
        for path in *paths {
            if std::path::Path::new(path).exists() {
                return Ok((name, path.to_string()));
            }
        }
    }

    // Windows fallback: hardcoded paths
    #[cfg(windows)]
    for (name, paths) in WINDOWS_BROWSER_PATHS {
        for path in *paths {
            if std::path::Path::new(path).exists() {
                return Ok((name, path.to_string()));
            }
        }
    }

    bail!("未找到 Chrome 或 Edge 浏览器，请手动在 config.json 中填写 cookie");
}

/// Query Windows registry for browser install paths
#[cfg(windows)]
fn find_browser_via_registry() -> Option<(&'static str, String)> {
    use winreg::enums::*;
    let app_paths: &[(&str, &str)] = &[
        ("Chrome", "chrome.exe"),
        ("Edge", "msedge.exe"),
    ];
    for (name, exe) in app_paths {
        let key_path =
            format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{}", exe);
        if let Ok(path) = get_registry_app_path(HKEY_LOCAL_MACHINE, &key_path) {
            if std::path::Path::new(&path).exists() {
                return Some((name, path));
            }
        }
        if let Ok(path) = get_registry_app_path(HKEY_CURRENT_USER, &key_path) {
            if std::path::Path::new(&path).exists() {
                return Some((name, path));
            }
        }
    }
    None
}

#[cfg(windows)]
fn get_registry_app_path(hive: winreg::HKEY, key_path: &str) -> Result<String> {
    use winreg::enums::*;
    use winreg::RegKey;
    let key = RegKey::predef(hive).open_subkey_with_flags(key_path, KEY_READ)?;
    Ok(key.get_value("")?)
}

/// Kill existing browser processes so the debug port can be opened.
#[cfg(any(windows, target_os = "macos"))]
fn kill_browser_processes(browser_name: &str) {
    use std::io::{self, Write};

    let proc_name = browser_process_name(browser_name);

    let processes = find_running_browser_processes(browser_name);
    if processes.is_empty() {
        return;
    }

    println!();
    println!(
        "{} 浏览器进程正在后台运行（{} 个实例）。",
        browser_name,
        processes.len()
    );
    println!("  浏览器后台进程会阻止调试端口开启，需要先关闭。");
    print!("  关闭现有 {} 进程？[y/N] ", proc_name);
    io::stdout().flush().ok();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return;
    }

    if input.trim().to_lowercase() != "y" {
        println!("  已跳过，尝试继续...");
        return;
    }

    println!("  正在关闭浏览器进程...");
    kill_browser_process(proc_name);
    // Give the OS time to fully clean up
    std::thread::sleep(Duration::from_secs(2));
}

#[cfg(windows)]
fn browser_process_name(browser_name: &str) -> &str {
    if browser_name == "Edge" { "msedge.exe" } else { "chrome.exe" }
}

#[cfg(target_os = "macos")]
fn browser_process_name(browser_name: &str) -> &str {
    if browser_name == "Edge" { "Microsoft Edge" } else { "Google Chrome" }
}

#[cfg(windows)]
fn kill_browser_process(proc_name: &str) {
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/IM", proc_name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(target_os = "macos")]
fn kill_browser_process(proc_name: &str) {
    let _ = std::process::Command::new("pkill")
        .args(["-f", proc_name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(windows)]
fn find_running_browser_processes(browser_name: &str) -> Vec<String> {
    let exe = browser_process_name(browser_name);
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {}", exe), "/FO", "CSV", "/NH"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout
                .lines()
                .filter(|line| line.contains(exe))
                .map(|s| s.to_string())
                .collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(target_os = "macos")]
fn find_running_browser_processes(browser_name: &str) -> Vec<String> {
    let proc_name = browser_process_name(browser_name);
    let output = std::process::Command::new("pgrep")
        .args(["-l", proc_name])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.lines().map(|s| s.to_string()).collect()
        }
        _ => Vec::new(),
    }
}

fn launch_browser(browser_path: &str) -> Result<()> {
    std::process::Command::new(browser_path)
        .arg(format!("--remote-debugging-port={}", DEBUG_PORT))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("https://bbs.hupu.com")
        .spawn()
        .context("无法启动浏览器，请关闭所有浏览器窗口后重试，或手动配置 config.json")?;
    Ok(())
}

async fn wait_for_cdp(timeout: Duration) -> Result<()> {
    let start = tokio::time::Instant::now();
    loop {
        if start.elapsed() > timeout {
            bail!(
                "等待浏览器启动超时。\n\
                 可能原因：已有浏览器窗口在运行（调试端口无法开启）。\n\
                 请关闭所有 Chrome/Edge 窗口后重试。"
            );
        }
        if get_cdp_targets().await.is_ok() {
            return Ok(());
        }
        sleep(Duration::from_secs(1)).await;
    }
}

async fn wait_for_login(timeout: Duration) -> Result<String> {
    let start: tokio::time::Instant = tokio::time::Instant::now();
    loop {
        if start.elapsed() > timeout {
            bail!("等待登录超时，请在浏览器中登录虎扑后重试");
        }
        if let Ok(cookie) = try_extract_cookies().await {
            if cookie.contains("smidV2=") {
                return Ok(cookie);
            }
        }
        sleep(Duration::from_secs(2)).await;
    }
}

fn save_config(cookie: &str) -> Result<()> {
    let puid = crate::config::Config::parse_puid_from_cookie(cookie).unwrap_or_default();
    let shumei_id = crate::config::Config::parse_smid_from_cookie(cookie)?;

    // Preserve existing deepseek config if config already exists
    let (deepseek_api_key, deepseek_max_concurrency) = match crate::config::try_get() {
        Some(existing) => (existing.deepseek_api_key, existing.deepseek_max_concurrency),
        None => (String::new(), 3),
    };

    let config = crate::config::Config {
        cookie: cookie.to_string(),
        shumei_id,
        puid,
        deepseek_api_key,
        deepseek_max_concurrency,
    };

    crate::config::save(&config)?;
    Ok(())
}
