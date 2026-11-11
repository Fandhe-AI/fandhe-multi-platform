#!/usr/bin/env bash
# scripts/audit/unsafe-audit.sh の has_forbid_attr の自己チェック（Issue #1183）。
#
# スクリプト本体は実行すると監査が走るため、関数定義だけを抜き出して一時ファイルの
# crate ルートに対して判定させる。誤判定 4 例が「未適用」、正しい形が「適用済み」になることを assert する。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target_script="${script_dir}/../unsafe-audit.sh"
work_dir="$(mktemp -d)"
trap 'rm -rf "${work_dir}"' EXIT

eval "$(sed -n '/^has_forbid_attr() {/,/^}/p' "${target_script}")"

fail_count=0
# check <説明> <期待: ok|ng> <ファイル内容>
check() {
  local desc="$1" expected="$2" f="${work_dir}/lib.rs" actual=ng
  printf '%s\n' "$3" >"${f}"
  has_forbid_attr "${f}" && actual=ok
  if [[ "${expected}" == "${actual}" ]]; then
    echo "[has-forbid-attr-test] OK: ${desc}"
  else
    echo "[has-forbid-attr-test] NG: ${desc}（期待: ${expected} / 実際: ${actual}）" >&2
    fail_count=$((fail_count + 1))
  fi
}

check '単独 forbid' ok '  #![ forbid( unsafe_code ) ]'
check 'not(target_os) と対の deny' ok '#![cfg_attr(not(target_os = "ios"), forbid(unsafe_code))]
#![cfg_attr(target_os = "ios", deny(unsafe_code))]'
check 'not(any(..)) 複数行折り返しと対の deny' ok '#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]'

check '誤: feature 条件' ng '#![cfg_attr(not(feature = "x"), forbid(unsafe_code))]'
check '誤: 外側属性' ng '#[cfg_attr(not(unix), forbid(unsafe_code))] fn f() {}'
check '誤: 別々の行の属性をまたぐ' ng '#![cfg_attr(not(test), allow(x))]
#![cfg_attr(any(a, b), forbid(unsafe_code))]'
check '誤: ブロックコメント内' ng '/* #![forbid(unsafe_code)] */'
check '誤: 複数行ブロックコメント内' ng '/*
#![forbid(unsafe_code)]
*/'
check '誤: 行コメント内' ng '// #![forbid(unsafe_code)]'
check '誤: 対の deny が無い' ng '#![cfg_attr(not(target_os = "ios"), forbid(unsafe_code))]'
check '誤: 対の deny の OS が違う' ng '#![cfg_attr(not(target_os = "ios"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]'

if [ "${fail_count}" -ne 0 ]; then
  echo "[has-forbid-attr-test] 失敗 ${fail_count} 件" >&2
  exit 1
fi
echo "[has-forbid-attr-test] 全て成功"
