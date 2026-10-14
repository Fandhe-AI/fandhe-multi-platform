//! エラーコード enum・詳細メッセージ保持・panic 遮断ヘルパ（TASK-3.2-S4 / #61、REQ-3）。
//!
//! `docs/architecture/swiftui-bridge-architecture.md` §3.3「エラー伝播」・§3.1「所有権・
//! 寿命規約」（無効ハンドル時の書き込み先分離）の実装。設計文書が規定する 2 層方式
//! （`i32` エラーコード enum ＋ 詳細メッセージ取得 API）と panic 遮断を提供する。
//!
//! # 呼び出し文脈
//!
//! `ffi` モジュールが `extern "C"` エクスポート関数の内部から本モジュールを利用する
//! （Issue #59）。具体的には以下の使い分けを `ffi` 層へ強制する:
//!
//! - 有効なハンドル（`FandheStore` 等）使用時のエラー詳細は [`ErrorSlot`]（per-handle
//!   ストレージ）へ [`ErrorSlot::record`] する。
//! - ハンドル非依存の失敗（`store` が `NULL`、ハンドル生成自体の失敗）・**無効ハンドル
//!   使用時**（メンバーシップ検査失敗）のエラー詳細は [`set_thread_last_error`] で
//!   スレッドローカルへ書く。無効ハンドルの実体（解放済みかもしれないメモリ）へは
//!   書き込まないことで use-after-free を回避する（設計文書 §3.1 所有権・寿命規約、
//!   §3.3 エラー伝播）。
//! - `fandhe_last_error_message`（S2 スコープ）は [`thread_last_error`] /
//!   [`ErrorSlot::message`] のいずれも**非破壊の読み取り**として実装する必要がある
//!   （設計文書: 「純粋な読み取り専用アクセサであり一切書き込まない」）。本モジュールの
//!   両アクセサは読み取り時に値を消費（take）しない。
//! - 各エクスポート関数の本体は [`catch_panic`] で包み、Rust の panic が FFI 境界を
//!   越えて Swift 側の未定義動作を引き起こさないようにする。
//!
//! 本モジュールは `codec` と同様 `unsafe` を一切含まない（クレートルートの
//! `#![deny(unsafe_code)]` に対し `#[allow(unsafe_code)]` を使わない。`ffi` モジュールのみが
//! `unsafe` を許可される設計、設計文書 §3.1「unsafe 境界の規約」）。

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

use crate::codec::CodecError;

/// エラーメッセージ 1 件あたりの格納上限（バイト数）。
///
/// panic ペイロードや内部エラーの `Display` 出力に任意長・任意内容の文字列が
/// 混入し得るため、格納前に切り詰めて肥大・情報過多を防ぐ（A05 対策、security.md 準拠）。
pub const MAX_ERROR_MESSAGE_LEN: usize = 4 * 1024;

/// FFI 境界（`ffi` モジュール、S2: #59）が Swift 側へ返す `i32` エラーコード。
///
/// `0` は成功、負値はエラー種別を表す（設計文書 §3.3「エラー伝播」）。正値は `ffi` 層の
/// セマンティクス（例: `fandhe_dispatch_event` の再描画要否）のために予約し、本 enum では
/// 定義しない。
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FandheErrorCode {
    /// 成功（エラーなし）。
    Success = 0,
    /// Swift 側から渡されたポインタが `NULL` だった（`store` 引数等）。
    NullPointer = -1,
    /// ハンドルが生存中ハンドルのレジストリに存在しない（解放済み・元々不正）。
    InvalidHandle = -2,
    /// イベントペイロードが上限バイト数を超えている（[`CodecError::PayloadTooLarge`] 対応）。
    PayloadTooLarge = -3,
    /// JSON のシリアライズ・デシリアライズに失敗した（[`CodecError::Json`] 対応）。
    JsonError = -4,
    /// `catch_unwind` が Rust 側の panic を捕捉した。
    PanicCaught = -5,
    /// 上記のいずれにも分類できない内部エラー。
    Internal = -6,
}

