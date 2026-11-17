//! Phase 8（端末機能の型検証、親 #752 / Issue #746）: 端末機能（ローカル保存・HTTP・カメラ・
//! 傾き検知）の抽象トレイトと desktop 向け共通実装を置くクレート。
//!
//! 依存方向は本クレート → `fandhe-core`（現時点では未使用）の一方向のみを許容し、`fandhe-core`
//! から本クレートを参照することはない（coding-rust.md の二層アーキテクチャ原則）。
//! 呼び出し元は `fandhe-egui-proxy` の feature `device`（既定 on）で、画面 ↔ 端末実装の
//! 接続方法（`Box<dyn Storage>` 等をどう渡すか）は #747 以降の設計事項（本クレートは
//! トレイトと同期実装の提供のみに責務を限定する）。
//!
//! Phase 8 が spec 要件化されなかった場合、本クレートと `fandhe-egui-proxy` 側の
//! feature `device` を消せば元に戻せる構成にしている。切り離し手順は
//! docs/architecture/device-feature-removal.md を参照。
//!
//! `#745`（StateStore の apply API + 完了キュー）は本クレートと並列で開発中のため、
//! worker スレッド → mpsc → apply の配線はここでは行わない。各トレイトは
//! `Send + Sync` なオブジェクトセーフの同期 API とし、非同期化・スレッド化は
//! #745 の API が確定してから #747〜#749 側で行う。
#![forbid(unsafe_code)]

/// カメラのライブ映像の縮小の部品（`downscale_packed`・`downscale_yuv420`・`preview_size`）だけ。`fandhe-cap-ar` と
/// `fandhe-web-dom` が共有する純関数で、機能の契約は `fandhe-cap-camera-preview-stream` が持つ（Issue #812 / #1329、
/// consumer-app-structure.md §8）。
pub mod camera_preview_stream;
/// Phase 11（端末機能の拡充ループ、親 #1154、Issue #1404）: 方位（コンパス）。
pub mod compass;
/// Phase 11（端末機能の拡充ループ、親 #1131、Issue #1304）: 端末・OS 情報（機種・OS 版・空き容量）。
pub mod device_info;
/// Phase 11（端末機能の拡充ループ、親 #1013、Issue #1014）: ドラッグ & ドロップ受け取り。
pub mod drag_drop;
/// Phase 11（端末機能の拡充ループ、親 #148、Issue #762）: 画面のスポイト（画面上の 1 点の色を `#rrggbb` で返す）。
pub mod eye_dropper;
/// Phase 11（端末機能の拡充ループ、Issue #872）: ファイル選択ダイアログ。
pub mod file_picker;
pub mod http;
/// Phase 11（端末機能の拡充ループ、親 #86、Issue #267）: 音声 1 本の再生・一時停止・停止。
pub mod media_playback;
/// Issue #1188: MP4 box 読みの共有部品（音声変換は機能 crate `fandhe-cap-media-transcode`、動画変換 `video_transcode` が使う）。
pub mod media_transcode;
/// Issue #1188: base64url の共有部品（パスキー本体は機能 crate `fandhe-cap-passkey-signin`）。
pub mod passkey;
/// Phase 11（端末機能の拡充ループ、親 #1121、Issue #1254）: 画像 1 枚の写真ライブラリ保存。
pub mod photo_library;
/// 写真 1 枚の選択の共有部品（型・検証・寸法の検査・表示用の復号）。契約と OS 実装は機能 crate `fandhe-cap-photo-picker`（Issue #1355）。
pub mod photo_picker;
pub mod storage;
/// Phase 11（端末機能の拡充ループ、親 #115、Issue #535）: 端末内翻訳（固定の原文を 1 回訳す）。
pub mod translation;
/// Phase 11（端末機能の拡充ループ、親 #1116、Issue #1215）: カスタム URL scheme の受信。
pub mod url_scheme;
/// Wi-Fi の電源状態の判定（`radio_off`）だけ。wifi-info と wifi-connect が共有する純関数で、機能の契約は `fandhe-cap-wifi-info`
/// が持つ（Issue #514 / #1329、consumer-app-structure.md §8）。
pub mod wifi_info;

use std::path::PathBuf;

