#!/usr/bin/env bash
# workspace 全クレートの `unsafe` 使用状況を一覧化・監査する再現可能スクリプト。
#
# 対応: REQ-7 / TASK-7.1（docs/spec/05-tasks.md）。
# 呼び出し元: 開発者がローカルで直接実行する（`docs/architecture/unsafe-audit.md` の
# 「監査手順」から参照される）。現時点では CI への組み込みは行っていない
# （PR 本文の申し送り事項を参照）。
#
# 処理内容:
#   1. `cargo metadata` で workspace メンバーのクレート情報（名前・クレートルート・
#      対象ディレクトリ）を取得する（jq で構造的にパースし、シェル展開に生入力を渡さない）。
#   2. 各クレートルート（lib.rs / main.rs / build.rs 等のターゲット src_path）に
#      `#![forbid(unsafe_code)]` が付与されているか検査する
#      （FORBID_EXEMPT_CRATES に列挙したクレートは対象外。coding-rust.md が forbid を
#      求めるのは共通コアのみで、FFI 境界を持つプラットフォーム統合レイヤーは対象外のため、
#      新規クレート追加時は本リストへ明示的に登録する運用とする）。
#   3. 各クレートの src/ tests/ examples/ benches/ 配下および crate root 直下の build.rs
#      から `unsafe` トークンを単語境界一致（grep -w）で検出する。コメント行
#      （trim 後に `//` から始まる行。`#![forbid(unsafe_code)]` の行を含む）は
#      「使用箇所」として数えない。
#
# SAFETY コメントの有無はこのスクリプトでは判定しない。clippy の lint が構文を読んで検査する
# （workspace の `[workspace.lints]`: `undocumented_unsafe_blocks`・`missing_safety_doc`・
# `unsafe_op_in_unsafe_fn`、非公開の `unsafe fn` は `clippy.toml` の `check-private-items`。
# Issue #1269、受け付ける形は docs/architecture/unsafe-audit.md）。行単位の照合は別の文の
# SAFETY を借りる穴を塞げないため廃止した。
#
# 違反判定（exit 1）:
#   - クレートルートに `#![forbid(unsafe_code)]` が無い（FORBID_EXEMPT_CRATES 記載クレートを除く）
#
# macOS 標準の BSD ツール（sed 等）でも最後まで走ること（#1264）。GNU 専用の書き方を足さない。
#
# 出力: クレートごとに forbid 適用有無・unsafe 検出件数・各検出箇所（ファイル:行）を表示し、
# 最後に集計（forbid 適用クレート数 / 全クレート数、unsafe 総検出件数）を出力する。
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." >/dev/null 2>&1 && pwd -P)"
cd -- "${REPO_ROOT}"

