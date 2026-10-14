//! `extern "C"` エクスポート層（TASK-3.2-S2 / #59、REQ-3）。
//!
//! `docs/architecture/swiftui-bridge-architecture.md` §3.1〜§3.3 の凍結シグネチャの実体。
//! `platforms/apple/FandheSwiftUIBridge/Sources/CFandheBridge/include/fandhe_bridge.h`
//! （S5 / #62 が手書きした C ヘッダ）が宣言する 7 関数・2 型と 1 対 1 対応させる
//! （関数名・引数・戻り値の型を完全一致させること。ヘッダ側が SSOT に対する手書きの
//! 写しであり、本モジュールが実体を提供する）。
//!
//! # unsafe 境界
//!
//! クレートルートは `#![deny(unsafe_code)]`（forbid ではなく deny を採用する理由は
//! `crate` ルートの doc コメント参照）。本モジュールのみ `#[allow(unsafe_code)]` で
//! 例外的に `unsafe` を許可し、生ポインタの参照外し・`Box::from_raw` 等をここに閉じ込める
//! （`codec` / `error` モジュールは `unsafe` を一切含まない安全コードのまま維持する）。
//! 各 `unsafe` ブロックに `// SAFETY:` コメントを付す（coding-rust.md 準拠）。
//!
//! # ハンドル安全性（所有権・寿命規約）
//!
//! 設計文書 §3.1「所有権・寿命規約」が定める「二重解放・解放後使用対策」を、生存中
//! ハンドルのアドレス集合を保持する外部レジストリ（[`registry`]）で実装する。
//! ハンドルを受け取る各エクスポート関数は、ハンドルが指す実体を参照する前に
//! このレジストリへの**メンバーシップ判定のみ**（アドレス値の比較）で有効性を検査し、
//! 無効ならハンドル実体を一切読まずにエラーを返す。[`fandhe_store_free`] は
//! 「レジストリからの除去」を実体の解放より**先に**行い、単一ミューテックスで
//! 直列化することで、除去直後の再呼び出し（二重解放・use-after-free）を検出する。
//!
//! # エラー伝播
//!
//! 各エクスポート関数の本体は panic が FFI 境界を越えないよう捕捉する（`i32` を返す
//! 関数は [`crate::error::catch_panic`] を利用し、それ以外の戻り値型（`FandheStore`
//! ハンドルのポインタ・[`FandheBuffer`]）を返す関数は本モジュール内の
//! [`catch_unwind_or`] を利用する）。有効なハンドル使用時のエラー詳細は
//! `FandheStore::errors`（per-handle の [`crate::error::ErrorSlot`]）へ、
//! ハンドル非依存・無効ハンドル使用時の詳細はスレッドローカル
//! （[`crate::error::set_thread_last_error`]）へ書く使い分けを徹底する
//! （`crate::error` モジュール冒頭のコメント・設計文書 §3.3 参照）。
//! [`fandhe_last_error_message`] のみ、この 2 経路への**書き込みを一切行わない**
//! 純粋な読み取り専用アクセサとして実装する（設計文書 §3.3 個別記述）。

#![allow(unsafe_code)]

mod buffer;

pub use buffer::FandheBuffer;

use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, OnceLock};

use fandhe_core::{sample_screen, SampleState, Screen, StateStore};

use crate::codec::{decode_event, encode_snapshot};
use crate::error::{
    catch_panic, set_thread_last_error, thread_last_error, ErrorSlot, FandheErrorCode,
};

/// `FandheStore` ハンドルの実体。C 側（`fandhe_bridge.h`）は不透明ハンドルとしてのみ
/// 扱うため、内部レイアウトは ABI 契約の対象外（フィールド追加・並び替えは自由）。
///
/// 設計文書 §3.1「ジェネリクスの取り扱い（不透明ハンドルパターン）」に従い、初期実装は
/// `fandhe_core::sample_screen` / `SampleState` で単相化した `StateStore<SampleState>` の
/// みを公開する。任意のアプリ状態への一般化は本クレートの対象外（設計文書「4. 再考条件」）。
pub struct FandheStore {
    /// 状態と再描画バージョンを保持する（`fandhe_core::StateStore`）。
    store: StateStore<SampleState>,
    /// 走査対象の画面定義。状態 `S` に依存しないため、ハンドル生成時に一度だけ構築する。
    screen: Screen<SampleState>,
    /// このハンドルに紐づく直近のエラー詳細（有効ハンドル使用時の書き込み先、
    /// `crate::error` モジュール冒頭のコメント参照）。
    errors: ErrorSlot,
}

