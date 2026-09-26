#!/usr/bin/env bash
# 焦点监视: 记录 _NET_ACTIVE_WINDOW 变化, 用于验证键盘是否抢焦点
# 用法: focus-watch.sh <时长秒> <输出文件>
DUR=${1:-20}
OUT=${2:-/tmp/focus.log}
: > "$OUT"
LAST=""
END=$((SECONDS + DUR))
while [ $SECONDS -lt $END ]; do
  AW=$(xprop -root _NET_ACTIVE_WINDOW 2>/dev/null | grep -oE "window id # 0x[0-9a-f]+" | grep -oE "0x[0-9a-f]+")
  if [ "$AW" != "$LAST" ]; then
    NAME=$(xdotool getwindowname "$AW" 2>/dev/null || echo "?")
    echo "$(date +%H:%M:%S.%3N) ACTIVE=$AW ($NAME)" >> "$OUT"
    LAST="$AW"
  fi
  sleep 0.1
done
