//! Phase 11（親 #858）型サンプル「クリップボード」（Issue #859）の effect ドライバ。
//! `fandhe_core::screens::clipboard_text`（端末依存ゼロ）の `pending` 要求を観測し、
//! `fandhe_device::clipboard::Clipboard` で OS のクリップボードを読み書きして
//! `CompletionSender` 経由で結果を返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。Web（wasm32）のクリップボードは本経路を
//! 通らず `fandhe-web-dom` 側の `mount` が `navigator.clipboard` へ直接配線する
//! （`crates/fandhe-web-dom/src/mount.rs` 参照）。
//!
//! # スレッドを起こさない理由（`http_fetch`/`camera_capture` との差）
//!
//! HTTP 取得・カメラ撮影は数秒〜数十秒かかるため別スレッド + mailbox で待つが、
//! クリップボードは 3 OS とも同期 API で即座に返る。そのため本ドライバは呼ばれた
//! スレッド（egui の update ループ）でそのまま OS を呼び、同じ呼び出しの中で完了を
//! 送る。iOS の `UIPasteboard` は main スレッド前提の API であり、update ループが
//! main スレッドで回る iOS ではこの方式がそのまま要件を満たす（別スレッドへ
//! 逃がすと逆に契約違反になる）。
//!
//! ponytail: 同期呼び出し。OS 側で数百 ms 以上ブロックする事例（iOS の許可アラート
//! 表示中など）が実機で観測されたら `camera_capture` の mailbox 方式へ上げる。

use std::sync::OnceLock;

use fandhe_core::screens::clipboard_text::{
    finish_copy, finish_paste, take_request, ClipboardRequest, ClipboardTextState,
};
use fandhe_device::clipboard::Clipboard;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// プロセス内で共有するクリップボード実装（接続失敗はエラーとして保持する）。
///
/// デスクトップの `ArboardClipboard` はハンドルを作り直すと X11 で書き込んだ内容が
/// 失われる（`fandhe_device::clipboard::ArboardClipboard` の doc 参照）ため、
/// `OnceLock` で 1 個だけ保持して使い回す。接続に失敗した場合もその結果を保持し、
/// 呼び出しのたびに再接続を試みない（失敗はディスプレイサーバ不在など持続的な要因）。
static CLIPBOARD: OnceLock<Result<Box<dyn Clipboard>, String>> = OnceLock::new();

/// OS クリップボードへ接続できないときに画面へ出す案内。
const UNAVAILABLE: &str = "この環境では OS のクリップボードを使えません";

/// 実行対象プラットフォームに応じたクリップボード実装を返す。
///
/// デスクトップ（macOS/Windows/Linux）は `ArboardClipboard`、iOS は
/// [`crate::device::ios::IosClipboard`]、Android は
/// [`crate::device::android::AndroidClipboard`]。
///
/// **接続失敗を `DummyClipboard` で埋めない**（codex-review 指摘 P1、PR #860）。
/// プロセス内ダミーへ退避すると、OS のクリップボードには何も書けていないのに画面へ
/// 「コピーしました」と表示され、他アプリへの貼り付けでは以前の内容が使われる
/// （ユーザーへ嘘の成功を返す最悪の挙動）。失敗は失敗として呼び出し元へ返し、
/// 画面の `status` にも出す（coding-rust.md「回復可能なエラーは Result で返す」）。
/// `DummyClipboard` はテストと、端末機能を持たない明示的なデモ用途に限る。
fn shared_clipboard() -> Result<&'static dyn Clipboard, &'static str> {
    let cached = CLIPBOARD.get_or_init(|| {
        #[cfg(target_os = "ios")]
        {
            Ok(Box::new(crate::device::ios::IosClipboard) as Box<dyn Clipboard>)
        }
        #[cfg(target_os = "android")]
        {
            Ok(Box::new(crate::device::android::AndroidClipboard) as Box<dyn Clipboard>)
        }
        // `ArboardClipboard` はデスクトップ 3 OS でのみ存在する
        // （arboard が Android/iOS のバックエンドを持たない、
        // `fandhe_device::clipboard` の doc 参照）。
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            use fandhe_device::clipboard::ArboardClipboard;
            ArboardClipboard::new()
                .map(|c| Box::new(c) as Box<dyn Clipboard>)
                .map_err(|e| e.to_string())
        }
        // 上記いずれにも当たらないターゲット（feature `device` が有効な想定外の
        // プラットフォーム）。ダミーで成功を装わず未対応として扱う。
        #[cfg(not(any(
            target_os = "android",
            target_os = "ios",
            target_os = "macos",
            target_os = "windows",
            target_os = "linux"
        )))]
        {
            Err(DeviceError::Unsupported("clipboard").to_string())
        }
    });
    match cached {
        Ok(clipboard) => Ok(clipboard.as_ref()),
        // 接続時のエラー文言は内部要因（ディスプレイサーバ名等）を含み得るため画面へは
        // 転送せず、固定の案内へ畳む（security.md A09）。
        Err(_) => Err(UNAVAILABLE),
    }
}

