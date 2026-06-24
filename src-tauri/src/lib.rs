mod bisub;
mod db;
mod engine;
mod queue;
mod settings;
mod tools;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use queue::AppState;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Semaphore;

#[cfg(target_os = "windows")]
use window_vibrancy::{apply_acrylic, apply_mica, clear_acrylic, clear_blur, clear_mica};

// ---------- 窗口材质 ----------

#[tauri::command]
fn set_backdrop(window: tauri::WebviewWindow, kind: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let _ = clear_mica(&window);
        let _ = clear_acrylic(&window);
        let _ = clear_blur(&window);

        let is_dark = matches!(window.theme(), Ok(tauri::Theme::Dark));

        match kind.as_str() {
            "mica" => apply_mica(&window, None).map_err(|e| e.to_string())?,
            "acrylic" => {
                let tint = if is_dark { (10, 12, 18, 95) } else { (250, 250, 252, 95) };
                apply_acrylic(&window, Some(tint)).map_err(|e| e.to_string())?
            }
            // Aero：浅色 Acrylic（Win11 上能真实模糊桌面、不闪烁），发亮通透的玻璃感
            "aero" => {
                apply_acrylic(&window, Some((224, 235, 252, 120))).map_err(|e| e.to_string())?
            }
            "none" => {}
            other => return Err(format!("未知的背景材质: {other}")),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (&window, &kind);
    }
    Ok(())
}

// ---------- 工具（yt-dlp / ffmpeg）----------

#[tauri::command]
async fn ensure_ytdlp(app: AppHandle) -> Result<String, String> {
    tools::ensure_ytdlp(&app).await?;
    engine::version(&app).await
}

#[tauri::command]
async fn update_ytdlp(app: AppHandle) -> Result<String, String> {
    tools::update_ytdlp(&app).await?;
    engine::version(&app).await
}

#[tauri::command]
async fn ensure_ffmpeg(app: AppHandle) -> Result<bool, String> {
    tools::ensure_ffmpeg(&app).await?;
    Ok(true)
}

#[tauri::command]
async fn ensure_aria2c(app: AppHandle) -> Result<bool, String> {
    tools::ensure_aria2c(&app).await?;
    Ok(true)
}

#[tauri::command]
fn uninstall_aria2c(app: AppHandle) -> Result<(), String> {
    tools::uninstall_aria2c(&app)
}

#[tauri::command]
async fn ensure_deno(app: AppHandle) -> Result<bool, String> {
    tools::ensure_deno(&app).await?;
    Ok(true)
}

#[tauri::command]
fn uninstall_deno(app: AppHandle) -> Result<(), String> {
    tools::uninstall_deno(&app)
}

#[tauri::command]
async fn ytdlp_version(app: AppHandle) -> Result<String, String> {
    engine::version(&app).await
}

#[tauri::command]
fn ffmpeg_ready(app: AppHandle) -> Result<bool, String> {
    Ok(tools::resolve_ffmpeg(&app).is_some())
}

#[derive(serde::Serialize)]
struct ToolStatus {
    ytdlp_ready: bool,
    ytdlp_version: String,
    ytdlp_path: String,
    ytdlp_system: bool,
    ytdlp_bundled: bool,
    ffmpeg_ready: bool,
    ffmpeg_path: String,
    ffmpeg_system: bool,
    ffmpeg_bundled: bool,
    aria2c_ready: bool,
    aria2c_path: String,
    aria2c_system: bool,
    aria2c_bundled: bool,
    js_ready: bool,
    js_runtime: String,
    js_path: String,
    deno_bundled: bool,
}

#[tauri::command]
async fn tool_status(app: AppHandle) -> Result<ToolStatus, String> {
    let bundled_ytdlp = tools::ytdlp_path(&app).ok();
    let ytdlp = tools::resolve_ytdlp(&app);
    let (ytdlp_ready, ytdlp_version, ytdlp_path, ytdlp_system) = match &ytdlp {
        Some(p) => {
            let is_system = bundled_ytdlp.as_deref() != Some(p.as_path());
            let ver = engine::version(&app).await.unwrap_or_default();
            (true, ver, p.to_string_lossy().to_string(), is_system)
        }
        None => (false, String::new(), String::new(), false),
    };

    let bundled_ffmpeg = tools::ffmpeg_path(&app).ok();
    let ffmpeg = tools::resolve_ffmpeg(&app);
    let (ffmpeg_ready, ffmpeg_path, ffmpeg_system) = match &ffmpeg {
        Some(p) => {
            let is_system = bundled_ffmpeg.as_deref() != Some(p.as_path());
            (true, p.to_string_lossy().to_string(), is_system)
        }
        None => (false, String::new(), false),
    };

    let js = tools::resolve_js_runtime(&app);

    let bundled_aria2c = tools::aria2c_path(&app).ok();
    let aria2c = tools::resolve_aria2c(&app);
    let (aria2c_ready, aria2c_path, aria2c_system) = match &aria2c {
        Some(p) => {
            let is_system = bundled_aria2c.as_deref() != Some(p.as_path());
            (true, p.to_string_lossy().to_string(), is_system)
        }
        None => (false, String::new(), false),
    };

    Ok(ToolStatus {
        ytdlp_ready,
        ytdlp_version,
        ytdlp_path,
        ytdlp_system,
        ytdlp_bundled: tools::bundled_ytdlp_exists(&app),
        ffmpeg_ready,
        ffmpeg_path,
        ffmpeg_system,
        ffmpeg_bundled: tools::bundled_ffmpeg_exists(&app),
        aria2c_ready,
        aria2c_path,
        aria2c_system,
        aria2c_bundled: tools::bundled_aria2c_exists(&app),
        js_ready: js.is_some(),
        js_runtime: js.as_ref().map(|(n, _)| n.clone()).unwrap_or_default(),
        js_path: js.as_ref().map(|(_, p)| p.to_string_lossy().to_string()).unwrap_or_default(),
        deno_bundled: tools::bundled_deno_exists(&app),
    })
}

