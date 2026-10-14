#!/usr/bin/env bash
# Issue #1215（親 #1116）: カスタム URL scheme（fandhe://）の Mac 目視用に、デスクトップ実行体
# （fandhe-egui-proxy-desktop）を最小の .app に包み、LaunchServices に登録する。
#
# なぜ必要か: CFBundleURLTypes は LaunchServices に登録された .app でしか効かない。素の実行体に
# macos/Info.plist を埋め込んでも scheme は届かない。そのため macos/Info.plist（SSOT）を
# .app にコピーし、CFBundleURLTypes と LSEnvironment（FANDHE_PROXY_APP=url_scheme_show。
# LaunchServices 経由の起動は環境変数が渡らないため）を足す。配布用の設定は変えない。
#
# 出力: target/macos-url-scheme/FandheUrlScheme.app（git 管理外、ad-hoc 署名のローカル確認用）
#
# 使い方:
#   scripts/apple/bundle-macos-url-scheme.sh              # ビルド + .app 化 + lsregister
#   scripts/apple/bundle-macos-url-scheme.sh --build-only # lsregister を省く（検査用）
#
# Mac 目視の手順（人手確認は #1212 で追う）:
#   起動済み:       open target/macos-url-scheme/FandheUrlScheme.app
#                   open 'fandhe://hello?x=1'   -> 画面に URL と受信時刻が出る
#   コールドスタート: アプリを終了してから open 'fandhe://hello?x=1'
#                   -> アプリが起動し、起動直後の画面に URL が出る
#   失敗系:         open 'fandhe:' や制御文字を含む URL を送ると「検査で弾いた件数」だけ増え、
#                   URL は表示されない（https:// 等は別 scheme のためそもそも届かない）
set -euo pipefail

if [ "$(uname)" != "Darwin" ]; then
  echo "macOS 専用です" >&2
  exit 1
fi

build_only=0
case "${1:-}" in
  "") ;;
  --build-only) build_only=1 ;;
  *) echo "使い方: $0 [--build-only]" >&2; exit 2 ;;
esac

root="$(cd "$(dirname "$0")/../.." && pwd)"
app="$root/target/macos-url-scheme/FandheUrlScheme.app"
plist="$app/Contents/Info.plist"
exe=fandhe-egui-proxy-desktop

(cd "$root" && cargo build -p fandhe-egui-proxy --bin "$exe")

rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$root/target/debug/$exe" "$app/Contents/MacOS/$exe"
cp "$root/crates/fandhe-egui-proxy/macos/Info.plist" "$plist"

plutil -insert CFBundleExecutable -string "$exe" "$plist"
plutil -insert CFBundlePackageType -string APPL "$plist"
plutil -insert CFBundleURLTypes -json \
  '[{"CFBundleURLName":"com.fandhe.egui-proxy-desktop.url","CFBundleURLSchemes":["fandhe"]}]' "$plist"
plutil -insert LSEnvironment -json '{"FANDHE_PROXY_APP":"url_scheme_show"}' "$plist"
plutil -lint "$plist"

codesign --force --sign - "$app"

if [ "$build_only" -eq 0 ]; then
  lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
  "$lsregister" -f "$app"
fi

echo "作成: $app"
