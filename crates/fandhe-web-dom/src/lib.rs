//! Web ターゲット向け DOM 出力レンダラ（TASK-4.4-S2 / REQ-4 / M3）。
//!
//! PoC-5（`docs/spec/03-poc/web-wasm/README.md`）で、独自レンダラ方式
//! （wasm + WebGL2 canvas、`fandhe-egui-proxy` の `web.rs` が検証専用プロキシとして
//! 先行実装）ではブラウザ標準のアクセシビリティツリーに要素が一切公開されないこと
//! （AccessKit に Web バックエンドが存在しない構造的制約）が実証済みである。
//! 本クレートは、アクセシビリティ・SEO が重要な画面向けに `fandhe-core` の
//! `Screen<S>` / `Node<S>` を標準 HTML 要素の DOM ツリーとして出力する
//! **最小レンダラ**を提供し、独自レンダラ方式とのハイブリッド構成の一方を担う。
//!
//! # モジュール構成
//!
//! - [`dom_tree`]: ターゲット非依存の中間表現 `DomNode`。`Screen<S>` + `&S` からの変換を担う
//! - [`html`]: `DomNode` → HTML 文字列のシリアライズ（SEO / SSR 用途・native テスト用）
//! - [`ime`]: ターゲット非依存。DOM composition イベント（`compositionstart` /
//!   `compositionupdate` / `compositionend`）を `fandhe-core` の `ImeEvent` へ変換する
//!   純粋な変換レイヤ（TASK-8.5-S21 / REQ-8、Issue #202）
//! - [`mount`]: `wasm32` 専用。`DomNode` を `web-sys` 経由で実 DOM へ構築し、
//!   `Button` / `TextField` のイベントを状態 `S` へ配線する（[`ime`] の変換関数経由で
//!   composition イベントも配線する）
//! - [`clipboard`]: `wasm32` 専用。「クリップボード」画面の Web 経路（Issue #859）
//! - [`tts`]: `wasm32` 専用。「読み上げ」画面の Web 経路（Issue #867）
//! - [`file_picker`]: `wasm32` 専用。「ファイル選択」画面の Web 経路（Issue #875）。
//!   「TSV 読み込み」画面（Issue #981）とも `<input type="file">` 配線を共有する
//! - [`location`]: 「位置情報」画面の Web 経路（Issue #891）。純関数（コード → 失敗種別の
//!   対応・再試行検知）は native からもテストするため、他 4 モジュールと異なり
//!   クレート全体を `wasm32` へ gate しない（内側の `web` サブモジュールのみ gate する）
//! - [`share`]: 「共有」画面の Web 経路（Issue #900）。[`location`] と同じ理由で純関数
//!   （例外の `name` → 結果種別の対応）のみ `wasm32` gate の外に置く
//! - [`bluetooth_le`]: 「BLE スキャン」画面の Web 経路（Issue #1063）。[`location`] と同じ理由で
//!   純関数（例外の `name` → 失敗種別・選択 1 台 → 行）のみ `wasm32` gate の外に置く
//! - [`contact_picker`]: 「連絡先を選ぶ」画面の Web 経路（Issue #1223）。[`location`] と同じ理由で
//!   純関数（例外の `name` → 結果・選択結果 → 結果）のみ `wasm32` gate の外に置く
//! - [`biometric_auth`]: 「生体認証」画面の Web 経路（Issue #909）。[`location`] と同じ
//!   理由で純関数（UV フラグ判定・例外の `name` → 結果種別の対応・hex 符号化）のみ
//!   `wasm32` gate の外に置く
//! - [`text_scale`]: 「文字サイズ追従」画面の Web 経路（Issue #915）。[`location`] と同じ
//!   理由で純関数（`getComputedStyle` の値 → 倍率の変換）のみ `wasm32` gate の外に置く
//! - [`power_state`]: 「電源状態」画面の Web 経路（Issue #1277）。[`location`] と同じ理由で
//!   純関数（Battery API の生値 → 表示・例外の `name` → 失敗種別）のみ `wasm32` gate の外に置く
//! - [`device_info`]: 「端末・OS 情報」画面の Web 経路（Issue #1307）。[`location`] と同じ理由で
//!   純関数（ブラウザの生値 → OS 名・版・機種・メモリ）のみ `wasm32` gate の外に置く
//! - [`audio_devices`]: 「音声デバイス」画面の Web 経路（Issue #1333）。[`location`] と同じ理由で
//!   純関数（`enumerateDevices` の kind / label → 入出力の一覧）のみ `wasm32` gate の外に置く
//! - [`file_watch`]: 「フォルダの変更監視」画面の Web 経路（Issue #1414）。`FileSystemObserver` + OPFS。
//!   [`location`] と同じ理由で純関数のみ `wasm32` gate の外に置く
//! - [`midi`]: 「MIDI 入力」画面の Web 経路（Issue #1401）。Web MIDI API。
//!   [`location`] と同じ理由で純関数のみ `wasm32` gate の外に置く
//! - [`local_fonts`]: 「OS 搭載フォント」画面の Web 経路（Issue #1379）。[`location`] と同じ理由で
//!   純関数（`FontData` の family → 一覧・例外の `name` → 失敗種別）のみ `wasm32` gate の外に置く
//! - [`multi_display`]: 「複数ディスプレイ」画面の Web 経路（Issue #1344）。[`location`] と同じ理由で
//!   純関数（`getScreenDetails` の生値 → 画面一覧・例外の `name` → 失敗種別）のみ `wasm32` gate の外に置く
//! - [`mic_record`]: 「マイク録音」画面の Web 経路（Issue #958）。[`location`] と同じ理由で
//!   純関数（例外の `name` → 結果種別の対応・録音結果の検証・mime の決定）のみ `wasm32`
//!   gate の外に置く
//! - [`screen_capture`]: 「画面の録画・キャプチャ」画面の Web 経路（Issue #1338）。[`location`] と同じ理由で
//!   純関数（例外の `name` → 結果種別・`Blob.size` 等 → 録画結果の検証）のみ `wasm32` gate の外に置く
//! - [`http_fetch`]: 「HTTP 取得（GET 限定）」画面の Web 経路（Issue #985）。[`location`]
//!   と同じ理由で純関数（2xx 判定・本文上限判定）のみ `wasm32` gate の外に置く
//! - [`storage_note`]: 「メモ保存」画面の Web 経路（Issue #987）。[`location`] と同じ理由で
//!   純関数（キー変換・例外の `name` → エラー種別の対応）のみ `wasm32` gate の外に置く
//! - [`camera_capture`]: 「カメラ撮影」画面の Web 経路（Issue #991）。[`location`] と同じ
//!   理由で純関数（例外の `name` → 結果種別の対応・サムネイル化の境界検証）のみ `wasm32`
//!   gate の外に置く
//! - [`notification`]: 「通知」画面の Web 経路（Issue #1007）。[`location`] と同じ理由で
//!   純関数（権限の解決値・例外の `name` → 結果種別の対応）のみ `wasm32` gate の外に置く
//! - [`drag_drop`]: 「ドラッグ & ドロップ受け取り」画面の Web 経路（親 #1013、Issue #1017）。
//!   [`location`] と同じ理由で純関数（事前判定・読了後の再確認・超過分の集約）のみ
//!   `wasm32` gate の外に置く
//! - [`speech_recognition`]: 「音声認識」画面の Web 経路（親 #1023、Issue #1027）。
//!   [`location`] と同じ理由で純関数（エラーコード → 結果の対応・認識文字列の検証）のみ
//!   `wasm32` gate の外に置く
//! - [`background_task`]: 「バックグラウンド処理」画面の Web 経路（親 #1042、
//!   Issue #1047）。[`location`] と同じ理由で純関数（予約状態の判定・例外の `name` →
//!   結果種別の対応・実行記録の読み取り結果の写像）のみ `wasm32` gate の外に置く
//! - [`url_scheme`]: 「カスタム URL scheme」画面の Web 経路（親 #1116、Issue #1219）。
//!   [`location`] と同じ理由で純関数（登録結果の分類・ハンドラ URL）のみ `wasm32` gate の外に置く
//! - [`network_status`]: 「ネットワーク接続状態」画面の Web 経路（親 #1130、Issue #1285）。
//!   純関数（`onLine` / `connection.type` → 表示値）のみ `wasm32` gate の外に置く
//! - [`media_transcode`]: 「動画・音声の変換」画面の Web 経路（親 #91、Issue #315）。WebCodecs を Reflect で呼ぶ。
//! - [`media_playback`]: 「音声の再生」画面の Web 経路（親 #86、Issue #271）。[`location`] と同じ理由で
//!   純関数（`play()` 拒否の写像・位置の検証・期限・中断判定）のみ `wasm32` gate の外に置く
//! - [`accessibility_prefs`]: 「アクセシビリティ設定の追従」画面の Web 経路（親 #1132、Issue #1311）。
//!   純関数（`matchMedia` の結果 → 設定値・読み取り期限・dt）のみ `wasm32` gate の外に置く
//! - [`screen_wake_lock`]: 「画面スリープの抑止」画面の Web 経路（親 #1129、Issue #1281）。
//!   純関数（取得結果の写像・取り直し判定・世代照合）のみ `wasm32` gate の外に置く
//! - [`compass`]: 「方位（コンパス）」画面の Web 経路（親 #1154、Issue #1407）。
//!   純関数（イベント値 → 方位・許可結果の写像・tick 判定・世代照合）のみ `wasm32` gate の外に置く
//! - [`ar`]: 「AR 表示」画面の Web 経路（親 #90、Issue #309）。WebXR `immersive-ar` + `dom-overlay`。
//!   純関数（姿勢 → 状態・失敗の写像・tick 判定）のみ `wasm32` gate の外に置く
//! - [`fullscreen_immersive`]: 「全画面表示」画面の Web 経路（親 #1133、Issue #1315）。
//!   純関数（発送判定・到達判定）のみ `wasm32` gate の外に置く
//!
//! # 他 sub-issue との分界（TASK-4.4）
//!
//! - 画面単位の出力方式選択導線（本レンダラと独自レンダラの切り替え API）は
//!   S3（#81）のスコープ
//! - アクセシビリティ・SEO 要件の実ブラウザ確認テストは S4（#82）のスコープ
//! - 本 sub-issue（S2）は DOM ツリーへの変換・HTML シリアライズ・実 DOM マウントの
//!   最小実装（全再構築のみ、差分更新なし）にとどめる
//!
//! `#![forbid(unsafe_code)]`: `mount` モジュールの `wasm-bindgen` 連携も safe API
//! （`Closure` 等）のみで構成し、`unsafe` を要求しない（coding-rust.md 準拠）。
#![forbid(unsafe_code)]

