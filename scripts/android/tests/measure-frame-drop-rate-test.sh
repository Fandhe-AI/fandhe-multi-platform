#!/usr/bin/env bash
# scripts/android/measure-frame-drop-rate.sh の自己チェック（#647 S3 / Issue #658）。
#
# `--parse` のオフライン集計に閉じたケース（実機不要）と、不正パッケージ名で
# adb を一切呼ばないこと（A03 対策の検証）を assert する。
# assert_eq / assert_contains 様式・jq 非依存の方針を踏襲する。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target_script="${script_dir}/../measure-frame-drop-rate.sh"

work_dir="$(mktemp -d)"
trap 'rm -rf "${work_dir}"' EXIT

fail_count=0

assert_eq() {
  local desc="$1" expected="$2" actual="$3"
  if [[ "${expected}" != "${actual}" ]]; then
    echo "[measure-frame-drop-rate-test] NG: ${desc}（期待: ${expected} / 実際: ${actual}）" >&2
    fail_count=$((fail_count + 1))
  else
    echo "[measure-frame-drop-rate-test] OK: ${desc}"
  fi
}

assert_contains() {
  local desc="$1" haystack="$2" pattern="$3"
  if echo "${haystack}" | grep -qE "${pattern}"; then
    echo "[measure-frame-drop-rate-test] OK: ${desc}"
  else
    echo "[measure-frame-drop-rate-test] NG: ${desc}（${pattern} が見つかりません: ${haystack}）" >&2
    fail_count=$((fail_count + 1))
  fi
}

# --- フィクスチャ: 60Hz（周期 16666667ns）、間隔は 1,1,1,2,3 周期 + 無効行 2 件 ---
fixture="${work_dir}/fixture.latency"
cat >"${fixture}" <<'EOF'
16666667
0 100000000 100000000
0 116666667 116666667
0 133333334 133333334
0 150000001 150000001
0 183333335 183333335
0 233333338 233333338
0 0 0
0 9223372036854775807 9223372036854775807
EOF

# --- ケース A: 既定閾値（2 周期）での集計 ---
out_a="$("${target_script}" --parse "${fixture}" --package com.example.a --layer layerA)"
assert_contains "ケース A: JSON が返る" "${out_a}" '"threshold_periods":2'
assert_contains "ケース A: frames=6（有効行数）" "${out_a}" '"frames":6'
assert_contains "ケース A: intervals=5" "${out_a}" '"intervals":5'
assert_contains "ケース A: dropped=1（3 周期の 1 件のみ超過）" "${out_a}" '"dropped":1'
assert_contains "ケース A: drop_rate=0.2000" "${out_a}" '"drop_rate":0.2000'
assert_contains "ケース A: histogram に 1/2/3 周期がすべて出る" "${out_a}" '"1":3'

# --- ケース B: 0 / INT64_MAX 行が除外される（frames はケース A と同じ 6 のまま） ---
assert_contains "ケース B: 無効行を frames にカウントしない" "${out_a}" '"frames":6'

# --- ケース C: 閾値を 1 周期に下げると dropped が増える ---
out_c="$(FANDHE_FRAME_DROP_PERIODS=1 "${target_script}" --parse "${fixture}")"
assert_contains "ケース C: dropped=2（2 周期・3 周期の両方が超過）" "${out_c}" '"dropped":2'

# --- ケース D: 不正なパッケージ名は adb を呼ばずに終了コード 1 ---
dummy_bin="${work_dir}/dummy-bin"
mkdir -p "${dummy_bin}"
cat >"${dummy_bin}/adb" <<'EOF'
#!/usr/bin/env bash
echo "adb が呼ばれました: $*" >&2
exit 1
EOF
chmod +x "${dummy_bin}/adb"

set +e
PATH="${dummy_bin}:${PATH}" "${target_script}" '../evil' >"${work_dir}/out-d.txt" 2>"${work_dir}/err-d.txt"
code_d=$?
set -e
assert_eq "ケース D: 不正パッケージ名は終了コード 1" "1" "${code_d}"
assert_contains "ケース D: adb を呼ばない" "$(cat "${work_dir}/err-d.txt")" "パッケージ名"
if grep -q "adb が呼ばれました" "${work_dir}/err-d.txt"; then
  echo "[measure-frame-drop-rate-test] NG: ケース D: adb が呼ばれてしまった" >&2
  fail_count=$((fail_count + 1))
fi

