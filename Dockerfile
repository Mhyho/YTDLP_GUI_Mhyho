# YTDLP_GUI_Mhyho —— 容器化运行版（GUI 需宿主转发 X11/Wayland）
#
# 构建（在含 .rpm 的目录作为上下文）：
#   docker build -f Dockerfile -t ytdlp_gui_mhyho <含 rpm 的目录>
#
# 运行（Linux 宿主，X11）：
#   xhost +local:docker
#   docker run --rm -e DISPLAY=$DISPLAY \
#     -v /tmp/.X11-unix:/tmp/.X11-unix \
#     -v "$HOME/Downloads:/root/Downloads" \
#     ytdlp_gui_mhyho
#
# 基于 fedora:44 以匹配 rpm 构建时的 glibc 版本。

FROM fedora:44

# 运行时依赖：WebKitGTK + GTK + 媒体工具 + JS 运行时（供 yt-dlp 解 nsig）
RUN dnf install -y \
        webkit2gtk4.1 gtk3 librsvg2 \
        ffmpeg-free aria2 nodejs \
        xdg-utils mesa-libGL \
    && dnf clean all

# 安装应用（rpm 放在构建上下文）
COPY *.rpm /tmp/
RUN dnf install -y /tmp/*.rpm && rm -f /tmp/*.rpm && dnf clean all

# 容器内软件渲染更稳
ENV WEBKIT_DISABLE_COMPOSITING_MODE=1

ENTRYPOINT ["aerodl"]
