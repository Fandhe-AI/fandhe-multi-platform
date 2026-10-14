# shellcheck shell=bash
# source 専用ファイル。setup-android-env.sh が導入した JDK・Android SDK・NDK・Gradle への
# 環境変数（JAVA_HOME / ANDROID_HOME / ANDROID_NDK_HOME / PATH）をシェルへ反映する。
#
# 呼び出し元: ローカル開発者が `source scripts/android/android-env.sh` として利用するほか、
# CI（.github/workflows/ci.yml の android-env ジョブ）から同様に source される想定。
# 単体実行はしない（シバン無し・実行権限も付与しない。source されることのみを前提とする）。
#
# .claude/rules/ci.md の「シェル profile を書き換えない」方針に合わせ、
# ~/.zshrc 等の永続化は行わず、利用側が都度 source する運用とする。

SCRIPT_DIR_ANDROID_ENV="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")" >/dev/null 2>&1 && pwd -P)"

set -a
# shellcheck source=versions.env
# shellcheck disable=SC1091
. "${SCRIPT_DIR_ANDROID_ENV}/versions.env"
set +a

FANDHE_ANDROID_ROOT="${FANDHE_ANDROID_ROOT:-${HOME}/.fandhe/android}"

export JAVA_HOME="${FANDHE_ANDROID_ROOT}/jdk/${JDK_EXTRACTED_DIR}"
export ANDROID_HOME="${FANDHE_ANDROID_ROOT}/sdk"
export ANDROID_SDK_ROOT="${ANDROID_HOME}"
export ANDROID_NDK_HOME="${ANDROID_HOME}/ndk/${NDK_REVISION}"
export GRADLE_HOME="${FANDHE_ANDROID_ROOT}/gradle/gradle-${GRADLE_VERSION}"
# TASK-11.3（REQ-11、Issue #246）: verify-aab-install.sh が `.aab` → `.apks` 変換・
# インストールに使う bundletool 本体（単一 jar）へのパス。他スクリプトからの
# 参照点をここへ一元化する（JAVA_HOME 等と同じ設計）。
export BUNDLETOOL_JAR="${FANDHE_ANDROID_ROOT}/bundletool/${BUNDLETOOL_JAR_NAME}"

export PATH="${JAVA_HOME}/bin:${GRADLE_HOME}/bin:${ANDROID_HOME}/cmdline-tools/latest/bin:${ANDROID_HOME}/platform-tools:${PATH}"

unset SCRIPT_DIR_ANDROID_ENV
