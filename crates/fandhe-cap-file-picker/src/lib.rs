//! ファイル選択（file-picker）と TSV 読み込み（tsv-parse）の機能 crate（REQ-21 / TASK-21.4、Issue #1354、手順は
//! `docs/operations/capability-crate-migration.md`）。OS 標準の「開く」ダイアログで 1 ファイルを選ばせ、内容を読み切って返す。
//! tsv-parse は同じ選択経路（`driver` の `PickHooks`・`bridge`）を使って TSV を読み込む画面の effect（`tsv_import_effect`）で、
//! 2 つの id が選択経路を共有するため 1 つの crate にまとめた（宣言の `id` は file-picker。対応表の `impl` は両方ともこの crate を指す）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`・`tsv_import`）・OS 実装（macOS・Windows は `desktop`、
//! iOS は `ios`、Android は `android`）を 1 つに集める。画面の状態型を参照してよいのは `driver`・`tsv_import` だけ。
//! 画面の状態型に依存しない入口は、同期の [`platform_file_picker`]（全ターゲットで公開。macOS・Windows は
//! ダイアログを開いて結果を返し、iOS・Android は `Unsupported` を返す）と、iOS・Android 向けの非同期の [`begin_file_pick`]（開始）・
//! [`poll_file_pick`]（結果を 1 度だけ取得）。画面 effect と同じ slot を共有し、片方が進行中・未取得なら他方は `Internal`（busy）で断る。他の機能（写真選択・ドラッグ & ドロップ・ファイル関連付け・フォルダ監視）も
//! 使う部品（`PickedFile`・読み取り・受け取りの検証・Windows のダイアログ起動）は `fandhe-device::file_picker` に残り、この crate は
//! それを再公開して使う（`docs/operations/capability-crate-migration.md` §7）。
//!
//! 呼び出し元は最終成果物（`fandhe-gallery`）の画面レジストリ（`file_pick_effect`・`tsv_import_effect`）と、Activity 再生成時の
//! `reset_pending_tracking`（Android）。Web の実体は `fandhe-web-dom` に残る。
//!
//! 守る範囲: 1 プロセス内での要求と結果の順序・嘘の成功の禁止（キャンセルと失敗を区別し、古い世代の結果は捨てる。ダイアログを使えない
//! 環境でダミー実装へ倒さない）・選んだファイルのパスと中身をログに出さないこと・サイズ上限（`MAX_FILE_BYTES`）。守らない範囲:
//! OS がコールバックしない（シーン破棄等）ときの固着（固定タイムアウトは掛けない）、Activity 再生成をまたいだ保留中の要求の復元
//! （`reset_pending_tracking` を呼ぶ構成では破棄し、呼ばない構成では static の要求 id で新しい Activity が結果を 1 度だけ拾う）。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(target_os = "android")]
mod android;
mod contract;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod desktop;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
mod tsv_import;

pub use contract::*;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use desktop::RfdFilePicker;
pub use driver::*;
pub use error::FilePickerError;
pub use tsv_import::tsv_import_effect;