/// 生存中ハンドルのアドレス集合を保持する外部レジストリを返す（モジュール冒頭の
/// コメント「ハンドル安全性」参照）。プロセス内で単一のレジストリを共有する。
fn registry() -> &'static Mutex<HashSet<usize>> {
    static REGISTRY: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// `ptr` をレジストリへ登録する（`fandhe_store_new` 成功時にのみ呼ぶ）。
fn register_handle(ptr: *mut FandheStore) {
    let mut guard = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.insert(ptr as usize);
}

/// `ptr` が現在レジストリに登録されている（＝生存中の有効なハンドルである）かを判定する。
///
/// アドレス値の比較のみを行い、`ptr` が指す実体を一切読まない（無効ハンドル・解放済み
/// ハンドルに対して安全に呼べる、設計文書 §3.1 参照）。
fn is_registered(ptr: *const FandheStore) -> bool {
    let guard = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.contains(&(ptr as usize))
}

/// `ptr` をレジストリから除去する。除去に成功した（＝直前まで生存中だった）場合のみ
/// `true` を返す。`fandhe_store_free` が実体解放の**前**に呼ぶことで、除去と解放の
/// 間に他呼び出しが割り込まない単一ミューテックスの直列化を実現する。
fn unregister_handle(ptr: *mut FandheStore) -> bool {
    let mut guard = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.remove(&(ptr as usize))
}

/// panic ペイロード（`&str` / `String` の場合のみ抽出、それ以外は定型文）を
/// 人間可読なメッセージへ変換する。`crate::error` の同名ロジックは非公開のため、
/// `i32` 以外の戻り値を持つ本モジュールのエクスポート関数向けに同等の変換をここへ
/// 複製する（`catch_panic` は `i32` 専用のため転用できない）。
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic に捕捉不能なペイロードが含まれていました".to_string()
    }
}

/// `f` を `catch_unwind` で包み、panic 時は `on_panic` で既定値を得る（`i32` 以外の
/// 戻り値型向けの `catch_panic`（`crate::error`）相当品。`f` は panic 安全性を
/// `AssertUnwindSafe` で仮定する。単一スレッド・単一呼び出しの FFI 境界関数の
/// 本体として使う想定で、panic 発生後に状態を再利用しない運用のため許容する
/// （`crate::error::catch_panic` の doc コメントと同じ設計判断）。
fn catch_unwind_or<T>(on_panic: impl FnOnce() -> T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            set_thread_last_error(panic_message(&*payload));
            on_panic()
        }
    }
}

/// ビルド生成物の ABI 世代を返す（設計文書 §3.1「命名・ABI 規約」）。
///
/// Swift 側（`FandheBridgeContract.expectedAbiVersion`）はリンク時にこの値と期待値の
/// 一致を検査し、不整合バイナリの誤リンクによる未定義動作を実行時に検出する（A08）。
/// スナップショット JSON のスキーマ世代（`codec::SNAPSHOT_SCHEMA_VERSION`）と同じ
/// `1` から開始する。
#[no_mangle]
pub extern "C" fn fandhe_abi_version() -> u32 {
    1
}

/// `FandheStore` ハンドルを新規生成する。失敗時（内部初期化中の panic 捕捉）は
/// `NULL` を返す。この時点ではハンドルが存在しないため、詳細エラーは
/// スレッドローカル経路（`fandhe_last_error_message(NULL)`）へ書く。
#[no_mangle]
pub extern "C" fn fandhe_store_new() -> *mut FandheStore {
    catch_unwind_or(std::ptr::null_mut, || {
        let handle = FandheStore {
            store: StateStore::new(SampleState::default()),
            screen: sample_screen(),
            errors: ErrorSlot::new(),
        };
        let ptr = Box::into_raw(Box::new(handle));
        register_handle(ptr);
        ptr
    })
}

