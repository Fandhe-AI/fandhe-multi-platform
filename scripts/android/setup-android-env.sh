#!/usr/bin/env bash
# Android NDK・Gradle（および前提の JDK・Android SDK commandline-tools）を
# ローカル開発環境・CI（GitHub Actions）の双方へ冪等に導入するインストーラ。
#
# 対応: REQ-18 / TASK-18.3（docs/spec/05-tasks.md）。
# 呼び出し元: ローカルでは開発者が直接実行、CI では .github/workflows/ci.yml の
# android-env ジョブから呼ばれる（同一コードパスで導入・検証を行う）。
#
# 設計方針（.claude/rules/ci.md 準拠）:
#   - システム領域（/usr 等）へは一切書かず、ユーザー HOME 配下のみに導入する（sudo 不使用）。
#   - シェル profile（~/.zshrc 等）は書き換えない。呼び出し側が android-env.sh を source する。
#   - 各コンポーネントは「期待バージョンが導入済みなら skip」の冪等設計。
#     ただし CI の concurrency.cancel-in-progress によりジョブが再実行を跨いで
#     キャンセルされる可能性があるため、ダウンロード系（JDK・Gradle・SDK パッケージ
#     [platform-tools/platforms/build-tools/NDK]）は一時ディレクトリに展開してから
#     最終パスへ atomic mv する。「最終パスが存在する（SDK パッケージは配布物同梱の
#     source.properties が最終パスに存在する）」ことのみを完了マーカーとして扱うことで、
#     中断による半展開状態を「導入済み」と誤認しない。
#   - concurrency.cancel-in-progress は ref（PR・ブランチ）単位のため、別 PR の
#     android-env ジョブが同一 CI runner の HOME（既定の FANDHE_ANDROID_ROOT）を
#     共有したまま並列実行され得る。ジョブごとに一意な作業ディレクトリへ分離すると
#     「導入済みなら skip」の共有キャッシュ効果（数 GB のダウンロード・展開を毎回
#     繰り返さない）を失うため、代わりに main() 先頭で FANDHE_ANDROID_ROOT 単位の
#     flock を取得し、導入・検証全体をジョブ間で直列化する（Bugbot 指摘: Shared SDK root race）。
#
# --verify-only: インストールを一切行わず、導入済みバージョンが versions.env の
# 期待値と一致するかどうかのみを検査する（ローカル受け入れ確認と CI 検証を同一パスにする）。
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
# shellcheck source=versions.env
set -a
# shellcheck disable=SC1091
. "${SCRIPT_DIR}/versions.env"
set +a

# 導入先ルート。CI・ローカルともに環境変数で上書き可能（並列実行時の衝突回避にも使える）。
FANDHE_ANDROID_ROOT="${FANDHE_ANDROID_ROOT:-${HOME}/.fandhe/android}"
JDK_ROOT="${FANDHE_ANDROID_ROOT}/jdk"
SDK_ROOT="${FANDHE_ANDROID_ROOT}/sdk"
GRADLE_ROOT="${FANDHE_ANDROID_ROOT}/gradle"
BUNDLETOOL_ROOT="${FANDHE_ANDROID_ROOT}/bundletool"
TMP_ROOT="${FANDHE_ANDROID_ROOT}/.tmp"

VERIFY_ONLY=0
if [ "${1:-}" = "--verify-only" ]; then
  VERIFY_ONLY=1
fi

log() {
  echo "[setup-android-env] $*"
}

fail() {
  echo "[setup-android-env] ERROR: $*" >&2
  exit 1
}

# チェックサムコマンドの OS 差異を吸収する。GNU coreutils の sha256sum / sha1sum が
# 無い macOS（Darwin 25 以降は /sbin に標準搭載されるが、それより前は非搭載）では
# BSD 標準の shasum -a へフォールバックする（PR #543 codex-review P1 対応）。
# どちらの実装も出力 1 列目がハッシュ値である点は共通。
compute_checksum() {
  local algo_bits="$1" file="$2"
  if command -v "sha${algo_bits}sum" >/dev/null 2>&1; then
    "sha${algo_bits}sum" "${file}" | awk '{print $1}'
  else
    shasum -a "${algo_bits}" "${file}" | awk '{print $1}'
  fi
}

# ダウンロード + チェックサム検証（sha256/sha1 いずれか）を行い、検証済みファイルを
# 呼び出し元指定のパスへ配置する。検証に失敗した場合は即 fail し、後続へ進めない
# （REQ-18 のセキュリティ考慮事項 A08: 整合性の不備の防止）。
download_and_verify() {
  local url="$1" dest="$2" algo="$3" expected="$4"
  # 実際にダウンロードが発生する直前にのみ空き容量を確認する（skip 判定より前段の
  # main() で無条件チェックすると、全コンポーネント導入済みの再実行時にも
  # 空き容量不足で失敗してしまうため。詳細は check_disk_space のコメント参照）。
  check_disk_space
  mkdir -p "$(dirname -- "${dest}")"
  log "ダウンロード: ${url}"
  curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "${dest}" "${url}"

  local actual
  case "${algo}" in
    sha256)
      actual="$(compute_checksum 256 "${dest}")"
      ;;
    sha1)
      actual="$(compute_checksum 1 "${dest}")"
      ;;
    *)
      fail "未知のチェックサムアルゴリズム: ${algo}"
      ;;
  esac

  if [ "${actual}" != "${expected}" ]; then
    fail "チェックサム不一致: ${dest} (expected=${expected} actual=${actual})"
  fi
  log "チェックサム検証 OK: ${dest}"
}

