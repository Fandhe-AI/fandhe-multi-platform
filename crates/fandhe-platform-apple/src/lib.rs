//! Apple（iOS / macOS）向けの共通層。機能ごとの crate が ObjC / Swift 薄層を自分の中に持てるように、
//! 全機能で共有する部品だけをここに置く（TASK-21.2 / Issue #1168、親 #1159）。
//!
//! - `include/fandhe_ios_common.h`: 全 `.m` が `#import` する共有ヘッダ。`build` モジュールの
//!   `compile_objc` が `-I` へ常に足すため、利用側は何も指定しなくてよい。
//! - `build`（feature `build`）: build script 用の `.m` / Swift コンパイルと framework リンクの helper。
//!   build 依存として使う。Linux・Windows のホストでもコンパイルされるため、本 crate 全体は
//!   `cfg` で囲わない。
//! - [`is_main_thread`]: main thread 限定の OS 呼び出しの前に使う判定（iOS・macOS のみ）。
//! - [`app_group_id`]・`macos_app_group_dir`: 拡張を持つ機能（home-widget・quick-settings-tile）が共有する
//!   App Group の ID / コンテナパスの組み立てと Team ID 検証。
//!
//! - [`ios_scene_install`]（iOS）: UIScene 化シム `ios/fandhe_ios_scene.m` を有効にする（Issue #1292）。
//!   Info.plist の `UIApplicationSceneManifest` が名指す `FandheSceneDelegate` はレンダラに依存せず、
//!   外部ディスプレイ・URL・ショートカットの通知を受ける機能 crate が本 crate に依存するため、ここに置く。
//!   winit が scene に対応（upstream #4723）したら削除する。
//!
//! `ios_text_input` は winit の補完でレンダラ固有のため、本 crate には置かない。

#![deny(unsafe_code)]

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod ios_scene {
    unsafe extern "C" {
        /// `ios/fandhe_ios_scene.m`（`build.rs` が iOS で常にコンパイル）が定義する引数なしの C 関数。
        fn fandhe_ios_scene_install();
    }

    pub(super) fn install() {
        // SAFETY: 引数・戻り値なしの C 関数。前提条件はなく、冪等で副作用は通知の購読登録だけ。
        unsafe { fandhe_ios_scene_install() }
    }
}

/// UIScene 化シムを有効にする。`FandheSceneDelegate` をリンクに残し、winit の window を scene へ付ける購読を登録する。
///
/// レンダラの起動関数（`fandhe_render_egui::launch::run_ios`）が `UIApplicationMain` より前に呼ぶ。
/// 冪等。呼ばないと Info.plist の delegate が見つからず、画面が出ない。
#[cfg(target_os = "ios")]
pub fn ios_scene_install() {
    ios_scene::install();
}

#[cfg(feature = "build")]
pub mod build;

#[cfg(any(target_os = "ios", target_os = "macos"))]
#[allow(unsafe_code)]
mod main_thread {
    // SAFETY: `pthread_main_np` は libSystem（Darwin の全バイナリに常時リンク済み）が
    // 提供する引数無しの単純な問い合わせ関数で、呼び出しスレッドを変更しない
    // （呼び出し元スレッドが main スレッドなら非 0 を返す）。
    unsafe extern "C" {
        fn pthread_main_np() -> i32;
    }

    pub(super) fn is_main_thread() -> bool {
        // SAFETY: 上記 `pthread_main_np` 宣言のコメント参照。副作用も前提条件も無く
        // 常に安全に呼び出せる。
        unsafe { pthread_main_np() != 0 }
    }
}

/// 呼び出しスレッドが main スレッドかどうかを返す。
///
/// UIKit / AppKit を触る ObjC 実装は main スレッド専用の呼び出し契約を持つ。公開型は誰でも構築でき
/// ワーカースレッドからの誤用をコンパイラは検出できないため、FFI 呼び出しの前にこれで検査し、
/// main 以外ならエラーを返して unsafe な前提を呼び出し側へ漏らさない（coding-rust.md の FFI 例外規定）。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub fn is_main_thread() -> bool {
    main_thread::is_main_thread()
}

/// Team ID（`[A-Z0-9]{10}`）と App Group 名（`[a-z]+`）から App Group ID `<Team ID>.com.fandhe.<group>` を作る。
///
/// 形式違いは `None`。ホーム画面ウィジェットとクイック設定タイル・コントロールなど、拡張を持つ機能が
/// 同じ検証（パストラバーサル防止、security.md A01/A03）を複製せずに使う。`group` は宣言の `app-groups` に
/// 対応する固定値だけを渡す前提（利用者入力を渡さない）。
pub fn app_group_id(team: &str, group: &str) -> Option<String> {
    let team_ok = team.len() == 10
        && team
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    let group_ok = !group.is_empty() && group.bytes().all(|b| b.is_ascii_lowercase());
    (team_ok && group_ok).then(|| format!("{team}.com.fandhe.{group}"))
}

/// macOS の App Group コンテナのパスを組む。Team ID（`FANDHE_DEVELOPMENT_TEAM`、実行時 → ビルド時）・
/// ホームが取れなければ `None`（呼び出し側が `Unsupported` へ倒す。ダミー成功にしない）。
#[cfg(target_os = "macos")]
pub fn macos_app_group_dir(group: &str) -> Option<std::path::PathBuf> {
    let team = std::env::var("FANDHE_DEVELOPMENT_TEAM")
        .ok()
        .or_else(|| option_env!("FANDHE_DEVELOPMENT_TEAM").map(str::to_string))?;
    let id = app_group_id(&team, group)?;
    #[allow(deprecated)]
    let home = std::env::home_dir()?;
    Some(home.join("Library/Group Containers").join(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_group_id_validates_team_and_group() {
        assert_eq!(
            app_group_id("ABCDE12345", "widget").as_deref(),
            Some("ABCDE12345.com.fandhe.widget")
        );
        for bad in [
            "",
            "abcde12345",
            "ABCDE1234",
            "ABCDE123456",
            "../ABCDEFG",
            "ABCDE/2345",
        ] {
            assert!(app_group_id(bad, "widget").is_none(), "{bad}");
        }
        for bad in ["", "Widget", "../x", "a.b", "w1"] {
            assert!(app_group_id("ABCDE12345", bad).is_none(), "{bad}");
        }
    }
}
