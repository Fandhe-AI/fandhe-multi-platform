#!/usr/bin/env bash
# Issue #1215（親 #1116）: カスタム URL scheme（fandhe://）の Mac 目視用に、デスクトップ実行体
# （fandhe-egui-proxy-desktop）を最小の .app に包み、LaunchServices に登録する。
#
# なぜ必要か: CFBundleURLTypes は LaunchServices に登録された .app でしか効かない。素の実行体に
# macos/Info.plist を埋め込んでも scheme は届かない。そのため macos/Info.plist（SSOT）を
# .app にコピーし、CFBundleURLTypes と LSEnvironment（FANDHE_PROXY_APP=url_scheme_show。
# LaunchServices 経由の起動は環境変数が渡らないため）を足す。配布用の設定は変えない。
#
# Issue #416（親 #102）: ファイルの関連付け（.txt）の Mac 目視にも同じ .app を使う。
# CFBundleDocumentTypes（public.plain-text、LSHandlerRank=Alternate で既定アプリを奪わない）を
# 常に足し、--screen で LSEnvironment の画面を選ぶ。
#
# 出力: target/macos-url-scheme/FandheUrlScheme.app（git 管理外、ad-hoc 署名のローカル確認用）
#
# 使い方:
#   scripts/apple/bundle-macos-url-scheme.sh              # ビルド + .app 化 + lsregister
#   scripts/apple/bundle-macos-url-scheme.sh --build-only # lsregister を省く（検査用）
#   scripts/apple/bundle-macos-url-scheme.sh --screen file_association_show
#                                                         # 起動する画面を選ぶ（既定 url_scheme_show）
#
# Mac 目視の手順（人手確認は #1212 で追う）:
#   起動済み:       open target/macos-url-scheme/FandheUrlScheme.app
#                   open 'fandhe://hello?x=1'   -> 画面に URL と受信時刻が出る
#   コールドスタート: アプリを終了してから open 'fandhe://hello?x=1'
#                   -> アプリが起動し、起動直後の画面に URL が出る
#   失敗系:         open 'fandhe:' や制御文字を含む URL を送ると「検査で弾いた件数」だけ増え、
#                   URL は表示されない（https:// 等は別 scheme のためそもそも届かない）
#
# ファイルの関連付けの Mac 目視（人手確認は #414 で追う）:
#   --screen file_association_show で作り、Finder で .txt を右クリック -> 「このアプリケーションで開く」
#   起動済み / コールドスタートの両方で、名前・サイズ・先頭 16 バイトが出る
#   失敗系:         フォルダ名を x.txt にしたもの・読み取り権限のない .txt・8 MiB 超の .txt は
#                   「開けませんでした」と理由が出る（成功にならない）。.md は届かない
set -euo pipefail

if [ "$(uname)" != "Darwin" ]; then
  echo "macOS 専用です" >&2
  exit 1
fi

build_only=0
screen=url_scheme_show
while [ $# -gt 0 ]; do
  case "$1" in
    --build-only) build_only=1; shift ;;
    --screen)
      [ $# -ge 2 ] || { echo "使い方: $0 [--build-only] [--screen <name>]" >&2; exit 2; }
      screen="$2"; shift 2 ;;
    *) echo "使い方: $0 [--build-only] [--screen <name>]" >&2; exit 2 ;;
  esac
done
# 画面名は許可リストで照合する（plist の JSON へ未検証の文字列を入れない。A03）。
case "$screen" in
  url_scheme_show|file_association_show) ;;
  *) echo "未対応の画面名です: --screen は url_scheme_show か file_association_show" >&2; exit 2 ;;
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
plutil -insert CFBundleDocumentTypes -json \
  '[{"CFBundleTypeName":"Plain Text","CFBundleTypeRole":"Viewer","LSHandlerRank":"Alternate","LSItemContentTypes":["public.plain-text"]}]' "$plist"
# $screen は上の許可リストを通った固定の識別子だけ。
plutil -insert LSEnvironment -json "{\"FANDHE_PROXY_APP\":\"$screen\"}" "$plist"
plutil -lint "$plist"

codesign --force --sign - "$app"

if [ "$build_only" -eq 0 ]; then
  lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
  "$lsregister" -f "$app"
fi

echo "作成: $app"
