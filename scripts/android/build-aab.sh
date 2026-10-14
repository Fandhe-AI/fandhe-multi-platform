#!/usr/bin/env bash
# TASK-11.2-S2（REQ-11 / M3、Issue #226、親: TASK-11.2 系列 / #224）:
# cargo-ndk が生成する Android 向け cdylib（libfandhe_egui_proxy.so、
# crates/fandhe-egui-proxy/src/android.rs の android_main エントリ。Activity 方式は
# TASK-9.3-S1 / Issue #242 で GameActivity に確定、docs/architecture/android-activity-method.md
# 参照）を
# platforms/android/ の Gradle プロジェクトへ統合し、`.aab` を単一スクリプトから
# 生成する。開発署名鍵による署名処理は TASK-11.2-S3（#227）で本スクリプトへ
# 統合済み（下記環境変数インターフェース・フェーズ 1 / フェーズ 3 参照）。
#
# 呼び出し文脈: scripts/ios/build-simulator-app.sh（TASK-11.1-S2）の
# 「S1（CLI 構成設計）未マージでも単独実行可能なスクリプト + 環境変数インターフェース」
# という先例に倣う。TASK-11.2-S1（.aab 生成パイプライン CLI 構成設計、#225）は
# 本コミット時点で open・未マージのため、本スクリプトは単独実行可能な形で実装し、
# 下記の環境変数インターフェースを経由して S1 の CLI から呼び出せるようにする。
#
# sub-issue 間のスコープ境界（重要）:
#   - 開発署名鍵による signingConfigs 統合・署名検証は本スクリプトと
#     platforms/android/app/build.gradle.kts で実装済み（TASK-11.2-S3 / #227）。
#     keystore の生成手順・秘密情報の取り扱いは
#     docs/operations/android-aab-signing.md を参照。
#   - エミュレータへのインストール・起動確認は TASK-11.3（#246）のスコープ。
#   - CI（.github/workflows/ci.yml）への `.aab` 生成ステップ組み込み・
#     GitHub Secrets 経由での鍵配布は TASK-11.2-S4（#228）へ申し送る。
#
# 環境変数インターフェース（S1 の CLI から呼び出す契約として明記。
# scripts/ios/build-simulator-app.sh の FANDHE_IOS_* と同じ設計方針）:
#   FANDHE_ANDROID_PROFILE           既定: release（release のみ対応。debug は
#                                    本スクリプトの対象外とする。デバッグ署名鍵付きの
#                                    bundleDebug を既定経路にしないための意図的な
#                                    制限。security.md A05 対策）。それ以外の値は
#                                    成果物パス導出規約が崩れるため拒否する。
#   FANDHE_ANDROID_ABIS              既定: "arm64-v8a armeabi-v7a x86_64"（空白区切り）。
#                                    cargo-ndk へ渡す ABI 一覧（-t フラグ）。同じ一覧を
#                                    `-PfandheAndroidAbis=<カンマ区切り>` として Gradle へも
#                                    転送し、platforms/android/app/build.gradle.kts の
#                                    ndk.abiFilters に反映させる（stale ABI 混入対策）。
#   FANDHE_ANDROID_KEYSTORE          既定: 空（署名しない・未署名 `.aab` を生成する。
#                                    S2 までと同じ後方互換の既定挙動）。開発署名鍵
#                                    （`.jks` / PKCS12）のパス。非空の場合のみ署名する。
#   FANDHE_ANDROID_KEYSTORE_PASSWORD 既定: 空。keystore パスフレーズ。環境変数経由の
#                                    みで受け取り、ログ・エラーメッセージへ一切
#                                    出力しない（security.md 準拠）。
#                                    FANDHE_ANDROID_KEYSTORE 非空時は必須（部分設定は
#                                    fail-closed でエラーにする。下記フェーズ 1 参照）。
#   FANDHE_ANDROID_KEY_ALIAS         既定: "fandhe-dev"。鍵エイリアス。
#   FANDHE_ANDROID_KEY_PASSWORD      既定: FANDHE_ANDROID_KEYSTORE_PASSWORD と同値
#                                    （PKCS12 keystore では store と鍵のパスワードが
#                                    同一であることが多いための既定流用）。
#
# keystore・パスワードの生成手順は docs/operations/android-aab-signing.md を参照。
#
# 使い方:
#   scripts/android/build-aab.sh                                    # 未署名 .aab
#   FANDHE_ANDROID_KEYSTORE=/path/to/dev.jks \
#   FANDHE_ANDROID_KEYSTORE_PASSWORD=xxxx \
#     scripts/android/build-aab.sh                                  # 開発署名済み .aab
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