# forbid(unsafe_code) の適用を免除するクレート名の一覧（SSOT）。
# coding-rust.md の規約上、共通コアには forbid を必須とするが、FFI 境界を持つ
# プラットフォーム統合レイヤーのクレートは forbid を付けられない想定のため、
# 該当クレートを追加する際はここへ明示的に登録する（scripts/android/versions.env と
# 同様に、監査基準の変更を diff で追跡可能にする狙い）。
#
# fandhe-egui-proxy: Android・iOS ターゲットのみ `#![forbid(unsafe_code)]` を
# `#![cfg_attr(not(any(target_os = "android", target_os = "ios")), forbid(unsafe_code))]`
# へ緩めており（android.rs の `#[no_mangle]` エントリ、TASK-1.5-S2。device/ios.rs の
# FFI 呼び出し、Issue #751）、本スクリプトが grep -qF で検索する厳密な
# `#![forbid(unsafe_code)]` の文字列と一致しない。
# lib.rs 冒頭のドキュメンテーションコメントに同文字列が偶然含まれるため現状は
# 判定を通過するが、コメント文言の変更で判定が壊れる偶然の一致に依存しないよう
# 明示的に免除登録する（Bugbot 指摘対応）。
#
# fandhe-swiftui-bridge: クレートルートは forbid 属性ではなく `#![deny(unsafe_code)]`
# を採用する（`ffi` モジュールのみ `unsafe` を許可し他モジュールは禁止するという
# 設計上の境界を forbid では表現できないため、
# docs/architecture/swiftui-bridge-architecture.md §3.1「unsafe 境界の規約」・
# TASK-3.2-S3 / #60 参照）。forbid ではなく deny を採用している事実を、
# grep -qF の偶然の一致に頼らずここへ明示的に登録することで正しく反映する
# （fandhe-egui-proxy の運用に倣う）。
#
# fandhe-winit-ime: クレートルートは forbid 属性ではなく `#![deny(unsafe_code)]` を
# 採用する（`macos_secure_input` モジュールのみ Carbon FFI（SecureEventInput）で
# `unsafe` が不可避なため、`ImePurpose::Password` の学習・予測変換抑止を代替担保する
# Issue #586 で追加。fandhe-swiftui-bridge と同じ理由・同じ方式）。
#
# fandhe-platform-android: クレートルートは forbid ではなく `#![deny(unsafe_code)]`。
# `with_activity`（JavaVM と Activity の取得）だけが `unsafe` を許可する（Issue #1166）。
# fandhe-platform-apple: クレートルートは forbid 属性ではなく `#![deny(unsafe_code)]` を
# 採用する（`main_thread` モジュールのみ `pthread_main_np` の FFI 宣言と呼び出しで `unsafe` が
# 不可避なため。fandhe-egui-proxy の `device/ios.rs` から移した 1 組で、TASK-21.2 / Issue #1168）。
# fandhe-platform-windows: クレートルートは forbid ではなく `#![deny(unsafe_code)]`。
# `ComInit` の `CoInitializeEx` / `CoUninitialize` の 2 呼び出しだけが `unsafe` を許可する
# （fandhe-egui-proxy の `windows_notification.rs` から移した 1 組で、TASK-21.4 / Issue #1184）。
# fandhe-cap-camera-capture: Android・iOS のみ `deny(unsafe_code)` へ緩める（cfg_attr）。Android の JNI エクスポート
# `#[unsafe(no_mangle)]` 1 関数と iOS の ObjC 呼び出し（FFI）にだけ `#[allow(unsafe_code)]` を付ける（Issue #1170）。
# fandhe-cap-share-receive: Android・macOS ターゲットのみ `forbid` を `deny` へ緩める
# （`cfg_attr`）。unsafe は Android の JNI 実体（`#[unsafe(no_mangle)]` 1 関数）と、macOS の
# FFI 宣言・呼び出し・`from_raw_parts` に限る（共有の受け取り、TASK-21.3 / Issue #1173）。
# fandhe-gallery: クレートルートは Android のみ `deny(unsafe_code)` で、`android_main` の
# `#[no_mangle]` 1 関数にだけ `#[allow(unsafe_code)]` を付ける（android-activity の固定シンボルは
# 最終成果物の cdylib が持つ必要がある。Issue #1165）。
# fandhe-cap-document-scan: クレートルートは iOS・Android のみ `deny(unsafe_code)`（それ以外の OS は
# forbid）。iOS の FFI（`ios.rs`）と Android の JNI エクスポート（`android.rs`）にだけ SAFETY 付きで
# `allow` を付ける（機能 crate に OS 実装を持たせるため。TASK-21.3 / Issue #1171）。
FORBID_EXEMPT_CRATES=("fandhe-gallery" "fandhe-egui-proxy" "fandhe-swiftui-bridge" "fandhe-winit-ime" "fandhe-platform-android" "fandhe-platform-apple" "fandhe-platform-windows" "fandhe-cap-camera-capture" "fandhe-cap-share-receive" "fandhe-cap-document-scan")

is_forbid_exempt() {
  local target="$1"
  local exempt
  for exempt in "${FORBID_EXEMPT_CRATES[@]:-}"; do
    [ "${exempt}" = "${target}" ] && return 0
  done
  return 1
}

# grep -n の 1 マッチ（file:line_no）を判定する共通処理。呼び出し元スコープの
# crate_unsafe_count / total_unsafe_count を更新する。
# ディレクトリ走査（grep -rn）・単一ファイル走査（build.rs 向け grep -n）の
# 両方から同一ロジックで呼べるようにするための切り出し。
judge_unsafe_match() {
  local file="$1" line_no="$2"
  local line_content trimmed_content

  line_content="$(sed -n "${line_no}p" "${file}")"
  # コメント行（trim 後に // から始まる行。#![forbid(unsafe_code)] や
  # 「unsafe は禁止」のような日本語コメント中の言及を誤検出しないため）は
  # 使用箇所としてカウントしない。
  trimmed_content="${line_content#"${line_content%%[![:space:]]*}"}"
  case "${trimmed_content}" in
    '//'*) return 0 ;;
  esac

  crate_unsafe_count=$((crate_unsafe_count + 1))
  total_unsafe_count=$((total_unsafe_count + 1))
  echo "  unsafe 検出: ${file}:${line_no}: ${line_content}"
}

if ! command -v cargo >/dev/null 2>&1; then
  echo "エラー: cargo が見つかりません。Rust ツールチェーンを導入してください。" >&2
  exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "エラー: jq が見つかりません。" >&2
  exit 1
fi

