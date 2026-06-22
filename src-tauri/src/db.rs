//! 持久化层：用 SQLite 保存下载历史/队列。对应 YTDLnis 的 Room 数据库。
//! 只负责存取，不含业务逻辑。

use rusqlite::{params, Connection};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

/// 一条下载记录（队列项 / 历史项共用）
#[derive(Serialize, Clone)]
pub struct DownloadItem {
    pub id: String,
    pub url: String,
    pub title: String,
    pub format: String,
    pub out_dir: String,
    pub filepath: String,
    pub status: String, // queued | running | completed | failed | cancelled
    pub error: String,
    pub thumbnail: String,
    pub created_at: i64,
    pub options: String, // 下载选项的 JSON（缩略图/字幕/章节/元数据/额外参数等）
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 打开数据库（位于应用数据目录），建表
pub fn open(app: &AppHandle) -> Result<Connection, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析应用数据目录: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let conn = Connection::open(dir.join("aerodl.db")).map_err(|e| e.to_string())?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS downloads (
            id          TEXT PRIMARY KEY,
            url         TEXT NOT NULL,
            title       TEXT NOT NULL DEFAULT '',
            format      TEXT NOT NULL DEFAULT '',
            out_dir     TEXT NOT NULL DEFAULT '',
            filepath    TEXT NOT NULL DEFAULT '',
            status      TEXT NOT NULL DEFAULT 'queued',
            error       TEXT NOT NULL DEFAULT '',
            thumbnail   TEXT NOT NULL DEFAULT '',
            created_at  INTEGER NOT NULL DEFAULT 0,
            options     TEXT NOT NULL DEFAULT '{}'
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    // 迁移：为旧库补 options 列（已存在则忽略报错）
    let _ = conn.execute(
        "ALTER TABLE downloads ADD COLUMN options TEXT NOT NULL DEFAULT '{}'",
        [],
    );
    Ok(conn)
}

pub fn insert(conn: &Connection, item: &DownloadItem) -> Result<(), String> {
    conn.execute(
        "INSERT INTO downloads
            (id, url, title, format, out_dir, filepath, status, error, thumbnail, created_at, options)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            item.id, item.url, item.title, item.format, item.out_dir, item.filepath,
            item.status, item.error, item.thumbnail, item.created_at, item.options
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn set_status(conn: &Connection, id: &str, status: &str, error: Option<&str>) -> Result<(), String> {
    conn.execute(
        "UPDATE downloads SET status=?1, error=?2 WHERE id=?3",
        params![status, error.unwrap_or(""), id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn set_completed(conn: &Connection, id: &str, filepath: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE downloads SET status='completed', error='', filepath=?1 WHERE id=?2",
        params![filepath, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn status_of(conn: &Connection, id: &str) -> Option<String> {
    conn.query_row("SELECT status FROM downloads WHERE id=?1", params![id], |r| {
        r.get::<_, String>(0)
    })
    .ok()
}

pub fn get(conn: &Connection, id: &str) -> Option<DownloadItem> {
    conn.query_row(
        "SELECT id,url,title,format,out_dir,filepath,status,error,thumbnail,created_at,options
         FROM downloads WHERE id=?1",
        params![id],
        row_to_item,
    )
    .ok()
}

pub fn list(conn: &Connection) -> Result<Vec<DownloadItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id,url,title,format,out_dir,filepath,status,error,thumbnail,created_at
             FROM downloads ORDER BY created_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], row_to_item)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn delete(conn: &Connection, id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM downloads WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn clear_finished(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "DELETE FROM downloads WHERE status IN ('completed','failed','cancelled')",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 启动时把上次残留的 running/queued 标记为中断（任务不会跨重启存活）
pub fn reset_stale(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "UPDATE downloads SET status='failed', error='应用关闭导致中断'
         WHERE status IN ('running','queued')",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn row_to_item(r: &rusqlite::Row) -> rusqlite::Result<DownloadItem> {
    Ok(DownloadItem {
        id: r.get(0)?,
        url: r.get(1)?,
        title: r.get(2)?,
        format: r.get(3)?,
        out_dir: r.get(4)?,
        filepath: r.get(5)?,
        status: r.get(6)?,
        error: r.get(7)?,
        thumbnail: r.get(8)?,
        created_at: r.get(9)?,
        options: r.get(10)?,
    })
}