# android-env.sh が versions.env の読み込みと JAVA_HOME / ANDROID_HOME /
# ANDROID_NDK_HOME / GRADLE_HOME の export を行う（scripts/android/build-android.sh と
# 同じ前提。cargo-ndk は ANDROID_NDK_HOME からしか NDK を発見できず、
# 本スクリプトの Gradle 呼び出しも GRADLE_HOME 経由の Gradle 本体を使うため必須）。
# shellcheck source=android-env.sh
# shellcheck disable=SC1091
. "${script_dir}/android-env.sh"

package="fandhe-egui-proxy"
lib_name="fandhe_egui_proxy"
gradle_project_dir="${repo_root}/platforms/android"
jnilibs_out_dir="${repo_root}/target/android/jniLibs"
aab_path="${gradle_project_dir}/app/build/outputs/bundle/release/app-release.aab"

profile="${FANDHE_ANDROID_PROFILE:-release}"
abis="${FANDHE_ANDROID_ABIS:-arm64-v8a armeabi-v7a x86_64}"
# 開発署名鍵（TASK-11.2-S3 / #227）。値そのものは fail() / log() へ渡さない
# （security.md 準拠。パスワードをログ・エラーメッセージへ出力しない）。
keystore_path="${FANDHE_ANDROID_KEYSTORE:-}"
keystore_password="${FANDHE_ANDROID_KEYSTORE_PASSWORD:-}"

log() {
  echo "[build-aab] $*"
}

fail() {
  echo "[build-aab] ERROR: $*" >&2
  exit 1
}

# FANDHE_ANDROID_PROFILE は release のみを受け付ける（上記スコープ境界コメント参照）。
# 未クォート展開・eval は行わない（security.md A03 対策）。
case "${profile}" in
  release) ;;
  *)
    fail "FANDHE_ANDROID_PROFILE は release のみ対応: ${profile}"
    ;;
esac

# --- フェーズ 1: 前提検査 ---------------------------------------------------
command -v cargo >/dev/null 2>&1 || fail "cargo が見つかりません（rustup 導入後に再実行してください）"
command -v cargo-ndk >/dev/null 2>&1 || fail "cargo-ndk が見つかりません（scripts/android/setup-android-env.sh を先に実行してください）"
[ -d "${ANDROID_NDK_HOME}" ] \
  || fail "ANDROID_NDK_HOME（${ANDROID_NDK_HOME}）が見つかりません。scripts/android/setup-android-env.sh を先に実行してください"
[ -d "${GRADLE_HOME}" ] \
  || fail "GRADLE_HOME（${GRADLE_HOME}）が見つかりません。scripts/android/setup-android-env.sh を先に実行してください"
[ -d "${JAVA_HOME}" ] \
  || fail "JAVA_HOME（${JAVA_HOME}）が見つかりません。scripts/android/setup-android-env.sh を先に実行してください"
command -v gradle >/dev/null 2>&1 || fail "gradle が見つかりません（PATH に GRADLE_HOME/bin が含まれているか確認してください）"

# 開発署名鍵の前提検査（TASK-11.2-S3 / #227）。cargo-ndk の長時間ビルドに入る前に
# fail-fast する。部分設定（keystore のみ／パスワードのみ）は「署名したつもりで
# 未署名になる」事故を防ぐため即エラーにする（fail-closed。security.md A05 対策）。
if [ -n "${keystore_path}" ] && [ -z "${keystore_password}" ]; then
  fail "FANDHE_ANDROID_KEYSTORE を設定する場合は FANDHE_ANDROID_KEYSTORE_PASSWORD も必須です"
fi
if [ -z "${keystore_path}" ] && [ -n "${keystore_password}" ]; then
  fail "FANDHE_ANDROID_KEYSTORE_PASSWORD を設定する場合は FANDHE_ANDROID_KEYSTORE も必須です"
