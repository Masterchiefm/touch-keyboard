#!/usr/bin/env bash
# 触屏键盘 uinput 权限配置脚本 (Wayland 会话必需)
# 用法: sudo bash scripts/setup-uinput.sh
set -e

RULE="/etc/udev/rules.d/99-touch-keyboard-uinput.rules"
SRC="$(cd "$(dirname "$0")" && pwd)/99-touch-keyboard-uinput.rules"

cp "$SRC" "$RULE"
groupadd -f input 2>/dev/null || true
usermod -aG input "$SUDO_USER" || true
udevadm control --reload-rules
udevadm trigger --name-match=uinput || modprobe uinput || true

echo "完成: 已安装 $RULE 并将 $SUDO_USER 加入 input 组。"
echo "请注销并重新登录一次 (或重启) 使组权限生效, 然后运行触摸屏键盘。"
