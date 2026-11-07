#!/usr/bin/env bash
# 成果物（APK / .app）に、選ばなかった機能の宣言が 0 件であることを検査する薄い入口。
#
# 対応: REQ-21 / TASK-21.3（Issue #1178）。判定の本体は `cargo fandhe verify-scope`
# （crates/cargo-fandhe/src/scope.rs）。ここは成果物から外部ツールの出力を集めて渡すだけ。
# 呼び出し元: 開発者のローカル、および最小アプリ側の CI。本体の必須 CI には載せない
# （判定ロジックは `cargo test -p cargo-fandhe` が守る）。
#
# 使い方:
#   verify-capability-scope.sh [--catalog <Cargo.toml>] [--manifest-path <アプリの Cargo.toml>] \
#       android --apk <apk> [--gradle-deps <gradle dependencies の出力ファイル>]
#   verify-capability-scope.sh [--catalog <Cargo.toml>] [--manifest-path <アプリの Cargo.toml>] \
#       ios|macos --app <X.app>
#
# 集める観測:
#   android: aapt2 dump xmltree（AndroidManifest.xml）、任意で Gradle の依存ツリー
#   ios/macos: codesign の entitlements、otool -L、plutil -p Info.plist、PlugIns/ の一覧
#              （拡張の appex も同じ観測を足す）。未署名で entitlements が取れなければ警告して続行
# 秘密情報は扱わない（引数はファイルパスのみ）。一時ファイルは mktemp -d 配下に作り終了時に削除する。
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." >/dev/null 2>&1 && pwd -P)"

die() {
  echo "エラー: $*" >&2
  exit 1
}

catalog=()
manifest=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --catalog) [[ $# -ge 2 ]] || die "--catalog に値が要ります"; catalog=(--catalog "$2"); shift 2 ;;
    --manifest-path) [[ $# -ge 2 ]] || die "--manifest-path に値が要ります"; manifest=(--manifest-path "$2"); shift 2 ;;
    *) break ;;
  esac
done
os="${1:-}"
[[ -n "${os}" ]] || die "OS（android / ios / macos）を指定してください"
shift

apk=""
app=""
gradle_deps=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --apk) [[ $# -ge 2 ]] || die "--apk に値が要ります"; apk="$2"; shift 2 ;;
    --app) [[ $# -ge 2 ]] || die "--app に値が要ります"; app="$2"; shift 2 ;;
    --gradle-deps) [[ $# -ge 2 ]] || die "--gradle-deps に値が要ります"; gradle_deps="$2"; shift 2 ;;
    *) die "未知の引数: $1" ;;
  esac
done

work="$(mktemp -d)"
trap 'rm -rf -- "${work}"' EXIT
obs="${work}/observed.txt"
: >"${obs}"

find_aapt2() {
  if command -v aapt2 >/dev/null 2>&1; then
    command -v aapt2
    return
  fi
  local sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
  [[ -n "${sdk}" && -d "${sdk}/build-tools" ]] || return 1
  local found
  found="$(find "${sdk}/build-tools" -name aapt2 -type f 2>/dev/null | sort | tail -n 1)"
  [[ -n "${found}" ]] || return 1
  printf '%s\n' "${found}"
}

observe_android() {
  [[ -f "${apk}" ]] || die "--apk に APK ファイルを指定してください"
  local aapt2
  aapt2="$(find_aapt2)" || die "aapt2 が見つかりません。ANDROID_HOME を設定するか PATH に通してください（scripts/android/android-env.sh 参照）"
  "${aapt2}" dump xmltree --file AndroidManifest.xml "${apk}" >>"${obs}" ||
    die "aapt2 が失敗しました"
  if [[ -n "${gradle_deps}" ]]; then
    [[ -f "${gradle_deps}" ]] || die "--gradle-deps のファイルがありません"
    cat -- "${gradle_deps}" >>"${obs}"
  fi
}

# 1 つのバンドル（アプリ本体または appex）の観測を足す。
observe_bundle() {
  local bundle="$1" plist exe
  if [[ "${os}" == "macos" && -d "${bundle}/Contents" ]]; then
    plist="${bundle}/Contents/Info.plist"
    exe="${bundle}/Contents/MacOS"
  else
    plist="${bundle}/Info.plist"
    exe="${bundle}"
  fi
  [[ -f "${plist}" ]] || die "${plist} がありません"
  plutil -p "${plist}" >>"${obs}"
  local name
  name="$(plutil -extract CFBundleExecutable raw -o - "${plist}")" || die "CFBundleExecutable を読めません"
  otool -L "${exe}/${name}" >>"${obs}" || die "otool が失敗しました"
  codesign -d --entitlements :- "${bundle}" >>"${obs}" 2>/dev/null ||
    echo "警告: ${bundle} の entitlements を取れません（未署名）。Info.plist と framework だけ照合します" >&2
}

observe_apple() {
  [[ "$(uname -s)" == "Darwin" ]] || die "Apple 成果物の検査は macOS でのみ実行できます"
  [[ -d "${app}" ]] || die "--app に .app ディレクトリを指定してください"
  observe_bundle "${app}"
  local plugins="${app}/PlugIns"
  [[ "${os}" == "macos" && -d "${app}/Contents/PlugIns" ]] && plugins="${app}/Contents/PlugIns"
  if [[ -d "${plugins}" ]]; then
    local appex
    for appex in "${plugins}"/*.appex; do
      [[ -d "${appex}" ]] || continue
      basename -- "${appex}" >>"${obs}"
      observe_bundle "${appex}"
    done
  fi
}

case "${os}" in
  android) observe_android ;;
  ios | macos) observe_apple ;;
  *) die "未知の OS: ${os}（android / ios / macos）" ;;
esac

cd -- "${REPO_ROOT}"
if command -v cargo-fandhe >/dev/null 2>&1; then
  runner=(cargo-fandhe)
else
  runner=(cargo run -q -p cargo-fandhe --)
fi
"${runner[@]}" verify-scope ${catalog[@]+"${catalog[@]}"} ${manifest[@]+"${manifest[@]}"} --os "${os}" "${obs}"
