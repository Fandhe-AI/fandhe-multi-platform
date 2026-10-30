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
#
# Issue #606（親 #122）: 共有の受け取り（share_receive_show）の Mac 目視にも同じ .app を使う。
# NSServices（サービスメニュー「Fandhe で受け取る」、NSSendTypes は public.utf8-plain-text）を
# 常に足し、--screen share_receive_show で起動する画面を選ぶ。メニュー項目名は
# crates/fandhe-core/src/screens/share_receive_show.rs の SERVICE_MENU_TITLE と一致させる。
# NSPortName は NSApp.servicesProvider が登録するポート名と一致させる必要があるため、
# CFBundleName を実行体名へ揃えて両方に同じ値を使う（どちらの規則でも一致する）。
#
# 共有の受け取りの Mac 目視（人手確認は #601）:
#   --screen share_receive_show で作り、テキストエディットで文字を選び、
#   メニューの「サービス」から「Fandhe で受け取る」を選ぶ（右クリックのサービスでも可）
#   起動中:         本文・文字数・受信時刻が出る
#   コールドスタート: アプリを終了した状態から送る（届くかは目視で確かめる。届かなければ
#                   対応表の「守らない範囲」へ移す）
#   失敗系:         16 KiB 超を送ると「検査で弾いた件数」だけ増え、本文は出ない。
#                   素の cargo run ではサービスに出ず、画面は「まだ受け取っていません」のまま
#   サービスに出ないとき: 一度ログアウト/ログインするか、/System/Library/CoreServices/pbs -flush
#                   でサービスのキャッシュを捨てる（反映が遅れることがある）
#
# Issue #1073（親 #137）: アプリ別の言語設定の Mac 目視（#1054）にも同じ .app を使う。
# --screen per_app_language_show で作り、システム設定 → 一般 → 言語と地域 → アプリケーションで
# このアプリの言語を変えて再起動し、「言語を読む」でタグが変わるかを確かめる。
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
  url_scheme_show|file_association_show|share_receive_show|per_app_language_show) ;;
  *) echo "未対応の画面名です: --screen は url_scheme_show / file_association_show / share_receive_show / per_app_language_show" >&2; exit 2 ;;
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
# NSServices の NSPortName と servicesProvider のポート名を一致させるため、名前を実行体名に揃える。
plutil -replace CFBundleName -string "$exe" "$plist"
plutil -insert CFBundlePackageType -string APPL "$plist"
plutil -insert CFBundleURLTypes -json \
  '[{"CFBundleURLName":"com.fandhe.egui-proxy-desktop.url","CFBundleURLSchemes":["fandhe"]}]' "$plist"
plutil -insert CFBundleDocumentTypes -json \
  '[{"CFBundleTypeName":"Plain Text","CFBundleTypeRole":"Viewer","LSHandlerRank":"Alternate","LSItemContentTypes":["public.plain-text"]}]' "$plist"
# サービスメニュー（Issue #606）。NSMenuItem.default は SERVICE_MENU_TITLE、NSMessage は
# macos/fandhe_macos_share_receive.m の receiveSharedText:userData:error: と対。
plutil -insert NSServices -json \
  "[{\"NSMenuItem\":{\"default\":\"Fandhe で受け取る\"},\"NSMessage\":\"receiveSharedText\",\"NSPortName\":\"$exe\",\"NSSendTypes\":[\"public.utf8-plain-text\"]}]" "$plist"
# $screen は上の許可リストを通った固定の識別子だけ。
plutil -insert LSEnvironment -json "{\"FANDHE_PROXY_APP\":\"$screen\"}" "$plist"
plutil -lint "$plist"

codesign --force --sign - "$app"

if [ "$build_only" -eq 0 ]; then
  lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
  "$lsregister" -f "$app"
fi

echo "作成: $app"
