#!/usr/bin/env bash
# Xvfb 冒烟测试: 启动虚拟显示器 -> 启动键盘 -> 截图 -> 模拟点击按键 -> 验证 XTEST 注入
set -x
export DISPLAY=:99

pkill -f "Xvfb :99" 2>/dev/null; sleep 0.5
Xvfb :99 -screen 0 1280x720x24 &
sleep 1.5

pkill -f "target/debug/touch-keyboard" 2>/dev/null; sleep 0.5
cd "$(dirname "$0")/../src-tauri"
WEBKIT_DISABLE_COMPOSITING_MODE=1 ./target/debug/touch-keyboard > /tmp/tk.log 2>&1 &
APP_PID=$!
sleep 10

echo "=== 进程状态 ==="
ps -p $APP_PID -o pid,cmd | tail -1
echo "=== 应用日志 ==="
cat /tmp/tk.log
echo "=== 窗口树 ==="
xwininfo -root -tree 2>/dev/null | grep -v "^     children" | head -30
echo "=== 截图 ==="
import -window root /tmp/tk-full.png && echo OK
identify /tmp/tk-full.png
