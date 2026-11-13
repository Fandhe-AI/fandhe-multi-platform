#!/usr/bin/env bash
# scripts/metrics/measure-binary-size.sh の自己チェック（Issue #1294）。
#
# 偽の java と空の bundletool jar を FANDHE_ANDROID_ROOT 配下に置き、20MiB 判定が
# get-size の結果で行われること、bundletool が使えないときにファイルサイズへ倒れず
# 失敗することを assert する（実 bundletool・実機は不要）。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target_script="${script_dir}/../measure-binary-size.sh"
android_dir="${script_dir}/../../android"
# shellcheck disable=SC1091
. "${android_dir}/versions.env"

work_dir="$(mktemp -d)"
trap 'rm -rf "${work_dir}"' EXIT

fail_count=0
ok() { echo "[measure-binary-size-test] OK: $1"; }
ng() { echo "[measure-binary-size-test] NG: $1" >&2; fail_count=$((fail_count + 1)); }
assert_contains() {
  if printf '%s' "$2" | grep -qE "$3"; then ok "$1"; else ng "$1（${3} が無い: $2）"; fi
}
assert_not_contains() {
  if printf '%s' "$2" | grep -qE "$3"; then ng "$1（${3} が出た: $2）"; else ok "$1"; fi
}

root="${work_dir}/root"
mkdir -p "${root}/jdk/${JDK_EXTRACTED_DIR}/bin" "${root}/bundletool"
: >"${root}/bundletool/${BUNDLETOOL_JAR_NAME}"
# 偽 java: build-apks は --output を作り、get-size は FAKE_SIZES の中身を出す。
cat >"${root}/jdk/${JDK_EXTRACTED_DIR}/bin/java" <<'JAVA'
#!/usr/bin/env bash
case "$3" in
  build-apks)
    [ "${FAKE_BUILD_FAIL:-0}" = 1 ] && exit 1
    for a in "$@"; do case "$a" in --output=*) : >"${a#--output=}" ;; esac; done ;;
  get-size) printf '%b' "${FAKE_SIZES}" ;;
esac
JAVA
chmod +x "${root}/jdk/${JDK_EXTRACTED_DIR}/bin/java"

aab="${work_dir}/app.aab"
head -c 100 /dev/zero >"${aab}"
so="${work_dir}/lib.so"
head -c 100 /dev/zero >"${so}"

run() { FANDHE_ANDROID_ROOT="${root}" bash "${target_script}" "$@" 2>&1; }

# 20 MiB = 20971520
out="$(FAKE_SIZES='"ABI","MIN","MAX"\n"arm64-v8a","1000","20971520"\n"x86_64","1000","2000"\n' run "${aab}")"
assert_contains "MAX が 20MiB ちょうどで判定 OK" "${out}" '判定 OK.*配信サイズ'
assert_contains "CSV にファイルサイズが残る" "${out}" "^${aab},100,"

out="$(FAKE_SIZES='"ABI","MIN","MAX"\n"arm64-v8a","1000","2000"\n"x86_64","1000","20971521"\n' run "${aab}")"
assert_contains "どれか 1 ABI が超過で判定 NG" "${out}" '判定 NG'

rc=0
out="$(FAKE_SIZES='"ABI","MIN","MAX"\n' run "${aab}")" || rc=$?
if [ "${rc}" -ne 0 ]; then ok "get-size が空なら非 0"; else ng "get-size が空なのに成功"; fi
assert_not_contains "空結果で判定を出さない" "${out}" '判定 (OK|NG)'

rc=0
out="$(FAKE_BUILD_FAIL=1 FAKE_SIZES='' run "${aab}")" || rc=$?
if [ "${rc}" -ne 0 ]; then ok "build-apks 失敗で非 0"; else ng "build-apks 失敗なのに成功"; fi

rm "${root}/bundletool/${BUNDLETOOL_JAR_NAME}"
rc=0
out="$(FAKE_SIZES='"ABI","MIN","MAX"\n"a","1","2"\n' run "${aab}")" || rc=$?
if [ "${rc}" -ne 0 ]; then ok "jar 欠落で非 0"; else ng "jar 欠落なのに成功"; fi
assert_not_contains "jar 欠落でファイルサイズ判定へ倒れない" "${out}" '判定 (OK|NG)'

out="$(run "${so}")"
assert_contains ".so は判定対象外" "${out}" '判定対象外'
assert_not_contains ".so に OK/NG を出さない" "${out}" '判定 (OK|NG)'

if [ "${fail_count}" -ne 0 ]; then
  echo "失敗: ${fail_count} 件" >&2
  exit 1
fi
echo "[measure-binary-size-test] 全 OK"