violation_count=0
crate_count=0
forbid_ok_count=0
total_unsafe_count=0

echo "=== unsafe 監査（対象: workspace 全クレート） ==="
echo "（SAFETY コメントの検査は clippy の lint が行う。docs/architecture/unsafe-audit.md 参照）"
echo "対象コミット: $(git rev-parse HEAD 2>/dev/null || echo '不明（git 管理外）')"
echo

# cargo metadata の出力を jq で構造的にパースし、各クレートについて
# 「名前 \t クレートルート（lib/bin ターゲットの src_path、複数ある場合は US(0x1f) 区切り） \t クレートディレクトリ」
# を TSV で受け取る（自由入力を含まないため安全）。
# 区切り文字に \n を使うと @tsv がエスケープして 1 行にまとめてしまうため、
# パスに出現し得ない US (unit separator, 0x1f) を用いる。
#
# process substitution (`< <(...)`) にパイプを直接渡すと、その中で
# cargo metadata / jq が失敗しても `set -e` / pipefail はメインシェルの
# コマンドとして扱わないため検知できず、crate_count が 0 のまま
# 「監査結果: 違反なし」と誤判定してしまう。そのためコマンド置換
# `var="$(... | ...)"` に一度収め、代入文自体の終了ステータスに
# `set -e` を効かせて失敗を検知できるようにする。
metadata_tsv="$(
  cargo metadata --no-deps --format-version 1 |
    jq -r '
      .packages[] |
      [
        .name,
        ([.targets[].src_path] | join("")),
        (.manifest_path | rtrimstr("/Cargo.toml"))
      ] | @tsv
    '
)"

if [ -z "${metadata_tsv}" ]; then
  echo "エラー: cargo metadata / jq からクレート情報を取得できませんでした（0 パッケージ）。" >&2
  exit 1
fi

while IFS=$'\t' read -r pkg_name crate_roots crate_dir; do
  crate_count=$((crate_count + 1))
  echo "--- クレート: ${pkg_name} (${crate_dir}) ---"

  # このクレートに forbid 属性が 1 箇所でも付与されていれば適用済みとみなす。
  crate_forbid_ok=0
  while IFS= read -r -d $'\x1f' root_path || [ -n "${root_path}" ]; do
    [ -z "${root_path}" ] && continue
    if [ -f "${root_path}" ] && grep -qF '#![forbid(unsafe_code)]' -- "${root_path}"; then
      crate_forbid_ok=1
      echo "  forbid(unsafe_code): 適用済み（${root_path}）"
    fi
  done <<< "${crate_roots}"$'\x1f'

  if [ "${crate_forbid_ok}" -eq 1 ]; then
    forbid_ok_count=$((forbid_ok_count + 1))
  elif is_forbid_exempt "${pkg_name}"; then
    echo "  forbid(unsafe_code): 未適用（FORBID_EXEMPT_CRATES により免除）"
  else
    echo "  forbid(unsafe_code): 未適用"
    echo "  違反: クレートルートに #![forbid(unsafe_code)] がありません" >&2
    violation_count=$((violation_count + 1))
  fi

  # src/ tests/ examples/ benches/ および crate root 直下の build.rs から
  # unsafe トークンを検出する（コメント行は使用箇所としてカウントしない）。
  crate_unsafe_count=0
  for sub_dir in src tests examples benches; do
    target_dir="${crate_dir}/${sub_dir}"
    [ -d "${target_dir}" ] || continue

    while IFS=: read -r file line_no _rest; do
      [ -z "${file}" ] && continue
      judge_unsafe_match "${file}" "${line_no}"
    done < <(grep -rn --include='*.rs' -w 'unsafe' -- "${target_dir}" || true)
  done

  build_rs="${crate_dir}/build.rs"
  if [ -f "${build_rs}" ]; then
    while IFS=: read -r file line_no _rest; do
      [ -z "${file}" ] && continue
      judge_unsafe_match "${file}" "${line_no}"
    done < <(grep -Hn -w 'unsafe' -- "${build_rs}" || true)
  fi

  if [ "${crate_unsafe_count}" -eq 0 ]; then
    echo "  unsafe 検出件数: 0"
  fi
  echo
done <<< "${metadata_tsv}"

echo "=== 集計 ==="
echo "クレート数: ${crate_count}"
echo "forbid(unsafe_code) 適用クレート数: ${forbid_ok_count}/${crate_count}"
echo "unsafe 検出件数（合計）: ${total_unsafe_count}"

if [ "${violation_count}" -gt 0 ]; then
  echo "監査結果: 違反 ${violation_count} 件" >&2
  exit 1
fi

echo "監査結果: 違反なし"
exit 0