fi
if [ -n "${keystore_path}" ]; then
  [ -f "${keystore_path}" ] || fail "FANDHE_ANDROID_KEYSTORE で指定された keystore が見つかりません: ${keystore_path}"
  # build.gradle.kts の file() はプロジェクトディレクトリ（platforms/android/）基準で
  # 相対パスを解決するため、呼び出し元の作業ディレクトリに依存しないよう
  # 絶対パスへ正規化してから Gradle 側（signingConfigs.storeFile）へ引き継ぐ。
  keystore_path="$(cd -- "$(dirname -- "${keystore_path}")" >/dev/null 2>&1 && pwd -P)/$(basename -- "${keystore_path}")"
  export FANDHE_ANDROID_KEYSTORE="${keystore_path}"
  log "開発署名鍵を使用します（署名済み .aab を生成）"
else
  log "FANDHE_ANDROID_KEYSTORE 未設定のため未署名の .aab を生成します"
fi

# --- フェーズ 2: cargo-ndk によるクロスビルド（cdylib、jniLibs レイアウト出力） ---
# rust-toolchain.toml（rustc 固定）を確実に効かせるため、リポジトリルートで実行する
# （build-android.sh と同じ理由。rustup target はカレントディレクトリの
# active toolchain に紐づく）。
cd "${repo_root}"

# jniLibs_out_dir を実行のたびにクリアしてから cargo-ndk を実行する。
# `cargo ndk -o` は今回のビルド対象 ABI 分の .so しかコピーしないため、
# 過去に FANDHE_ANDROID_ABIS をより広い集合で実行した際の残置 ABI ディレクトリが
# 残ったままだと、以降 build.gradle.kts の jniLibs.srcDirs 経由でそれらが
# 拾われ続けて .aab に混入してしまう
#（Cursor Bugbot 指摘: stale ABI libs packaged into AAB）。
log "jniLibs 出力先をクリア: ${jnilibs_out_dir}"
rm -rf "${jnilibs_out_dir}"

declare -a cargo_ndk_args=()
# shellcheck disable=SC2206
abi_list=(${abis})
[ "${#abi_list[@]}" -gt 0 ] || fail "FANDHE_ANDROID_ABIS が空です。少なくとも 1 つ ABI を指定してください"
for abi in "${abi_list[@]}"; do
  cargo_ndk_args+=(-t "${abi}")
done
cargo_ndk_args+=(
  --platform "${ANDROID_MIN_API}"
  -o "${jnilibs_out_dir}"
  build
  -p "${package}"
  --release
)

log "cargo ndk ${cargo_ndk_args[*]}"
cargo ndk "${cargo_ndk_args[@]}"

# --- フェーズ 2 検証: 各 ABI で .so が生成されていること ---------------------
for abi in "${abi_list[@]}"; do
  so_path="${jnilibs_out_dir}/${abi}/lib${lib_name}.so"
  [ -f "${so_path}" ] || fail ".so が見つかりません: ${so_path}"
  log "  - ${so_path} 存在確認 OK"
done

# --- フェーズ 3: Gradle による .aab 生成（未署名） --------------------------
# 本スクリプトは署名を行わない。bundleRelease はデフォルトの signingConfig が
# 未設定のため未署名の .aab を出力する（S3 / #227 が signingConfigs を追加した
# 後は、本フェーズの成果物は開発署名済みになる。挿入ポイントは
# platforms/android/app/build.gradle.kts のコメント参照）。
#
# fandheAndroidAbis プロジェクトプロパティで今回要求した ABI 一覧を Gradle 側へ
# 明示的に伝える。build.gradle.kts はこれを defaultConfig.ndk.abiFilters に
# 反映し、jniLibs.srcDirs 経由で万一余分な ABI ディレクトリが残っていても
# .aab へは要求 ABI のみが収載されるようにする（フェーズ 2 の rm -rf と
# 二重の防御。Cursor Bugbot 指摘: stale ABI libs packaged into AAB）。
abi_csv="$(IFS=,; echo "${abi_list[*]}")"
log "gradle -p ${gradle_project_dir} :app:bundleRelease -PfandheAndroidAbis=${abi_csv}"
gradle -p "${gradle_project_dir}" :app:bundleRelease --console=plain \
  "-PfandheAndroidAbis=${abi_csv}"