# ---------------------------------------------------------------------------
# JDK（Eclipse Temurin）
#
# Linux（GitHub Actions）と macOS arm64（開発機、Apple Silicon）の両方に
# 対応する。配布物のアーカイブ名・URL・チェックサムは uname -s で選択する
# （#541。前例: scripts/build/build-and-sign.sh の uname -s 分岐）。
# ---------------------------------------------------------------------------

# JDK のダウンロード元情報（アーカイブ名・URL・sha256）を OS に応じて選択し、
# 呼び出し元の変数（グローバル JDK_SELECTED_*）へ設定する。
# macOS は Apple Silicon（arm64）のみ対応。Intel Mac は対象外のため fail-fast する
# （docs/environment の調達記録が Apple Silicon 前提のため。#541）。
select_jdk_source() {
  local os_name arch_name
  os_name="$(uname -s)"
  case "${os_name}" in
    Darwin)
      arch_name="$(uname -m)"
      [ "${arch_name}" = "arm64" ] \
        || fail "macOS は Apple Silicon（arm64）のみ対応です（検出: ${arch_name}）"
      JDK_SELECTED_ARCHIVE_NAME="${JDK_MACOS_ARCHIVE_NAME}"
      JDK_SELECTED_URL="${JDK_MACOS_URL}"
      JDK_SELECTED_SHA256="${JDK_MACOS_SHA256}"
      ;;
    *)
      # Linux（GitHub Actions）は現状維持。
      JDK_SELECTED_ARCHIVE_NAME="${JDK_ARCHIVE_NAME}"
      JDK_SELECTED_URL="${JDK_URL}"
      JDK_SELECTED_SHA256="${JDK_SHA256}"
      ;;
  esac
}

install_jdk() {
  local final_dir="${JDK_ROOT}/${JDK_EXTRACTED_DIR}"
  if [ -d "${final_dir}" ]; then
    log "JDK は導入済みのため skip: ${final_dir}"
    return 0
  fi

  select_jdk_source

  local work
  work="$(mktemp -d "${TMP_ROOT}/jdk.XXXXXX")"
  local archive="${work}/${JDK_SELECTED_ARCHIVE_NAME}"
  download_and_verify "${JDK_SELECTED_URL}" "${archive}" sha256 "${JDK_SELECTED_SHA256}"

  mkdir -p "${JDK_ROOT}"
  tar -xzf "${archive}" -C "${work}"
  if [ -d "${work}/${JDK_EXTRACTED_DIR}/Contents/Home" ]; then
    # macOS Temurin バンドル構造（<top>/Contents/Home 配下に bin/ 等の本体一式）を
    # フラット化する。この Mac の既存手動配置（bin/java が JAVA_HOME 直下に来る）と
    # 同じ結果を自動化する（#541）。
    mv "${work}/${JDK_EXTRACTED_DIR}/Contents/Home" "${final_dir}"
  else
    # 展開後トップレベルディレクトリを最終パスへ atomic mv する。
    # 中断時に半展開ディレクトリが final_dir 名で残ることを防ぐ（冪等性の根拠）。
    mv "${work}/${JDK_EXTRACTED_DIR}" "${final_dir}"
  fi
  rm -rf "${work}"
  log "JDK 導入完了: ${final_dir}"
}

verify_jdk() {
  local final_dir="${JDK_ROOT}/${JDK_EXTRACTED_DIR}"
  [ -x "${final_dir}/bin/java" ] || fail "JDK が未導入です: ${final_dir}/bin/java が見つかりません"
  local version_output
  version_output="$("${final_dir}/bin/java" -version 2>&1 || true)"
  echo "${version_output}" | grep -qF "${JDK_VERSION%%+*}" \
    || fail "JDK バージョン不一致: 期待 ${JDK_VERSION} / 実際 ${version_output}"
  log "JDK 検証 OK: ${version_output}"
}