impl FandheErrorCode {
    /// FFI 境界を越える生の `i32` 値へ変換する（`extern "C"` 関数の戻り値に直渡しする想定）。
    pub const fn as_i32(self) -> i32 {
        self as i32
    }
}

impl From<FandheErrorCode> for i32 {
    fn from(code: FandheErrorCode) -> Self {
        code.as_i32()
    }
}

impl From<&CodecError> for FandheErrorCode {
    /// `codec` モジュールの型付きエラーを FFI エラーコードへ写像する。
    ///
    /// `codec` は `Result<_, CodecError>` を返すところまでが責務であり、`i32` への変換は
    /// 本モジュールの責務という設計文書 §3.2 の分界に対応する。
    fn from(err: &CodecError) -> Self {
        match err {
            CodecError::PayloadTooLarge { .. } => FandheErrorCode::PayloadTooLarge,
            CodecError::Json(_) => FandheErrorCode::JsonError,
        }
    }
}

/// `message` を [`MAX_ERROR_MESSAGE_LEN`] 以内へ切り詰める。UTF-8 文字境界の途中で
/// 切らないよう、上限を超えた場合は上限以下の直近の文字境界まで戻る。
fn truncate_message(mut message: String) -> String {
    if message.len() <= MAX_ERROR_MESSAGE_LEN {
        return message;
    }
    let mut boundary = MAX_ERROR_MESSAGE_LEN;
    while boundary > 0 && !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    message.truncate(boundary);
    message
}

thread_local! {
    /// プロセス内スレッドローカルの「最終エラー」（設計文書 §3.3 詳細メッセージ取得 API）。
    ///
    /// ハンドル非依存の失敗、および無効ハンドル使用時（per-handle ストレージへの書き込みが
    /// use-after-free になり得るため）のエラー詳細をここへ書く。
    static THREAD_LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// スレッドローカルの最終エラーメッセージを上書き保存する（[`MAX_ERROR_MESSAGE_LEN`] で切り詰め）。
///
/// `ffi` 層がハンドル非依存の失敗・無効ハンドル使用時に呼ぶ想定（本モジュール冒頭のコメント参照）。
pub fn set_thread_last_error(message: String) {
    let message = truncate_message(message);
    THREAD_LAST_ERROR.with(|cell| {
        *cell.borrow_mut() = Some(message);
    });
}

/// スレッドローカルの最終エラーメッセージを読み取る（非破壊。値を消費しない）。
///
/// `fandhe_last_error_message`（S2 スコープ）は純粋な読み取り専用アクセサとして実装する
/// 必要があるため（設計文書 §3.3）、本関数は呼び出し後も値を保持し続ける。
pub fn thread_last_error() -> Option<String> {
    THREAD_LAST_ERROR.with(|cell| cell.borrow().clone())
}

/// 有効なハンドル（`FandheStore` 等）使用時のエラー詳細を保持する per-handle ストレージ。
///
/// S2 の `FandheStore` がフィールドとして埋め込む前提の契約。「有効ハンドル使用時のエラーは
/// ここへ、無効ハンドル使用時はスレッドローカルへ」という書き分け（use-after-free 回避、
/// 設計文書 §3.1 所有権・寿命規約）の実体側の受け皿となる。
///
/// `Mutex` を採用し `Sync` を自己保証する（`ffi` 層が単一ミューテックスで直列化する前提に
/// 依存しない防御的設計）。
#[derive(Debug, Default)]
pub struct ErrorSlot(Mutex<Option<String>>);

impl ErrorSlot {
    /// 空の `ErrorSlot` を作る。
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// エラーメッセージを上書き保存する（[`MAX_ERROR_MESSAGE_LEN`] で切り詰め）。
    ///
    /// 内部 `Mutex` がポイズニングされていた場合でも panic せず、`into_inner` で中身を
    /// 回収して処理を継続する（[`message`](Self::message) / [`clear`](Self::clear) と同じ
    /// 回復パターン）。本 crate は per-handle 操作を単一ミューテックスで直列化する設計
    /// （設計文書 §3.1）のため、通常この状態には到達しないが、到達した場合でも panic を
    /// 伝播させない防御的実装とする。
    pub fn record(&self, message: String) {
        let message = truncate_message(message);
        let mut guard = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(message);
    }

    /// 直近のエラーメッセージを読み取る（非破壊。値を消費しない）。
    pub fn message(&self) -> Option<String> {
        let guard = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.clone()
    }

    /// 保持しているエラーメッセージを消去する。
    pub fn clear(&self) {
        let mut guard = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = None;
    }
}

/// panic ペイロード（`&str` / `String` の場合のみ抽出、それ以外は定型文）を人間可読な
/// メッセージへ変換する。
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic に捕捉不能なペイロードが含まれていました".to_string()
    }
}