/// `store` を解放する。戻り値は外部レジストリへのメンバーシップ判定結果
/// （`0`: 成功、負値: 既に解放済み／無効なハンドル）。
///
/// 判定（レジストリからの除去）→ 実体の解放、の順で単一ミューテックスにより
/// 直列化する（モジュール冒頭のコメント「ハンドル安全性」参照。二重解放対策）。
///
/// # Safety
///
/// `store` は `NULL`、または [`fandhe_store_new`] が返した（かつ未解放の）値で
/// なければならない。本関数はハンドル実体を参照する前にレジストリへの
/// メンバーシップ検査を行うため、`NULL`・解放済み・元から不正なポインタは
/// 実体を読まず安全にエラーとして扱う（ただし C ABI の呼び出し規約自体は
/// 呼び出し元が保証する必要がある）。
#[no_mangle]
pub unsafe extern "C" fn fandhe_store_free(store: *mut FandheStore) -> i32 {
    catch_panic(|| {
        if store.is_null() {
            set_thread_last_error("fandhe_store_free: store が NULL です".to_string());
            return FandheErrorCode::NullPointer.as_i32();
        }
        if !unregister_handle(store) {
            set_thread_last_error(
                "fandhe_store_free: 既に解放済み、または無効なハンドルです".to_string(),
            );
            return FandheErrorCode::InvalidHandle.as_i32();
        }
        // SAFETY: 直前の `unregister_handle` がレジストリからの除去に成功しており
        // （＝呼び出し時点でこのハンドルは生存中で、かつ除去はミューテックスで
        // 直列化されている）、この `Box` はここで一度だけ復元・解放される
        // （二重解放不可。モジュール冒頭のコメント参照）。
        unsafe {
            drop(Box::from_raw(store));
        }
        FandheErrorCode::Success.as_i32()
    })
}

/// 現在の描画スナップショットを JSON（UTF-8 バイト列）として取得する。
///
/// 無効ハンドル（`NULL` 含む）の場合は実体を参照せず `NULL` ポインタ・長さ 0 の
/// バッファを返す（設計文書 §3.3 `fandhe_render_snapshot` 個別記述）。戻り値は
/// 必ず [`fandhe_buffer_free`] で解放すること。
///
/// # Safety
///
/// `store` は `NULL`、または [`fandhe_store_new`] が返した（かつ未解放の）値で
/// なければならない（`fandhe_store_free` の doc コメントと同じ理由で、無効な
/// 値でも実体を読まず安全にエラー扱いされる）。
#[no_mangle]
pub unsafe extern "C" fn fandhe_render_snapshot(store: *const FandheStore) -> FandheBuffer {
    catch_unwind_or(FandheBuffer::empty, || {
        if store.is_null() {
            set_thread_last_error("fandhe_render_snapshot: store が NULL です".to_string());
            return FandheBuffer::empty();
        }
        if !is_registered(store) {
            set_thread_last_error("fandhe_render_snapshot: 無効なハンドルです".to_string());
            return FandheBuffer::empty();
        }
        // SAFETY: 直前の `is_registered` によるメンバーシップ検査に成功しており、
        // `store` は現在生存中の有効なハンドルであることを確認済み。
        let handle = unsafe { &*store };
        match encode_snapshot(&handle.screen, handle.store.state(), handle.store.version()) {
            Ok(bytes) => FandheBuffer::from_vec(bytes),
            Err(err) => {
                // 有効ハンドル使用時のエラー詳細は per-handle ストレージへ（設計文書
                // §3.1 所有権・寿命規約、`crate::error` モジュール冒頭のコメント参照）。
                handle.errors.record(err.to_string());
                FandheBuffer::empty()
            }
        }
    })
}