# ---------------------------------------------------------------------------
# Android SDK commandline-tools + sdkmanager 経由パッケージ
# ---------------------------------------------------------------------------
install_cmdline_tools() {
  # sdkmanager の慣例に従い cmdline-tools/latest 配下から実行できるようにする
  # （sdkmanager --sdk_root 実行時の探索パスと一致させるため）。
  # 実体は cmdline-tools/${CMDLINE_TOOLS_VERSION}（バージョン付きディレクトリ）に展開し、
  # cmdline-tools/latest はそこへのシンボリックリンクとする。完了マーカーをバージョン付き
  # ディレクトリの存在にすることで、JDK/Gradle/NDK と同様に versions.env の
  # CMDLINE_TOOLS_VERSION 更新が確実に再導入を促すようにする（"latest" という
  # バージョン非依存の固定名だけを完了マーカーにすると、sdkmanager が存在する限り
  # バージョンを上げても永久に再導入されなくなるため）。
  local versioned_dir="${SDK_ROOT}/cmdline-tools/${CMDLINE_TOOLS_VERSION}"
  local latest_link="${SDK_ROOT}/cmdline-tools/latest"
  if [ -x "${versioned_dir}/bin/sdkmanager" ]; then
    log "Android commandline-tools は導入済みのため skip: ${versioned_dir}"
    relink_cmdline_tools_latest "${versioned_dir}" "${latest_link}"
    return 0
  fi

  local work
  work="$(mktemp -d "${TMP_ROOT}/cmdline-tools.XXXXXX")"
  local archive="${work}/${CMDLINE_TOOLS_ARCHIVE_NAME}"
  download_and_verify "${CMDLINE_TOOLS_URL}" "${archive}" sha1 "${CMDLINE_TOOLS_SHA1}"

  mkdir -p "${SDK_ROOT}/cmdline-tools"
  unzip -q "${archive}" -d "${work}/extracted"
  # zip 展開直後は cmdline-tools/ 直下に bin/ 等が展開される（sdkmanager 配布物の構造）。
  # バージョン付きディレクトリへ配置し直し、後段で latest シンボリックリンクを張る。
  mv "${work}/extracted/cmdline-tools" "${versioned_dir}"
  rm -rf "${work}"
  log "Android commandline-tools 導入完了: ${versioned_dir}"
  relink_cmdline_tools_latest "${versioned_dir}" "${latest_link}"

  # ライセンス同意（非対話・CI/ローカル共通。ローカル完結でシークレット不要）。
  # JAVA_HOME 未設定の runner では sdkmanager 自体が起動できず失敗するため、
  # install_jdk（本関数より先に main から呼ばれる）で導入した JDK を明示的に指定する。
  yes | JAVA_HOME="${JDK_ROOT}/${JDK_EXTRACTED_DIR}" "${latest_link}/bin/sdkmanager" \
    --sdk_root="${SDK_ROOT}" --licenses >/dev/null 2>&1 || true
}

# cmdline-tools/latest を常に versioned_dir を指すシンボリックリンクへ張り替える。
# sdkmanager の探索規約（cmdline-tools/latest 配下を期待）を満たしつつ、
# バージョンアップ時の完了マーカー（バージョン付きディレクトリ）と整合させるための橋渡し。
relink_cmdline_tools_latest() {
  local versioned_dir="$1" latest_link="$2"
  if [ -L "${latest_link}" ] && [ "$(readlink -- "${latest_link}")" = "${versioned_dir}" ]; then
    return 0
  fi
  # 旧バージョンへのリンク・旧レイアウトの実ディレクトリのいずれが残っていても張り替える。
  rm -rf "${latest_link}"
  ln -s "${versioned_dir}" "${latest_link}"
}

# パッケージ名（"platforms;android-35" 等）からローカル配置ディレクトリ名（";" 以降）を取り出す。
sdk_package_dir_name() {
  echo "${1#*;}"
}

# パッケージ名（"platforms;android-35" 等）から SDK_ROOT 相対の配置パスを取り出す
# （";" を "/" に置換。"platform-tools" のように ";" を含まないパッケージはそのまま）。
sdk_package_rel_path() {
  case "$1" in
    *\;*) echo "${1/;//}" ;;
    *) echo "$1" ;;
  esac
}

