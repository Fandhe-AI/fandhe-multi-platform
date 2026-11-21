//! 選んだ端末機能の起動前の登録（Issue #1200）。
//!
//! 利用者が機能ごとの手順を知らずに済むよう、入口がここで有効な feature の分だけ呼ぶ（機能の追加・削除は
//! feature の追加・削除だけで済ませる。`docs/architecture/consumer-app-structure.md` §4）。
//! 呼ぶ関数の網羅は `tests/features.rs` が機能 crate の crate ルートと突き合わせて検査する。
//!
//! 終了処理・画面の破棄・Android の再生成時の破棄はここに置かない。機能 crate が初めて使われたときに
//! `fandhe_runtime::teardown` へ自分で登録し、描画層と Android の入口がその表を回す（Issue #1453）。
//! 起動前の登録は初めて使われるより前に要るため、表へ遅らせられない。

/// `UIApplicationMain`（iOS）・`run_native`（デスクトップ）より前に呼ぶ登録。
///
/// 起動と同じターンで届く通知（URL・ファイルを開く・背景転送の完了・検索から開く）を取りこぼさないよう、
/// イベントループの開始前に main thread で登録する。Windows のタスク スケジューラによる再起動と、
/// URL 起動を起動済みのインスタンスへ転送した場合は、画面を出さずにここでプロセスを終える。
pub(crate) fn before_launch() {
    // Issue #1044: タスク スケジューラが専用引数付きで本 exe を再起動したときは、画面を出さずに
    // 実行結果を書いて終了する（`fandhe-cap-background-task` の `windows.rs` モジュール doc 参照）。
    #[cfg(all(target_os = "windows", feature = "background-task"))]
    {
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        if fandhe_cap_background_task::parse_scheduled_args(&args) {
            std::process::exit(fandhe_cap_background_task::run_scheduled());
        }
    }
    // Issue #1333: ファイル起動の経路は、単一インスタンスの調停を持つ url-scheme の `startup` より前に登録する
    // （登録が無いとファイル起動は転送されない）。
    #[cfg(all(target_os = "windows", feature = "file-association"))]
    fandhe_cap_file_association::windows::register_route();
    // Issue #1216: URL・ファイル起動で既存インスタンスがあれば pipe で転送して終える。受ける側なら受け口へ入れて続ける。
    // 調停は url-scheme の `startup` が持つ。file-association だけを選んだアプリも同じ調停を通す
    // （通さないとファイル起動の引数が受け口へ届かない）。
    #[cfg(all(
        target_os = "windows",
        any(feature = "url-scheme", feature = "file-association")
    ))]
    {
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        #[cfg(feature = "url-scheme")]
        let forwarded = fandhe_cap_url_scheme::windows::win::startup(&args);
        #[cfg(not(feature = "url-scheme"))]
        let forwarded = fandhe_cap_file_association::windows::startup(&args);
        if let Some(code) = forwarded {
            std::process::exit(code);
        }
    }
    // Issue #1045: `BGTaskScheduler` の launch handler は `UIApplicationMain` より前に登録する契約。
    #[cfg(all(target_os = "ios", feature = "background-task"))]
    fandhe_cap_background_task::register();
    // Issue #1215・#1217: URL scheme の受信（macOS は Apple Event、iOS は scene の通知）。
    #[cfg(all(any(target_os = "ios", target_os = "macos"), feature = "url-scheme"))]
    fandhe_cap_url_scheme::register();
    // Issue #1325: 背景転送の完了通知（`handleEventsForBackgroundURLSession`）は起動と同じターンで届く。
    #[cfg(all(target_os = "ios", feature = "background-upload"))]
    fandhe_cap_background_upload::register();
    #[cfg(all(target_os = "ios", feature = "background-download"))]
    fandhe_cap_background_download::register();
    // Issue #606: 共有の受け取り（サービスメニュー）の provider。
    #[cfg(all(target_os = "macos", feature = "share-receive"))]
    fandhe_cap_share_receive::register();
    // Issue #560・#561: OS の検索から開かれたときの受信。接頭辞は画面の effect が照合する
    // `fandhe_core::screens::spotlight_index::URL_PREFIX` と同じ値を渡す。
    #[cfg(all(
        any(target_os = "ios", target_os = "macos"),
        feature = "spotlight-index"
    ))]
    fandhe_cap_spotlight_index::register(fandhe_core::screens::spotlight_index::URL_PREFIX);
    // Issue #416・#417: ファイルを開く（macOS は odoc の Apple Event、iOS は scene の通知）。
    #[cfg(all(
        any(target_os = "ios", target_os = "macos"),
        feature = "file-association"
    ))]
    fandhe_cap_file_association::register();
}
