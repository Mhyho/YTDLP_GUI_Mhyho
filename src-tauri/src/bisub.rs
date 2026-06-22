//! 双语字幕：下载主/副两条字幕 → 按时间轴合并成带样式的 ASS
//! （主字幕在上、大；副字幕在下、小）→ 用 ffmpeg 混流为可开关的软字幕轨。

use std::path::{Path, PathBuf};

use tauri::AppHandle;
use tokio::process::Command;

use crate::engine;
use crate::settings;
use crate::tools;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct Cue {
    start: u64, // ms
    end: u64,
    text: String,
}

/// 主流程
pub async fn run(
    app: &AppHandle,
    url: &str,
    video_path: &str,
    main_lang: &str,
    sec_lang: &str,
    cfg: &settings::Settings,
) -> Result<(), String> {
    let base = std::env::temp_dir().join(format!("aerodl_bisub_{}", crate::db::now()));
    let main_dir = base.join("m");
    let sec_dir = base.join("s");

    let main_srt = fetch_sub(app, url, main_lang, &main_dir, cfg)
        .await?
        .ok_or("主字幕下载失败（该语言可能不可用）")?;
    let sec_srt = fetch_sub(app, url, sec_lang, &sec_dir, cfg)
        .await?
        .ok_or("副字幕下载失败（该语言可能不可用）")?;

    let main_cues = parse_srt(&std::fs::read_to_string(&main_srt).map_err(|e| e.to_string())?);
    let sec_cues = parse_srt(&std::fs::read_to_string(&sec_srt).map_err(|e| e.to_string())?);
    if main_cues.is_empty() {
        return Err("主字幕为空".into());
    }

    let ass = merge_to_ass(&main_cues, &sec_cues);
    let ass_path = base.join("bilingual.ass");
    std::fs::write(&ass_path, ass).map_err(|e| e.to_string())?;

    mux_subtitle(app, Path::new(video_path), &ass_path).await?;

    let _ = std::fs::remove_dir_all(&base);
    Ok(())
}

/// 用 yt-dlp 下载某语言字幕到目录（人工/自动/机翻均可），转 srt，返回该 srt 路径
async fn fetch_sub(
    app: &AppHandle,
    url: &str,
    lang: &str,
    dir: &Path,
    cfg: &settings::Settings,
) -> Result<Option<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let exe = tools::resolve_ytdlp(app).ok_or("yt-dlp 尚未安装")?;

    let out_tmpl = format!("subtitle:{}/sub.%(ext)s", dir.to_string_lossy());
    let mut cmd = engine::ytdlp_cmd(&exe);
    cmd.args([
        "--skip-download",
        "--write-subs",
        "--write-auto-subs",
        "--sub-langs",
        lang,
        "--convert-subs",
        "srt",
        "--no-playlist",
        "--no-warnings",
        "-o",
        &out_tmpl,
    ]);
    engine::apply_cookies(&mut cmd, cfg);
    engine::apply_js_runtime(&mut cmd, app);
    cmd.arg(url);

    let out = cmd.output().await.map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }

    // 目录里找到的第一个 .srt 即为结果
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.extension().and_then(|e| e.to_str()) == Some("srt") {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// 解析 SRT
fn parse_srt(content: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    for block in normalized.split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        // 找到含 "-->" 的时间行
        let ts_idx = lines.iter().position(|l| l.contains("-->"));
        let Some(ts_idx) = ts_idx else { continue };
        let ts = lines[ts_idx];
        let parts: Vec<&str> = ts.split("-->").collect();
        if parts.len() != 2 {
            continue;
        }
        let (Some(start), Some(end)) = (parse_ts(parts[0]), parse_ts(parts[1])) else {
            continue;
        };
        let text = lines[ts_idx + 1..].join(" ").trim().to_string();
        if text.is_empty() {
            continue;
        }
        cues.push(Cue { start, end, text });
    }
    cues
}

/// "HH:MM:SS,mmm" / "HH:MM:SS.mmm" → 毫秒
fn parse_ts(s: &str) -> Option<u64> {
    let s = s.trim().replace(',', ".");
    let (hms, ms) = s.split_once('.').unwrap_or((s.as_str(), "0"));
    let parts: Vec<&str> = hms.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<u64>().ok()?, m.parse::<u64>().ok()?, s.parse::<u64>().ok()?),
        [m, s] => (0, m.parse::<u64>().ok()?, s.parse::<u64>().ok()?),
        _ => return None,
    };
    let ms: u64 = format!("{:0<3}", ms).chars().take(3).collect::<String>().parse().ok()?;
    Some(((h * 3600 + m * 60 + sec) * 1000) + ms)
}

fn ass_ts(ms: u64) -> String {
    let cs = (ms % 1000) / 10;
    let total_s = ms / 1000;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    format!("{h}:{m:02}:{s:02}.{cs:02}")
}

fn ass_escape(t: &str) -> String {
    t.replace('\n', " ").replace('{', "(").replace('}', ")")
}

/// 以主字幕时间轴为基准，叠加时间重叠的副字幕，生成带样式 ASS
fn merge_to_ass(main: &[Cue], sec: &[Cue]) -> String {
    let mut out = String::new();
    out.push_str(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: 384\nPlayResY: 288\nWrapStyle: 2\n\n\
         [V4+ Styles]\n\
         Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Default,Arial,28,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,14,1\n\n\
         [Events]\n\
         Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );

    for mc in main {
        // 收集时间重叠的副字幕
        let sec_text: String = sec
            .iter()
            .filter(|sc| sc.start < mc.end && sc.end > mc.start)
            .map(|sc| sc.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        let line = if sec_text.trim().is_empty() {
            format!("{{\\fs28}}{}", ass_escape(&mc.text))
        } else {
            // 主字幕大(上) + 换行 + 副字幕小(下)
            format!(
                "{{\\fs28}}{}\\N{{\\fs18}}{}",
                ass_escape(&mc.text),
                ass_escape(&sec_text)
            )
        };
        out.push_str(&format!(
            "Dialogue: 0,{},{},Default,,0,0,0,,{}\n",
            ass_ts(mc.start),
            ass_ts(mc.end),
            line
        ));
    }
    out
}

/// ffmpeg 把 ASS 作为软字幕轨混流进视频（不重编码画面/音频）
async fn mux_subtitle(app: &AppHandle, video: &Path, ass: &Path) -> Result<(), String> {
    let ffmpeg = tools::resolve_ffmpeg(app).ok_or("需要 ffmpeg 才能合成双语字幕")?;
    let out = video.with_extension("bisub.mkv");

    let mut std_cmd = std::process::Command::new(&ffmpeg);
    #[cfg(windows)]
    std_cmd.creation_flags(CREATE_NO_WINDOW);
    let mut cmd = Command::from(std_cmd);
    cmd.args([
        "-y",
        "-i",
        &video.to_string_lossy(),
        "-i",
        &ass.to_string_lossy(),
        "-map",
        "0",
        "-map",
        "1",
        "-c",
        "copy",
        "-metadata:s:s:0",
        "title=双语字幕",
        &out.to_string_lossy(),
    ]);

    let res = cmd.output().await.map_err(|e| e.to_string())?;
    if !res.status.success() {
        let _ = std::fs::remove_file(&out);
        return Err(String::from_utf8_lossy(&res.stderr).trim().to_string());
    }

    // 用合成后的文件替换原视频
    std::fs::remove_file(video).map_err(|e| e.to_string())?;
    std::fs::rename(&out, video).map_err(|e| e.to_string())?;
    Ok(())
}