[ -f "${aab_path}" ] || fail ".aab の生成に失敗しました: ${aab_path}"
log ".aab 生成 OK: ${aab_path}"

# --- フェーズ 3 検証: .aab 内に要求 ABI（abi_list）分の .so が収載されていること ---
# `unzip -l | grep` は set -o pipefail 下で grep が先に終了すると unzip 側が
# SIGPIPE で終了しうる（scripts/ios/build-simulator-app.sh と同じ注意点）ため、
# 一覧をいったん変数へ capture してから照合する。
#
# 変数へ capture した後の照合でも、`printf '%s\n' "${var}" | grep -q ...` の形は
# 同じ問題を再発させる（CI 実障害。Issue #236 PR #498 レビュー指摘）: `grep -q` は
# 最初の一致で即座に読み取りを止めるため、パイプ上流の `printf` が SIGPIPE を受けて
# 非 0 終了（例: exit 141、標準エラーに `printf: write error: Broken pipe`）する。
# `set -o pipefail` 下ではパイプ全体の終了ステータスが「最後に非 0 だったコマンド」
# （この場合は printf 側）に引きずられるため、`grep -q` 自体は一致を検出して
# 成功していても `if ! printf ... | grep -q ...` が偽陰性で失敗扱いになる
# （実際には armeabi-v7a も .aab 内に収載されていたが、この偽陰性により
# 「見つからない」と誤検知して exit code 1 になっていた）。
# 一致判定に `grep -q` を使う箇所はパイプではなく here-string（`<<<`）で変数を
# 直接渡す。here-string は一時ファイル経由でリダイレクトされるため、
# `grep -q` が早期に読み取りを終えてもパイプ書き込み側が存在せず SIGPIPE が
# 発生しない。
#
# `unzip -l` と `python3 -m zipfile -l`（unzip 未インストール環境向けフォールバック）は
# 列順が異なる（unzip -l は「Length Date Time Name」でファイル名が末尾、
# python3 -m zipfile -l は「File Name Modified Size」でファイル名が先頭）。
# 下記フェーズ 3 の署名ブロック検出（`\.(RSA|DSA|EC)$` の末尾アンカー付き正規表現）は
# 「ファイル名が行末に来る」前提のため、python3 フォールバック経路では常に不一致になり
# 署名モード・未署名モードいずれの検証も無効化されてしまう（A08 整合性チェックの
# silent pass）。aab_listing とは別に「ファイル名のみ 1 行 1 エントリ」の
# aab_entry_names を使用ツールに応じて正しい列から抽出し、署名ブロック検出は
# aab_listing ではなく aab_entry_names に対して行う（ABI 検出側の
# grep -q "base/lib/${abi}/..." や grep -oE 'base/lib/[^/]+/' は非アンカーの部分一致で
# 列順に依存しないため aab_listing のままで問題ない）。
if command -v unzip >/dev/null 2>&1; then
  aab_listing="$(unzip -l "${aab_path}")"
  # unzip -l はファイル名が各行の最終フィールド（末尾）。
  aab_entry_names="$(printf '%s\n' "${aab_listing}" | awk '{print $NF}')"
elif command -v python3 >/dev/null 2>&1; then
  aab_listing="$(python3 -m zipfile -l "${aab_path}")"
  # python3 -m zipfile -l はファイル名が各行の第 1 フィールド（先頭）。
  # 1 行目はヘッダー（"File Name Modified Size"）のため除外する。
  aab_entry_names="$(printf '%s\n' "${aab_listing}" | tail -n +2 | awk '{print $1}')"
else
  fail "unzip も python3 も見つからないため .aab の構造検証ができません"
fi

for abi in "${abi_list[@]}"; do
  if ! grep -q "base/lib/${abi}/lib${lib_name}\.so" <<<"${aab_listing}"; then
    fail ".aab 内に base/lib/${abi}/lib${lib_name}.so が見つかりません"
  fi
  log "  - base/lib/${abi}/lib${lib_name}.so 収載確認 OK"
