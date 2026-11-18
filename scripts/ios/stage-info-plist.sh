#!/usr/bin/env bash
# カタログアプリの iOS バンドル（.app）へ Info.plist とプライバシーマニフェストを入れる（Issue #1194）。
#
# Info.plist は `cargo fandhe gen` の出力（crates/fandhe-gallery/gen/ios/Info.plist。機能 crate の宣言と
# アプリ設定から作る）を使い、手書きの Info.plist は持たない。生成は呼び出し側が先に
# scripts/build/gen-gallery.sh で行う。実行体名（CFBundleExecutable）は組み立て時の値のため、
# バンドル直下に置いた実行体の名前へ揃える（gen は cargo の bin 名を書く）。
# プライバシーマニフェストはアプリ全体の値として platforms/apple/FandheIosApp/Resources に手で持つ。
#
# 使い方: scripts/ios/stage-info-plist.sh <app_dir> <実行体名>
set -euo pipefail
unset CDPATH
[[ $# -eq 2 ]] || { echo "usage: $(basename "$0") <app_dir> <実行体名>" >&2; exit 2; }
app_dir="$1"
exe_name="$2"
# 実行体名は plist へ書くだけだが、パス区切りや制御文字は受け付けない（A03）。
[[ "${exe_name}" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "[stage-info-plist] 実行体名が不正です" >&2; exit 2; }
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." >/dev/null 2>&1 && pwd -P)"
src="${repo_root}/crates/fandhe-gallery/gen/ios/Info.plist"
privacy="${repo_root}/platforms/apple/FandheIosApp/Resources/PrivacyInfo.xcprivacy"
[[ -f "${src}" ]] || { echo "[stage-info-plist] ${src} が無い。先に scripts/build/gen-gallery.sh を実行する" >&2; exit 1; }
[[ -d "${app_dir}" ]] || { echo "[stage-info-plist] ${app_dir} が無い" >&2; exit 1; }

cp "${src}" "${app_dir}/Info.plist"
python3 - "${app_dir}/Info.plist" "${exe_name}" <<'PYEOF'
import plistlib
import sys

with open(sys.argv[1], "rb") as f:
    plist = plistlib.load(f)
plist["CFBundleExecutable"] = sys.argv[2]
with open(sys.argv[1], "wb") as f:
    plistlib.dump(plist, f)
PYEOF
cp "${privacy}" "${app_dir}/PrivacyInfo.xcprivacy"
