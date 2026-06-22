//! 工具管理层：定位 / 下载 yt-dlp.exe 与 ffmpeg(.exe)。
//!
//! 设计原则（对应 YTDLnis 的 YTDLUpdater + packages）：不 fork yt-dlp，
//! 而是把官方发布的可执行文件下载到应用数据目录，需要时可再拉最新版替换。
//! 与 UI 完全解耦，只负责"文件在哪、怎么拿到"。

use std::path::PathBuf;

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 在系统 PATH 中查找可执行文件（Windows: where）
pub fn which(exe: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("where")
            .arg(exe)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout);
        s.lines()
            .next()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        let out = std::process::Command::new("which").arg(exe).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout);
        s.lines()
            .next()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
    }
}

/// 可执行文件名：Windows 加 .exe，其它平台不加
fn bin(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// 给下载的二进制赋可执行权限（Unix 必需）
fn make_executable(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perm = meta.permissions();
            perm.set_mode(0o755);
            let _ = std::fs::set_permissions(path, perm);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// 解析最终使用的 yt-dlp：优先应用自带（便于版本管理），其次系统 PATH
pub fn resolve_ytdlp(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(p) = ytdlp_path(app) {
        if p.exists() {
            return Some(p);
        }
    }
    which("yt-dlp")
}

/// 解析最终使用的 ffmpeg：优先系统 PATH（省空间，ffmpeg 版本无所谓），其次应用自带
pub fn resolve_ffmpeg(app: &AppHandle) -> Option<PathBuf> {
    if let Some(p) = which("ffmpeg") {
        return Some(p);
    }
    if let Ok(p) = ffmpeg_path(app) {
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// 解析最终使用的 aria2c：优先系统 PATH，其次应用自带
pub fn resolve_aria2c(app: &AppHandle) -> Option<PathBuf> {
    if let Some(p) = which("aria2c") {
        return Some(p);
    }
    if let Ok(p) = aria2c_path(app) {
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// 解析最终使用的 deno：优先系统 PATH，其次应用自带
pub fn resolve_deno(app: &AppHandle) -> Option<PathBuf> {
    if let Some(p) = which("deno") {
        return Some(p);
    }
    if let Ok(p) = deno_path(app) {
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// 解析可用的 JS 运行时（给 yt-dlp 解 nsig / 处理 PO Token）。
/// 优先系统 node（多数机器已装、无需下载），其次 deno（系统或自带）。
/// 返回 (yt-dlp 运行时名, 可执行路径)。
pub fn resolve_js_runtime(app: &AppHandle) -> Option<(String, PathBuf)> {
    if let Some(p) = which("node") {
        return Some(("node".into(), p));
    }
    if let Some(p) = resolve_deno(app) {
        return Some(("deno".into(), p));
    }
    None
}

/// 应用自带的副本是否存在（用于界面显示"清理自带"）
pub fn bundled_ytdlp_exists(app: &AppHandle) -> bool {
    ytdlp_path(app).map(|p| p.exists()).unwrap_or(false)
}
pub fn bundled_ffmpeg_exists(app: &AppHandle) -> bool {
    ffmpeg_path(app).map(|p| p.exists()).unwrap_or(false)
}
pub fn bundled_aria2c_exists(app: &AppHandle) -> bool {
    aria2c_path(app).map(|p| p.exists()).unwrap_or(false)
}
pub fn bundled_deno_exists(app: &AppHandle) -> bool {
    deno_path(app).map(|p| p.exists()).unwrap_or(false)
}

/// 删除应用自带的 yt-dlp
pub fn uninstall_ytdlp(app: &AppHandle) -> Result<(), String> {
    let p = ytdlp_path(app)?;
    if p.exists() {
        std::fs::remove_file(&p).map_err(|e| e.to_string())?;
    }
    Ok(())
}
/// 删除应用自带的 ffmpeg + ffprobe
pub fn uninstall_ffmpeg(app: &AppHandle) -> Result<(), String> {
    let dir = tools_dir(app)?;
    for n in [bin("ffmpeg"), bin("ffprobe")] {
        let p = dir.join(n);
        if p.exists() {
            std::fs::remove_file(&p).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
/// 删除应用自带的 aria2c
pub fn uninstall_aria2c(app: &AppHandle) -> Result<(), String> {
    let p = aria2c_path(app)?;
    if p.exists() {
        std::fs::remove_file(&p).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 删除应用自带的 deno
pub fn uninstall_deno(app: &AppHandle) -> Result<(), String> {
    let p = deno_path(app)?;
    if p.exists() {
        std::fs::remove_file(&p).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 确保有 JS 运行时可用：系统已有 node/deno 则直接用，否则下载 deno
pub async fn ensure_deno(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some((_, p)) = resolve_js_runtime(app) {
        return Ok(p);
    }
    let dir = tools_dir(app)?;
    let zip_path = dir.join("deno_tmp.zip");
    download_with_progress(app, DENO_ZIP_URL, &zip_path, "deno", "下载 deno").await?;

    let _ = app.emit(
        "tool-progress",
        ToolProgress { tool: "deno".into(), percent: 100, stage: "解压 deno".into() },
    );

    let dir_clone = dir.clone();
    let zip_clone = zip_path.clone();
    let deno_name = bin("deno");
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let f = std::fs::File::open(&zip_clone).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().replace('\\', "/");
            let base = name.rsplit('/').next().unwrap_or("");
            if base == "deno" || base == "deno.exe" {
                let out = dir_clone.join(&deno_name);
                let mut out_file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
                std::io::copy(&mut entry, &mut out_file).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;

    let _ = std::fs::remove_file(&zip_path);
    let exe = deno_path(app)?;
    if !exe.exists() {
        return Err("解压后未找到 deno".into());
    }
    make_executable(&exe);
    Ok(exe)
}

/// 确保 aria2c 可用（系统已装则用，否则从 aria2 官方 GitHub release 下载 win-64bit 版）
pub async fn ensure_aria2c(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(p) = resolve_aria2c(app) {
        return Ok(p);
    }
    #[cfg(not(windows))]
    {
        return Err("请用包管理器安装 aria2（如 sudo dnf install aria2）".into());
    }
    #[cfg(windows)]
    {
    // 查询最新 release，找 win-64bit 的 zip 资源
    let client = reqwest::Client::builder()
        .user_agent("aerodl")
        .build()
        .map_err(|e| e.to_string())?;
    let txt = client
        .get("https://api.github.com/repos/aria2/aria2/releases/latest")
        .send()
        .await
        .map_err(|e| format!("查询 aria2 release 失败: {e}"))?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let rel: serde_json::Value =
        serde_json::from_str(&txt).map_err(|e| format!("解析 release JSON 失败: {e}"))?;
    let assets = rel
        .get("assets")
        .and_then(|a| a.as_array())
        .ok_or("release 无 assets 字段")?;
    let dl = assets
        .iter()
        .find(|a| {
            a.get("name")
                .and_then(|n| n.as_str())
                .map(|n| n.contains("win-64bit"))
                .unwrap_or(false)
        })
        .and_then(|a| a.get("browser_download_url"))
        .and_then(|u| u.as_str())
        .ok_or("未找到 win-64bit 资源")?
        .to_string();

    let dir = tools_dir(app)?;
    let zip_path = dir.join("aria2_tmp.zip");
    download_with_progress(app, &dl, &zip_path, "aria2c", "下载 aria2c").await?;

    let _ = app.emit(
        "tool-progress",
        ToolProgress {
            tool: "aria2c".into(),
            percent: 100,
            stage: "解压 aria2c".into(),
        },
    );

    let dir_clone = dir.clone();
    let zip_clone = zip_path.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let f = std::fs::File::open(&zip_clone).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().replace('\\', "/");
            if name.ends_with("/aria2c.exe") || name == "aria2c.exe" {
                let out = dir_clone.join("aria2c.exe");
                let mut out_file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
                std::io::copy(&mut entry, &mut out_file).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;

    let _ = std::fs::remove_file(&zip_path);

    let exe = aria2c_path(app)?;
    if !exe.exists() {
        return Err("解压后未找到 aria2c.exe".into());
    }
    Ok(exe)
    }
}

#[cfg(windows)]
const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
#[cfg(not(windows))]
const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux";

#[cfg(windows)]
const FFMPEG_ZIP_URL: &str =
    "https://github.com/yt-dlp/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip";

#[cfg(windows)]
const DENO_ZIP_URL: &str =
    "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip";
#[cfg(not(windows))]
const DENO_ZIP_URL: &str =
    "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-unknown-linux-gnu.zip";

/// 下载进度事件载荷（发给前端）
#[derive(Clone, Serialize)]
pub struct ToolProgress {
    pub tool: String,
    pub percent: u64,
    pub stage: String,
}

/// 应用数据下的 tools 目录（自动创建）
pub fn tools_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析应用数据目录: {e}"))?
        .join("tools");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn ytdlp_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(tools_dir(app)?.join(bin("yt-dlp")))
}

pub fn ffmpeg_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(tools_dir(app)?.join(bin("ffmpeg")))
}

pub fn aria2c_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(tools_dir(app)?.join(bin("aria2c")))
}

pub fn deno_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(tools_dir(app)?.join(bin("deno")))
}

/// 流式下载到目标文件，按字节进度发 `tool-progress` 事件
async fn download_with_progress(
    app: &AppHandle,
    url: &str,
    dest: &std::path::Path,
    tool: &str,
    stage: &str,
) -> Result<(), String> {
    let resp = reqwest::get(url)
        .await
        .map_err(|e| format!("请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("下载失败: {e}"))?;

    let total = resp.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut last_pct: u64 = u64::MAX;

    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| format!("创建文件失败: {e}"))?;
    let mut stream = resp.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("传输中断: {e}"))?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;

        let pct = if total > 0 { downloaded * 100 / total } else { 0 };
        if pct != last_pct {
            last_pct = pct;
            let _ = app.emit(
                "tool-progress",
                ToolProgress {
                    tool: tool.to_string(),
                    percent: pct,
                    stage: stage.to_string(),
                },
            );
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 确保 yt-dlp 可用（系统已装则直接用，否则下载自带版），返回其路径
pub async fn ensure_ytdlp(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(p) = resolve_ytdlp(app) {
        return Ok(p);
    }
    let path = ytdlp_path(app)?;
    download_with_progress(app, YTDLP_URL, &path, "yt-dlp", "下载 yt-dlp").await?;
    make_executable(&path);
    Ok(path)
}

/// 强制更新 yt-dlp 到最新版（覆盖）
pub async fn update_ytdlp(app: &AppHandle) -> Result<PathBuf, String> {
    let path = ytdlp_path(app)?;
    download_with_progress(app, YTDLP_URL, &path, "yt-dlp", "更新 yt-dlp").await?;
    make_executable(&path);
    Ok(path)
}

/// 确保 ffmpeg 可用（系统已装则直接用，否则下载官方静态构建 zip 并解压）
pub async fn ensure_ffmpeg(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(p) = resolve_ffmpeg(app) {
        return Ok(p);
    }
    #[cfg(not(windows))]
    {
        return Err("请用包管理器安装 ffmpeg（如 sudo dnf install ffmpeg）".into());
    }
    #[cfg(windows)]
    {
    let ffmpeg = ffmpeg_path(app)?;
    let dir = tools_dir(app)?;
    let zip_path = dir.join("ffmpeg_tmp.zip");
    download_with_progress(app, FFMPEG_ZIP_URL, &zip_path, "ffmpeg", "下载 ffmpeg").await?;

    // 解压：从 zip 中抽出 bin/ffmpeg.exe 与 bin/ffprobe.exe，平铺到 tools 目录
    let _ = app.emit(
        "tool-progress",
        ToolProgress {
            tool: "ffmpeg".into(),
            percent: 100,
            stage: "解压 ffmpeg".into(),
        },
    );

    let dir_clone = dir.clone();
    let zip_clone = zip_path.clone();
    // zip crate 是同步的，放到阻塞线程里做
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let f = std::fs::File::open(&zip_clone).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().replace('\\', "/");
            let wanted = name.ends_with("bin/ffmpeg.exe") || name.ends_with("bin/ffprobe.exe");
            if !wanted {
                continue;
            }
            let filename = name.rsplit('/').next().unwrap_or("").to_string();
            let out = dir_clone.join(filename);
            let mut out_file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out_file).map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;

    let _ = std::fs::remove_file(&zip_path);

    if !ffmpeg.exists() {
        return Err("解压后未找到 ffmpeg.exe".into());
    }
    Ok(ffmpeg)
    }
}