/// `extern "C"` エクスポート関数の本体 `f` を `catch_unwind` で包み、panic が FFI 境界を
/// 越えて Swift 側の未定義動作を引き起こさないようにする（`ffi` モジュールから呼ばれる、
/// 設計文書 §3.3「エラー伝播」3.）。
///
/// panic を捕捉した場合は [`FandheErrorCode::PanicCaught`] の `i32` 値を返し、panic
/// ペイロードから抽出したメッセージを [`set_thread_last_error`] へ格納する。呼び出し元が
/// 有効なハンドルを扱っている最中に panic した場合でも、実体（per-handle ストレージ）は
/// panic 発生時点で整合性が保証できないため、あえてスレッドローカル側へ書く
/// （ハンドル非依存時・無効ハンドル時と同じ経路を再利用する設計判断。ffi 層の単一
/// ミューテックスによる直列化を前提に `AssertUnwindSafe` を採用する。panic 後は状態を
/// 再利用せずエラー返却のみを行う運用のため、キャプチャした状態への再アクセスは発生しない）。
///
/// `catch_unwind` 自体は safe API のため、本関数は `unsafe` を使わずに実装できる。
pub fn catch_panic<F>(f: F) -> i32
where
    F: FnOnce() -> i32,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(code) => code,
        Err(payload) => {
            set_thread_last_error(panic_message(&*payload));
            FandheErrorCode::PanicCaught.as_i32()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    /// 各エラーコードの `i32` 値が安定していること（`Success` は 0、他は負値かつ相互に異なる）。
    #[test]
    fn error_codes_have_stable_and_distinct_i32_values() {
        assert_eq!(FandheErrorCode::Success.as_i32(), 0);

        let codes = [
            FandheErrorCode::NullPointer,
            FandheErrorCode::InvalidHandle,
            FandheErrorCode::PayloadTooLarge,
            FandheErrorCode::JsonError,
            FandheErrorCode::PanicCaught,
            FandheErrorCode::Internal,
        ];
        for code in codes {
            assert!(code.as_i32() < 0, "{code:?} は負値であるべき");
        }

        let mut values: Vec<i32> = codes.iter().map(|c| c.as_i32()).collect();
        let original_len = values.len();
        values.sort_unstable();
        values.dedup();
        assert_eq!(values.len(), original_len, "エラーコードの値が重複している");
    }

    /// `CodecError::PayloadTooLarge` / `CodecError::Json` が正しいエラーコードへ写像されること。
    #[test]
    fn from_codec_error_maps_variants_correctly() {
        let payload_too_large = CodecError::PayloadTooLarge { len: 100, max: 10 };
        assert_eq!(
            FandheErrorCode::from(&payload_too_large),
            FandheErrorCode::PayloadTooLarge
        );

        let json_err =
            CodecError::Json(serde_json::from_str::<serde_json::Value>("{not json").unwrap_err());
        assert_eq!(FandheErrorCode::from(&json_err), FandheErrorCode::JsonError);
    }

    /// スレッドローカル最終エラー: set → 非破壊 get（2 回読んでも同値）、上書き動作。
    #[test]
    fn thread_last_error_set_and_non_destructive_get() {
        assert_eq!(thread_last_error(), None);

        set_thread_last_error("最初のエラー".to_string());
        assert_eq!(thread_last_error(), Some("最初のエラー".to_string()));
        // 非破壊であることの確認（2 回読んでも同値）。
        assert_eq!(thread_last_error(), Some("最初のエラー".to_string()));

        set_thread_last_error("上書きされたエラー".to_string());
        assert_eq!(thread_last_error(), Some("上書きされたエラー".to_string()));
    }

    /// スレッドローカル最終エラーが他スレッドと共有されないこと。
    #[test]
    fn thread_last_error_is_not_shared_across_threads() {
        set_thread_last_error("メインスレッドのエラー".to_string());

        let handle = std::thread::spawn(thread_last_error);
        let other_thread_value = handle.join().expect("スレッド join に成功するはず");

        assert_eq!(other_thread_value, None);
        // 呼び出し元スレッドの値は影響を受けない。
        assert_eq!(
            thread_last_error(),
            Some("メインスレッドのエラー".to_string())
        );
    }

    /// `ErrorSlot`: record → message → clear、上書き動作。
    #[test]
    fn error_slot_record_message_and_clear() {
        let slot = ErrorSlot::new();
        assert_eq!(slot.message(), None);

        slot.record("エラー詳細".to_string());
        assert_eq!(slot.message(), Some("エラー詳細".to_string()));
        // 非破壊読み取りであること。
        assert_eq!(slot.message(), Some("エラー詳細".to_string()));

        slot.record("上書きされた詳細".to_string());
        assert_eq!(slot.message(), Some("上書きされた詳細".to_string()));

        slot.clear();
        assert_eq!(slot.message(), None);
    }

    /// `catch_panic`: 正常系は戻り値をそのまま素通しする。
    #[test]
    fn catch_panic_passes_through_normal_return_value() {
        let result = catch_panic(|| 42);
        assert_eq!(result, 42);
    }

    /// `catch_panic`: panic 系は `PanicCaught` を返し、`&str` ペイロードのメッセージを
    /// スレッドローカル最終エラーへ格納する。
    #[test]
    fn catch_panic_catches_str_panic_and_records_message() {
        let result = catch_panic(|| panic!("固定文字列パニック"));
        assert_eq!(result, FandheErrorCode::PanicCaught.as_i32());
        assert_eq!(thread_last_error(), Some("固定文字列パニック".to_string()));
    }

    /// `catch_panic`: `panic!("{}", ...)` 由来（`String` ペイロード）のメッセージも抽出できる。
    #[test]
    fn catch_panic_catches_string_panic_and_records_message() {
        let detail = "動的生成メッセージ".to_string();
        let result = catch_panic(|| panic!("{}", detail));
        assert_eq!(result, FandheErrorCode::PanicCaught.as_i32());
        assert_eq!(thread_last_error(), Some("動的生成メッセージ".to_string()));
    }

    /// メッセージ切り詰め: 上限超過時に上限以内へ収まり、UTF-8 境界を壊さないこと。
    #[test]
    fn message_truncation_respects_length_limit_and_utf8_boundary() {
        // マルチバイト文字（3 バイト/文字の「あ」）で上限をまたぐ長さの文字列を作る。
        let repeated = "あ".repeat(MAX_ERROR_MESSAGE_LEN); // 明らかに上限を超える長さ
        let slot = ErrorSlot::new();
        slot.record(repeated);

        let stored = slot.message().expect("メッセージが格納されているはず");
        assert!(stored.len() <= MAX_ERROR_MESSAGE_LEN);
        // 切り詰め後も有効な UTF-8 文字列であること（String 型である時点で保証されるが、
        // 文字境界破壊がないことの意図を明示するため文字列として読めることを確認する）。
        assert!(stored.chars().all(|c| c == 'あ'));
    }
}