/// 端末機能共通のエラー型。呼び出し元（画面・プラットフォームレイヤ）に実装詳細
/// （ファイルシステムの内部構造・URL の生文字列全体）を漏らさないよう、
/// メッセージには key 名やスキーム名程度に留める（security.md A09 ログ出力の方針）。
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    /// ファイル読み書きが失敗した（`storage` モジュール）。
    #[error("入出力エラー: {0}")]
    Io(#[from] std::io::Error),
    /// `storage::Storage` に渡された key が許可文字集合・パストラバーサル制約に違反する。
    #[error("不正な key: {0}")]
    InvalidKey(String),
    /// `http` モジュールの通信・許可 URL 検証・応答上限超過などの失敗。
    #[error("HTTP エラー: {0}")]
    Http(String),
    /// 撮影画像のサイズが許容範囲外（0 または上限超過）。カメラ撮影は機能 crate `fandhe-cap-camera-capture` が自前の `CameraError` で返し、本バリアントは他機能（地図・PDF 等）の画像で使う。
    #[error("画像サイズが不正または大きすぎます: {0}x{1}")]
    InvalidImageSize(u32, u32),
    /// 現在のプラットフォームでは該当機能を提供できない（例: `FsStorage::desktop_default` の
    /// ホームディレクトリ取得不可）。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// 端末側の権限設定でアクセスが拒否された（Issue #749。desktop の `DummyCamera` は
    /// 常に成功するため到達しないが、Android/iOS の実カメラ実装（#750/#751）が
    /// `Intent`/`UIImagePickerController` の権限拒否コールバックから返す想定の
    /// バリアント）。第 1 引数は機能名（`"camera"` 等、内部識別子のみで端末固有情報は含めない）。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// ユーザーが端末側の選択 UI を能動的にキャンセルした（正常系。#751。
    /// iOS の `UIImagePickerController` のキャンセル、Android の Intent
    /// `RESULT_CANCELED`（#750）が返す想定のバリアント。第 1 引数は機能名
    /// （`"camera"` 等、内部識別子のみ）。
    #[error("操作がキャンセルされました: {0}")]
    Cancelled(&'static str),
    /// 端末機能自体は利用可能だが、今回の呼び出しが一時的な内部要因で失敗した
    /// （PR #791 レビュー指摘: iOS の `fandhe_ios_camera_capture` コード 4 —
    /// presenter 不在・エンコード失敗・二重呼び出し等。`Unsupported`（機種として
    /// 非対応）と混同すると「カメラなし端末」という誤った案内になるため区別する。
    /// 第 1 引数は機能名（`"camera"` 等、内部識別子のみ）で、呼び出し元は
    /// `CaptureFailure::Other` 相当の汎用失敗表示へフォールバックする想定）。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// 端末機能へ渡した入力が契約違反（読み上げ `Tts::speak` に渡したテキストの NUL
    /// 混入等）。第 1 引数は機能名（`"tts text"` 等、内部識別子のみ）。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// 端末側で機能そのものが無効化されている（OS 設定で位置情報サービスが
    /// OFF 等）。`Unsupported`（この機種・OS では非対応）とも `Internal`
    /// （一時的な内部要因）とも意味が異なるため区別する（`location` モジュール
    /// doc の対応表参照、Issue #888）。第 1 引数は機能名（`"location"` 等、
    /// 内部識別子のみ）。
    #[error("端末側で機能が無効化されています: {0}")]
    Disabled(&'static str),
}

/// カメラ実装が返す静止画（RGBA・行優先・パディングなし）。
///
/// `rgba.len()` は常に `width * height * 4` になる契約（`fandhe-cap-camera-capture` の `DummyCamera` のテスト参照）。
#[derive(Debug, Clone)]
pub struct Image {
    /// 画像の幅（ピクセル）。
    pub width: u32,
    /// 画像の高さ（ピクセル）。
    pub height: u32,
    /// RGBA・行優先・パディングなしの生バイト列（長さは `width * height * 4`）。
    pub rgba: Vec<u8>,
}

/// `Storage` 実装がファイルを読み書きする起点ディレクトリ。
///
/// プラットフォームレイヤ（`fandhe-egui-proxy` 等）が構築して `storage::FsStorage::new` へ
/// 渡す契約で、本クレート自身は Android / iOS のサンドボックスパスを知らない
/// （依存方向の一方向性を保つため、プラットフォーム固有 API はここに置かない）。
#[derive(Debug, Clone)]
pub struct DataDir(pub PathBuf);