/// イベント（JSON バイト列）を `store` へディスパッチする。戻り値は再描画要否・
/// エラーコードを表す `i32`（`0` 以上: 成功〔`1`: 状態が更新され再描画が必要、
/// `0`: 一致するハンドラがなく状態は変化していない〕、負値: エラー種別）。
///
/// `event_json` / `len` は本呼び出し中のみ有効とみなし、呼び出し完了後に
/// Rust 側が保持することはない（設計文書 §3.1「Swift → Rust のポインタ」）。
///
/// # Safety
///
/// `store` は `NULL`、または [`fandhe_store_new`] が返した（かつ未解放の）値で
/// なければならない。`event_json` は `NULL`、または少なくとも `len` バイト分
/// 有効な読み取り可能メモリを指し、本呼び出しの間だけ有効であればよい
/// （設計文書 §3.1「Swift → Rust のポインタ」）。
#[no_mangle]
pub unsafe extern "C" fn fandhe_dispatch_event(
    store: *mut FandheStore,
    event_json: *const u8,
    len: usize,
) -> i32 {
    catch_panic(|| {
        if store.is_null() {
            set_thread_last_error("fandhe_dispatch_event: store が NULL です".to_string());
            return FandheErrorCode::NullPointer.as_i32();
        }
        if !is_registered(store) {
            set_thread_last_error("fandhe_dispatch_event: 無効なハンドルです".to_string());
            return FandheErrorCode::InvalidHandle.as_i32();
        }
        if event_json.is_null() {
            set_thread_last_error("fandhe_dispatch_event: event_json が NULL です".to_string());
            return FandheErrorCode::NullPointer.as_i32();
        }
        // SAFETY: `event_json` / `len` は呼び出し元（Swift 側）が本呼び出し中のみ
        // 有効であることを保証する契約（設計文書 §3.1「Swift → Rust のポインタ」）。
        // 本関数はこのスライスを呼び出し完了後に一切保持しない。
        let bytes = unsafe { std::slice::from_raw_parts(event_json, len) };
        // SAFETY: 直前の `is_registered` によるメンバーシップ検査に成功しており、
        // `store` は現在生存中の有効なハンドルであることを確認済み。
        let handle = unsafe { &mut *store };
        match decode_event(bytes) {
            Ok(event) => {
                let handled = handle.store.dispatch(&handle.screen, &event);
                i32::from(handled)
            }
            Err(err) => {
                let code = FandheErrorCode::from(&err);
                // 有効ハンドル使用時のエラー詳細は per-handle ストレージへ。
                handle.errors.record(err.to_string());
                code.as_i32()
            }
        }
    })
}

/// [`fandhe_render_snapshot`] / [`fandhe_last_error_message`] が返した [`FandheBuffer`]
/// を解放する。Rust 確保バッファは必ず本関数でのみ解放し、Swift 側で `free()` 等を
/// 直接呼ばない（設計文書 §3.1「Rust 確保バッファ」）。
#[no_mangle]
pub extern "C" fn fandhe_buffer_free(buffer: FandheBuffer) {
    catch_unwind_or(
        || {
            // 解放処理自体で panic した場合、`buffer` は既に `catch_unwind` の
            // `AssertUnwindSafe` クロージャへ move 済みで復元できないため、
            // ここでは追加の解放を試みず終了する（二重解放を避ける安全側判断）。
        },
        move || {
            // SAFETY: `buffer` は `fandhe_render_snapshot` / `fandhe_last_error_message`
            // が返した値そのものであるという Swift 側との契約（ヘッダの doc コメント）
            // により、`FandheBuffer::from_vec` が確保したバッファか空バッファのいずれか。
            unsafe {
                buffer.free();
            }
        },
    )
}

