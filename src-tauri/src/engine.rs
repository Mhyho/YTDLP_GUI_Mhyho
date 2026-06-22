//! 下载引擎层：调用 yt-dlp（配合 ffmpeg）完成版本查询、信息解析、带进度下载。
//! 只懂"进程与命令"，不碰窗口/UI。对应 YTDLnis 的 RuntimeManager + YTDLPUtil。

use std::path::Path;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::settings;
use crate::tools;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 构造一个隐藏控制台窗口的 yt-dlp 命令
pub(crate) fn ytdlp_cmd(exe: &Path) -> Command {
    let mut std_cmd = std::process::Command::new(exe);
    #[cfg(windows)]
    std_cmd.creation_flags(CREATE_NO_WINDOW);
    Command::from(std_cmd)
}

/// 返回 yt-dlp 版本号
pub async fn version(app: &AppHandle) -> Result<String, String> {
    let exe = tools::resolve_ytdlp(app).ok_or("yt-dlp 尚未安装")?;
    let output = ytdlp_cmd(&exe)
        .arg("--version")
        .output()
        .await
        .map_err(|e| format!("无法启动 yt-dlp: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// 返回给前端的精简格式信息
#[derive(Serialize)]
pub struct MediaFormat {
    pub format_id: String,
    pub ext: String,
    pub resolution: String,
    pub note: String,
    pub filesize: Option<u64>,
    pub vcodec: String,
    pub acodec: String,
    pub codec_label: String,  // 友好编码名：AV1 / HEVC / VP9 / H.264 / Opus / AAC ...
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub tbr: Option<f64>,     // 平均总码率 kbps
    pub quality: u32,         // 预计画质评分 0-100（编码效率 × 码率密度）
    pub hdr: bool,            // 是否 HDR
}

/// 可用字幕语言
#[derive(Serialize)]
pub struct SubLang {
    pub code: String,
    pub name: String,
    pub auto: bool, // 是否为自动生成/翻译
}

/// 返回给前端的精简媒体信息
#[derive(Serialize)]
pub struct MediaInfo {
    pub title: String,
    pub uploader: String,
    pub duration: Option<f64>,
    pub thumbnail: String,
    pub formats: Vec<MediaFormat>,
    pub subtitles: Vec<SubLang>,
}

/// 从 -J 结果提取可用字幕语言（人工 + 自动）
fn extract_subs(json: &serde_json::Value) -> Vec<SubLang> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (key, auto) in [("subtitles", false), ("automatic_captions", true)] {
        if let Some(obj) = json.get(key).and_then(|v| v.as_object()) {
            for (code, entries) in obj {
                let name = entries
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|e| e.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or(code)
                    .to_string();
                if seen.insert(format!("{auto}:{code}")) {
                    out.push(SubLang { code: code.clone(), name, auto });
                }
            }
        }
    }
    out
}

fn s(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// 把 yt-dlp 的 codec 串映射成友好名（视频优先，否则取音频）
fn codec_label(vcodec: &str, acodec: &str) -> String {
    let v = vcodec.to_lowercase();
    let vlabel = if v.starts_with("av01") || v.contains("av1") {
        "AV1"
    } else if v.starts_with("hev") || v.starts_with("hvc") || v.contains("hevc") || v.contains("h265") {
        "HEVC"
    } else if v.starts_with("vp9") || v.starts_with("vp09") {
        "VP9"
    } else if v.starts_with("vp8") || v.starts_with("vp08") {
        "VP8"
    } else if v.starts_with("avc") || v.contains("h264") {
        "H.264"
    } else {
        ""
    };
    if !vlabel.is_empty() {
        return vlabel.to_string();
    }

    let a = acodec.to_lowercase();
    if a.contains("opus") {
        "Opus".into()
    } else if a.contains("mp4a") || a.contains("aac") {
        "AAC".into()
    } else if a.contains("mp3") {
        "MP3".into()
    } else if a.contains("flac") {
        "FLAC".into()
    } else if a.contains("vorbis") {
        "Vorbis".into()
    } else if a.contains("ac-3") || a.contains("eac-3") || a.contains("ec-3") {
        "AC3".into()
    } else if !a.is_empty() && a != "none" {
        acodec.to_string()
    } else {
        "—".into()
    }
}

/// 编码效率系数：同码率下越高越清晰
fn codec_efficiency(vcodec: &str) -> f64 {
    let v = vcodec.to_lowercase();
    if v.starts_with("av01") || v.contains("av1") {
        1.5
    } else if v.starts_with("hev") || v.starts_with("hvc") || v.contains("hevc") || v.contains("h265") {
        1.3
    } else if v.starts_with("vp9") || v.starts_with("vp09") {
        1.2
    } else if v.starts_with("avc") || v.contains("h264") {
        1.0
    } else {
        1.0
    }
}

/// 某分辨率达到"高画质"所需的 H.264 参考码率（kbps）。
/// 取自 YouTube/Netflix 公布的推荐码率阶梯，按高度插值，高帧率上调。
fn reference_bitrate_kbps(height: i64, fps: f64) -> f64 {
    // (高度上限, H.264 高画质参考码率 kbps)
    // 校准到 YouTube 实际"高画质"码率档（H.264 等效），使优秀流落在 75-100
    let ladder = [
        (144, 170.0),
        (240, 430.0),
        (360, 850.0),
        (480, 1400.0),
        (720, 2200.0),
        (1080, 4500.0),
        (1440, 13000.0),
        (2160, 26000.0),
        (4320, 90000.0),
    ];
    let mut base = ladder.last().unwrap().1;
    for (h, b) in ladder {
        if height <= h {
            base = b;
            break;
        }
    }
    if fps > 35.0 {
        base *= 1.5; // 50/60fps 需要约 1.5 倍码率
    }
    base
}

/// 预计画质评分 0-100：实际码率 ÷ (该分辨率高画质参考码率 ÷ 编码效率)。
/// 含义 = "达到该分辨率‘优秀线’的百分之多少"，并对 AV1/HEVC/VP9 做效率折算。
fn quality_score(
    vcodec: &str,
    acodec: &str,
    height: Option<i64>,
    fps: Option<f64>,
    tbr: Option<f64>,
) -> u32 {
    let tbr = tbr.unwrap_or(0.0);
    if tbr <= 0.0 {
        return 0;
    }
    let has_video = !vcodec.is_empty() && vcodec != "none";
    if !has_video {
        // 纯音频：256kbps（AAC 高码率）约定为满分
        let has_audio = !acodec.is_empty() && acodec != "none";
        if !has_audio {
            return 0;
        }
        return ((tbr / 256.0) * 100.0).min(100.0).round() as u32;
    }

    let h = height.unwrap_or(0);
    if h <= 0 {
        return 0;
    }
    let fps = fps.unwrap_or(30.0);
    let eff = codec_efficiency(vcodec);
    // AV1/HEVC 更省码率，所需参考码率相应下调
    let needed = reference_bitrate_kbps(h, fps) / eff;
    ((tbr / needed) * 100.0).min(100.0).round() as u32
}

/// 用 yt-dlp -J 解析链接，抽取标题/格式等信息
pub async fn fetch_info(app: &AppHandle, url: &str) -> Result<MediaInfo, String> {
    let exe = tools::resolve_ytdlp(app).ok_or("yt-dlp 尚未安装")?;
    let cfg = settings::load(app);
    let mut cmd = ytdlp_cmd(&exe);
    cmd.args(["-J", "--no-playlist", "--no-warnings"]);
    apply_cookies(&mut cmd, &cfg);
    apply_js_runtime(&mut cmd, app);
    let output = cmd
        .arg(url)
        .output()
        .await
        .map_err(|e| format!("无法启动 yt-dlp: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("解析 JSON 失败: {e}"))?;

    let formats = json
        .get("formats")
        .and_then(|f| f.as_array())
        .map(|arr| {
            arr.iter()
                .map(|f| {
                    let vcodec = s(f, "vcodec");
                    let acodec = s(f, "acodec");
                    let height = f.get("height").and_then(|x| x.as_i64());
                    let fps = f.get("fps").and_then(|x| x.as_f64());
                    let tbr = f
                        .get("tbr")
                        .and_then(|x| x.as_f64())
                        .or_else(|| f.get("vbr").and_then(|x| x.as_f64()))
                        .or_else(|| f.get("abr").and_then(|x| x.as_f64()));
                    MediaFormat {
                        format_id: s(f, "format_id"),
                        ext: s(f, "ext"),
                        resolution: {
                            let r = s(f, "resolution");
                            if r.is_empty() { s(f, "format_note") } else { r }
                        },
                        note: s(f, "format_note"),
                        filesize: f
                            .get("filesize")
                            .and_then(|x| x.as_u64())
                            .or_else(|| f.get("filesize_approx").and_then(|x| x.as_u64())),
                        codec_label: codec_label(&vcodec, &acodec),
                        quality: quality_score(&vcodec, &acodec, height, fps, tbr),
                        hdr: {
                            let dr = s(f, "dynamic_range").to_uppercase();
                            dr.contains("HDR") || s(f, "format_note").to_uppercase().contains("HDR")
                        },
                        height,
                        fps,
                        tbr,
                        vcodec,
                        acodec,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(MediaInfo {
        title: s(&json, "title"),
        uploader: s(&json, "uploader"),
        duration: json.get("duration").and_then(|x| x.as_f64()),
        thumbnail: s(&json, "thumbnail"),
        subtitles: extract_subs(&json),
        formats,
    })
}

/// 下载进度事件载荷
#[derive(Clone, Serialize)]
pub struct DownloadProgress {
    pub id: String,
    pub percent: f64,
    pub speed: String,
    pub eta: String,
}

/// 下载日志事件载荷
#[derive(Clone, Serialize)]
pub struct DownloadLog {
    pub id: String,
    pub line: String,
}

/// 执行下载：流式解析进度，通过事件推送给前端
pub async fn download(
    app: &AppHandle,
    id: &str,
    url: &str,
    format: &str,
    out_dir: &str,
    options: &str,
) -> Result<String, String> {
    let cfg = settings::load(app);
    let dlopts: DownloadOptions = serde_json::from_str(options).unwrap_or_default();
    let exe = tools::resolve_ytdlp(app).ok_or("yt-dlp 尚未安装")?;
    let ffmpeg = tools::resolve_ffmpeg(app);

    let out_template = format!("{}/%(title)s.%(ext)s", out_dir.trim_end_matches(['/', '\\']));

    let mut cmd = ytdlp_cmd(&exe);
    cmd.args([
        "-f",
        format,
        "-o",
        &out_template,
        "--no-playlist",
        "--newline",
        "--no-warnings",
        // 抗 YouTube 大流限流 403：分块下载（遇 403 自动重取 URL）+ 重试
        "--http-chunk-size",
        "10M",
        "--retries",
        "10",
        "--fragment-retries",
        "10",
        // 机器可读进度，便于精确解析
        "--progress-template",
        "download:AERODL|%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s",
    ]);

    apply_cookies(&mut cmd, &cfg);

    let mut using_aria2c = false;
    if cfg.use_aria2c {
        if let Some(a) = tools::resolve_aria2c(app) {
            using_aria2c = true;
            let conns = cfg.aria2c_connections.max(1);
            cmd.args(["--downloader", &a.to_string_lossy()]);
            cmd.args([
                "--downloader-args",
                &format!(
                    "aria2c:-x{conns} -s{conns} -k1M --summary-interval=1 --console-log-level=warn"
                ),
            ]);
        }
    }

    apply_js_runtime(&mut cmd, app);

    for arg in build_option_args(&dlopts) {
        cmd.arg(arg);
    }

    if let Some(f) = &ffmpeg {
        cmd.args(["--ffmpeg-location", &f.to_string_lossy()]);
    }
    cmd.arg(url);

    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true); // 取消时 abort 任务即可连带杀掉 yt-dlp 子进程

    let mut child = cmd.spawn().map_err(|e| format!("启动下载失败: {e}"))?;
    let stdout = child.stdout.take().ok_or("无法获取 stdout")?;
    let stderr = child.stderr.take().ok_or("无法获取 stderr")?;

    // 后台逐行读 stderr：收集错误文本；若用 aria2c，顺便从中解析进度
    let app_err = app.clone();
    let id_err = id.to_string();
    let err_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut raw: Vec<u8> = Vec::new();
        let mut collected = String::new();
        loop {
            raw.clear();
            let n = match reader.read_until(b'\n', &mut raw).await {
                Ok(n) => n,
                Err(_) => break,
            };
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&raw);
            let line = line.trim_end_matches(['\n', '\r']);
            if using_aria2c {
                if let Some((pct, speed, eta)) = parse_aria2c(line) {
                    let _ = app_err.emit(
                        "download-progress",
                        DownloadProgress { id: id_err.clone(), percent: pct, speed, eta },
                    );
                }
            }
            collected.push_str(line);
            collected.push('\n');
        }
        collected
    });

    // 注意：yt-dlp 在中文 Windows 上的输出可能是系统码页/非 UTF-8，
    // 必须按字节读取再 lossy 解码，否则 .lines() 会因无效 UTF-8 直接报错中断。
    let mut reader = BufReader::new(stdout);
    let mut raw: Vec<u8> = Vec::new();
    let mut last_path = String::new();
    loop {
        raw.clear();
        let n = reader
            .read_until(b'\n', &mut raw)
            .await
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim_end_matches(['\n', '\r']).to_string();

        if let Some(rest) = line.strip_prefix("AERODL|") {
            let parts: Vec<&str> = rest.split('|').collect();
            let percent = parts
                .get(0)
                .map(|p| p.trim().trim_end_matches('%').trim())
                .and_then(|p| p.parse::<f64>().ok())
                .unwrap_or(0.0);
            let _ = app.emit(
                "download-progress",
                DownloadProgress {
                    id: id.to_string(),
                    percent,
                    speed: parts.get(1).map(|x| x.trim().to_string()).unwrap_or_default(),
                    eta: parts.get(2).map(|x| x.trim().to_string()).unwrap_or_default(),
                },
            );
        } else {
            if let Some(p) = parse_dest(&line) {
                last_path = p;
            }
            if using_aria2c {
                if let Some((pct, speed, eta)) = parse_aria2c(&line) {
                    let _ = app.emit(
                        "download-progress",
                        DownloadProgress { id: id.to_string(), percent: pct, speed, eta },
                    );
                }
            }
            let _ = app.emit(
                "download-log",
                DownloadLog {
                    id: id.to_string(),
                    line,
                },
            );
        }
    }

    let status = child.wait().await.map_err(|e| e.to_string())?;
    let errs = err_task.await.unwrap_or_default();

    if status.success() {
        // 双语字幕：下载后合成并混流
        if dlopts.bilingual_on() && !last_path.is_empty() {
            let _ = app.emit(
                "download-log",
                DownloadLog { id: id.to_string(), line: "正在合成双语字幕…".into() },
            );
            if let Err(e) = crate::bisub::run(
                app, url, &last_path, dlopts.bi_main.trim(), dlopts.bi_secondary.trim(), &cfg,
            )
            .await
            {
                let _ = app.emit(
                    "download-log",
                    DownloadLog { id: id.to_string(), line: format!("双语字幕合成失败: {e}") },
                );
            }
        }
        Ok(last_path)
    } else {
        Err(if errs.trim().is_empty() {
            "下载失败".into()
        } else {
            errs.trim().to_string()
        })
    }
}

/// 把 cookies 设置加到命令上
pub(crate) fn apply_cookies(cmd: &mut Command, cfg: &settings::Settings) {
    match cfg.cookies_mode.as_str() {
        "file" if !cfg.cookies_file.trim().is_empty() => {
            cmd.args(["--cookies", cfg.cookies_file.trim()]);
        }
        "browser" if !cfg.cookies_browser.trim().is_empty() => {
            let b = cfg.cookies_browser.trim();
            // yt-dlp 没有 operagx：用 opera 引擎 + Opera GX 的配置目录
            let value = if b == "operagx" {
                match std::env::var("APPDATA") {
                    Ok(appdata) => format!("opera:{appdata}\\Opera Software\\Opera GX Stable"),
                    Err(_) => "opera".to_string(),
                }
            } else {
                b.to_string()
            };
            cmd.args(["--cookies-from-browser", &value]);
        }
        _ => {}
    }
}

/// 给 yt-dlp 指定 JS 运行时（node/deno）——用于解 nsig 与 PO Token（yt-dlp 内部自动处理）
pub(crate) fn apply_js_runtime(cmd: &mut Command, app: &AppHandle) {
    if let Some((name, path)) = tools::resolve_js_runtime(app) {
        cmd.args(["--js-runtimes", &format!("{name}:{}", path.to_string_lossy())]);
    }
}

/// 每条下载的可选项（前端以 JSON 传入）
#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub struct DownloadOptions {
    embed_thumbnail: bool,
    embed_subs: bool,
    auto_subs: bool,
    sub_langs: String,
    embed_chapters: bool,
    embed_metadata: bool,
    custom_title: String,
    custom_artist: String,
    live_from_start: bool,
    sponsorblock: bool,
    recode_mp4: bool,
    extra: String, // 高级：原始参数（空格分隔）
    // 双语字幕：主字幕(上,大) + 副字幕(下,小)
    bilingual: bool,
    bi_main: String,
    bi_secondary: String,
}

impl DownloadOptions {
    fn bilingual_on(&self) -> bool {
        self.bilingual && !self.bi_main.trim().is_empty() && !self.bi_secondary.trim().is_empty()
    }
}

/// 把下载选项翻译成 yt-dlp 参数
fn build_option_args(o: &DownloadOptions) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut need_meta = o.embed_metadata;
    let bilingual = o.bilingual_on();

    if o.embed_thumbnail {
        a.push("--embed-thumbnail".into());
    }
    if bilingual {
        // 双语字幕由我们自己后处理合成，这里只确保用 MKV 容器
        a.push("--merge-output-format".into());
        a.push("mkv".into());
    } else if o.embed_subs || o.auto_subs {
        // --embed-subs 是"软字幕轨"（可开关、可选择），不是硬编码烧进画面
        a.push("--embed-subs".into());
        if o.auto_subs {
            a.push("--write-auto-subs".into());
        }
        let langs = if o.sub_langs.trim().is_empty() {
            "en".to_string()
        } else {
            o.sub_langs.trim().to_string()
        };
        a.push("--sub-langs".into());
        a.push(langs);
        if !o.recode_mp4 {
            a.push("--merge-output-format".into());
            a.push("mkv".into());
        }
    }
    if o.embed_chapters {
        a.push("--embed-chapters".into());
    }
    if !o.custom_title.trim().is_empty() {
        need_meta = true;
        a.push("--parse-metadata".into());
        a.push(format!("{}:%(meta_title)s", o.custom_title.trim()));
    }
    if !o.custom_artist.trim().is_empty() {
        need_meta = true;
        a.push("--parse-metadata".into());
        a.push(format!("{}:%(meta_artist)s", o.custom_artist.trim()));
    }
    if need_meta {
        a.push("--embed-metadata".into());
    }
    if o.live_from_start {
        a.push("--live-from-start".into());
    }
    if o.sponsorblock {
        a.push("--sponsorblock-remove".into());
        a.push("all".into());
    }
    if o.recode_mp4 && !bilingual {
        a.push("--recode-video".into());
        a.push("mp4".into());
    }
    if !o.extra.trim().is_empty() {
        for tok in o.extra.split_whitespace() {
            a.push(tok.to_string());
        }
    }
    a
}

/// 从 yt-dlp 日志行尽力解析出最终文件路径
fn parse_dest(line: &str) -> Option<String> {
    let l = line.trim();
    if let Some(rest) = l.strip_prefix("[download] Destination:") {
        return Some(rest.trim().to_string());
    }
    if let Some(rest) = l.strip_prefix("[ExtractAudio] Destination:") {
        return Some(rest.trim().to_string());
    }
    if let Some(idx) = l.find("Merging formats into \"") {
        let after = &l[idx + "Merging formats into \"".len()..];
        if let Some(end) = after.find('"') {
            return Some(after[..end].to_string());
        }
    }
    None
}

/// 解析 aria2c 进度行，形如：
/// `[#abc123 100MiB/531MiB(18%) CN:16 DL:50MiB ETA:8s]`
fn parse_aria2c(line: &str) -> Option<(f64, String, String)> {
    // 百分比：取 '(' 与 '%)' 之间
    let open = line.find('(')?;
    let pctend = line[open..].find("%)")? + open;
    let pct: f64 = line[open + 1..pctend].trim().parse().ok()?;

    let grab = |key: &str| -> String {
        if let Some(i) = line.find(key) {
            let rest = &line[i + key.len()..];
            rest.split([' ', ']'])
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        } else {
            String::new()
        }
    };
    let speed = grab("DL:");
    let eta = grab("ETA:");
    Some((pct, speed, eta))
}