/// `DeviceError` を画面へ渡す文言へ変換する。`DeviceError` の内部構造（`std::io::Error`
/// 等）は `fandhe-core` 側へ漏らさず `Display` の文言のみを転送する
/// （security.md A09、`device.rs` の `to_note_storage_error` と同じ判断）。
fn to_message(e: DeviceError) -> String {
    e.to_string()
}

/// `clipboard` の要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう
/// `clipboard_text_effect` 本体から切り出す）。
fn drive(
    state: &ClipboardTextState,
    sender: &CompletionSender<ClipboardTextState>,
    clipboard: &dyn Clipboard,
) {
    let Some(request) = take_request(state) else {
        return;
    };

    // 受信側（ProxyApp）がドロップ済み（アプリ終了中）の送信失敗は無視してよい
    // （coding-rust.md: panic を API 契約にしない、`device/tilt.rs` と同じ判断）。
    match request {
        ClipboardRequest::Copy(text) => {
            let result = clipboard.set_text(&text).map_err(to_message);
            let _ = sender.send(Box::new(move |s: &mut ClipboardTextState| {
                finish_copy(s, result);
            }));
        }
        ClipboardRequest::Paste => {
            let result = clipboard.get_text().map_err(to_message);
            let _ = sender.send(Box::new(move |s: &mut ClipboardTextState| {
                finish_paste(s, result);
            }));
        }
    }
}

/// OS クリップボードへ接続できないときに、要求を失敗として畳む
/// （要求が残ったままだと画面のボタンが戻らず操作不能になる）。
fn fail_request(
    state: &ClipboardTextState,
    sender: &CompletionSender<ClipboardTextState>,
    message: &'static str,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let message = message.to_string();
    let _ = match request {
        ClipboardRequest::Copy(_) => sender.send(Box::new(move |s: &mut ClipboardTextState| {
            finish_copy(s, Err(message));
        })),
        ClipboardRequest::Paste => sender.send(Box::new(move |s: &mut ClipboardTextState| {
            finish_paste(s, Err(message));
        })),
    };
}

