//! 機能 crate（`fandhe-cap-*`）が共有する、OS に触れない型・検証・定数・符号化を置くクレート
//! （docs/architecture/consumer-app-structure.md §8。Phase 8 / Issue #746 で端末機能の土台として作り、
//! 機能 crate への移行後は共有部品だけが残る。名前は #1215 で決める）。
//!
//! 呼び出し元は機能 crate と `fandhe-web-dom`。利用者は直接依存せず、共有型は機能 crate が再公開する。
//! 依存方向は本クレート → `fandhe-core`（任意の feature `display-decode` のときだけ）の一方向で、
//! `fandhe-core` から本クレートを参照することはない（coding-rust.md の二層アーキテクチャ原則）。
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

// エラー型は共有しない（consumer-app-structure.md §8）。各モジュールが使うバリアントだけを持つ
// （`file_picker::PickError`・`storage::StorageError`・`http::UrlRejected` など。旧 `DeviceError` は #1194 で撤去）。

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
