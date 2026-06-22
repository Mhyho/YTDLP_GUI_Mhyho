//! 应用设置：持久化为 app_data_dir/settings.json。
//! 含 cookies 来源、是否用 aria2c、并发数等。与 UI 解耦，只管存取。

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    /// 同时进行的最大下载数（重启后生效）
    pub max_concurrent: usize,
    /// cookies 来源: "none" | "file" | "browser"
    pub cookies_mode: String,
    /// cookies.txt 路径（cookies_mode == "file" 时用）
    pub cookies_file: String,
    /// 从哪个浏览器读 cookie（cookies_mode == "browser" 时用）: edge|chrome|firefox|brave|...
    pub cookies_browser: String,
    /// 是否用 aria2c 作为下载器
    pub use_aria2c: bool,
    /// aria2c 并发连接数
    pub aria2c_connections: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_concurrent: 3,
            cookies_mode: "none".into(),
            cookies_file: String::new(),
            cookies_browser: "edge".into(),
            use_aria2c: false,
            aria2c_connections: 16,
        }
    }
}

fn settings_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析应用数据目录: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("settings.json"))
}

pub fn load(app: &AppHandle) -> Settings {
    let Ok(path) = settings_path(app) else {
        return Settings::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app)?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(())
}

// ---- 每日自动更新的状态（独立于用户设置，避免被前端保存覆盖）----

fn update_state_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("last_update_day.txt"))
}

/// 上次自动更新的"纪元日"（epoch 天数）
pub fn last_update_day(app: &AppHandle) -> i64 {
    update_state_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

pub fn set_last_update_day(app: &AppHandle, day: i64) {
    if let Some(p) = update_state_path(app) {
        let _ = std::fs::write(p, day.to_string());
    }
}