/// 直近のエラーの詳細メッセージ（UTF-8 バイト列）を取得する。
///
/// 参照先の優先順位（設計文書 §3.3 `fandhe_last_error_message` 個別記述）:
/// (1) `store` が `NULL` の場合、(2) メンバーシップ検査に失敗した無効ハンドルの場合は
/// スレッドローカルの最終エラーを参照する。(3) 検査に成功した有効なハンドルの場合のみ
/// per-handle ストレージを参照する。
///
/// **本関数は純粋な読み取り専用アクセサであり、いずれの経路にも一切書き込まない**
/// （自身が「無効ハンドル」等のエラーを新規に書き込むと、直前の呼び出しが残した
/// 詳細メッセージを取得前に上書き・破壊してしまうため）。エラーが存在しない場合は
/// `NULL` ポインタ・長さ 0 のバッファを返す（エラーではない）。戻り値は必ず
/// [`fandhe_buffer_free`] で解放すること。
///
/// # Safety
///
/// `store` は `NULL`、または [`fandhe_store_new`] が返した値でなければならない
/// （解放済み・元から不正なポインタであっても実体を読まず安全にフォールバックする、
/// `fandhe_store_free` の doc コメントと同じ理由）。
#[no_mangle]
pub unsafe extern "C" fn fandhe_last_error_message(store: *const FandheStore) -> FandheBuffer {
    // panic 時も本関数は書き込みを行わない契約のため、`set_thread_last_error` を
    // 呼ばず空バッファへ落とす（`catch_unwind_or` ではなく生の `catch_unwind` を使う）。
    catch_unwind(AssertUnwindSafe(|| {
        let valid_handle = !store.is_null() && is_registered(store);
        if !valid_handle {
            return thread_last_error()
                .map(|message| FandheBuffer::from_vec(message.into_bytes()))
                .unwrap_or_else(FandheBuffer::empty);
        }
        // SAFETY: 直前に `is_registered` でメンバーシップ検査に成功しており、
        // `store` は現在生存中の有効なハンドルであることを確認済み。
        let handle = unsafe { &*store };
        handle
            .errors
            .message()
            .map(|message| FandheBuffer::from_vec(message.into_bytes()))
            .unwrap_or_else(FandheBuffer::empty)
    }))
    .unwrap_or_else(|_| FandheBuffer::empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    /// `fandhe_store_new` を呼ぶテストを直列化する排他ロック。
    ///
    /// [`registry`] はプロセス内で共有され、生存中ハンドルの**アドレス集合**のみで
    /// 有効性を判定する（モジュール冒頭のコメント「ハンドル安全性」参照）。
    /// このため、あるテストが `fandhe_store_free` で解放したアドレスを、並列に走る
    /// 別テストの `fandhe_store_new` が偶然再取得すると、解放済みポインタがレジストリ
    /// 上「有効」に見えてしまう（ABA 問題。設計文書
    /// `docs/architecture/swiftui-bridge-architecture.md` §3.1 に記載済みの既知の
    /// 残存リスクで、本実装コードの契約どおりの挙動）。
    /// 二重解放・use-after-free の検出を検証するテスト（[`double_free_is_rejected`] /
    /// [`last_error_message_falls_back_to_thread_local_for_invalid_handle`]）がこの
    /// 競合で偽陰性になるのを防ぐため、ハンドルを生成する全テストをこのロックで
    /// 直列化する（#965）。新規依存（`serial_test` 等）は導入せず std のみで足りる。
    ///
    /// 一部テストの assert 失敗によるロックの poison は、`registry()` と同じ流儀で
    /// 無視して回復する（1 テストの失敗が他テストを `PoisonError` で巻き込まないため）。
    fn serialize_handle_tests() -> MutexGuard<'static, ()> {
        static HANDLE_TESTS: Mutex<()> = Mutex::new(());
        HANDLE_TESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `fandhe_abi_version` はビルド固定の世代番号を返す。
    #[test]
    fn abi_version_is_stable() {
        assert_eq!(fandhe_abi_version(), 1);
    }

    /// 正常系: 生成 → スナップショット取得 → イベントディスパッチ → 解放が一巡する。
    ///
    /// `fandhe_store_new` を呼ぶため [`serialize_handle_tests`] で直列化する（#965）。
    #[test]
    fn store_lifecycle_happy_path() {
        let _guard = serialize_handle_tests();
        let store = fandhe_store_new();
        assert!(!store.is_null());

        // SAFETY: `store` は直前の `fandhe_store_new` が返した未解放の値そのもの。
        let snapshot = unsafe { fandhe_render_snapshot(store) };
        assert!(!snapshot.ptr.is_null());
        // SAFETY: `snapshot.ptr` は `fandhe_render_snapshot` が確保したバッファの
        // 先頭を指し、`snapshot.len` バイト分有効であることをその契約が保証する。
        let json = unsafe { std::slice::from_raw_parts(snapshot.ptr, snapshot.len) };
        assert!(std::str::from_utf8(json).is_ok());
        fandhe_buffer_free(snapshot);

        let event = r#"{"type":"button_clicked","label":"カウントを増やす"}"#.as_bytes();
        // SAFETY: `store` は未解放の有効なハンドル。`event` はこの呼び出し中のみ
        // 参照される、呼び出し元が所有するスタック上のバイト列。
        let result = unsafe { fandhe_dispatch_event(store, event.as_ptr(), event.len()) };
        assert_eq!(result, 1, "状態が更新され再描画が必要なはず");

        // SAFETY: `store` は未解放の有効なハンドル。本呼び出しでちょうど 1 回解放する。
        let free_result = unsafe { fandhe_store_free(store) };
        assert_eq!(free_result, FandheErrorCode::Success.as_i32());
    }

    /// `NULL` ハンドルは各関数で安全にエラー扱いされ、実体を参照しない。
    #[test]
    fn null_store_is_rejected_everywhere() {
        // SAFETY: `NULL` は各関数の契約上、実体を参照せず安全にエラー扱いされる。
        unsafe {
            assert!(fandhe_render_snapshot(std::ptr::null()).ptr.is_null());
            assert_eq!(
                fandhe_dispatch_event(std::ptr::null_mut(), std::ptr::null(), 0),
                FandheErrorCode::NullPointer.as_i32()
            );
            assert_eq!(
                fandhe_store_free(std::ptr::null_mut()),
                FandheErrorCode::NullPointer.as_i32()
            );
        }
    }

    /// 解放後のハンドル（use-after-free）・二重解放が検出されエラーになる。
    ///
    /// `fandhe_store_new` を呼ぶため [`serialize_handle_tests`] で直列化する（#965、
    /// 直列化しないと ABA により解放済みハンドルが他テストの生存ハンドルへ偽陽性で
    /// 一致し、本テストの assert が誤って落ちることがある）。
    #[test]
    fn double_free_is_rejected() {
        let _guard = serialize_handle_tests();
        let store = fandhe_store_new();
        // SAFETY: `store` は直前に生成された未解放の有効なハンドル。
        let free_result = unsafe { fandhe_store_free(store) };
        assert_eq!(free_result, FandheErrorCode::Success.as_i32());

        // SAFETY: 解放済みハンドルの再利用は各関数の契約上、実体を読まず
        // 安全にエラー扱いされる（本テストが検証する対象そのもの）。
        unsafe {
            // 解放済みハンドルへの再利用はいずれも無効ハンドルとして扱われる。
            assert_eq!(
                fandhe_store_free(store),
                FandheErrorCode::InvalidHandle.as_i32()
            );
            assert!(fandhe_render_snapshot(store).ptr.is_null());
        }
    }

    /// 不正な JSON イベントは panic せずエラーコードを返し、詳細メッセージが
    /// per-handle ストレージ経由で取得できる。
    ///
    /// `fandhe_store_new` を呼ぶため [`serialize_handle_tests`] で直列化する（#965）。
    #[test]
    fn malformed_event_json_records_handle_scoped_error() {
        let _guard = serialize_handle_tests();
        let store = fandhe_store_new();

        let bad_event = b"{not valid json";
        // SAFETY: `store` は未解放の有効なハンドル。`bad_event` はこの呼び出し中のみ
        // 参照される、呼び出し元が所有するスタック上のバイト列。
        let result = unsafe { fandhe_dispatch_event(store, bad_event.as_ptr(), bad_event.len()) };
        assert!(result < 0, "不正な JSON はエラーコード（負値）を返すはず");

        // SAFETY: `store` は未解放の有効なハンドル。
        let message = unsafe { fandhe_last_error_message(store) };
        assert!(!message.ptr.is_null());
        // SAFETY: `message` は直前の `fandhe_last_error_message` が返した値そのもの。
        unsafe {
            message.free();
        }

        // SAFETY: `store` は未解放の有効なハンドル。本呼び出しでちょうど 1 回解放する。
        let free_result = unsafe { fandhe_store_free(store) };
        assert_eq!(free_result, FandheErrorCode::Success.as_i32());
    }

    /// 無効ハンドルに対する `fandhe_last_error_message` はスレッドローカル経路へ
    /// フォールバックする（per-handle ストレージ〔解放済みかもしれないメモリ〕を
    /// 参照しない）。
    ///
    /// `fandhe_store_new` を呼ぶため [`serialize_handle_tests`] で直列化する（#965、
    /// 直列化しないと ABA により解放済みハンドルへの操作が他テストの生存ハンドルを
    /// 参照してしまい、スレッドローカルへ書き込まれず本テストの assert が誤って
    /// 落ちることがある）。
    #[test]
    fn last_error_message_falls_back_to_thread_local_for_invalid_handle() {
        let _guard = serialize_handle_tests();
        let store = fandhe_store_new();
        // SAFETY: `store` は直前に生成された未解放の有効なハンドル。
        let free_result = unsafe { fandhe_store_free(store) };
        assert_eq!(free_result, FandheErrorCode::Success.as_i32());

        // SAFETY: 解放済みハンドルへの操作は契約上、実体を読まずスレッドローカルへ
        // エラーを書き込む（本テストが検証する対象そのもの）。
        let _ = unsafe { fandhe_render_snapshot(store) };

        // SAFETY: 解放済みハンドルでもメンバーシップ検査失敗によりスレッドローカル
        // 経路へ安全にフォールバックする。
        let message = unsafe { fandhe_last_error_message(store) };
        assert!(
            !message.ptr.is_null(),
            "解放済みハンドルの直近エラーがスレッドローカル経由で取得できるはず"
        );
        // SAFETY: `message` は直前の `fandhe_last_error_message` が返した値そのもの。
        unsafe {
            message.free();
        }
    }
}
