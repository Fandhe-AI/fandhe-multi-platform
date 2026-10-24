#!/usr/bin/env bash
# Issue #1065（親 #1064）: macOS のホーム画面ウィジェット用ホストアプリ
# （FandheWidgetHost + 同梱の WidgetKit 拡張 FandheTextWidget）をビルド・導入する。
# `platforms/apple/project.yml`（SSOT）から xcodegen で .xcodeproj を作り、xcodebuild で
# ビルドする。xcodegen / Xcode は自動インストールしない（generate-xcodeproj.sh と同じ流儀）。
#
# 使い方:
#   scripts/apple/build-macos-widget-host.sh --build-only
#       署名なし（CODE_SIGNING_ALLOWED=NO）でビルドだけ確認する。CI 相当の確認用。
#   FANDHE_DEVELOPMENT_TEAM=<Team ID> scripts/apple/build-macos-widget-host.sh [--install]
#       署名ありでビルドする。--install で ~/Applications へ置き、1 回起動して
#       LaunchServices に登録する。Team ID はログに出さない。
#
# 成果物は target/apple-widget 配下（gitignore 済み）。/Applications やシステム設定は触らない。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
apple_dir="${repo_root}/platforms/apple"
derived="${repo_root}/target/apple-widget"
app_path="${derived}/Build/Products/Debug/FandheWidgetHost.app"

build_only=0
install=0
for arg in "$@"; do
  case "${arg}" in
    --build-only) build_only=1 ;;
    --install) install=1 ;;
    *)
      echo "[build-macos-widget-host] 不明な引数: ${arg}" >&2
      exit 2
      ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "[build-macos-widget-host] エラー: macOS 上でのみ実行できます。" >&2
  exit 1
fi
for tool in xcodegen xcodebuild; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "[build-macos-widget-host] エラー: ${tool} が見つかりません（brew install xcodegen / Xcode を導入してください）。" >&2
    exit 1
  fi
done

sign_args=()
if [[ "${build_only}" -eq 1 ]]; then
  if [[ "${install}" -eq 1 ]]; then
    echo "[build-macos-widget-host] エラー: --build-only と --install は併用できません。" >&2
    exit 2
  fi
  sign_args=(CODE_SIGNING_ALLOWED=NO)
else
  if [[ -z "${FANDHE_DEVELOPMENT_TEAM:-}" ]]; then
    echo "[build-macos-widget-host] エラー: FANDHE_DEVELOPMENT_TEAM が未設定です（署名なし確認は --build-only）。" >&2
    exit 1
  fi
  if [[ ! "${FANDHE_DEVELOPMENT_TEAM}" =~ ^[A-Z0-9]{10}$ ]]; then
    echo "[build-macos-widget-host] エラー: FANDHE_DEVELOPMENT_TEAM は英大文字・数字 10 桁です。" >&2
    exit 1
  fi
  sign_args=("DEVELOPMENT_TEAM=${FANDHE_DEVELOPMENT_TEAM}" -allowProvisioningUpdates)
fi

echo "[build-macos-widget-host] xcodegen generate"
xcodegen generate --spec "${apple_dir}/project.yml" --project "${apple_dir}" >/dev/null

echo "[build-macos-widget-host] xcodebuild (FandheWidgetHost)"
# DEVELOPMENT_TEAM を含む行はログから伏せる。
if ! xcodebuild -project "${apple_dir}/Fandhe.xcodeproj" -scheme FandheWidgetHost \
  -configuration Debug -destination 'platform=macOS' -derivedDataPath "${derived}" \
  "${sign_args[@]}" build 2>&1 | { grep -v 'DEVELOPMENT_TEAM' || true; } | tail -n 25; then
  echo "[build-macos-widget-host] エラー: xcodebuild が失敗しました。" >&2
  exit 1
fi

appex="${app_path}/Contents/PlugIns/FandheTextWidget.appex"
if [[ ! -d "${appex}" ]]; then
  echo "[build-macos-widget-host] エラー: 拡張が同梱されていません: ${appex}" >&2
  exit 1
fi
echo "[build-macos-widget-host] ビルド確認 OK: ${app_path}"

if [[ "${install}" -eq 1 ]]; then
  dest="${HOME}/Applications"
  mkdir -p "${dest}"
  rm -rf "${dest}/FandheWidgetHost.app"
  ditto "${app_path}" "${dest}/FandheWidgetHost.app"
  # 1 回起動して LaunchServices へ登録する（ホストは自分で終了する）。
  open -g "${dest}/FandheWidgetHost.app"
  echo "[build-macos-widget-host] 導入しました: ${dest}/FandheWidgetHost.app"
  echo "[build-macos-widget-host] ウィジェットの追加: 通知センターを開く →「ウィジェットを編集」→ 「Fandhe テキスト」を追加"
  echo "[build-macos-widget-host] 画面側: FANDHE_DEVELOPMENT_TEAM=<Team ID> FANDHE_PROXY_APP=home_widget_show cargo run -p fandhe-egui-proxy ..."
  echo "[build-macos-widget-host] コントロールの追加（macOS 26 以降）: コントロールセンターまたはメニューバーの編集 → 「Fandhe カウント」を追加して押す"
  echo "[build-macos-widget-host] 画面側: FANDHE_DEVELOPMENT_TEAM=<Team ID> FANDHE_PROXY_APP=quick_settings_tile_show cargo run -p fandhe-egui-proxy ..."
fi