pub mod dom_tree;
pub mod html;
pub mod ime;
mod png_data_url;

#[cfg(target_arch = "wasm32")]
pub mod mount;

/// Phase 11 型サンプル「クリップボード」の Web 経路（Issue #859、`wasm32` 専用）。
#[cfg(target_arch = "wasm32")]
pub mod clipboard;

/// Phase 11 型サンプル「読み上げ」の Web 経路（Issue #867、`wasm32` 専用）。
#[cfg(target_arch = "wasm32")]
pub mod tts;

/// Phase 11 型サンプル「ファイル選択」の Web 経路（Issue #875、`wasm32` 専用）。
#[cfg(target_arch = "wasm32")]
pub mod file_picker;

/// 型サンプル「位置情報」の Web 経路（Issue #891）。純関数は native からもテストできるよう
/// クレート単位の `wasm32` gate を外す（モジュール doc 参照）。
pub mod location;

/// 型サンプル「共有」の Web 経路（Issue #900）。[`location`] と同じ理由で純関数は
/// native からもテストできるようクレート単位の `wasm32` gate を外す。
pub mod share;

/// 型サンプル「BLE スキャン」の Web 経路（Issue #1063）。[`location`] と同じ理由で純関数は
/// native からもテストできるようクレート単位の `wasm32` gate を外す。
pub mod bluetooth_le;

