//! Phase 11 の拡充ループ（親 #953）型サンプル「マイク録音」の Web 経路（Issue #958）。
//!
//! `fandhe_core::screens::mic_record`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、`navigator.mediaDevices.
//! getUserMedia` + `MediaRecorder` で録音して結果を書き戻す。
//!
//! Web は `fandhe-device` の `MicRecorder` トレイトを通らない（`location.rs`/`share.rs` と
//! 同じ理由。`fandhe-egui-proxy` の feature `device` は wasm32 に依存を持ち込まない設計、
//! `fandhe_device::mic_record` モジュール doc の「対応不可プラットフォーム」節参照）。
//! そのため本モジュールが DOM レンダラ側の対応実装にあたる。ただし録音結果の不変条件
//! （0 バイト禁止・`duration_ms == 0` 禁止・[`fandhe_device::mic_record::MAX_RECORDING_BYTES`]
//! 超過禁止）だけは [`fandhe_device::mic_record::build_recording`] を共有する
//! （検証ロジックの SSOT を 1 箇所に保つ、`location.rs` の `is_valid_fix` と同じ方針）。
//!
//! # 期限は `MAX_RECORD_MS` の 1 定数で揃える
//!
//! Rust 側は [`fandhe_core::screens::mic_record::tick`] が [`fandhe_core::screens::
//! mic_record::MAX_RECORD_MS`] で `StopRequested` へ進める。OS 側は `setTimeout` を
//! 同じ定数で張り、`MediaRecorder.stop()` を呼ぶ。両者とも `state() ==
//! RecordingState::Recording` を確認してから止めるため、二重停止は無害
//! （`InvalidStateError` を作らない、`device-bridge-contracts.md` 「期限・キャンセル」節）。
//!
//! # 経過秒数の反映に `requestAnimationFrame` ではなく `setInterval` を使う理由
//!
//! `gamepad.rs` の毎フレームポーリングとは異なり、本画面が必要とするのは 1 秒粒度の
//! 表示のみ。タブが非表示（他タブを見ている間）でも `rAF` は間引かれて停止し得るが、
//! `setTimeout`/`setInterval` はその間も（間引かれつつ）動き続けるため、非表示中も
//! [`MAX_RECORD_MS`](fandhe_core::screens::mic_record::MAX_RECORD_MS) 到達で自動停止する
//! 経路（`tick` → `StopRequested`）が機能する。経過ミリ秒は `started_at_ms` からの
//! 実測差分（`Date::now()`）で毎回算出し、コールバックの間引きで `tick` の呼び出し間隔が
//! 伸びても値がずれない（累積加算ではなく実測差分、`Performance` feature は増やさない）。
//!
//! # 純関数を `wasm32` gate の外に置く理由
//!
//! [`outcome_from_error_name`]・[`summary_from_bytes`]・[`resolve_mime`] はブラウザ API を
//! 呼ばない変換・検証ロジックのみを持つ。`location.rs`/`share.rs` と同じ理由で native の
//! `cargo test` から固定できるよう、ブラウザ API を直接使う部分（[`mod@web`]）だけを
//! `wasm32` へ gate する。
//!
//! # 秘密値・ログ（security.md A09）
//!
//! 録音バイト列は画面状態に保持しない（[`fandhe_core::screens::mic_record::
//! RecordSummary`] はサイズ・mime・長さのみ）。例外オブジェクトの `message`・デバイス名・
//! バイト列は `console` へ出力・保存・送信しない（`name` のみを見て固定文言へ畳む）。
//!
//! # 既知の天井（対象外、実装計画「対象外」節）
//!
//! `demo.rs` の各エントリポイントは `current_screen(&S::default())` で初期状態の
//! `Screen` を固定するため（[`mount_with_effect`](super::mount::mount_with_effect) が
//! 状態から `Screen` を再構築しない設計、`biometric_check` と同型の既存の天井）、本画面も
//! 「停止」ボタンは Web の検証エントリポイントでは表示されない。v1 は
//! [`fandhe_core::screens::mic_record::MAX_RECORD_MS`] 到達による自動停止で正常系を
//! 観測する。`take_stop_request` 経路（`tick` 上限からの `StopRequested`）自体は
//! [`mod@web`] が処理するため、状態機械としては停止操作に対応している。

use fandhe_core::screens::mic_record::RecordSummary;
use fandhe_device::mic_record::build_recording;

