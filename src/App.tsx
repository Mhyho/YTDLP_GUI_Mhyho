import { createSignal, createEffect, createMemo, onMount, onCleanup, For, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./App.css";

type Backdrop = "mica" | "acrylic" | "aero" | "none";
type Tool = "yt-dlp" | "ffmpeg" | "aria2c" | "deno";

type MediaFormat = {
  format_id: string; ext: string; resolution: string; note: string;
  filesize: number | null; vcodec: string; acodec: string;
  codec_label: string; height: number | null; fps: number | null; tbr: number | null; quality: number; hdr: boolean;
};
type SubLang = { code: string; name: string; auto: boolean };
type MediaInfo = { title: string; uploader: string; duration: number | null; thumbnail: string; formats: MediaFormat[]; subtitles: SubLang[]; };
type ToolStatus = {
  ytdlp_ready: boolean; ytdlp_version: string; ytdlp_path: string; ytdlp_system: boolean; ytdlp_bundled: boolean;
  ffmpeg_ready: boolean; ffmpeg_path: string; ffmpeg_system: boolean; ffmpeg_bundled: boolean;
  aria2c_ready: boolean; aria2c_path: string; aria2c_system: boolean; aria2c_bundled: boolean;
  js_ready: boolean; js_runtime: string; js_path: string; deno_bundled: boolean;
};
type DownloadItem = {
  id: string; url: string; title: string; format: string; out_dir: string;
  filepath: string; status: string; error: string; thumbnail: string; created_at: number;
};
type Settings = {
  max_concurrent: number; cookies_mode: string; cookies_file: string; cookies_browser: string;
  use_aria2c: boolean; aria2c_connections: number;
};
type Prog = { percent: number; speed: string; eta: string };

const BACKDROPS: { id: Backdrop; label: string }[] = [
  { id: "mica", label: "Mica" }, { id: "acrylic", label: "Acrylic" },
  { id: "aero", label: "Aero 玻璃" }, { id: "none", label: "关闭" },
];
const NONE = "__none__";
const BEST_V = "bv*";
const BEST_A = "ba";
const BROWSERS: { value: string; label: string }[] = [
  { value: "edge", label: "Edge" },
  { value: "chrome", label: "Chrome" },
  { value: "firefox", label: "Firefox" },
  { value: "operagx", label: "Opera GX" },
  { value: "opera", label: "Opera" },
  { value: "brave", label: "Brave" },
  { value: "chromium", label: "Chromium" },
  { value: "vivaldi", label: "Vivaldi" },
];
const PARAM_PRESETS: { label: string; flag: string }[] = [
  { label: "限速 1M/s", flag: "--limit-rate 1M" },
  { label: "请求间隔 5s（防限流）", flag: "--sleep-interval 5" },
  { label: "不使用服务器时间", flag: "--no-mtime" },
  { label: "保存视频简介", flag: "--write-description" },
  { label: "单独保存缩略图文件", flag: "--write-thumbnail" },
  { label: "不覆盖已存在文件", flag: "--no-overwrites" },
  { label: "出错继续（忽略错误）", flag: "--ignore-errors" },
  { label: "仅播放列表第 1-5 项", flag: "--playlist-items 1-5" },
  { label: "限制文件名为 ASCII", flag: "--restrict-filenames" },
  { label: "嵌入后保留原始文件", flag: "--keep-video" },
];
const STATUS_LABEL: Record<string, string> = {
  queued: "排队中", running: "下载中", completed: "已完成", failed: "失败", cancelled: "已取消",
};
const ENSURE: Record<Tool, string> = { "yt-dlp": "ensure_ytdlp", ffmpeg: "ensure_ffmpeg", aria2c: "ensure_aria2c", deno: "ensure_deno" };
const UNINSTALL: Record<Tool, string> = { "yt-dlp": "uninstall_ytdlp", ffmpeg: "uninstall_ffmpeg", aria2c: "uninstall_aria2c", deno: "uninstall_deno" };

function fmtSize(n: number | null): string {
  if (!n) return "";
  const u = ["B", "KB", "MB", "GB"]; let i = 0; let v = n;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(1)}${u[i]}`;
}
function fmtDuration(s: number | null): string {
  if (!s) return "";
  const m = Math.floor(s / 60); const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
}
function fmtTime(epoch: number): string { return new Date(epoch * 1000).toLocaleString(); }
function qColor(q: number): string { return q >= 75 ? "#1a9e4b" : q >= 40 ? "#e8a33d" : "#e0484d"; }
function fmtBitrate(kbps: number | null): string {
  if (!kbps) return "";
  return kbps >= 1000 ? `${(kbps / 1000).toFixed(1)} Mbps` : `${Math.round(kbps)} kbps`;
}

function App() {
  const [backdrop, setBackdrop] = createSignal<Backdrop>("aero");
  const [sysDark, setSysDark] = createSignal(true);

  const [tools, setTools] = createSignal<ToolStatus>({
    ytdlp_ready: false, ytdlp_version: "", ytdlp_path: "", ytdlp_system: false, ytdlp_bundled: false,
    ffmpeg_ready: false, ffmpeg_path: "", ffmpeg_system: false, ffmpeg_bundled: false,
    aria2c_ready: false, aria2c_path: "", aria2c_system: false, aria2c_bundled: false,
    js_ready: false, js_runtime: "", js_path: "", deno_bundled: false,
  });
  const [busyTool, setBusyTool] = createSignal<"" | Tool>("");
  const [toolStage, setToolStage] = createSignal("");
  const [toolPct, setToolPct] = createSignal(0);

  const [settings, setSettings] = createSignal<Settings>({
    max_concurrent: 3, cookies_mode: "none", cookies_file: "", cookies_browser: "edge",
    use_aria2c: false, aria2c_connections: 16,
  });

  const [url, setUrl] = createSignal("");
  const [parsing, setParsing] = createSignal(false);
  const [info, setInfo] = createSignal<MediaInfo | null>(null);
  const [videoSel, setVideoSel] = createSignal(BEST_V);
  const [audioSel, setAudioSel] = createSignal(BEST_A);
  const [outDir, setOutDir] = createSignal("");
  const [msg, setMsg] = createSignal("");
  const [msgOk, setMsgOk] = createSignal(true);

  const [showOpts, setShowOpts] = createSignal(false);
  const [opts, setOpts] = createSignal({
    embed_thumbnail: false, embed_subs: false, auto_subs: false, sub_langs: "en",
    embed_chapters: false, embed_metadata: false, custom_title: "", custom_artist: "",
    live_from_start: false, sponsorblock: false, recode_mp4: false, extra: "",
    bilingual: false, bi_main: "", bi_secondary: "",
  });
  const setOpt = (patch: Partial<ReturnType<typeof opts>>) => setOpts((o) => ({ ...o, ...patch }));
  const [pickLang, setPickLang] = createSignal("");
  const [pickParam, setPickParam] = createSignal("");

  const subList = () => opts().sub_langs.split(",").map((s) => s.trim()).filter(Boolean);
  const addSubLang = (code: string) => {
    if (!code) return;
    const cur = subList();
    if (!cur.includes(code)) setOpt({ sub_langs: [...cur, code].join(",") });
  };
  const removeSubLang = (code: string) => setOpt({ sub_langs: subList().filter((c) => c !== code).join(",") });
  const addParam = (flag: string) => {
    if (!flag) return;
    const cur = opts().extra.trim();
    setOpt({ extra: (cur ? cur + " " : "") + flag });
  };

  const [items, setItems] = createSignal<DownloadItem[]>([]);
  const [progress, setProgress] = createSignal<Record<string, Prog>>({});

  const t = tools;
  // Aero 是浅色发亮玻璃，固定浅色文字方案；其余跟随系统明暗
  const effectiveTheme = () => (backdrop() === "aero" ? "light" : sysDark() ? "dark" : "light");
  createEffect(() => {
    document.documentElement.dataset.theme = effectiveTheme();
    document.documentElement.dataset.backdrop = backdrop();
  });

  const videoFormats = createMemo(() =>
    (info()?.formats ?? [])
      .filter((f) => f.vcodec && f.vcodec !== "none")
      .sort((a, b) => (b.height ?? 0) - (a.height ?? 0) || b.quality - a.quality));
  const audioFormats = createMemo(() =>
    (info()?.formats ?? [])
      .filter((f) => f.acodec && f.acodec !== "none" && (!f.vcodec || f.vcodec === "none"))
      .sort((a, b) => b.quality - a.quality));

  // 当前选中的视频流（"最佳视频"取排序后第一个）
  const selectedVideo = createMemo(() => {
    const v = videoSel();
    if (v === NONE) return null;
    if (v === BEST_V) return videoFormats()[0] ?? null;
    return videoFormats().find((f) => f.format_id === v) ?? null;
  });
  // 当前选中的音频流
  const selectedAudio = createMemo(() => {
    const a = audioSel();
    if (a === NONE) return null;
    if (a === BEST_A) return audioFormats()[0] ?? null;
    return audioFormats().find((f) => f.format_id === a) ?? null;
  });
  const formatExpr = createMemo(() => {
    const v = videoSel(); const a = audioSel();
    if (v !== NONE && a !== NONE) { const base = `${v}+${a}`; return v === BEST_V && a === BEST_A ? `${base}/b` : base; }
    if (v !== NONE) return v === BEST_V ? "bv*/b" : v;
    if (a !== NONE) return a === BEST_A ? "ba/b" : a;
    return "";
  });

  onMount(async () => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    setSysDark(mq.matches);
    const onTheme = (e: MediaQueryListEvent) => setSysDark(e.matches);
    mq.addEventListener("change", onTheme);

    const unlisten: UnlistenFn[] = [];
    unlisten.push(await listen<{ tool: string; percent: number; stage: string }>("tool-progress", (e) => {
      setToolStage(e.payload.stage); setToolPct(e.payload.percent);
    }));
    unlisten.push(await listen<{ id: string; percent: number; speed: string; eta: string }>("download-progress", (e) => {
      setProgress((p) => ({ ...p, [e.payload.id]: { percent: e.payload.percent, speed: e.payload.speed, eta: e.payload.eta } }));
    }));
    unlisten.push(await listen("queue-changed", () => reloadList()));
    unlisten.push(await listen("tools-updated", () => { setToolStage(""); refreshTools(); }));

    onCleanup(() => { mq.removeEventListener("change", onTheme); unlisten.forEach((u) => u()); });

    invoke("set_backdrop", { kind: backdrop() }).catch(console.error);
    try { setOutDir(await invoke<string>("default_download_dir")); } catch {}
    try { setSettings(await invoke<Settings>("get_settings")); } catch {}
    refreshTools();
    reloadList();
  });

  async function reloadList() {
    try { setItems(await invoke<DownloadItem[]>("list_downloads")); } catch (e) { console.error(e); }
  }
  async function refreshTools() {
    try { setTools(await invoke<ToolStatus>("tool_status")); } catch (e) { console.error(e); }
  }
  function changeBackdrop(kind: Backdrop) { setBackdrop(kind); invoke("set_backdrop", { kind }).catch(console.error); }

  async function updateSettings(patch: Partial<Settings>) {
    const next = { ...settings(), ...patch };
    setSettings(next);
    try { await invoke("save_settings", { settings: next }); } catch (e) { console.error(e); }
  }

  async function install(tool: Tool) {
    setBusyTool(tool); setToolStage(`准备下载 ${tool}`); setToolPct(0);
    try { await invoke(ENSURE[tool]); await refreshTools(); setToolStage(""); }
    catch (e) { setToolStage(`${tool} 安装失败: ${e}`); }
    finally { setBusyTool(""); }
  }
  async function uninstall(tool: Tool) {
    try { await invoke(UNINSTALL[tool]); await refreshTools(); }
    catch (e) { setToolStage(`清理失败: ${e}`); }
  }

  async function parse() {
    if (!url().trim()) return;
    setParsing(true); setInfo(null); setMsg("");
    try {
      const i = await invoke<MediaInfo>("fetch_info", { url: url().trim() });
      setInfo(i); setVideoSel(BEST_V); setAudioSel(BEST_A);
      setPickLang(i.subtitles[0]?.code ?? "");
    } catch (e) { setMsgOk(false); setMsg(`解析失败: ${e}`); }
    finally { setParsing(false); }
  }

  async function enqueue() {
    const f = formatExpr();
    if (!url().trim() || !f) return;
    try {
      await invoke("enqueue_download", {
        url: url().trim(), title: info()?.title ?? "", format: f, outDir: outDir(),
        thumbnail: info()?.thumbnail ?? "", options: JSON.stringify(opts()),
      });
      setMsgOk(true); setMsg("已加入下载队列"); setInfo(null); setUrl("");
    } catch (e) { setMsgOk(false); setMsg(`入队失败: ${e}`); }
  }

  const act = (cmd: string, id: string) => invoke(cmd, { id }).catch(console.error);

  // IDM 式分段数：用 aria2c 时按连接数分段（上限 32），否则不分段
  const segCount = () =>
    settings().use_aria2c && tools().aria2c_ready
      ? Math.min(Math.max(settings().aria2c_connections, 1), 32)
      : 1;

  // 可复用的组件状态行
  const ToolRow = (p: { name: string; tool: Tool; ready: boolean; label: string; system: boolean; bundled: boolean; path: string; installLabel: string }) => (
    <div class="tool-block">
      <div class="tool-row">
        <span class="tool-name">{p.name}</span>
        <Show when={p.ready} fallback={<>
          <span class="badge bad">未安装</span>
          <button class="primary sm" disabled={busyTool() !== ""} onClick={() => install(p.tool)}>{p.installLabel}</button>
        </>}>
          <span class="badge ok">{p.label}</span>
          <span class="badge soft">{p.system ? "系统 PATH" : "应用自带"}</span>
          <Show when={p.bundled}><button class="link" onClick={() => uninstall(p.tool)}>清理自带</button></Show>
        </Show>
      </div>
      <Show when={p.path}><div class="tool-path mono">{p.path}</div></Show>
    </div>
  );

  return (
    <div class="app">
      <div class="titlebar" data-tauri-drag-region>YTDLP_GUI_Mhyho</div>

      <div class="content">
        {/* 核心组件 */}
        <div class="card">
          <div class="section-title">核心组件</div>
          <ToolRow name="yt-dlp" tool="yt-dlp" ready={t().ytdlp_ready} label={t().ytdlp_version || "已就绪"} system={t().ytdlp_system} bundled={t().ytdlp_bundled} path={t().ytdlp_path} installLabel="安装" />
          <ToolRow name="ffmpeg" tool="ffmpeg" ready={t().ffmpeg_ready} label="已就绪" system={t().ffmpeg_system} bundled={t().ffmpeg_bundled} path={t().ffmpeg_path} installLabel="安装（~80MB）" />
          <ToolRow name="aria2c" tool="aria2c" ready={t().aria2c_ready} label="已就绪" system={t().aria2c_system} bundled={t().aria2c_bundled} path={t().aria2c_path} installLabel="安装（~5MB）" />
          {/* JS 运行时：YouTube 解 nsig / PO Token 必需 */}
          <div class="tool-block">
            <div class="tool-row">
              <span class="tool-name">JS 运行时</span>
              <Show when={t().js_ready} fallback={<>
                <span class="badge bad">未检测到</span>
                <button class="primary sm" disabled={busyTool() !== ""} onClick={() => install("deno")}>下载 deno（~40MB）</button>
              </>}>
                <span class="badge ok">{t().js_runtime}</span>
                <Show when={t().deno_bundled}><button class="link" onClick={() => uninstall("deno")}>清理自带</button></Show>
              </Show>
              <span class="dim">YouTube 解析下载必需</span>
            </div>
            <Show when={t().js_path}><div class="tool-path mono">{t().js_path}</div></Show>
          </div>
          <Show when={busyTool() !== "" || toolStage()}>
            <div class="progress-wrap">
              <Show when={busyTool() !== ""}><div class="progress"><div class="bar" style={{ width: `${toolPct()}%` }} /></div></Show>
              <span class="dim">{toolStage()} {busyTool() !== "" ? `${toolPct()}%` : ""}</span>
            </div>
          </Show>
        </div>

        {/* 新建下载 */}
        <Show when={t().ytdlp_ready}>
          <div class="card">
            <div class="section-title">新建下载</div>
            <div class="row">
              <input class="grow" placeholder="粘贴视频/播放列表链接…" value={url()}
                onInput={(e) => setUrl(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && parse()} />
              <button disabled={parsing()} onClick={parse}>{parsing() ? "解析中…" : "解析"}</button>
            </div>

            <Show when={info()}>
              <div class="info">
                <Show when={info()!.thumbnail}><img class="thumb" src={info()!.thumbnail} alt="" /></Show>
                <div class="meta">
                  <div class="title-text">{info()!.title}</div>
                  <div class="dim">{info()!.uploader}{info()!.duration ? ` · ${fmtDuration(info()!.duration)}` : ""}</div>
                </div>
              </div>
              <div class="field-grid">
                <div class="field">
                  <label class="dim">视频流</label>
                  <div class="select-row">
                    <select class="grow" value={videoSel()} onChange={(e) => setVideoSel(e.currentTarget.value)}>
                      <option value={BEST_V}>最佳视频</option>
                      <option value={NONE}>不下载（纯音频）</option>
                      <For each={videoFormats()}>{(f) => <option value={f.format_id}>{f.resolution || f.note}{f.fps ? `${Math.round(f.fps)}fps` : ""} · {f.codec_label} · 画质{f.quality} · {f.ext}{f.filesize ? ` · ${fmtSize(f.filesize)}` : ""}</option>}</For>
                    </select>
                    <Show when={selectedVideo()?.hdr}><span class="hdr-badge">HDR</span></Show>
                  </div>
                  <Show when={selectedVideo()}>
                    <div class="qbar">
                      <div class="qbar-track">
                        <div class="qbar-fill" style={{ width: `${selectedVideo()!.quality}%`, background: qColor(selectedVideo()!.quality) }} />
                        <Show when={selectedVideo()!.tbr}>
                          <span class="qbar-rate">平均码率 {fmtBitrate(selectedVideo()!.tbr)}</span>
                        </Show>
                      </div>
                      <span class="qbar-num" style={{ color: qColor(selectedVideo()!.quality) }}>画质 {selectedVideo()!.quality}</span>
                    </div>
                  </Show>
                </div>
                <div class="field">
                  <label class="dim">音频流</label>
                  <select value={audioSel()} onChange={(e) => setAudioSel(e.currentTarget.value)}>
                    <option value={BEST_A}>最佳音频</option>
                    <option value={NONE}>不下载（纯视频）</option>
                    <For each={audioFormats()}>{(f) => <option value={f.format_id}>{f.codec_label}{f.tbr ? ` · ${Math.round(f.tbr)}kbps` : ""} · 音质{f.quality} · {f.ext}{f.filesize ? ` · ${fmtSize(f.filesize)}` : ""}</option>}</For>
                  </select>
                  <Show when={selectedAudio()}>
                    <div class="qbar">
                      <div class="qbar-track">
                        <div class="qbar-fill" style={{ width: `${selectedAudio()!.quality}%`, background: qColor(selectedAudio()!.quality) }} />
                        <Show when={selectedAudio()!.tbr}>
                          <span class="qbar-rate">平均码率 {fmtBitrate(selectedAudio()!.tbr)}</span>
                        </Show>
                      </div>
                      <span class="qbar-num" style={{ color: qColor(selectedAudio()!.quality) }}>音质 {selectedAudio()!.quality}</span>
                    </div>
                  </Show>
                </div>
              </div>
              <Show when={!formatExpr()}><p class="dim warn">⚠ 视频和音频不能同时为"不下载"</p></Show>
            </Show>

            <div class="field">
              <label class="dim">保存到</label>
              <div class="row">
                <input class="grow" value={outDir()} onInput={(e) => setOutDir(e.currentTarget.value)} />
                <button onClick={() => invoke("open_path", { path: outDir() })}>打开</button>
              </div>
            </div>

            <div class="field">
              <button class="link" onClick={() => setShowOpts(!showOpts())}>
                {showOpts() ? "▾ 下载选项" : "▸ 下载选项（缩略图 / 字幕 / 章节 / 元数据 / 直播 / 额外参数）"}
              </button>
            </div>
            <Show when={showOpts()}>
              <div class="opts">
                <div class="opts-grid">
                  <label class="check"><input type="checkbox" checked={opts().embed_thumbnail} onChange={(e) => setOpt({ embed_thumbnail: e.currentTarget.checked })} /> 嵌入缩略图</label>
                  <label class="check"><input type="checkbox" checked={opts().embed_metadata} onChange={(e) => setOpt({ embed_metadata: e.currentTarget.checked })} /> 嵌入元数据</label>
                  <label class="check"><input type="checkbox" checked={opts().embed_chapters} onChange={(e) => setOpt({ embed_chapters: e.currentTarget.checked })} /> 嵌入章节</label>
                  <label class="check"><input type="checkbox" checked={opts().embed_subs} onChange={(e) => setOpt({ embed_subs: e.currentTarget.checked })} /> 嵌入字幕（软字幕轨/MKV，可开关）</label>
                  <label class="check"><input type="checkbox" checked={opts().sponsorblock} onChange={(e) => setOpt({ sponsorblock: e.currentTarget.checked })} /> 移除 SponsorBlock 片段</label>
                  <label class="check"><input type="checkbox" checked={opts().recode_mp4} onChange={(e) => setOpt({ recode_mp4: e.currentTarget.checked })} /> 转码为 MP4</label>
                  <label class="check"><input type="checkbox" checked={opts().live_from_start} onChange={(e) => setOpt({ live_from_start: e.currentTarget.checked })} /> 直播：从开头下载</label>
                </div>

                <Show when={opts().embed_subs}>
                  <label class="check"><input type="checkbox" checked={opts().auto_subs} onChange={(e) => setOpt({ auto_subs: e.currentTarget.checked })} /> 包含自动生成字幕</label>
                  <Show when={info() && info()!.subtitles.length}>
                    <div class="field">
                      <label class="dim">从可用字幕中添加（人工 / 自动转写翻译）</label>
                      <div class="row">
                        <select class="grow" value={pickLang()} onChange={(e) => setPickLang(e.currentTarget.value)}>
                          <For each={info()!.subtitles}>{(s) => <option value={s.code}>{s.code} · {s.name}{s.auto ? "（自动）" : "（人工）"}</option>}</For>
                        </select>
                        <button onClick={() => addSubLang(pickLang())}>添加</button>
                      </div>
                    </div>
                  </Show>
                  <Show when={subList().length}>
                    <div class="chips">
                      <For each={subList()}>{(c) => <span class="chip">{c}<button class="chip-x" onClick={() => removeSubLang(c)}>×</button></span>}</For>
                    </div>
                  </Show>
                  <div class="field">
                    <label class="dim">字幕语言（逗号分隔，可手动编辑；all = 全部）</label>
                    <input value={opts().sub_langs} onInput={(e) => setOpt({ sub_langs: e.currentTarget.value })} />
                  </div>
                </Show>

                {/* 双语字幕 */}
                <Show when={info() && info()!.subtitles.length}>
                  <label class="check"><input type="checkbox" checked={opts().bilingual} onChange={(e) => setOpt({ bilingual: e.currentTarget.checked })} /> 双语字幕（主上·大 + 副下·小，合成为软轨）</label>
                  <Show when={opts().bilingual}>
                    <div class="field-grid">
                      <div class="field">
                        <label class="dim">主字幕（上，大）</label>
                        <select value={opts().bi_main} onChange={(e) => setOpt({ bi_main: e.currentTarget.value })}>
                          <option value="">选择语言…</option>
                          <For each={info()!.subtitles}>{(s) => <option value={s.code}>{s.code} · {s.name}{s.auto ? "（自动）" : "（人工）"}</option>}</For>
                        </select>
                      </div>
                      <div class="field">
                        <label class="dim">副字幕（下，小）</label>
                        <select value={opts().bi_secondary} onChange={(e) => setOpt({ bi_secondary: e.currentTarget.value })}>
                          <option value="">选择语言…</option>
                          <For each={info()!.subtitles}>{(s) => <option value={s.code}>{s.code} · {s.name}{s.auto ? "（自动）" : "（人工）"}</option>}</For>
                        </select>
                      </div>
                    </div>
                    <p class="dim">需 ffmpeg；下载后自动按时间轴合成，输出 MKV 可开关软轨。</p>
                  </Show>
                </Show>

                <div class="field-grid">
                  <div class="field">
                    <label class="dim">自定义标题（嵌入元数据）</label>
                    <input value={opts().custom_title} onInput={(e) => setOpt({ custom_title: e.currentTarget.value })} placeholder="留空=使用原标题" />
                  </div>
                  <div class="field">
                    <label class="dim">自定义作者</label>
                    <input value={opts().custom_artist} onInput={(e) => setOpt({ custom_artist: e.currentTarget.value })} placeholder="留空=使用原作者" />
                  </div>
                </div>

                <div class="field">
                  <label class="dim">高级 · 额外 yt-dlp 参数</label>
                  <div class="row">
                    <select class="grow" value={pickParam()} onChange={(e) => setPickParam(e.currentTarget.value)}>
                      <option value="">— 选择常用参数 —</option>
                      <For each={PARAM_PRESETS}>{(p) => <option value={p.flag}>{p.label}（{p.flag}）</option>}</For>
                    </select>
                    <button onClick={() => addParam(pickParam())}>添加</button>
                  </div>
                  <input class="mono" value={opts().extra} onInput={(e) => setOpt({ extra: e.currentTarget.value })} placeholder="可手动编辑，空格分隔；如 --no-mtime --sleep-interval 5" />
                </div>
              </div>
            </Show>

            <Show when={!t().ffmpeg_ready}><p class="dim warn">⚠ 未检测到 ffmpeg，合并高清视频+音频会失败。请先在上方安装。</p></Show>

            <div class="row download-row">
              <button class="primary" disabled={!url().trim() || !formatExpr()} onClick={enqueue}>加入下载队列</button>
              <span class="dim mono">-f {formatExpr() || "—"}</span>
            </div>
            <Show when={msg()}><div class={`result ${msgOk() ? "ok" : "error"}`}>{msg()}</div></Show>
          </div>
        </Show>

        {/* 队列 / 历史 */}
        <div class="card">
          <div class="row spread">
            <div class="section-title nomargin">下载队列 / 历史（{items().length}）</div>
            <Show when={items().some((i) => ["completed", "failed", "cancelled"].includes(i.status))}>
              <button class="link" onClick={() => invoke("clear_finished").catch(console.error)}>清除已完成</button>
            </Show>
          </div>
          <Show when={items().length === 0}><p class="dim">还没有下载任务。</p></Show>
          <For each={items()}>
            {(it) => {
              const p = () => progress()[it.id];
              const pct = () => (it.status === "completed" ? 100 : p()?.percent ?? 0);
              return (
                <div class="dl-item">
                  <Show when={it.thumbnail} fallback={<div class="thumb-sm ph" />}><img class="thumb-sm" src={it.thumbnail} alt="" /></Show>
                  <div class="dl-main">
                    <div class="dl-title">{it.title || it.url}</div>
                    <div class="dl-sub">
                      <span class={`badge st-${it.status}`}>{STATUS_LABEL[it.status] ?? it.status}</span>
                      <Show when={it.status === "running"}><span class="dim mono">{pct().toFixed(1)}% · {p()?.speed ?? ""} · ETA {p()?.eta ?? ""}</span></Show>
                      <Show when={it.status === "failed" && it.error}><span class="dim err-text">{it.error}</span></Show>
                      <Show when={it.status === "completed"}><span class="dim">{fmtTime(it.created_at)}</span></Show>
                    </div>
                    <Show when={it.status === "running" || it.status === "queued"}>
                      <div class={`progress thin ${segCount() > 1 ? "seg" : ""}`} style={{ "--segs": String(segCount()) }}>
                        <div class="bar" style={{ width: `${pct()}%` }} />
                      </div>
                    </Show>
                  </div>
                  <div class="dl-actions">
                    <Show when={it.status === "running" || it.status === "queued"}><button class="sm" onClick={() => act("cancel_download", it.id)}>取消</button></Show>
                    <Show when={it.status === "failed" || it.status === "cancelled"}><button class="sm" onClick={() => act("retry_download", it.id)}>重试</button></Show>
                    <Show when={it.status === "completed"}><button class="sm" onClick={() => invoke("open_path", { path: it.out_dir })}>打开</button></Show>
                    <button class="sm" onClick={() => act("remove_download", it.id)}>删除</button>
                  </div>
                </div>
              );
            }}
          </For>
        </div>

        {/* 设置 */}
        <div class="card">
          <div class="section-title">设置</div>

          <div class="field">
            <label class="dim">Cookies 来源（下载会员/私有/受限内容，也能减少 bot 拦截 / 403）</label>
            <select value={settings().cookies_mode} onChange={(e) => updateSettings({ cookies_mode: e.currentTarget.value })}>
              <option value="none">不使用</option>
              <option value="browser">从浏览器读取</option>
              <option value="file">cookies.txt 文件</option>
            </select>
          </div>
          <Show when={settings().cookies_mode === "browser"}>
            <div class="field">
              <label class="dim">浏览器（读取时该浏览器最好处于关闭状态）</label>
              <select value={settings().cookies_browser} onChange={(e) => updateSettings({ cookies_browser: e.currentTarget.value })}>
                <For each={BROWSERS}>{(b) => <option value={b.value}>{b.label}</option>}</For>
              </select>
            </div>
          </Show>
          <Show when={settings().cookies_mode === "file"}>
            <div class="field">
              <label class="dim">cookies.txt 路径</label>
              <input value={settings().cookies_file} placeholder="C:\\path\\to\\cookies.txt"
                onInput={(e) => updateSettings({ cookies_file: e.currentTarget.value })} />
            </div>
          </Show>

          <div class="field">
            <label class="dim">下载器</label>
            <label class="check">
              <input type="checkbox" checked={settings().use_aria2c} disabled={!t().aria2c_ready}
                onChange={(e) => updateSettings({ use_aria2c: e.currentTarget.checked })} />
              使用 aria2c 多线程下载{!t().aria2c_ready ? "（需先在上方安装 aria2c）" : ""}
            </label>
          </div>
          <Show when={settings().use_aria2c && t().aria2c_ready}>
            <div class="field">
              <label class="dim">aria2c 连接数</label>
              <input class="num" type="number" min="1" max="64" value={settings().aria2c_connections}
                onInput={(e) => updateSettings({ aria2c_connections: parseInt(e.currentTarget.value) || 16 })} />
            </div>
          </Show>

          <div class="field">
            <label class="dim">最大并发下载数（重启后生效）</label>
            <input class="num" type="number" min="1" max="10" value={settings().max_concurrent}
              onInput={(e) => updateSettings({ max_concurrent: parseInt(e.currentTarget.value) || 3 })} />
          </div>

          <p class="dim" style={{ "margin-top": "14px" }}>
            💡 下载 YouTube 需要：① 上方"JS 运行时"就绪（解 nsig / PO Token，yt-dlp 自动处理）；
            ② 多数视频还需 Cookies（选"从浏览器读取"且该浏览器已登录 YouTube）。
          </p>
        </div>

        {/* 外观 */}
        <div class="card">
          <div class="section-title">外观 · 窗口材质</div>
          <div class="row">
            <For each={BACKDROPS}>{(b) => <button class={backdrop() === b.id ? "active" : ""} onClick={() => changeBackdrop(b.id)}>{b.label}</button>}</For>
          </div>
        </div>
      </div>
    </div>
  );
}

export default App;