install_sdk_packages() {
  local sdkmanager="${SDK_ROOT}/cmdline-tools/latest/bin/sdkmanager"
  [ -x "${sdkmanager}" ] || fail "sdkmanager が見つかりません（install_cmdline_tools が先に必要です）"

  # sdkmanager 自身が各パッケージのダウンロードをチェックサム付きで検証するため、
  # ここでの追加検証は不要（versions.env コメント参照）。
  # NDK の存在のみを完了マーカーにすると、NDK 導入済み・他パッケージ欠落のケースで
  # それらが永久にインストールされないため、パッケージごとに個別判定して不足分のみ導入する。
  # 完了マーカーは各パッケージの source.properties（展開完了後に配置される配布物同梱の
  # メタデータファイル）の存在とする。ディレクトリ存在や adb 実行可否だけでは、
  # 後述のアトミック mv 導入前の中断で生じた半展開ディレクトリと区別できないため。
  local -a missing=()
  [ -f "${SDK_ROOT}/platform-tools/source.properties" ] || missing+=("${PLATFORM_TOOLS_PACKAGE}")
  [ -f "${SDK_ROOT}/platforms/$(sdk_package_dir_name "${PLATFORM_PACKAGE}")/source.properties" ] || missing+=("${PLATFORM_PACKAGE}")
  [ -f "${SDK_ROOT}/build-tools/$(sdk_package_dir_name "${BUILD_TOOLS_PACKAGE}")/source.properties" ] || missing+=("${BUILD_TOOLS_PACKAGE}")
  [ -f "${SDK_ROOT}/ndk/${NDK_REVISION}/source.properties" ] || missing+=("${NDK_PACKAGE}")

  if [ "${#missing[@]}" -eq 0 ]; then
    log "Android SDK パッケージ（platform-tools/platforms/build-tools/NDK）は全て導入済みのため skip"
    return 0
  fi

  # 実際にダウンロードが発生するパッケージが 1 つ以上ある場合のみ空き容量を確認する
  # （全パッケージ導入済みの再実行時に不要な容量チェックで落ちないようにするため）。
  check_disk_space

  # sdkmanager は --sdk_root で指定した先へ直接展開するため、JDK/Gradle のような
  # 「一時ディレクトリへ展開してから最終パスへ atomic mv」がそのままでは効かない
  # （sdkmanager 自身が最終パスへ書き込むため）。そこで一時的に独立した sdk_root を
  # 用意してそこへ導入させ、パッケージごとに展開が完全に終わったディレクトリ単位で
  # SDK_ROOT へ atomic mv する。ジョブがキャンセルされて一時 sdk_root 側が半展開の
  # まま残っても、完了マーカーとして参照する SDK_ROOT 側には一切影響しないため、
  # 後続の実行が半展開状態を「導入済み」と誤認することはない。
  # ライセンス同意は install_cmdline_tools（cmdline-tools 新規導入時のみ実行）だけでは
  # 不十分（cmdline-tools 導入済みで skip されるパスでは SDK_ROOT 側のライセンスが
  # 未同意のままになり得る）ため、ここでも SDK_ROOT に対して冪等に実行し直す。
  # CI の concurrency.cancel-in-progress によりライセンス同意前後でジョブが中断された
  # 場合に、対話プロンプトで無期限にハングしないよう yes パイプで応答する。
  yes | JAVA_HOME="${JDK_ROOT}/${JDK_EXTRACTED_DIR}" "${sdkmanager}" \
    --sdk_root="${SDK_ROOT}" --licenses >/dev/null 2>&1 || true

  local tmp_sdk_root
  tmp_sdk_root="$(mktemp -d "${TMP_ROOT}/sdk-packages.XXXXXX")"

  # 一時 sdk_root は SDK_ROOT と別ディレクトリのため、パッケージ導入に使う
  # sdkmanager 呼び出し向けにライセンス同意も個別に必要
  # （ライセンス同意状態は sdk_root 配下の licenses/ に保存されるため）。
  # </dev/null: 想定外の対話プロンプトが出た場合でも無期限にハングさせず即座に
  # 非ゼロ終了させる（フェイルファスト）。
  yes | JAVA_HOME="${JDK_ROOT}/${JDK_EXTRACTED_DIR}" "${sdkmanager}" \
    --sdk_root="${tmp_sdk_root}" --licenses >/dev/null 2>&1 || true

  log "sdkmanager でパッケージ導入（一時 sdk_root へ）: ${missing[*]}"
  JAVA_HOME="${JDK_ROOT}/${JDK_EXTRACTED_DIR}" "${sdkmanager}" \
    --sdk_root="${tmp_sdk_root}" "${missing[@]}" </dev/null

  local pkg rel_path src dst
  for pkg in "${missing[@]}"; do
    rel_path="$(sdk_package_rel_path "${pkg}")"
    src="${tmp_sdk_root}/${rel_path}"
    dst="${SDK_ROOT}/${rel_path}"
    [ -f "${src}/source.properties" ] \
      || fail "sdkmanager 導入後にパッケージが見つかりません: ${pkg} (${src}/source.properties)"
    mkdir -p "$(dirname -- "${dst}")"
    # 完了マーカー変更前（弱いディレクトリ存在判定）の時代に残った半展開ディレクトリが
    # dst に残っている場合に備え、置き換えてから配置し直す。ここは main() の flock で
    # 他ジョブとの直列化済みだが、rm -rf は多段の削除処理でディレクトリサイズに応じて
    # 時間を要し得るため、防御的にも「dst 不在」の窓を最小化する。rm -rf ではなく
    # 同一ファイルシステム（TMP_ROOT 配下）内への rename（mv）で退避してから src を
    # 配置することで、dst 不在の期間を 2 回の rename のみに縮める（最後の
    # rm -rf "${tmp_sdk_root}" で退避先ごとまとめて破棄される）。
    if [ -e "${dst}" ]; then
      mv "${dst}" "${tmp_sdk_root}/.stale-${pkg//;/-}"
    fi
    mv "${src}" "${dst}"
    log "SDK パッケージ導入完了（atomic mv）: ${pkg} -> ${dst}"
  done

  rm -rf "${tmp_sdk_root}"
}

