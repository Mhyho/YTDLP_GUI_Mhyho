//! 下载队列层：并发调度 + 取消/重试/删除。对应 YTDLnis 的 WorkManager。
//!
//! 设计：用 tokio Semaphore 限制并发数——所有任务一入队即 spawn，但在信号量处
//! 排队等待，自然形成 FIFO 并发队列，无需手写调度器。取消 = abort 任务，
//! 配合命令的 kill_on_drop 杀掉 yt-dlp 子进程。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Semaphore;

use crate::{db, engine};

/// 全局应用状态（通过 app.manage 注入）
pub struct AppState {
    pub db: Mutex<Connection>,
    pub sem: Arc<Semaphore>,
    pub running: Mutex<HashMap<String, JoinHandle<()>>>,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn gen_id() -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}", db::now(), n)
}

fn emit_changed(app: &AppHandle) {
    let _ = app.emit("queue-changed", ());
}

/// 入队一个新下载，返回其 id
pub fn enqueue(
    app: &AppHandle,
    url: String,
    title: String,
    format: String,
    out_dir: String,
    thumbnail: String,
    options: String,
) -> Result<String, String> {
    let state = app.state::<AppState>();
    let id = gen_id();
    let item = db::DownloadItem {
        id: id.clone(),
        url,
        title,
        format,
        out_dir,
        filepath: String::new(),
        status: "queued".into(),
        error: String::new(),
        thumbnail,
        created_at: db::now(),
        options,
    };
    {
        let conn = state.db.lock().unwrap();
        db::insert(&conn, &item)?;
    }
    spawn_worker(app, &id);
    emit_changed(app);
    Ok(id)
}

/// 为某条记录启动后台下载任务（用于入队与重试）
fn spawn_worker(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    let item = {
        let conn = state.db.lock().unwrap();
        db::get(&conn, id)
    };
    let Some(item) = item else { return };

    let sem = state.sem.clone();
    let app2 = app.clone();
    let id2 = id.to_string();

    // 用 Tauri 托管运行时 spawn：同步命令调用时也有运行时句柄，避免 "no reactor" panic
    let join = tauri::async_runtime::spawn(async move {
        // 在信号量处排队，控制并发
        let _permit = match sem.acquire_owned().await {
            Ok(p) => p,
            Err(_) => return,
        };

        // 等到轮到自己时，确认仍是 queued（可能期间被取消/删除）
        {
            let st = app2.state::<AppState>();
            let conn = st.db.lock().unwrap();
            if db::status_of(&conn, &id2).as_deref() != Some("queued") {
                return;
            }
            let _ = db::set_status(&conn, &id2, "running", None);
        }
        emit_changed(&app2);

        let res = engine::download(&app2, &id2, &item.url, &item.format, &item.out_dir, &item.options).await;

        {
            let st = app2.state::<AppState>();
            let conn = st.db.lock().unwrap();
            match &res {
                Ok(path) => {
                    let _ = db::set_completed(&conn, &id2, path);
                }
                Err(e) => {
                    let _ = db::set_status(&conn, &id2, "failed", Some(e));
                }
            }
        }
        {
            let st = app2.state::<AppState>();
            st.running.lock().unwrap().remove(&id2);
        }
        emit_changed(&app2);
    });

    state
        .running
        .lock()
        .unwrap()
        .insert(id.to_string(), join);
}

pub fn cancel(app: &AppHandle, id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    if let Some(h) = state.running.lock().unwrap().remove(id) {
        h.abort();
    }
    {
        let conn = state.db.lock().unwrap();
        let _ = db::set_status(&conn, id, "cancelled", Some("已取消"));
    }
    emit_changed(app);
    Ok(())
}

pub fn retry(app: &AppHandle, id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    {
        let conn = state.db.lock().unwrap();
        db::set_status(&conn, id, "queued", None)?;
    }
    spawn_worker(app, id);
    emit_changed(app);
    Ok(())
}

pub fn remove(app: &AppHandle, id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    if let Some(h) = state.running.lock().unwrap().remove(id) {
        h.abort();
    }
    {
        let conn = state.db.lock().unwrap();
        db::delete(&conn, id)?;
    }
    emit_changed(app);
    Ok(())
}

pub fn list(app: &AppHandle) -> Result<Vec<db::DownloadItem>, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    db::list(&conn)
}

pub fn clear_finished(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    {
        let conn = state.db.lock().unwrap();
        db::clear_finished(&conn)?;
    }
    emit_changed(app);
    Ok(())
}