/// 非対応ブラウザ・secure context 外・`MediaRecorder` 未定義時の案内。
/// native ビルドでは `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る
/// （`location.rs::failure_kind_from_code` と同じ理由）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const UNAVAILABLE: &str = "このブラウザではマイク録音を使えません";
/// `NotAllowedError`/`PermissionDeniedError`（マイクの使用を拒否された）ときの案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const NOT_ALLOWED: &str = "マイクの使用が許可されていません";
/// `NotFoundError`/`OverconstrainedError`（マイクが見つからない）ときの案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const NOT_FOUND: &str = "マイクが見つかりません";
/// 種別を特定できない失敗・取得不能なときの汎用案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const RECORD_FAILED: &str = "録音に失敗しました";
/// `pagehide`（タブを離れる・bfcache への退避）で録音中に破棄したときの案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const INTERRUPTED: &str = "録音が中断されました";

/// `RecordOutcome`（`fandhe_core::screens::mic_record`）を再エクスポートせず、
/// 呼び出し元（[`mod@web`]・テスト）が直接 `fandhe_core` から import する
/// （本モジュールは変換ロジックのみを持つ）。
use fandhe_core::screens::mic_record::RecordOutcome;

/// 例外の `name`（`DOMException` の種別）→ [`RecordOutcome`] への変換純関数
/// （ブラウザ API を呼ばないため native からもテストできる、モジュール doc 参照）。
///
/// `cancel_requested`（権限確認中に利用者が「停止」を押した）は例外の種類に関わらず
/// [`RecordOutcome::Cancelled`] を優先する（`fandhe_device::mic_record` の macOS/Android
/// 実装と同じ「利用者操作によるキャンセルは失敗ではない」方針、`biometric_check::
/// AuthOutcome` と同型）。`message` は受け取らない・転送しない（security.md A09）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_error_name(
    name: Option<&str>,
    cancel_requested: bool,
) -> Result<RecordOutcome, String> {
    if cancel_requested {
        return Ok(RecordOutcome::Cancelled);
    }
    match name {
        Some("NotAllowedError") | Some("PermissionDeniedError") => Err(NOT_ALLOWED.to_string()),
        Some("NotFoundError") | Some("OverconstrainedError") => Err(NOT_FOUND.to_string()),
        Some("NotSupportedError") | Some("SecurityError") => Err(UNAVAILABLE.to_string()),
        _ => Err(RECORD_FAILED.to_string()),
    }
}

/// 読み取ったバイト列・mime・録音時間から [`RecordSummary`] を組み立てる純関数。
/// 不変条件の検証は [`build_recording`] の SSOT に一本化し、`DeviceError` の内部識別子
/// （`"mic_record bytes"` 等）は画面へ出さず [`RECORD_FAILED`] の固定文言へ畳む
/// （`fandhe-egui-proxy::device::mic_record::to_message` と同じ方針）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn summary_from_bytes(
    bytes: Vec<u8>,
    mime: String,
    duration_ms: u64,
) -> Result<RecordSummary, String> {
    let recording =
        build_recording(bytes, mime, duration_ms).map_err(|_| RECORD_FAILED.to_string())?;
    Ok(RecordSummary {
        size_bytes: recording.bytes.len(),
        mime: recording.mime,
        duration_ms: recording.duration_ms,
    })
}