# --- ケース E: 引数なしは usage を出して終了コード 1 ---
set +e
"${target_script}" >"${work_dir}/out-e.txt" 2>"${work_dir}/err-e.txt"
code_e=$?
set -e
assert_eq "ケース E: 引数なしは終了コード 1" "1" "${code_e}"
assert_contains "ケース E: usage を出す" "$(cat "${work_dir}/out-e.txt")" "使い方"

# --- ケース F: 出力が JSON として妥当（python3 があれば検証、無ければ skip） ---
if command -v python3 >/dev/null 2>&1; then
  if echo "${out_a}" | python3 -c 'import json,sys; json.load(sys.stdin)' >/dev/null 2>&1; then
    echo "[measure-frame-drop-rate-test] OK: ケース F: 出力が妥当な JSON"
  else
    echo "[measure-frame-drop-rate-test] NG: ケース F: 出力が JSON として不正" >&2
    fail_count=$((fail_count + 1))
  fi
else
  echo "[measure-frame-drop-rate-test] SKIP: ケース F: python3 が見つかりません"
fi

# --- ケース G: --layer にダブルクォート/バックスラッシュを含む値でも JSON が壊れない ---
out_g="$("${target_script}" --parse "${fixture}" --package 'pkg' --layer 'SurfaceView "top"\\layer')"
assert_contains "ケース G: layer 内の \" がエスケープされる" "${out_g}" '\\"top\\"'
assert_contains "ケース G: layer 内の \\\\ がエスケープされる" "${out_g}" '\\\\layer'
if command -v python3 >/dev/null 2>&1; then
  if echo "${out_g}" | python3 -c 'import json,sys; json.load(sys.stdin)' >/dev/null 2>&1; then
    echo "[measure-frame-drop-rate-test] OK: ケース G: 出力が妥当な JSON"
  else
    echo "[measure-frame-drop-rate-test] NG: ケース G: 出力が JSON として不正" >&2
    fail_count=$((fail_count + 1))
  fi
fi

# --- ケース H: 1 行目が正整数でない不正入力は非 0 終了（P1 レビュー対応） ---
fixture_bad_header="${work_dir}/fixture-bad-header.latency"
cat >"${fixture_bad_header}" <<'EOF'
error: no layer found
0 100000000 100000000
EOF
set +e
"${target_script}" --parse "${fixture_bad_header}" >"${work_dir}/out-h.txt" 2>"${work_dir}/err-h.txt"
code_h=$?
set -e
assert_eq "ケース H: 不正な 1 行目は終了コード 1" "1" "${code_h}"
assert_contains "ケース H: エラーメッセージが出る" "$(cat "${work_dir}/err-h.txt")" "不正な入力"

# --- ケース I: データ行が 3 列の整数でない不正入力は非 0 終了 ---
fixture_bad_data="${work_dir}/fixture-bad-data.latency"
cat >"${fixture_bad_data}" <<'EOF'
16666667
0 100000000 100000000
0 abc 116666667
EOF
set +e
"${target_script}" --parse "${fixture_bad_data}" >"${work_dir}/out-i.txt" 2>"${work_dir}/err-i.txt"
code_i=$?
set -e
assert_eq "ケース I: 不正なデータ行は終了コード 1" "1" "${code_i}"
assert_contains "ケース I: エラーメッセージが出る" "$(cat "${work_dir}/err-i.txt")" "不正な入力"

# --- ケース J: 集計可能な interval が 0 件（有効行が 1 件以下）は非 0 終了 ---
fixture_no_intervals="${work_dir}/fixture-no-intervals.latency"
cat >"${fixture_no_intervals}" <<'EOF'
16666667
0 100000000 100000000
0 0 0
EOF
set +e
"${target_script}" --parse "${fixture_no_intervals}" >"${work_dir}/out-j.txt" 2>"${work_dir}/err-j.txt"
code_j=$?
set -e
assert_eq "ケース J: interval が 0 件は終了コード 1" "1" "${code_j}"
assert_contains "ケース J: エラーメッセージが出る" "$(cat "${work_dir}/err-j.txt")" "集計可能な presentation 間隔"