verify_sdk() {
  local sdkmanager="${SDK_ROOT}/cmdline-tools/latest/bin/sdkmanager"
  [ -x "${sdkmanager}" ] || fail "sdkmanager が未導入です"

  # cmdline-tools 自体のバージョンピン（versions.env の CMDLINE_TOOLS_VERSION）が
  # 実際に反映されているかを、配布物同梱の source.properties の Pkg.Revision で検証する。
  local cmdline_tools_props="${SDK_ROOT}/cmdline-tools/${CMDLINE_TOOLS_VERSION}/source.properties"
  [ -f "${cmdline_tools_props}" ] \
    || fail "Android commandline-tools が未導入です（期待バージョン ${CMDLINE_TOOLS_VERSION}）: ${cmdline_tools_props} が見つかりません"
  grep -qF "Pkg.Revision=${CMDLINE_TOOLS_VERSION}" "${cmdline_tools_props}" \
    || fail "Android commandline-tools バージョン不一致: ${cmdline_tools_props} に Pkg.Revision=${CMDLINE_TOOLS_VERSION} が見つかりません"
  log "Android commandline-tools 検証 OK (Pkg.Revision = ${CMDLINE_TOOLS_VERSION})"

  local ndk_props="${SDK_ROOT}/ndk/${NDK_REVISION}/source.properties"
  [ -f "${ndk_props}" ] || fail "NDK が未導入です: ${ndk_props} が見つかりません"
  grep -qF "${NDK_REVISION}" "${ndk_props}" \
    || fail "NDK バージョン不一致: ${ndk_props} に ${NDK_REVISION} が見つかりません"
  log "NDK 検証 OK: ${ndk_props} (Pkg.Revision = ${NDK_REVISION})"

  # install_sdk_packages の完了マーカー（source.properties の存在）と揃え、
  # 半展開ディレクトリを「導入済み」と誤検証しないようにする。
  [ -f "${SDK_ROOT}/platform-tools/source.properties" ] \
    || fail "platform-tools が未導入です: source.properties が見つかりません"
  [ -x "${SDK_ROOT}/platform-tools/adb" ] || fail "platform-tools が未導入です: adb が見つかりません"

  local platforms_dir
  platforms_dir="${SDK_ROOT}/platforms/$(sdk_package_dir_name "${PLATFORM_PACKAGE}")"
  [ -f "${platforms_dir}/source.properties" ] \
    || fail "platforms（${PLATFORM_PACKAGE}）が未導入です: source.properties が見つかりません"

  local build_tools_dir
  build_tools_dir="${SDK_ROOT}/build-tools/$(sdk_package_dir_name "${BUILD_TOOLS_PACKAGE}")"
  [ -f "${build_tools_dir}/source.properties" ] \
    || fail "build-tools（${BUILD_TOOLS_PACKAGE}）が未導入です: source.properties が見つかりません"
  log "SDK パッケージ検証 OK"
}

# ---------------------------------------------------------------------------
# Gradle
# ---------------------------------------------------------------------------
install_gradle() {
  local final_dir="${GRADLE_ROOT}/gradle-${GRADLE_VERSION}"
  if [ -x "${final_dir}/bin/gradle" ]; then
    log "Gradle は導入済みのため skip: ${final_dir}"
    return 0
  fi

  local work
  work="$(mktemp -d "${TMP_ROOT}/gradle.XXXXXX")"
  local archive="${work}/${GRADLE_ARCHIVE_NAME}"
  download_and_verify "${GRADLE_URL}" "${archive}" sha256 "${GRADLE_SHA256}"

  mkdir -p "${GRADLE_ROOT}"
  unzip -q "${archive}" -d "${work}/extracted"
  mv "${work}/extracted/gradle-${GRADLE_VERSION}" "${final_dir}"
  rm -rf "${work}"
  log "Gradle 導入完了: ${final_dir}"
}

verify_gradle() {
  local final_dir="${GRADLE_ROOT}/gradle-${GRADLE_VERSION}"
  [ -x "${final_dir}/bin/gradle" ] || fail "Gradle が未導入です: ${final_dir}/bin/gradle が見つかりません"
  local version_output
  version_output="$(JAVA_HOME="${JDK_ROOT}/${JDK_EXTRACTED_DIR}" "${final_dir}/bin/gradle" --version 2>&1 || true)"
  echo "${version_output}" | grep -qF "${GRADLE_VERSION}" \
    || fail "Gradle バージョン不一致: 期待 ${GRADLE_VERSION} / 実際 ${version_output}"
  log "Gradle 検証 OK"
}