/// 型サンプル「連絡先を選ぶ」の Web 経路（Issue #1223）。[`location`] と同じ理由で純関数は
/// native からもテストできるようクレート単位の `wasm32` gate を外す。
pub mod contact_picker;

/// 型サンプル「生体認証」の Web 経路（Issue #909）。[`location`] と同じ理由で純関数は
/// native からもテストできるようクレート単位の `wasm32` gate を外す。
pub mod biometric_auth;

/// 型サンプル「文字サイズ追従」の Web 経路（Issue #915）。[`location`] と同じ理由で
/// 純関数（`getComputedStyle` の値 → 倍率の変換）は native からもテストできるよう
/// クレート単位の `wasm32` gate を外す。
pub mod text_scale;

/// 型サンプル「ゲームコントローラ」の Web 経路（Issue #936）。[`location`] と同じ理由で
/// 純関数（生値 → スナップショット・次フレーム表示内容の変換）は native からも
/// テストできるようクレート単位の `wasm32` gate を外す。
pub mod gamepad;

/// 型サンプル「電源状態」の Web 経路（Issue #1277）。[`location`] と同じ理由で
/// 純関数（Battery API の生値 → 表示・例外の `name` → 失敗種別）は native からも
/// テストできるようクレート単位の `wasm32` gate を外す。
pub mod power_state;