/// `MediaRecorder.mimeType`/`Blob.type` から表示する mime を決める純関数。
/// Safari は `start()` 前の `mimeType` が空になることがあるため、`recorder_mime` が
/// 空なら `blob_type` へ、両方空なら固定の汎用値へ落とす（ブラウザ差の吸収を 1 関数に
/// まとめる、実装計画「純関数の契約」節）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn resolve_mime(recorder_mime: &str, blob_type: &str) -> String {
    if !recorder_mime.is_empty() {
        recorder_mime.to_string()
    } else if !blob_type.is_empty() {
        blob_type.to_string()
    } else {
        "application/octet-stream".to_string()
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::mic_record::{
        finish_record, mark_recording, mark_stopping, take_start_request, take_stop_request, tick,
        MicRecordState, MAX_RECORD_MS,
    };
    use js_sys::{Array, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{MediaRecorder, MediaStream, MediaStreamConstraints, RecordingState};

    use super::{
        outcome_from_error_name, resolve_mime, summary_from_bytes, INTERRUPTED, UNAVAILABLE,
    };
    use crate::mount::Applier;

    /// 1 回の録音セッションぶんの状態。すべての終了経路（成功・失敗・キャンセル・
    /// `pagehide`）が [`release`] を通して解放する（モジュール doc 参照）。
    struct Session {
        /// `pagehide` からこのセッションを破棄するときに使う（`discard_all_sessions`
        /// の唯一の呼び出し元）。`Applier` は `Rc` の複製のみで軽量（`location.rs::
        /// APPLIERS` の doc 参照）。
        applier: Applier<MicRecordState>,
        /// [`finish_record`] へ照合する世代番号（`Entry.generation` と同じ役割、
        /// `location.rs::Entry` doc 参照。古い世代の遅延コールバックを捨てる）。
        generation: u64,
        /// 権限確認中（`getUserMedia` の Promise 待ち）に「停止」が押されたかどうか。
        /// `recorder` がまだ無い間の停止要求はこのフラグに畳み、Promise 解決後に
        /// [`super::outcome_from_error_name`] で `Cancelled` へ変換する。
        cancel_requested: bool,
        stream: Option<MediaStream>,
        recorder: Option<MediaRecorder>,
        /// `dataavailable` で集めた `Blob` チャンク（`onstop` で結合する）。
        chunks: Array,
        /// 録音開始時刻（`Date::now()`）。経過秒数の実測・`duration_ms` の算出に使う
        /// （モジュール doc「`setInterval` を使う理由」節参照）。
        started_at_ms: Option<f64>,
        timeout_id: Option<i32>,
        interval_id: Option<i32>,
        // 発火はしないが、Drop されるまで JS 側から呼び出し可能な状態を保つために
        // 保持するだけのフィールド（`file_picker.rs::PickHandlers` と同型）。
        #[allow(dead_code)]
        closures: Vec<Closure<dyn FnMut(JsValue)>>,
        /// `MAX_RECORD_MS` 自動停止用タイマーの `Closure`。`forget()` すると録音の
        /// たびに Wasm ヒープへ永久リークするため、`release` で他の closures と同じく
        /// 通常の drop に任せる（PR #963 レビュー指摘）。
        #[allow(dead_code)]
        timeout_closure: Option<Closure<dyn FnMut()>>,
        /// 経過秒数反映タイマーの `Closure`。上記と同じ理由で保持する。
        #[allow(dead_code)]
        interval_closure: Option<Closure<dyn FnMut()>>,
    }

    thread_local! {
        /// マウント ID ごとの録音セッション（`location.rs::ACTIVE` と同じくマウント単位で
        /// 持つ。同一ページへ本画面を 2 つマウントしたときに一方が他方の録音を
        /// 止めないため）。
        static ACTIVE: RefCell<HashMap<u64, Session>> = RefCell::new(HashMap::new());
    }

    /// `pagehide` イベント購読クロージャの型（clippy::type_complexity 回避の
    /// エイリアス。`location.rs::web::PageLifecycleClosure` と同型だが、
    /// 定義先の `mod web` がモジュール private なためここでは再利用せず
    /// 同じ形を別名で持つ）。
    type PagehideClosure = Closure<dyn FnMut(web_sys::Event)>;

    thread_local! {
        /// `pagehide` 購読を（未登録なら）1 度だけ登録するためのハンドル
        /// （`location.rs::PAGEHIDE` と同型）。
        static PAGEHIDE: RefCell<Option<PagehideClosure>> = const { RefCell::new(None) };
    }

    /// `window` を取得する（`web_sys::window()` の欠如を毎回 `Option` 判定しない
    /// ための下ごしらえ）。
    fn window() -> Option<web_sys::Window> {
        web_sys::window()
    }

    /// `mount_id` のセッションを解放する（タイマー解除 → track 停止（マイク表示が
    /// 消える）→ recorder のハンドラを外す → `ACTIVE` から除去）。全終了経路
    /// （成功・失敗・キャンセル・`pagehide`）がここを通る（モジュール doc 参照）。
    fn release(mount_id: u64) {
        let Some(session) = ACTIVE.with(|active| active.borrow_mut().remove(&mount_id)) else {
            return;
        };
        if let Some(w) = window() {
            if let Some(id) = session.timeout_id {
                w.clear_timeout_with_handle(id);
            }
            if let Some(id) = session.interval_id {
                w.clear_interval_with_handle(id);
            }
        }
        if let Some(recorder) = &session.recorder {
            recorder.set_ondataavailable(None);
            recorder.set_onstop(None);
            recorder.set_onerror(None);
        }
        if let Some(stream) = &session.stream {
            for track in stream.get_tracks().iter() {
                if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                    track.stop();
                }
            }
        }
    }

    /// 要求を失敗として畳み、セッションを解放して [`Applier::apply`] へ渡す
    /// （`file_picker.rs::fail` と同型）。
    /// 呼び出し元の多くは `await` を挟んだ後にこれを呼ぶため、その間に
    /// `pagehide`/`stop` でセッションが破棄・差し替えされている場合がある
    /// （PR #963 レビュー指摘、P1 2 件）。ここで再照合しないと、破棄済みの
    /// `mount_id` に対して `release`/`finish_record` を呼び、`discard_all_sessions`
    /// が既に確定させた「中断」結果を、遅延到着した成功/失敗で上書きしてしまう。
    /// `release`/`finish_record` へ到達する全経路がここを通るため、ここ 1 箇所の
    /// 再照合で両方の指摘（`handle_stop` の Blob 読み込み・`getUserMedia` の
    /// Err 側）を塞ぐ（root cause を共有関数側で塞ぐ方針）。
    fn fail(
        applier: &Applier<MicRecordState>,
        mount_id: u64,
        generation: u64,
        message: &'static str,
    ) {
        if !matches_current(mount_id, generation) {
            return;
        }
        release(mount_id);
        applier.apply(move |s| finish_record(s, generation, Err(message.to_string())));
    }

    /// `target[name]` が呼び出し可能な関数として実在するかを確認する（`share.rs::
    /// has_function` と同型。型に現れない JS 側の実態を境界で検証する、security.md A03）。
    fn has_function(target: &JsValue, name: &str) -> bool {
        Reflect::get(target, &JsValue::from_str(name))
            .map(|value| value.is_function())
            .unwrap_or(false)
    }

    /// `stop_recorder`: `state() == Recording` のときだけ `stop()` を呼ぶ
    /// （`tick` 上限からの自動停止・`setTimeout`・明示的な「停止」の 3 経路すべてが
    /// これを通るため、`InvalidStateError` を作らない、モジュール doc 参照）。
    fn stop_recorder(mount_id: u64) {
        let recorder = ACTIVE.with(|active| {
            active
                .borrow()
                .get(&mount_id)
                .and_then(|s| s.recorder.clone())
        });
        if let Some(recorder) = recorder {
            if recorder.state() == RecordingState::Recording {
                let _ = recorder.stop();
            }
        }
    }

    /// `mount_id`・`generation` が現在のセッションと一致するかを確認する
    /// （`location.rs::update_last_applied` と同じ「古い世代の完了を照合して捨てる」
    /// 判定）。一致しなければ `false`。
    fn matches_current(mount_id: u64, generation: u64) -> bool {
        ACTIVE.with(|active| {
            active
                .borrow()
                .get(&mount_id)
                .is_some_and(|s| s.generation == generation)
        })
    }

    /// `onstop`（録音完了）の継続。`Closure` を自分自身の中で drop しないよう
    /// `spawn_local` へ逃がす（`file_picker.rs::on_change` と同じ回避策）。
    fn handle_stop(mount_id: u64, generation: u64, applier: Applier<MicRecordState>) {
        wasm_bindgen_futures::spawn_local(async move {
            if !matches_current(mount_id, generation) {
                // 古い世代（既に `stop`/`pagehide` で解放済み）の遅延コールバック。
                return;
            }
            let (chunks, mime, started_at_ms) = ACTIVE.with(|active| {
                let borrowed = active.borrow();
                let session = borrowed.get(&mount_id);
                (
                    session.map(|s| s.chunks.clone()),
                    session
                        .and_then(|s| s.recorder.as_ref())
                        .map(|r| r.mime_type()),
                    session.and_then(|s| s.started_at_ms),
                )
            });
            let (Some(chunks), Some(recorder_mime), Some(started_at_ms)) =
                (chunks, mime, started_at_ms)
            else {
                fail(&applier, mount_id, generation, super::RECORD_FAILED);
                return;
            };

            let blob = match web_sys::Blob::new_with_blob_sequence(&chunks) {
                Ok(blob) => blob,
                Err(_) => {
                    fail(&applier, mount_id, generation, super::RECORD_FAILED);
                    return;
                }
            };

            // 巨大な `ArrayBuffer` を確保する前に上限を確認する
            // （`file_picker.rs::read` の事前判定と同じ最適化）。
            if blob.size() as usize > fandhe_device::mic_record::MAX_RECORDING_BYTES {
                fail(&applier, mount_id, generation, super::RECORD_FAILED);
                return;
            }

            let buffer = match JsFuture::from(blob.array_buffer()).await {
                Ok(raw) => match raw.dyn_into::<js_sys::ArrayBuffer>() {
                    Ok(buffer) => buffer,
                    Err(_) => {
                        fail(&applier, mount_id, generation, super::RECORD_FAILED);
                        return;
                    }
                },
                Err(_) => {
                    fail(&applier, mount_id, generation, super::RECORD_FAILED);
                    return;
                }
            };

            let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
            let mime = resolve_mime(&recorder_mime, &blob.type_());
            let duration_ms = (js_sys::Date::now() - started_at_ms).max(0.0) as u64;

            // `array_buffer()` の await 中に `pagehide` でセッションが破棄され
            // 得るため、結果反映の直前で再照合する（PR #963 レビュー指摘 P1）。
            if !matches_current(mount_id, generation) {
                return;
            }
            release(mount_id);
            let result = summary_from_bytes(bytes, mime, duration_ms)
                .map(fandhe_core::screens::mic_record::RecordOutcome::Recorded);
            applier.apply(move |s| finish_record(s, generation, result));
        });
    }

    /// `onerror`（録音失敗）の継続。エラーオブジェクトの内容は読まない
    /// （モジュール doc「秘密値・ログ」節）。
    fn handle_error(mount_id: u64, generation: u64, applier: Applier<MicRecordState>) {
        wasm_bindgen_futures::spawn_local(async move {
            if !matches_current(mount_id, generation) {
                return;
            }
            fail(&applier, mount_id, generation, super::RECORD_FAILED);
        });
    }

    /// `getUserMedia` が成功したあとの続き（`MediaRecorder` の構築・イベント配線・
    /// `start()`・タイマー起動）。権限確認中の停止要求（`cancel_requested`）は
    /// ここで `Cancelled` へ畳む。
    fn start_recorder(
        mount_id: u64,
        generation: u64,
        applier: &Applier<MicRecordState>,
        stream: MediaStream,
        max_duration_ms: u64,
    ) {
        let cancel_requested = ACTIVE.with(|active| {
            active
                .borrow()
                .get(&mount_id)
                .map(|s| s.cancel_requested)
                .unwrap_or(false)
        });
        if cancel_requested {
            for track in stream.get_tracks().iter() {
                if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                    track.stop();
                }
            }
            release(mount_id);
            applier.apply(move |s| {
                finish_record(
                    s,
                    generation,
                    Ok(fandhe_core::screens::mic_record::RecordOutcome::Cancelled),
                )
            });
            return;
        }

        let recorder = match MediaRecorder::new_with_media_stream(&stream) {
            Ok(recorder) => recorder,
            Err(_) => {
                for track in stream.get_tracks().iter() {
                    if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                        track.stop();
                    }
                }
                fail(applier, mount_id, generation, UNAVAILABLE);
                return;
            }
        };

        let chunks = Array::new();
        let chunks_for_handler = chunks.clone();
        let on_data = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
            let Ok(event) = event.dyn_into::<web_sys::BlobEvent>() else {
                return;
            };
            if let Some(blob) = event.data() {
                chunks_for_handler.push(&blob);
            }
        });
        recorder.set_ondataavailable(Some(on_data.as_ref().unchecked_ref()));

        let applier_stop = applier.clone();
        let on_stop = Closure::<dyn FnMut(JsValue)>::new(move |_event: JsValue| {
            handle_stop(mount_id, generation, applier_stop.clone());
        });
        recorder.set_onstop(Some(on_stop.as_ref().unchecked_ref()));

        let applier_err = applier.clone();
        let on_error = Closure::<dyn FnMut(JsValue)>::new(move |_event: JsValue| {
            handle_error(mount_id, generation, applier_err.clone());
        });
        recorder.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        if recorder.start().is_err() {
            // `set()` 済みのハンドラを明示的に外す。`release` はまだ `session.recorder`
            // を持っていない（このローカル `recorder` を未格納のまま drop するため）
            // ため、ここで外さないとイベントハンドラが登録されたまま JS 側から
            // 参照され得る（PR #963 Bugbot 指摘）。
            recorder.set_ondataavailable(None);
            recorder.set_onstop(None);
            recorder.set_onerror(None);
            for track in stream.get_tracks().iter() {
                if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                    track.stop();
                }
            }
            fail(applier, mount_id, generation, super::RECORD_FAILED);
            return;
        }

        let started_at_ms = js_sys::Date::now();

        // `MAX_RECORD_MS` の自動停止（OS 側）。Rust 側の `tick` 自動停止と同じ定数
        // （モジュール doc「期限は 1 定数で揃える」節）。`generation` を照合してから
        // 止める（`matches_current`。古いセッションのタイマーが差し替え後の新しい
        // セッションを誤って止めないため、PR #963 レビュー指摘）。
        let timeout_closure = Closure::<dyn FnMut()>::new(move || {
            if matches_current(mount_id, generation) {
                stop_recorder(mount_id);
            }
        });
        let timeout_id = window().and_then(|w| {
            w.set_timeout_with_callback_and_timeout_and_arguments_0(
                timeout_closure.as_ref().unchecked_ref(),
                max_duration_ms as i32,
            )
            .ok()
        });

        // 経過秒数の 1 秒間隔の反映（モジュール doc「`setInterval` を使う理由」節）。
        let applier_tick = applier.clone();
        let interval_closure = Closure::<dyn FnMut()>::new(move || {
            if !matches_current(mount_id, generation) {
                return;
            }
            let elapsed = (js_sys::Date::now() - started_at_ms).max(0.0) as u64;
            applier_tick.apply(move |s| {
                let dt = elapsed.saturating_sub(s.elapsed_ms);
                tick(s, dt);
            });
        });
        let interval_id = window().and_then(|w| {
            w.set_interval_with_callback_and_timeout_and_arguments_0(
                interval_closure.as_ref().unchecked_ref(),
                1_000,
            )
            .ok()
        });

        ACTIVE.with(|active| {
            if let Some(session) = active.borrow_mut().get_mut(&mount_id) {
                session.stream = Some(stream);
                session.recorder = Some(recorder);
                session.chunks = chunks;
                session.started_at_ms = Some(started_at_ms);
                session.timeout_id = timeout_id;
                session.interval_id = interval_id;
                session.closures = vec![on_data, on_stop, on_error];
                // `forget()` せず Session に保持する。録音ごとに `forget` すると
                // タイマー Closure が Wasm ヒープに恒久的に蓄積するため、`release`
                // での通常の drop に委ねる（PR #963 codex レビュー指摘）。
                session.timeout_closure = Some(timeout_closure);
                session.interval_closure = Some(interval_closure);
            }
        });
    }

    /// 全マウントの録音セッションを破棄する。`pagehide`（タブを離れる・bfcache への
    /// 退避）から呼ばれる契約（`location.rs::stop_all_location_watches` と同型）。
    /// bfcache から復帰しても古い値を返し続けないよう、再開はしない（実装計画
    /// 「対象外」節、`pageshow` の再開なし）。
    fn discard_all_sessions() {
        let entries: Vec<(u64, u64, Applier<MicRecordState>)> = ACTIVE.with(|active| {
            active
                .borrow()
                .iter()
                .map(|(mount_id, session)| (*mount_id, session.generation, session.applier.clone()))
                .collect()
        });
        for (mount_id, generation, applier) in entries {
            release(mount_id);
            applier.apply(move |s| finish_record(s, generation, Err(INTERRUPTED.to_string())));
        }
    }

    /// `pagehide` 購読を（未登録なら）1 度だけ登録する。
    ///
    /// `ACTIVE` へセッションを登録した直後・`getUserMedia` 呼び出し前に呼ぶ契約
    /// （PR #963 レビュー指摘 P1）。権限プロンプト表示中に離脱されても
    /// `discard_all_sessions` が確実に効くようにするため。戻り値は購読が有効か
    /// （既存登録済み・今回登録成功のいずれか）で、呼び出し側はこれが `false` の
    /// ときは fail-closed で録音開始へ進んではならない。
    fn register_pagehide_once() -> bool {
        PAGEHIDE.with(|cell| {
            if cell.borrow().is_some() {
                return true;
            }
            let Some(w) = window() else {
                return false;
            };
            let closure =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| {
                    discard_all_sessions();
                });
            if w.add_event_listener_with_callback("pagehide", closure.as_ref().unchecked_ref())
                .is_err()
            {
                return false;
            }
            *cell.borrow_mut() = Some(closure);
            true
        })
    }

    /// `mount_with_effect` へ渡す effect。`mic_record` 画面の開始・停止要求を処理する。
    ///
    /// 要求が無ければ何もしない（`mount.rs` モジュール doc の「effect フック」節の契約）。
    pub fn mic_record_effect(state: &MicRecordState, applier: &Applier<MicRecordState>) {
        let mount_id = applier.mount_id();

        if let Some(request) = take_start_request(state) {
            // 既にセッションがあれば二重開始しない（`file_picker.rs::ACTIVE` と同じ判定）。
            if ACTIVE.with(|active| active.borrow().contains_key(&mount_id)) {
                return;
            }
            applier.apply(mark_recording);

            // 先に「起動不能」を仮登録してから前提検査に入る（`apply` が起こす再描画で
            // effect が再入しても「既にエントリがある」判定で no-op になるようにする、
            // `location.rs::mark_unavailable_and_report` の doc と同じ理由）。
            ACTIVE.with(|active| {
                active.borrow_mut().insert(
                    mount_id,
                    Session {
                        applier: applier.clone(),
                        generation: request.generation,
                        cancel_requested: false,
                        stream: None,
                        recorder: None,
                        chunks: Array::new(),
                        started_at_ms: None,
                        timeout_id: None,
                        interval_id: None,
                        closures: Vec::new(),
                        timeout_closure: None,
                        interval_closure: None,
                    },
                );
            });

            // `getUserMedia` の Promise 待ち（権限プロンプト表示中）にも `pagehide` を
            // 監視できるよう、ACTIVE 登録直後・`getUserMedia` 呼び出し前に登録する
            // （PR #963 レビュー指摘 P1）。登録できなければ離脱後の録音開始を防げない
            // ため fail-closed で録音そのものを始めない。
            if !register_pagehide_once() {
                fail(applier, mount_id, request.generation, UNAVAILABLE);
                return;
            }

            let Some(w) = window() else {
                fail(applier, mount_id, request.generation, UNAVAILABLE);
                return;
            };
            if !w.is_secure_context() {
                fail(applier, mount_id, request.generation, UNAVAILABLE);
                return;
            }
            let navigator = w.navigator();
            let Ok(media_devices) = navigator.media_devices() else {
                fail(applier, mount_id, request.generation, UNAVAILABLE);
                return;
            };
            // `MediaRecorder` 自体が実在するかを境界で検証する（`share.rs::has_function`
            // と同じ理由、型に現れない JS 側の実態、security.md A03）。
            // `navigator.mediaDevices` は関数ではなく `MediaDevices` オブジェクトのため、
            // 存在確認は直前の `media_devices()` の成功で足りる（PR #963 レビュー指摘）。
            let global: JsValue = js_sys::global().into();
            if !has_function(&global, "MediaRecorder") {
                fail(applier, mount_id, request.generation, UNAVAILABLE);
                return;
            }

            let constraints = MediaStreamConstraints::new();
            constraints.set_audio_bool(true);

            let promise = match media_devices.get_user_media_with_constraints(&constraints) {
                Ok(promise) => promise,
                Err(err) => {
                    let name = Reflect::get(&err, &JsValue::from_str("name"))
                        .ok()
                        .and_then(|v| v.as_string());
                    let result = outcome_from_error_name(name.as_deref(), false);
                    release(mount_id);
                    let generation = request.generation;
                    applier.apply(move |s| finish_record(s, generation, result));
                    return;
                }
            };

            let generation = request.generation;
            let max_duration_ms = request.max_duration_ms.min(MAX_RECORD_MS);
            let applier = applier.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match JsFuture::from(promise).await {
                    Ok(stream_value) => {
                        if !matches_current(mount_id, generation) {
                            // `pagehide`/`stop` で既に破棄済み。取得したストリームは
                            // すぐ閉じる（マイク表示を残さない）。
                            if let Ok(stream) = stream_value.dyn_into::<MediaStream>() {
                                for track in stream.get_tracks().iter() {
                                    if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>()
                                    {
                                        track.stop();
                                    }
                                }
                            }
                            return;
                        }
                        match stream_value.dyn_into::<MediaStream>() {
                            Ok(stream) => start_recorder(
                                mount_id,
                                generation,
                                &applier,
                                stream,
                                max_duration_ms,
                            ),
                            Err(_) => fail(&applier, mount_id, generation, super::RECORD_FAILED),
                        }
                    }
                    Err(err) => {
                        // 権限プロンプト表示中に `pagehide` でセッションが破棄され得る
                        // （成功側は既に照合済み。Err 側にも同じ照合が必要、PR #963
                        // レビュー指摘 P1）。破棄済みなら `discard_all_sessions` が
                        // 確定させた中断結果を上書きしない。
                        if !matches_current(mount_id, generation) {
                            return;
                        }
                        let name = Reflect::get(&err, &JsValue::from_str("name"))
                            .ok()
                            .and_then(|v| v.as_string());
                        let cancel_requested = ACTIVE.with(|active| {
                            active
                                .borrow()
                                .get(&mount_id)
                                .map(|s| s.cancel_requested)
                                .unwrap_or(false)
                        });
                        let result = outcome_from_error_name(name.as_deref(), cancel_requested);
                        release(mount_id);
                        applier.apply(move |s| finish_record(s, generation, result));
                    }
                }
            });
            return;
        }

        if let Some(request) = take_stop_request(state) {
            applier.apply(mark_stopping);
            let has_recorder = ACTIVE.with(|active| {
                let mut borrowed = active.borrow_mut();
                let Some(session) = borrowed.get_mut(&mount_id) else {
                    return false;
                };
                if session.generation != request.generation {
                    return false;
                }
                if session.recorder.is_none() {
                    // 権限確認中（`getUserMedia` の Promise 待ち）。解決後に
                    // `outcome_from_error_name` が `Cancelled` へ畳む。
                    session.cancel_requested = true;
                    return false;
                }
                true
            });
            if has_recorder {
                stop_recorder(mount_id);
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::mic_record_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_requested_wins_over_any_error_name() {
        assert_eq!(
            outcome_from_error_name(Some("NotAllowedError"), true),
            Ok(RecordOutcome::Cancelled)
        );
        assert_eq!(
            outcome_from_error_name(None, true),
            Ok(RecordOutcome::Cancelled)
        );
    }

    #[test]
    fn not_allowed_error_maps_to_permission_message() {
        assert_eq!(
            outcome_from_error_name(Some("NotAllowedError"), false),
            Err(NOT_ALLOWED.to_string())
        );
        assert_eq!(
            outcome_from_error_name(Some("PermissionDeniedError"), false),
            Err(NOT_ALLOWED.to_string())
        );
    }

    #[test]
    fn not_found_error_maps_to_not_found_message() {
        assert_eq!(
            outcome_from_error_name(Some("NotFoundError"), false),
            Err(NOT_FOUND.to_string())
        );
        assert_eq!(
            outcome_from_error_name(Some("OverconstrainedError"), false),
            Err(NOT_FOUND.to_string())
        );
    }

    #[test]
    fn unsupported_errors_map_to_unavailable_message() {
        assert_eq!(
            outcome_from_error_name(Some("NotSupportedError"), false),
            Err(UNAVAILABLE.to_string())
        );
        assert_eq!(
            outcome_from_error_name(Some("SecurityError"), false),
            Err(UNAVAILABLE.to_string())
        );
    }

    #[test]
    fn unknown_or_missing_name_is_generic_failure() {
        assert_eq!(
            outcome_from_error_name(None, false),
            Err(RECORD_FAILED.to_string())
        );
        assert_eq!(
            outcome_from_error_name(Some("AbortError"), false),
            Err(RECORD_FAILED.to_string())
        );
    }

    #[test]
    fn summary_from_bytes_rejects_empty() {
        assert!(summary_from_bytes(Vec::new(), "audio/webm".to_string(), 1_000).is_err());
    }

    #[test]
    fn summary_from_bytes_rejects_zero_duration() {
        assert!(summary_from_bytes(vec![1, 2, 3], "audio/webm".to_string(), 0).is_err());
    }

    #[test]
    fn summary_from_bytes_rejects_oversized() {
        let bytes = vec![0u8; fandhe_device::mic_record::MAX_RECORDING_BYTES + 1];
        assert!(summary_from_bytes(bytes, "audio/webm".to_string(), 1_000).is_err());
    }

    #[test]
    fn summary_from_bytes_accepts_valid_input() {
        let summary = summary_from_bytes(vec![1, 2, 3], "audio/webm".to_string(), 1_500).unwrap();
        assert_eq!(summary.size_bytes, 3);
        assert_eq!(summary.mime, "audio/webm");
        assert_eq!(summary.duration_ms, 1_500);
    }

    #[test]
    fn resolve_mime_prefers_recorder_mime() {
        assert_eq!(resolve_mime("audio/webm", "audio/mp4"), "audio/webm");
    }

    #[test]
    fn resolve_mime_falls_back_to_blob_type_when_recorder_mime_empty() {
        assert_eq!(resolve_mime("", "audio/mp4"), "audio/mp4");
    }

    #[test]
    fn resolve_mime_falls_back_to_octet_stream_when_both_empty() {
        assert_eq!(resolve_mime("", ""), "application/octet-stream");
    }
}