# ---------------------------------------------------------------------------
# bundletool（TASK-11.3 / REQ-11、Issue #246）
#
# `.aab` を実機/エミュレータへインストールするために必須（adb は `.aab` を直接
# インストールできない）。JDK/Gradle と同じ「一時ディレクトリへ展開 → 最終パスへ
# atomic mv」の完了マーカー方式を踏襲する。BUNDLETOOL_JAR_NAME にバージョン番号を
# 含めているため、versions.env の BUNDLETOOL_VERSION 更新がそのまま再導入を促す
# （cmdline-tools の CMDLINE_TOOLS_VERSION 判定と同じ設計）。
# ---------------------------------------------------------------------------
install_bundletool() {
  local final_path="${BUNDLETOOL_ROOT}/${BUNDLETOOL_JAR_NAME}"
  if [ -f "${final_path}" ]; then
    log "bundletool は導入済みのため skip: ${final_path}"
    return 0
  fi

  local work
  work="$(mktemp -d "${TMP_ROOT}/bundletool.XXXXXX")"
  local archive="${work}/${BUNDLETOOL_JAR_NAME}"
  download_and_verify "${BUNDLETOOL_URL}" "${archive}" sha256 "${BUNDLETOOL_SHA256}"

  mkdir -p "${BUNDLETOOL_ROOT}"
  # jar 自体をそのまま最終パスへ atomic mv する（展開不要な単一ファイル配布物）。
  mv "${archive}" "${final_path}"
  rm -rf "${work}"
  log "bundletool 導入完了: ${final_path}"
}

verify_bundletool() {
  local final_path="${BUNDLETOOL_ROOT}/${BUNDLETOOL_JAR_NAME}"
  [ -f "${final_path}" ] || fail "bundletool が未導入です: ${final_path} が見つかりません"
  log "bundletool 検証 OK: ${final_path}"
}

# ---------------------------------------------------------------------------
# rustup Android ターゲット（rustup 導入済みの場合のみ・冪等）
# ---------------------------------------------------------------------------
install_rust_targets() {
  if ! command -v rustup >/dev/null 2>&1; then
    log "rustup 未導入のため Android ターゲット追加を skip（TASK-1.6/TASK-11.2 側で対応）"
    return 0
  fi
  local installed target
  installed="$(rustup target list --installed 2>/dev/null || true)"
  for target in ${RUST_ANDROID_TARGETS}; do
    if echo "${installed}" | grep -qxF "${target}"; then
      log "rustup target ${target} は導入済みのため skip"
    else
      log "rustup target 追加: ${target}"
      rustup target add "${target}"
    fi
  done
}

verify_rust_targets() {
  if ! command -v rustup >/dev/null 2>&1; then
    log "rustup 未導入のため Android ターゲット検証を skip"
    return 0
  fi
  local installed target
  installed="$(rustup target list --installed 2>/dev/null || true)"
  for target in ${RUST_ANDROID_TARGETS}; do
    echo "${installed}" | grep -qxF "${target}" || fail "rustup target 未導入: ${target}"
  done
  log "rustup Android ターゲット検証 OK"
}

# ---------------------------------------------------------------------------
# cargo-ndk（TASK-1.5-S1 / REQ-1）
#
# rustup target add と同様、cargo が未導入の環境（cargo 自体の導入は本スクリプトの
# 責務外）では skip する。バージョン一致判定は `cargo ndk --version` の出力から
# semver 形式のトークンのみを抽出し CARGO_NDK_VERSION と完全一致で比較する
# （cargo install --list はサブコマンド名単位の一覧のみでバージョンを含まないため
# `cargo ndk --version` を使う。部分文字列一致だと 4.1.2 導入済み判定時に実体が
# 4.1.20 等でも誤って一致してしまい、versions.env が意図する厳密固定を損なうため
# 完全一致にしている。末尾トークンの単純抽出ではなく semver パターンで抽出するのは、
# 出力にビルドハッシュ・日付等の付加情報が付く場合でも誤って不一致/一致にならない
# ようにするため）。
# ---------------------------------------------------------------------------
cargo_ndk_version_matches() {
  local version_output installed_version
  version_output="$(cargo ndk --version 2>/dev/null || true)"
  # 出力例: "cargo-ndk 4.1.2"。semver 形式（メジャー.マイナー.パッチ + 任意の
  # プレリリース/ビルド識別子）の最初のトークンのみを抽出し完全一致で比較する。
  installed_version="$(printf '%s\n' "${version_output}" \
    | grep -oE '[0-9]+\.[0-9]+\.[0-9]+[A-Za-z0-9.+-]*' | head -n 1)"
  [ -n "${installed_version}" ] && [ "${installed_version}" = "${CARGO_NDK_VERSION}" ]
}

install_cargo_ndk() {
  if ! command -v cargo >/dev/null 2>&1; then
    log "cargo 未導入のため cargo-ndk 導入を skip（TASK-1.6/TASK-11.2 側で対応）"
    return 0
  fi
  if cargo_ndk_version_matches; then
    log "cargo-ndk ${CARGO_NDK_VERSION} は導入済みのため skip"
    return 0
  fi
  log "cargo-ndk ${CARGO_NDK_VERSION} を導入（cargo install --locked）"
  # --locked: Cargo.lock 記載の依存ハッシュを検証させ、サプライチェーン改変を検知する
  # （security.md A06/A08 対応。versions.env の CARGO_NDK_VERSION 厳密固定とあわせて
  # バージョン・依存ツリーの双方を固定する）。
  cargo install cargo-ndk --version "${CARGO_NDK_VERSION}" --locked
}