/// 型サンプル「アクセシビリティ設定の追従」の Web 経路（Issue #1311）。[`location`] と同じ理由で
/// 純関数（`matchMedia` の結果 → 設定値・読み取り期限・dt）は native からもテストできるよう
/// クレート単位の `wasm32` gate を外す。
pub mod accessibility_prefs;
/// 型サンプル「音声デバイス」の Web 経路（Issue #1333）。[`location`] と同じ理由で
/// 純関数（`kind` / `label` → 入出力の一覧）は native からもテストできるよう
/// クレート単位の `wasm32` gate を外す。
pub mod audio_devices;
/// 型サンプル「端末・OS 情報」の Web 経路（Issue #1307）。[`location`] と同じ理由で
/// 純関数（ブラウザの生値 → 表示用の読み取り結果）は native からもテストできるよう
/// クレート単位の `wasm32` gate を外す。
pub mod device_info;
/// 型サンプル「フォルダの変更監視」の Web 経路（Issue #1414）。[`location`] と同じ理由で
/// 純関数（一覧の生値 → 走査結果・`type` → 合図・差分 → 記録）は native からもテストできるよう
/// クレート単位の `wasm32` gate を外す。
pub mod file_watch;
/// 型サンプル「OS 搭載フォント」の Web 経路（Issue #1379）。[`location`] と同じ理由で
/// 純関数（`family` → 一覧・例外の `name` → 失敗種別）は native からもテストできるよう
/// クレート単位の `wasm32` gate を外す。
pub mod local_fonts;
/// 型サンプル「MIDI 入力」の Web 経路（Issue #1401）。[`location`] と同じ理由で
/// 純関数（例外の `name` → 失敗種別・件数の集計・バイト列 → ノート行）は native からもテストできるよう
/// クレート単位の `wasm32` gate を外す。
pub mod midi;
/// 型サンプル「複数ディスプレイ」の Web 経路（Issue #1344）。[`location`] と同じ理由で
/// 純関数（生値 → 画面一覧・例外の `name` → 失敗種別）は native からもテストできるよう
/// クレート単位の `wasm32` gate を外す。
pub mod multi_display;

/// 型サンプル「マイク録音」の Web 経路（Issue #958）。[`location`] と同じ理由で
/// 純関数（例外の `name` → 結果種別の対応・録音結果の検証・mime の決定）は native
/// からもテストできるようクレート単位の `wasm32` gate を外す。
pub mod mic_record;

/// 型サンプル「音声の再生」の Web 経路（Issue #271）。純関数は native からもテストする。
pub mod media_playback;

/// 型サンプル「動画・音声の変換」の Web 経路（Issue #315）。純関数は native からもテストする。
pub mod media_transcode;

/// 型サンプル「画面の録画・キャプチャ」の Web 経路（Issue #1338）。[`location`] と同じ理由で
/// 純関数（例外の `name` → 結果種別・録画結果の検証）は native からもテストできるよう
/// クレート単位の `wasm32` gate を外す。
pub mod screen_capture;

/// 型サンプル「HTTP 取得（GET 限定）」の Web 経路（Issue #985）。[`location`] と同じ理由で
/// 純関数（2xx 判定・本文上限判定）は native からもテストできるようクレート単位の
/// `wasm32` gate を外す。
pub mod http_fetch;

/// 型サンプル「メモ保存」の Web 経路（Issue #987）。[`location`] と同じ理由で
/// 純関数（キー変換・例外の `name` → エラー種別の対応）は native からもテストできるよう
/// クレート単位の `wasm32` gate を外す。
pub mod storage_note;

/// 型サンプル「傾きで玉が転がる」の Web 経路（Issue #989）。[`location`] と同じ理由で
/// 純関数（`DeviceOrientationEvent` → 重力ベクトルの変換・スキップ判定）は native
/// からもテストできるようクレート単位の `wasm32` gate を外す。
pub mod tilt;

