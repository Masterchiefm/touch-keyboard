#!/usr/bin/env bash
# TouchKeyboard 一键安装脚本
# 自动获取最新 GitHub Release 并安装:
#   - Debian/Ubuntu 及衍生 (有 apt-get 且能提权) → 下载 .deb 用 apt 安装 (自动补依赖)
#   - 其他情况 (无 apt / 无提权) → AppImage 装到 ~/.local/bin 并创建应用菜单项 (免 root)
# 再次运行即可升级到最新版 (deb 重装覆盖 / AppImage 原地覆盖)。
# 设计为可经管道执行 (curl -fsSL ... | bash): 不读 stdin, 不依赖仓库内其他文件。
#
# 用法:
#   curl -fsSL https://raw.githubusercontent.com/Masterchiefm/touch-keyboard/main/scripts/install.sh | bash
#   或克隆仓库后: bash scripts/install.sh
# 可选环境变量:
#   TOUCH_KB_VERSION=v0.2.0   安装指定版本 (默认最新, v 前缀可省)
#   GH_PROXY=<加速前缀>/       GitHub 加速代理 (拼接在 api/download URL 前, 国内网络不畅时用)
set -euo pipefail

REPO="Masterchiefm/touch-keyboard"
PROD="TouchKeyboard"   # 产物命名, 与 tauri.conf.json 的 productName 一致
PKG="touch-keyboard"   # deb 包名 (dpkg-deb -f 可查)
BIN_NAME="touch-keyboard"

GH_PROXY="${GH_PROXY:-}"
case "$GH_PROXY" in "" | */) ;; *) GH_PROXY="$GH_PROXY/" ;; esac

INSTALL_SUMMARY=""

msg()  { printf '\033[1;32m==> \033[0m%s\n' "$*"; }
warn() { printf '\033[1;33m==> 警告: \033[0m%s\n' "$*"; }
die()  { printf '\033[1;31m==> 失败: \033[0m%s\n' "$*" >&2; exit 1; }

gh_url() { printf '%s%s' "$GH_PROXY" "$1"; }

# ---- 网络工具 (curl 优先, 回退 wget) ----
http_get() { # $1=url → stdout
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 2 --max-time 60 "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -T 60 -O - "$1"
  else
    die "需要 curl 或 wget 之一, 请先安装"
  fi
}

fetch_file() { # $1=url $2=输出文件
  if command -v curl >/dev/null 2>&1; then
    curl -fL --retry 3 --retry-delay 1 --max-time 1800 -o "$2" "$1"
  else
    wget -q -T 1800 -O "$2" "$1"
  fi
}

# ---- 提权判定 ----
# curl|bash 场景下 stdin 是脚本而非终端, sudo 读密码走 /dev/tty, 有 tty 即可用
have_root() {
  [ "$(id -u)" -eq 0 ] && return 0
  command -v sudo >/dev/null 2>&1 || return 1
  sudo -n true 2>/dev/null && return 0
  [ -t 1 ] && return 0
  return 1
}

if [ "$(id -u)" -eq 0 ]; then SUDO=""
elif command -v sudo >/dev/null 2>&1; then SUDO="sudo"
else SUDO=""
fi

# ---- 架构 ----
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64) PKG_ARCH=amd64 ;;
  aarch64 | arm64) PKG_ARCH=arm64 ;;
  *) die "不支持的架构: $ARCH (Releases 目前仅提供 amd64 安装包)" ;;
esac

# ---- 版本 ----
if [ -n "${TOUCH_KB_VERSION:-}" ]; then
  case "$TOUCH_KB_VERSION" in
    v*) TAG="$TOUCH_KB_VERSION" ;;
    *) TAG="v$TOUCH_KB_VERSION" ;;
  esac
else
  msg "查询最新版本 ..."
  TAG="$(http_get "$(gh_url "https://api.github.com/repos/$REPO/releases/latest")" \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1 || true)"
  if [ -z "$TAG" ] && command -v curl >/dev/null 2>&1; then
    # API 限流/不可达时, 从 releases/latest 的 302 重定向解析 tag
    TAG="$(curl -fsS --max-time 60 -o /dev/null -w '%{redirect_url}' \
      "$(gh_url "https://github.com/$REPO/releases/latest")" 2>/dev/null \
      | sed -n 's#.*/tag/\([^?]*\)$#\1#p' || true)"
  fi
  [ -n "$TAG" ] || die "无法获取最新版本号。可打开 https://github.com/$REPO/releases 查看版本, \
并用 TOUCH_KB_VERSION=vx.y.z 指定后重试"
fi
VER="${TAG#v}"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
URL_BASE="$(gh_url "https://github.com/$REPO/releases/download/$TAG")"

install_deb() {
  # 返回非 0 = 未能装上 (如当前用户无 sudo 权限), 由主流程回落 AppImage
  local asset="${PROD}_${VER}_${PKG_ARCH}.deb"
  msg "下载 $asset ($TAG) ..."
  fetch_file "$URL_BASE/$asset" "$TMP/$asset" || {
    warn "下载失败: $URL_BASE/$asset"
    return 1
  }
  msg "安装 $asset (如提示请输入密码) ..."
  if ! $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y "$TMP/$asset"; then
    warn "apt 安装失败, 回退 dpkg 并修复依赖 ..."
    if ! $SUDO dpkg -i "$TMP/$asset"; then
      if ! $SUDO env DEBIAN_FRONTEND=noninteractive apt-get -f install -y; then
        warn "deb 安装失败 (当前用户可能无 sudo 权限), 转用 AppImage ..."
        return 1
      fi
    fi
  fi
  INSTALL_SUMMARY="已通过 apt 安装 $TAG (二进制: /usr/bin/$BIN_NAME, 应用菜单: TouchKeyboard 触屏键盘)
卸载: sudo apt remove $PKG"
}