verify_cargo_ndk() {
  if ! command -v cargo >/dev/null 2>&1; then
    log "cargo 未導入のため cargo-ndk 検証を skip"
    return 0
  fi
  cargo_ndk_version_matches \
    || fail "cargo-ndk バージョン不一致または未導入です（期待 ${CARGO_NDK_VERSION}）"
  log "cargo-ndk 検証 OK (${CARGO_NDK_VERSION})"
}

# ---------------------------------------------------------------------------
# 空きディスク容量チェック（NDK 込みで概ね 5GB 必要）
#
# 呼び出し元: download_and_verify（実ダウンロード発生時）と install_sdk_packages
# （sdkmanager 経由の未導入パッケージがある場合）からのみ呼ぶ。
# main() の先頭で無条件に呼ぶと、全コンポーネント導入済みで本来 no-op のはずの
# 再実行時にも、runner の空き容量が閾値を下回っただけで毎回失敗してしまうため
# （何もインストールしないなら容量チェックの意味がない）、実際に書き込みが
# 発生する直前でのみチェックする設計にしている。
# ---------------------------------------------------------------------------
check_disk_space() {
  local required_kb=$((5 * 1024 * 1024))
  local available_kb
  mkdir -p "${FANDHE_ANDROID_ROOT}"
  available_kb="$(df -Pk "${FANDHE_ANDROID_ROOT}" | awk 'NR==2 {print $4}')"
  if [ -z "${available_kb}" ] || [ "${available_kb}" -lt "${required_kb}" ]; then
    fail "空き容量不足の可能性: ${FANDHE_ANDROID_ROOT} の空き容量 ${available_kb:-unknown}KB (推奨 5GB 以上)"
  fi
}

# ---------------------------------------------------------------------------
# FANDHE_ANDROID_ROOT 単位の直列化ロック取得（flock 可搬層）
#
# flock(1) は util-linux 由来で macOS 標準には存在しない（ローカル開発機での
# 実行時に「他ジョブが占有中」という誤検知で即失敗する）。CI runner
# （Linux 前提）の既存挙動は変えたくないため、flock が存在する環境ではそのまま
# flock -w を使い、存在しない環境（macOS 等）のみ mkdir の atomic 性を使った
# 代替ロック（ディレクトリ作成の成否は POSIX で atomic）にフォールバックする。
# 待機・タイムアウト・失敗時メッセージの意味は両実装で揃える。
# ---------------------------------------------------------------------------
acquire_install_lock() {
  local lock_timeout_sec="$1"
  local lock_dir="${FANDHE_ANDROID_ROOT}/.install.lock.d"

  if command -v flock >/dev/null 2>&1; then
    exec 9>"${FANDHE_ANDROID_ROOT}/.install.lock"
    flock -w "${lock_timeout_sec}" 9 \
      || fail "他ジョブが ${FANDHE_ANDROID_ROOT} を占有中のためロック取得がタイムアウトしました（${lock_timeout_sec} 秒）"
    return 0
  fi

  log "flock コマンドが見つからないため mkdir ベースの代替ロックを使用します（macOS 等）"
  local waited=0
  while ! mkdir "${lock_dir}" 2>/dev/null; do
    # stale ロック回収: EXIT trap は SIGKILL・ホスト停止・クラッシュでは走らないため、
    # 所有プロセスの pid を記録しておき、同一ホスト上に存在しなければ残骸と判断
    # して回収する（flock はカーネルが自動解放するため flock 経路には不要）。
    # pid ファイル未作成（所有側が mkdir 直後で echo 前）の場合は誤回収を避けて待つ。
    # kill -0 が EPERM で失敗する他ユーザー所有プロセスは稀（開発機・runner とも
    # 同一ユーザー運用）のため考慮しない。
    local owner_pid
    owner_pid=$(cat "${lock_dir}/pid" 2>/dev/null || true)
    if [ -n "${owner_pid}" ] && ! kill -0 "${owner_pid}" 2>/dev/null; then
      log "stale ロックを回収します（所有 pid=${owner_pid} は存在しません）"
      rm -f "${lock_dir}/pid"
      rmdir "${lock_dir}" 2>/dev/null || true
      continue
    fi
    if [ "${waited}" -ge "${lock_timeout_sec}" ]; then
      fail "他ジョブが ${FANDHE_ANDROID_ROOT} を占有中のためロック取得がタイムアウトしました（${lock_timeout_sec} 秒）"
    fi
    sleep 1
    waited=$((waited + 1))
  done
  # stale 判定用に所有プロセスの pid を記録する（上記回収ロジックと対）。
  echo "$$" > "${lock_dir}/pid"
  # プロセス終了時（正常・異常終了問わず）にロックディレクトリを解放する。
  # flock 版は fd クローズで自動解放されるため、こちらも trap で同等の後始末を行う。
  # パスを trap のコマンド文字列へ即時展開すると、外部入力 FANDHE_ANDROID_ROOT に
  # シェル構文（' 等）を含めた場合に引用を脱出してコード注入できてしまうため、
  # グローバル変数 + 名前付き cleanup 関数経由でデータとして参照する
  # （lock_dir は関数ローカルのため trap 発火時に参照できない点も同時に解決）。
  INSTALL_LOCK_DIR="${lock_dir}"
  trap cleanup_install_lock EXIT
}