/// `registry.rs` の `effect` 区画から `clipboard_text` 画面用に配線される effect ドライバ。
pub fn clipboard_text_effect(
    state: &ClipboardTextState,
    sender: &CompletionSender<ClipboardTextState>,
) {
    match shared_clipboard() {
        Ok(clipboard) => drive(state, sender, clipboard),
        Err(message) => fail_request(state, sender, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::clipboard_text::{copy, paste, set_input};
    use fandhe_device::clipboard::DummyClipboard;

    fn sender_with_channel() -> (
        CompletionSender<ClipboardTextState>,
        std::sync::mpsc::Receiver<crate::app::Completion<ClipboardTextState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    /// 常に失敗する `Clipboard`（エラー文言の転送経路を検証する）。
    struct FailingClipboard;

    impl Clipboard for FailingClipboard {
        fn get_text(&self) -> Result<Option<String>, DeviceError> {
            Err(DeviceError::Unsupported("clipboard"))
        }
        fn set_text(&self, _text: &str) -> Result<(), DeviceError> {
            Err(DeviceError::Unsupported("clipboard"))
        }
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let state = ClipboardTextState::default();

        drive(&state, &sender, &DummyClipboard::new());

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drive_round_trips_through_os_clipboard() {
        // 「自アプリで書いて自アプリで読む」往復（Issue #859 の型サンプルの本体）を
        // `Clipboard` 実装込みで固定する。
        let clipboard = DummyClipboard::new();
        let (sender, rx) = sender_with_channel();
        let mut state = ClipboardTextState::default();
        set_input(&mut state, "往復".to_string());

        copy(&mut state);
        drive(&state, &sender, &clipboard);
        rx.recv().expect("コピー完了が送られる")(&mut state);
        assert_eq!(state.status, "コピーしました");

        paste(&mut state);
        drive(&state, &sender, &clipboard);
        rx.recv().expect("貼り付け完了が送られる")(&mut state);

        assert_eq!(state.pasted, Some("往復".to_string()));
    }

    #[test]
    fn drive_reports_copy_failure() {
        let (sender, rx) = sender_with_channel();
        let mut state = ClipboardTextState::default();
        set_input(&mut state, "失敗する".to_string());
        copy(&mut state);

        drive(&state, &sender, &FailingClipboard);
        rx.recv().expect("失敗でも完了は送られる")(&mut state);

        assert!(state.status.contains("失敗しました"), "{}", state.status);
        assert_eq!(state.pending, None, "失敗時も要求は下りる");
    }

    #[test]
    fn drive_reports_paste_failure() {
        let (sender, rx) = sender_with_channel();
        let mut state = ClipboardTextState::default();
        paste(&mut state);

        drive(&state, &sender, &FailingClipboard);
        rx.recv().expect("失敗でも完了は送られる")(&mut state);

        assert!(state.status.contains("失敗しました"), "{}", state.status);
        assert_eq!(state.pending, None);
    }

    #[test]
    fn drive_reports_empty_clipboard() {
        let (sender, rx) = sender_with_channel();
        let mut state = ClipboardTextState::default();
        paste(&mut state);

        drive(&state, &sender, &DummyClipboard::new());
        rx.recv().expect("空でも完了は送られる")(&mut state);

        assert_eq!(
            state.status,
            fandhe_core::screens::clipboard_text::EMPTY_CLIPBOARD_STATUS
        );
    }

    #[test]
    fn shared_clipboard_reports_failure_instead_of_faking_success() {
        // 接続できた場合は書き込めること、できない場合（ヘッドレス等）は
        // ダミーで成功を装わずエラーを返すことを確認する（codex-review P1、PR #860）。
        match shared_clipboard() {
            Ok(clipboard) => assert!(clipboard.set_text("fandhe clipboard self check").is_ok()),
            Err(message) => assert_eq!(message, UNAVAILABLE),
        }
    }

    #[test]
    fn fail_request_completes_copy_with_error() {
        // 接続不可のとき、要求を残したままにせず失敗として畳む（ボタンが戻る）。
        let (sender, rx) = sender_with_channel();
        let mut state = ClipboardTextState::default();
        set_input(&mut state, "書けない".to_string());
        copy(&mut state);

        fail_request(&state, &sender, UNAVAILABLE);
        rx.recv().expect("完了が送られる")(&mut state);

        assert_eq!(state.pending, None);
        assert!(state.status.contains(UNAVAILABLE), "{}", state.status);
        // 「コピーしました」という嘘の成功を出していないこと。
        assert!(!state.status.contains("コピーしました"), "{}", state.status);
    }

    #[test]
    fn fail_request_completes_paste_with_error() {
        let (sender, rx) = sender_with_channel();
        let mut state = ClipboardTextState::default();
        paste(&mut state);

        fail_request(&state, &sender, UNAVAILABLE);
        rx.recv().expect("完了が送られる")(&mut state);

        assert_eq!(state.pending, None);
        assert!(state.status.contains(UNAVAILABLE), "{}", state.status);
    }

    #[test]
    fn fail_request_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let state = ClipboardTextState::default();
        fail_request(&state, &sender, UNAVAILABLE);
        assert!(rx.try_recv().is_err());
    }
}