install_appimage() {
  local asset="${PROD}_${VER}_${PKG_ARCH}.AppImage"
  local bin_dir desktop_dir
  if [ "$(id -u)" -eq 0 ]; then
    bin_dir="/usr/local/bin"
    desktop_dir="/usr/local/share/applications"
  else
    bin_dir="$HOME/.local/bin"
    desktop_dir="$HOME/.local/share/applications"
  fi
  msg "下载 $asset ($TAG) ..."
  fetch_file "$URL_BASE/$asset" "$TMP/$asset" \
    || die "下载失败: $URL_BASE/$asset
请到 https://github.com/$REPO/releases 确认 $TAG 是否有 ${PKG_ARCH} 架构安装包"
  msg "安装到 $bin_dir/$BIN_NAME.AppImage ..."
  mkdir -p "$bin_dir" "$desktop_dir"
  chmod +x "$TMP/$asset"
  mv -f "$TMP/$asset" "$bin_dir/$BIN_NAME.AppImage"
  # 应用菜单入口 (AppImage 包不带 .desktop, 自建一份)
  cat > "$TMP/$BIN_NAME.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=TouchKeyboard 触屏键盘
Comment=Linux 触摸屏虚拟键盘 (完整/分裂布局, 悬浮球, 托盘常驻)
Exec="$bin_dir/$BIN_NAME.AppImage"
Icon=input-keyboard
Terminal=false
Categories=Utility;Accessibility;
EOF
  mv -f "$TMP/$BIN_NAME.desktop" "$desktop_dir/$BIN_NAME.desktop"
  case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) warn "$bin_dir 不在 PATH 中, 终端启动请用完整路径, 或将其加入 PATH" ;;
  esac
  if ! { [ -e /dev/fuse ] && { command -v fusermount >/dev/null 2>&1 || command -v fusermount3 >/dev/null 2>&1; }; }; then
    warn "未检测到 FUSE, AppImage 可能无法直接运行。可执行: sudo apt install libfuse2"
    echo "      或改用: $bin_dir/$BIN_NAME.AppImage --appimage-extract-and-run"
  fi
  INSTALL_SUMMARY="已安装 $TAG 到 $bin_dir/$BIN_NAME.AppImage (应用菜单: TouchKeyboard 触屏键盘)
卸载: rm $bin_dir/$BIN_NAME.AppImage $desktop_dir/$BIN_NAME.desktop"
}

# ---- uinput 权限 (仅 Wayland 会话必需; 内容与仓库 scripts/setup-uinput.sh 一致) ----
setup_uinput() {
  local rule="/etc/udev/rules.d/99-touch-keyboard-uinput.rules"
  local target_user="${SUDO_USER:-$(id -un)}"
  if [ -f "$rule" ] && id -nG "$target_user" 2>/dev/null | tr ' ' '\n' | grep -qx input; then
    msg "uinput 权限已配置, 跳过"
    return 0
  fi
  msg "配置 uinput 权限 (Wayland 会话按键注入必需) ..."
  if ! printf '%s\n' \
      '# 允许 input 组用户访问 uinput (触摸屏键盘注入按键所需)' \
      'KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"' \
      | $SUDO tee "$rule" >/dev/null; then
    warn "udev 规则写入失败 (当前用户可能无 sudo 权限), 跳过。请安装后手动执行 (然后注销重新登录):"
    echo "  echo 'KERNEL==\"uinput\", MODE=\"0660\", GROUP=\"input\", OPTIONS+=\"static_node=uinput\"' | sudo tee $rule"
    echo "  sudo groupadd -f input && sudo usermod -aG input \"$target_user\""
    return 1
  fi
  $SUDO groupadd -f input 2>/dev/null || true
  if command -v usermod >/dev/null 2>&1; then
    $SUDO usermod -aG input "$target_user" || true
  fi
  $SUDO udevadm control --reload-rules 2>/dev/null || true
  $SUDO udevadm trigger --name-match=uinput 2>/dev/null \
    || $SUDO modprobe uinput 2>/dev/null || true
  msg "已安装 udev 规则并把 $target_user 加入 input 组, 需注销重新登录 (或重启) 生效"
}

SESSION="${XDG_SESSION_TYPE:-}"
[ -n "$SESSION" ] || [ -z "${WAYLAND_DISPLAY:-}" ] || SESSION=wayland
if [ "$SESSION" = "wayland" ]; then
  if have_root; then
    setup_uinput || true   # 失败只警告 (函数内已提示), 不阻断安装
  else
    warn "Wayland 会话需要 uinput 权限, 但当前无法提权。请安装后手动执行 (然后注销重新登录):"
    echo "  echo 'KERNEL==\"uinput\", MODE=\"0660\", GROUP=\"input\", OPTIONS+=\"static_node=uinput\"' | sudo tee /etc/udev/rules.d/99-touch-keyboard-uinput.rules"
    echo "  sudo groupadd -f input && sudo usermod -aG input \"\$USER\""
  fi
else
  msg "提示: Wayland 会话需配置 uinput 权限后才能给原生应用打字 (X11 会话可跳过), 参见 README「uinput 权限配置」"
fi

# ---- 选择安装方式: Debian 系且能提权 → deb (装不上自动回落), 否则 AppImage (免 root) ----
if command -v apt-get >/dev/null 2>&1 && have_root && install_deb; then
  :
else
  install_appimage
fi

msg "安装完成: TouchKeyboard $TAG (再次运行本脚本即可升级到最新版)"
printf '%s\n' "$INSTALL_SUMMARY"
