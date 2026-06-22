# YTDLP_GUI_Mhyho

一个 Windows 桌面端的 **yt-dlp 图形界面下载器**，追求 Win11 原生质感、低占用、模块化与稳定。
基于 **Tauri 2 + Rust + SolidJS** 构建。

## ✨ 功能

- **下载引擎**：调用官方 `yt-dlp`，支持 1000+ 站点（YouTube / Bilibili / X(Twitter) / VK / 腾讯 / 爱奇艺 / 优酷 / 抖音 …）。
- **核心组件自管理**：`yt-dlp` / `ffmpeg` / `aria2c` / `deno` 自动检测系统已装或按需下载；显示实际使用路径；可清理自带副本。
- **每日自动更新**：每天首次启动自动更新自带的 yt-dlp。
- **JS 运行时**：自动用 node/deno 供 yt-dlp 解 nsig 与 PO Token（YouTube 必需）。
- **格式选择**：视频流 / 音频流独立选择；按 **分辨率 + 编码效率 + 平均码率** 给出 0–100 画质评分与**渐变色码率条**；标注编码（AV1/HEVC/VP9/H.264）与 **HDR**。
- **下载队列 + 历史**：并发可配、排队/进行/完成/失败/取消；取消/重试/打开/删除；SQLite 持久化。
- **IDM 式分段进度条**（启用 aria2c 时）。
- **窗口材质**：Mica / Acrylic / 仿 Win7 Aero 玻璃，跟随系统明暗。
- **下载选项**：嵌入缩略图 / 章节 / 元数据、自定义标题与作者、SponsorBlock、转码 MP4、直播从头下载、额外参数菜单。
- **字幕**：内封软字幕轨（MKV），可从可用语言菜单（含自动转写/翻译）添加；**双语字幕**（主上大 + 副下小，ffmpeg 自动合成软轨）。
- **Cookies**：从浏览器读取（含 Opera GX）或 cookies.txt，用于会员/私有/受限内容与反爬。

## 🧱 架构（模块隔离）

前端 SolidJS（仅渲染交互）⇄ Tauri IPC ⇄ Rust 核心：

| 模块 | 职责 |
|---|---|
| `tools.rs` | 定位/下载/卸载 yt-dlp、ffmpeg、aria2c、deno；JS 运行时解析 |
| `engine.rs` | 调用 yt-dlp：版本、解析格式、带进度下载、选项→参数、画质评分 |
| `bisub.rs` | 双语字幕：下载两条字幕 → 合并为带样式 ASS → ffmpeg 混流 |
| `queue.rs` | 并发队列（信号量）+ 取消/重试/删除 + 事件 |
| `db.rs` | SQLite 持久化下载历史/队列 |
| `settings.rs` | 设置持久化 + 每日更新状态 |

## 🛠️ 开发

前置：[Rust](https://rustup.rs)、[Node.js](https://nodejs.org)、WebView2（Win11 自带）。

```bash
npm install
npm run tauri dev      # 开发运行
npm run tauri build    # 构建发布版（产物在 src-tauri/target/release/bundle/）
```

## 📦 发布产物

`npm run tauri build` 生成的安装包/可执行文件位于：
`src-tauri/target/release/bundle/`（与源码分离）。

## 📄 许可

仅供个人学习使用。下载内容请遵守各网站条款与版权法律；不含任何 DRM 破解。

— by Mhyho