/// 型サンプル「ドラッグ & ドロップ受け取り」の Web 経路（親 #1013、Issue #1017）。
/// [`location`] と同じ理由で純関数は native からもテストできるようクレート単位の
/// `wasm32` gate を外す。
pub mod drag_drop;

/// 型サンプル「カメラ撮影」の Web 経路（Issue #991）。[`location`] と同じ理由で
/// 純関数（例外の `name` → 結果種別の対応・サムネイル化の境界検証）は native
/// からもテストできるようクレート単位の `wasm32` gate を外す。
pub mod camera_capture;

/// 型サンプル「通知」の Web 経路（Issue #1007）。[`location`] と同じ理由で
/// 純関数（権限の解決値・例外の `name` → 結果種別の対応）は native からもテスト
/// できるようクレート単位の `wasm32` gate を外す。
pub mod notification;

/// 型サンプル「音声認識」の Web 経路（親 #1023、Issue #1027）。[`location`] と同じ
/// 理由で純関数（エラーコード → 結果の対応・認識文字列の検証）は native からもテスト
/// できるようクレート単位の `wasm32` gate を外す。
pub mod speech_recognition;

/// 型サンプル「バックグラウンド処理」の Web 経路（親 #1042、Issue #1047）。
/// [`location`] と同じ理由で純関数（予約状態の判定・例外の `name` → 結果種別の
/// 対応・実行記録の読み取り結果の写像）は native からもテストできるようクレート
/// 単位の `wasm32` gate を外す。
pub mod background_task;

/// `wasm32` 専用。TASK-4.4-S4（Issue #82）のアクセシビリティ・SEO 要件の実ブラウザ確認
/// （`scripts/web/verify-dom-a11y.sh`）専用のエントリポイント。検証専用であり、
/// 画面単位の出力方式選択導線（S3、`fandhe-core::ui::RenderMode`）の正式なエントリポイントではない
/// （モジュールコメント参照）。
#[cfg(target_arch = "wasm32")]
pub mod demo;

pub use dom_tree::{build_dom_tree, render, DomNode, EventHook, Rendered};
pub use html::to_html;
pub use ime::{composition_phase_from_event_type, ime_event_from_composition, CompositionPhase};

#[cfg(target_arch = "wasm32")]
pub use mount::{mount, mount_with_effect, Applier, WebEffect};

/// 型サンプル「バッジの設定」の Web 経路（Issue #1363）。[`haptics`] と同じ理由で gate を外す。
pub mod app_badge;
/// 型サンプル「AR 表示」の Web 経路（Issue #309）。[`haptics`] と同じ理由で gate を外す。
pub mod ar;
/// 型サンプル「方位（コンパス）」の Web 経路（Issue #1407）。[`haptics`] と同じ理由で gate を外す。
pub mod compass;
/// 型サンプル「メール・SMS の作成」の Web 経路（Issue #1387）。[`haptics`] と同じ理由で gate を外す。
pub mod compose_mail_sms;
/// 型サンプル「全画面表示」の Web 経路（Issue #1315）。[`haptics`] と同じ理由で gate を外す。
pub mod fullscreen_immersive;
/// 型サンプル「触覚フィードバック」の Web 経路（Issue #1091）。[`location`] と同じ理由で
/// 純関数は native からもテストできるようクレート単位の `wasm32` gate を外す。
pub mod haptics;
/// 型サンプル「ネットワーク接続状態」の Web 経路（Issue #1285）。[`haptics`] と同じ理由で gate を外す。
pub mod network_status;
/// 型サンプル「単語分割」の Web 経路（Issue #1094）。[`haptics`] と同じ理由で純関数は native からも
/// テストできるようクレート単位の `wasm32` gate を外す。
pub mod nlp;
/// 型サンプル「印刷」の Web 経路（Issue #329）。[`haptics`] と同じ理由で gate を外す。
pub mod print_page;
/// 型サンプル「画面スリープの抑止」の Web 経路（Issue #1281）。[`haptics`] と同じ理由で gate を外す。
pub mod screen_wake_lock;
/// 型サンプル「カスタム URL scheme」の Web 経路（Issue #1219）。[`haptics`] と同じ理由で gate を外す。
pub mod url_scheme;