# acquire_install_lock（mkdir 代替ロック経路）が trap EXIT から呼ぶ後始末。
# ロックパスは INSTALL_LOCK_DIR（グローバル変数）でデータとして受け取る。
cleanup_install_lock() {
  rm -f -- "${INSTALL_LOCK_DIR}/pid" 2>/dev/null || true
  rmdir -- "${INSTALL_LOCK_DIR}" 2>/dev/null || true
}

main() {
  mkdir -p "${FANDHE_ANDROID_ROOT}"
  # 複数ジョブ（並列 PR や cancel-in-progress 後の再実行）が同一 CI runner の
  # HOME を共有すると、既定の FANDHE_ANDROID_ROOT（未設定時は一意化されない）配下の
  # SDK_ROOT を複数ジョブが同時に読み書きしうる。install_sdk_packages の
  # パッケージ差し替え（旧 dst の退避 → src の配置）は個々の mv はアトミックでも
  # 退避と配置の間に一瞬 dst が不在になる窓があり、他ジョブがその窓で SDK_ROOT を
  # 検証・参照すると失敗し得る（Bugbot 指摘: Shared SDK root race）。
  # FANDHE_ANDROID_ROOT ごとに一意な作業ディレクトリへ分離する方式は、ジョブ間で
  # 意図している「導入済みなら skip」の共有キャッシュ効果（数 GB のダウンロード・
  # 展開を毎回繰り返さない）を失わせるため採らず、代わりに導入・検証全体を
  # FANDHE_ANDROID_ROOT 単位の flock で直列化する。これにより他ジョブがその窓を
  # 観測すること自体を防ぐ（rename-aside による窓の縮小は多重の防御として残す）。
  # CI（.github/workflows/ci.yml の android-env ジョブ、timeout-minutes: 45）は
  # 本スクリプトを「導入」→「--verify-only 検証」の 2 回、同一ジョブ内で逐次呼び出す。
  #
  # 待機上限は「2 回の待機を合計してもジョブタイムアウトに収まる」固定値（旧: 20 分）
  # ではなく、ジョブタイムアウトに対して十分な単発の待機時間を確保する（Bugbot 指摘:
  # Lock wait shorter than install）。コールド NDK インストール（数 GB のダウンロード・
  # 展開）はホルダー側で 20 分を超えることがあり、その場合に待機側（既にインストール
  # 済みで --verify-only を実行するだけのジョブを含む）を待機タイムアウトで
  # 失敗させるのではなく、ホルダーの完了まで直列に待たせたい。待機側自身も
  # 同じ 45 分のジョブタイムアウトを持つため、待機上限をジョブタイムアウトに近い
  # 値まで引き上げても「真にデッドロックした場合は待機側自身のジョブタイムアウトで
  # 検知される」という安全側の性質は保たれる。checkout 等のオーバーヘッド分だけ
  # 余裕を残し、既定 2400 秒（40 分）とする（環境変数で上書き可能）。
  local lock_timeout_sec="${FANDHE_ANDROID_LOCK_TIMEOUT_SEC:-2400}"
  case "${lock_timeout_sec}" in
    '' | *[!0-9]*)
      fail "FANDHE_ANDROID_LOCK_TIMEOUT_SEC は秒数（整数）で指定してください: ${lock_timeout_sec}"
      ;;
  esac
  acquire_install_lock "${lock_timeout_sec}"

  if [ "${VERIFY_ONLY}" -eq 1 ]; then
    log "--verify-only モード: 導入は行わず検証のみ実施します"
    verify_jdk
    verify_sdk
    verify_gradle
    verify_bundletool
    verify_rust_targets
    verify_cargo_ndk
    log "全コンポーネント検証 OK"
    return 0
  fi

  mkdir -p "${TMP_ROOT}"
  install_jdk
  install_cmdline_tools
  install_sdk_packages
  install_gradle
  install_bundletool
  install_rust_targets
  install_cargo_ndk
  # 注意: ここで TMP_ROOT（共有の一時ディレクトリルート）自体を rm -rf しない。
  # 各 install_* は自分の mktemp -d 作業ディレクトリを完了時に個別削除済みであり、
  # CI runner 上で android-env ジョブが並行実行される場合（並列 PR や
  # cancel-in-progress による再実行）、他ジョブがまだ展開・ダウンロード中の
  # 作業ディレクトリを TMP_ROOT ごと削除してしまうとチェックサム不整合や展開失敗による
  # フレーキーな失敗を招く。クラッシュ等で残った孤児ディレクトリの掃除は本スクリプトの
  # 責務外とする（次回以降の mktemp -d は一意なサブディレクトリを作るため実害はない）。
  log "導入完了。source scripts/android/android-env.sh で環境変数を読み込んでください"
}

main "$@"