// ---------- 设置 ----------

#[tauri::command]
fn get_settings(app: AppHandle) -> settings::Settings {
    settings::load(&app)
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: settings::Settings) -> Result<(), String> {
    settings::save(&app, &settings)
}

#[tauri::command]
fn uninstall_ytdlp(app: AppHandle) -> Result<(), String> {
    tools::uninstall_ytdlp(&app)
}

#[tauri::command]
fn uninstall_ffmpeg(app: AppHandle) -> Result<(), String> {
    tools::uninstall_ffmpeg(&app)
}

// ---------- 解析 ----------

#[tauri::command]
async fn fetch_info(app: AppHandle, url: String) -> Result<engine::MediaInfo, String> {
    engine::fetch_info(&app, &url).await
}

// ---------- 队列 / 历史 ----------

#[tauri::command]
fn enqueue_download(
    app: AppHandle,
    url: String,
    title: String,
    format: String,
    out_dir: String,
    thumbnail: String,
    options: String,
) -> Result<String, String> {
    queue::enqueue(&app, url, title, format, out_dir, thumbnail, options)
}

#[tauri::command]
fn list_downloads(app: AppHandle) -> Result<Vec<db::DownloadItem>, String> {
    queue::list(&app)
}

#[tauri::command]
fn cancel_download(app: AppHandle, id: String) -> Result<(), String> {
    queue::cancel(&app, &id)
}

#[tauri::command]
fn retry_download(app: AppHandle, id: String) -> Result<(), String> {
    queue::retry(&app, &id)
}

#[tauri::command]
fn remove_download(app: AppHandle, id: String) -> Result<(), String> {
    queue::remove(&app, &id)
}

#[tauri::command]
fn clear_finished(app: AppHandle) -> Result<(), String> {
    queue::clear_finished(&app)
}

// ---------- 通用 ----------

#[tauri::command]
fn default_download_dir(app: AppHandle) -> Result<String, String> {
    let p = app
        .path()
        .download_dir()
        .map_err(|e| format!("无法获取下载目录: {e}"))?;
    Ok(p.to_string_lossy().to_string())
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    #[cfg(windows)]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";

    std::process::Command::new(program)
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();

            // 初始化数据库 + 清理上次残留的运行中任务
            let conn = db::open(&handle).expect("无法打开数据库");
            let _ = db::reset_stale(&conn);

            let cfg = settings::load(&handle);
            let concurrent = cfg.max_concurrent.clamp(1, 10);

            app.manage(AppState {
                db: Mutex::new(conn),
                sem: Arc::new(Semaphore::new(concurrent)),
                running: Mutex::new(HashMap::new()),
            });

            let window = app.get_webview_window("main").unwrap();
            #[cfg(target_os = "windows")]
            {
                let _ = apply_mica(&window, None);
            }

            // 每日首次启动：自动更新自带的 yt-dlp（它更新最频繁）
            let h = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let today = db::now() / 86400;
                if tools::bundled_ytdlp_exists(&h) && settings::last_update_day(&h) != today {
                    let _ = h.emit(
                        "tool-progress",
                        tools::ToolProgress {
                            tool: "yt-dlp".into(),
                            percent: 0,
                            stage: "每日自动更新 yt-dlp…".into(),
                        },
                    );
                    if tools::update_ytdlp(&h).await.is_ok() {
                        settings::set_last_update_day(&h, today);
                    }
                    let _ = h.emit("tools-updated", ());
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_backdrop,
            ensure_ytdlp,
            update_ytdlp,
            ensure_ffmpeg,
            ensure_aria2c,
            uninstall_aria2c,
            ensure_deno,
            uninstall_deno,
            ytdlp_version,
            ffmpeg_ready,
            tool_status,
            uninstall_ytdlp,
            uninstall_ffmpeg,
            get_settings,
            save_settings,
            fetch_info,
            enqueue_download,
            list_downloads,
            cancel_download,
            retry_download,
            remove_download,
            clear_finished,
            default_download_dir,
            open_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
