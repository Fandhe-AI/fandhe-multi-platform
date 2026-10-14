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
//! feature `device` を消せば元に戻せる構成にしている。切り離し手順は README.md
//! 「Phase 8（端末機能）の切り離し手順」を参照。
//!
//! `#745`（StateStore の apply API + 完了キュー）は本クレートと並列で開発中のため、
//! worker スレッド → mpsc → apply の配線はここでは行わない。各トレイトは
//! `Send + Sync` なオブジェクトセーフの同期 API とし、非同期化・スレッド化は
//! #745 の API が確定してから #747〜#749 側で行う。
#![forbid(unsafe_code)]

/// Phase 11（端末機能の拡充ループ、親 #1132、Issue #1308）: アクセシビリティ設定（視差軽減・コントラスト）。
pub mod accessibility_prefs;
/// Phase 11（端末機能の拡充ループ、親 #1141、Issue #1360）: アプリアイコンのバッジ・タスクバー進捗。
pub mod app_badge;
/// Phase 11（端末機能の拡充ループ、親 #1136、Issue #1330）: 音声デバイスの一覧（入力・出力の名前）。
pub mod audio_devices;
/// Phase 11（端末機能の拡充ループ、親 #1042、Issue #1043）: バックグラウンド処理。
pub mod background_task;
/// Phase 11（端末機能の拡充ループ、親 #905、Issue #906）: 生体認証。
pub mod biometric_auth;
/// Phase 11（端末機能の拡充ループ）: Bluetooth LE の周辺機器スキャン。
pub mod bluetooth_le;
/// Phase 11（端末機能の拡充ループ、親 #1118、Issue #1224）: 予定を 1 件追加する。
pub mod calendar;
pub mod camera;
/// Phase 11（端末機能の拡充ループ、Issue #859）: クリップボード読み書き。
pub mod clipboard;
/// Phase 11（端末機能の拡充ループ、親 #1154、Issue #1404）: 方位（コンパス）。
pub mod compass;
/// Phase 11（端末機能の拡充ループ、親 #1142、Issue #1384）: メール・SMS の作成画面を開く。
pub mod compose_mail_sms;
/// Phase 11（端末機能の拡充ループ、親 #1117、Issue #1220）: 連絡先を 1 件選ぶ。
pub mod contact_picker;
/// Phase 11（端末機能の拡充ループ、親 #1131、Issue #1304）: 端末・OS 情報（機種・OS 版・空き容量）。
pub mod device_info;
/// Phase 11（端末機能の拡充ループ、親 #1013、Issue #1014）: ドラッグ & ドロップ受け取り。
pub mod drag_drop;
/// Phase 11（端末機能の拡充ループ、Issue #872）: ファイル選択ダイアログ。
pub mod file_picker;
/// Phase 11 の拡充ループ（親 #1143、Issue #1410）: 作業フォルダ直下のファイルの作成・更新・削除の監視。
pub mod file_watch;
/// Phase 11（端末機能の拡充ループ、親 #1133、Issue #1312）: 全画面表示（システムバー非表示）。
pub mod fullscreen_immersive;
/// Phase 11（端末機能の拡充ループ、親 #932、Issue #933）: 継続 push 型のゲームコントローラ入力。
pub mod gamepad;
/// Phase 11（端末機能の拡充ループ）: location に乗せた Rust 側のジオフェンス判定。
pub mod geofence;
/// Phase 11（端末機能の拡充ループ）: 触覚フィードバック（軽い振動 1 回）。
pub mod haptics;
/// Phase 11（端末機能の拡充ループ）: ホーム画面ウィジェットの 1 行テキスト。
pub mod home_widget;
pub mod http;
/// Phase 11（端末機能の拡充ループ、親 #1120、Issue #1250）: 画像内の文字認識（OCR）。
pub mod image_recognition;
/// Phase 11（端末機能の拡充ループ、親 #1145、Issue #1376）: OS 搭載フォントのファミリー名の一覧。
pub mod local_fonts;
/// Phase 11（端末機能の拡充ループ、親 #887、Issue #888）: 継続 push 型の位置情報取得。
pub mod location;
/// Phase 11（端末機能の拡充ループ、親 #953、Issue #954）: マイク録音。
pub mod mic_record;
/// Phase 11（端末機能の拡充ループ、親 #1137、Issue #1398）: MIDI 入力（ノートオン / オフ）。
pub mod midi;
pub mod motion;
/// Phase 11（端末機能の拡充ループ、親 #1134、Issue #1341）: 複数ディスプレイ・外部ディスプレイの一覧。
pub mod multi_display;
/// Phase 11（端末機能の拡充ループ、親 #1130、Issue #1282）: ネットワーク接続状態（有無・種別・従量制）。
pub mod network_status;
/// Phase 11（端末機能の拡充ループ）: 単語分割。
pub mod nlp;
/// Phase 11（端末機能の拡充ループ、親 #1003、Issue #1004）: ローカル通知。
pub mod notification;
/// Phase 11（端末機能の拡充ループ、親 #1119、Issue #1247）: 歩数（累積歩数を 1 回取得）。
pub mod pedometer;
/// Phase 11（端末機能の拡充ループ、親 #1121、Issue #1254）: 画像 1 枚の写真ライブラリ保存。
pub mod photo_library;
/// Phase 11（端末機能の拡充ループ）: 写真 1 枚の選択。
pub mod photo_picker;
/// Phase 11（端末機能の拡充ループ、親 #1128、Issue #1274）: 電源状態（電池・省電力・熱）。
pub mod power_state;
/// Phase 11（端末機能の拡充ループ、親 #1135、Issue #1335）: 画面の録画・キャプチャ。
pub mod screen_capture;
/// Phase 11（端末機能の拡充ループ、親 #1129、Issue #1278）: 画面スリープの抑止。
pub mod screen_wake_lock;
/// Phase 11（端末機能の拡充ループ、親 #921、Issue #922）: 資格情報の安全な保存。
pub mod secure_credential;
/// Phase 11（端末機能の拡充ループ、親 #896、Issue #897）: 共有シート。
pub mod share;
/// Phase 11（端末機能の拡充ループ、親 #1023、Issue #1024）: 音声認識。
pub mod speech_recognition;
pub mod storage;
/// Phase 11（端末機能の拡充ループ、親 #1038、Issue #1039）: システムトレイ
/// （メニューバー / 通知領域アイコン）。
pub mod system_tray;
/// Phase 11（端末機能の拡充ループ、親 #911、Issue #912）: 文字サイズ設定の追従。
pub mod text_scale;
/// Phase 11（端末機能の拡充ループ、Issue #865）: 任意テキストの読み上げ（TTS）。
pub mod tts;
/// Phase 11（端末機能の拡充ループ、親 #1116、Issue #1215）: カスタム URL scheme の受信。
pub mod url_scheme;

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
    /// `camera::Camera::capture` が返す画像サイズが許容範囲外（0 または上限超過）。
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
    /// 端末機能へ渡した入力が契約違反（`tts::Tts::speak` に渡したテキストの NUL
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
/// `rgba.len()` は常に `width * height * 4` になる契約（`camera` モジュールのテスト参照）。
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
