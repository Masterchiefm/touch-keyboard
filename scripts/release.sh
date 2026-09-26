#!/usr/bin/env bash
# 发版脚本 (版本管理): 版本号只写在 src-tauri/tauri.conf.json 与 Cargo.toml,
# 两处必须一致 (应用内更新检测读 tauri.conf.json 的版本, Cargo.lock 随 Cargo.toml)。
#
# 用法: scripts/release.sh 0.2.0
# 效果: 同步两处版本号 → 提交 → 打 v0.2.0 标签 → 推送;
#       v* 标签触发 .github/workflows/build.yml 自动构建 .deb/.AppImage
#       并发布 GitHub Release (应用内「新版本检测」的检测目标)。
set -euo pipefail

cd "$(dirname "$0")/.."

if [ $# -ne 1 ]; then
  echo "用法: $0 <版本号>  例: $0 0.2.0" >&2
  exit 1
fi
VER="$1"
[[ "$VER" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "版本号须为 x.y.z 形式: $VER" >&2; exit 1; }

CONF=src-tauri/tauri.conf.json
CARGO=src-tauri/Cargo.toml

python3 - "$CONF" "$VER" <<'EOF'
import json, sys
p, ver = sys.argv[1], sys.argv[2]
with open(p) as f:
    conf = json.load(f)
conf["version"] = ver
with open(p, "w") as f:
    json.dump(conf, f, indent=2, ensure_ascii=False)
    f.write("\n")
print(f"{p} -> {ver}")
EOF

# Cargo.toml 的 [package] version 只在首次出现处替换
sed -i "0,/^version = /s/^version = .*/version = \"$VER\"/" "$CARGO"
grep -m1 "^version = " "$CARGO"
# 刷新 Cargo.lock 里的自身版本
(cd src-tauri && cargo update -p touch-keyboard --offline -q 2>/dev/null || cargo update -p touch-keyboard -q)

git add "$CONF" "$CARGO" src-tauri/Cargo.lock
git commit -m "release: v$VER"
git tag "v$VER"
echo "已提交并打标签 v$VER。推送触发 CI 构建发布:"
echo "  git push origin main --tags"