# --- ケース K: --layer に制御文字（改行・タブ）を含んでも妥当な JSON になる ---
# （P1 レビュー対応: json_escape の制御文字未対応 + `awk -v` 自体のエスケープ展開の
#   二重の不備を、ENVIRON 経由の受け渡し + \uXXXX エスケープで解消したことの確認）
out_k="$("${target_script}" --parse "${fixture}" --package 'pkg' --layer "$(printf 'line1\nline2\ttab')")"
if command -v python3 >/dev/null 2>&1; then
  if echo "${out_k}" | python3 -c 'import json,sys; d=json.load(sys.stdin)["apps"][0]; assert d["layer"]=="line1\nline2\ttab", d["layer"]' >/dev/null 2>&1; then
    echo "[measure-frame-drop-rate-test] OK: ケース K: 制御文字を含む layer が正しく往復する"
  else
    echo "[measure-frame-drop-rate-test] NG: ケース K: 制御文字入り layer の JSON が不正 or 内容不一致: ${out_k}" >&2
    fail_count=$((fail_count + 1))
  fi
else
  echo "[measure-frame-drop-rate-test] SKIP: ケース K: python3 が見つかりません"
fi

# --- ケース L: レイヤ allowlist が SurfaceView 標準表記（角括弧・丸括弧）を許容する ---
# （lib/sf-layer.sh の select_surfaceflinger_layer を直接 source して検証。Medium レビュー対応）
# shellcheck source=../lib/sf-layer.sh
# shellcheck disable=SC1091
. "${script_dir}/../lib/sf-layer.sh"
sf_list_fixture="$(printf 'Layer 0x1 (SurfaceView[com.example.a/com.example.a.MainActivity]#0)\nLayer 0x2 (StatusBar)\n')"
sf_layer_l="$(select_surfaceflinger_layer "${sf_list_fixture}" "com.example.a")"
assert_eq "ケース L: SurfaceView[...]#0 形式のレイヤ名を選択できる" \
  "Layer 0x1 (SurfaceView[com.example.a/com.example.a.MainActivity]#0)" "${sf_layer_l}"

# --- ケース M: パッケージ名の部分一致で別アプリのレイヤを誤選択しない ---
# （grep -F の単純部分一致対策。com.example.app に対して .debug / .application が
#   続く別パッケージのレイヤを候補にせず、境界の合う本来のレイヤのみ選ぶことを検証。
#   P1 レビュー対応、Issue #658）
sf_list_boundary="$(printf 'Layer 0x1 (SurfaceView[com.example.app.debug/com.example.app.debug.MainActivity]#0)\nLayer 0x2 (SurfaceView[com.example.application/com.example.application.MainActivity]#0)\nLayer 0x3 (SurfaceView[com.example.app/com.example.app.MainActivity]#0)\n')"
sf_layer_m="$(select_surfaceflinger_layer "${sf_list_boundary}" "com.example.app")"
assert_eq "ケース M: 部分一致する別パッケージのレイヤを誤選択しない" \
  "Layer 0x3 (SurfaceView[com.example.app/com.example.app.MainActivity]#0)" "${sf_layer_m}"

# --- ケース N: 同一優先度で複数候補が残る場合は fail-closed（戻り値 3） ---
sf_list_ambiguous="$(printf 'Layer 0x1 (SurfaceView[com.example.app/com.example.app.MainActivity]#0)\nLayer 0x2 (SurfaceView[com.example.app/com.example.app.SecondActivity]#0)\n')"
set +e
select_surfaceflinger_layer "${sf_list_ambiguous}" "com.example.app" >/dev/null
code_n=$?
set -e
assert_eq "ケース N: 複数候補は戻り値 3 で fail-closed" "3" "${code_n}"


# --- ケース O: "Background for SurfaceView" コンパニオンレイヤを誤って候補にしない ---
# （Cursor Bugbot High 指摘対応: SurfaceView 優先フィルタがコンパニオンレイヤにも
#   一致し、複数候補で fail-closed してしまうのを防ぐ、Issue #658）
sf_list_companion="$(printf 'Layer 0x1 (Background for SurfaceView[com.example.app/com.example.app.MainActivity]#0)\nLayer 0x2 (SurfaceView[com.example.app/com.example.app.MainActivity]#0)\n')"
sf_layer_o="$(select_surfaceflinger_layer "${sf_list_companion}" "com.example.app")"
assert_eq "ケース O: コンパニオンレイヤを除外し本来の SurfaceView のみ選択する" \
  "Layer 0x2 (SurfaceView[com.example.app/com.example.app.MainActivity]#0)" "${sf_layer_o}"

if [ "${fail_count}" -ne 0 ]; then
  echo "[measure-frame-drop-rate-test] ${fail_count} 件失敗" >&2
  exit 1
fi
echo "[measure-frame-drop-rate-test] 全ケース OK"