done

# --- フェーズ 3 検証: 要求外 ABI が混入していないこと -------------------------
# 上記は「要求した ABI が存在すること」のみの確認であり、フェーズ 2 の
# rm -rf・build.gradle.kts の ndk.abiFilters が効いていない場合に
# 「要求していない ABI が余分に含まれてしまう」退行を検知できない
# （Cursor Bugbot 指摘: stale ABI libs packaged into AAB のポストチェック強化案）。
# base/lib/<ABI>/ 配下に現れる ABI ディレクトリ名を列挙し、要求 ABI 一覧の
# 部分集合であることを確認する。
present_abis="$(printf '%s\n' "${aab_listing}" | grep -oE 'base/lib/[^/]+/' | sed -E 's#base/lib/([^/]+)/#\1#' | sort -u)"
while IFS= read -r present_abi; do
  [ -z "${present_abi}" ] && continue
  is_requested=0
  for abi in "${abi_list[@]}"; do
    if [ "${abi}" = "${present_abi}" ]; then
      is_requested=1
      break
    fi
  done
  if [ "${is_requested}" -ne 1 ]; then
    fail ".aab 内に要求外の ABI（${present_abi}）が混入しています（stale ABI 残置の疑い）"
  fi
done <<< "${present_abis}"
present_abis_display="$(printf '%s\n' "${present_abis}" | tr '\n' ' ')"
log "  - 要求外 ABI の混入なしを確認 OK（収載 ABI: ${present_abis_display}）"

# --- フェーズ 3 検証: 署名有無の分岐検証 --------------------------------------
# 署名済み .aab/.apk は META-INF/ 配下に .RSA/.DSA/.EC 等の署名ブロックファイルを
# 持つ。FANDHE_ANDROID_KEYSTORE の有無に応じて「署名されていること」
# 「署名されていないこと」のどちらかを機械的に検証する（A08「整合性の不備」対応。
# 「署名したつもりで未署名」「未署名のつもりで署名情報混入」の両方を検知する）。
if [ -n "${keystore_path}" ]; then
  if ! grep -qE '\.(RSA|DSA|EC)$' <<<"${aab_entry_names}"; then
    fail "開発署名鍵を指定しましたが .aab 内に署名ブロックが見つかりません（signingConfigs が適用されていない可能性）"
  fi
  log "  - 署名ブロック（META-INF/*.RSA 等）の存在を確認 OK"

  # jarsigner -verify は署名の実在・整合性を検証するのみで、署名自体は Gradle の
  # signingConfigs が行う（S1 設計 §7 の方針）。パスワードは jarsigner -verify の
  # 引数として不要なため、ここでもコマンドラインへ渡していない。
  # mktemp で衝突しない一時ファイルを都度払い出す（並列実行・残置対策）。
  jarsigner_log="$(mktemp)"
  if ! "${JAVA_HOME}/bin/jarsigner" -verify "${aab_path}" >"${jarsigner_log}" 2>&1; then
    cat "${jarsigner_log}" >&2
    rm -f "${jarsigner_log}"
    fail "jarsigner -verify が失敗しました: ${aab_path}"
  fi
  rm -f "${jarsigner_log}"
  log "  - jarsigner -verify OK（署名検証成功）"
else
  if grep -qE '\.(RSA|DSA|EC)$' <<<"${aab_entry_names}"; then
    fail "想定外の署名ブロックが .aab 内に存在します（FANDHE_ANDROID_KEYSTORE 未設定時は未署名のはず）"
  fi
  log "  - 未署名であることを確認 OK（開発署名は FANDHE_ANDROID_KEYSTORE 設定で有効化されます）"
fi

log "完了: ${aab_path}"
if [ -n "${keystore_path}" ]; then
  log "本成果物は開発署名鍵（TASK-11.2-S3 / #227）で署名済みです。"
else
  log "注意: 本成果物は未署名です。開発署名は FANDHE_ANDROID_KEYSTORE 等の環境変数で"
  log "      有効化できます（docs/operations/android-aab-signing.md 参照）。"
fi
log "エミュレータでのインストール確認（TASK-11.3 / #246）は本スクリプトの範囲外です。"
