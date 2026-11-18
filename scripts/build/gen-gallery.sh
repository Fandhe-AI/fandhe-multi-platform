#!/usr/bin/env bash
# カタログアプリ（fandhe-gallery）の各 OS の設定を、機能 crate の宣言とアプリ設定から作り直す（Issue #1194）。
#
# 出力: crates/fandhe-gallery/gen/（git 管理外。`cargo fandhe gen` の出力をそのまま置く）
#   - android/: platforms/android の殻が取り込む Manifest の overlay・Java / res の置き場・Gradle 依存
#   - ios/: .app を組むスクリプト（scripts/ios/*）が使う Info.plist・entitlements、拡張の Info.plist
#   - macos/: .app を組むスクリプト（scripts/apple/*）と Xcode の拡張が使う Info.plist・entitlements
#     （素の実行体への埋め込みは crates/fandhe-gallery/build.rs が同じ生成をして行う）
#
# 手書きの設定は持たないため、バンドル・APK を組むスクリプトはビルドの前に本スクリプトを呼ぶ。
# 宣言・アプリ設定の誤り（説明文の欠け等）はここで止まる。
set -euo pipefail
unset CDPATH
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." >/dev/null 2>&1 && pwd -P)"
cargo run --manifest-path "${repo_root}/Cargo.toml" -q -p cargo-fandhe -- \
  fandhe gen --manifest-path "${repo_root}/crates/fandhe-gallery/Cargo.toml"
