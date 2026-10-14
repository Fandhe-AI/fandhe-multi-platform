//! Rust 側で確保した JSON バイト列（描画スナップショット・エラー詳細メッセージ）を
//! Swift 側へ受け渡す `#[repr(C)]` の集約型（TASK-3.2-S2 / #59、REQ-3）。
//!
//! `docs/architecture/swiftui-bridge-architecture.md` §3.1「所有権・寿命規約」・§3.3
//! の凍結シグネチャに対応する。所有権は Rust 側に残り、Swift 側は
//! [`crate::ffi::fandhe_buffer_free`] を呼ぶまで解放してはならない契約（Swift 側で
//! `free()` を直接呼ばない）。C ヘッダ側の対応する宣言は
//! `platforms/apple/FandheSwiftUIBridge/Sources/CFandheBridge/include/fandhe_bridge.h` の
//! `FandheBuffer` を参照（フィールドレイアウト・意味を完全一致させること）。
//!
//! 本モジュールは `ffi` モジュール配下（`#![allow(unsafe_code)]` の適用範囲、
//! `crate::ffi` モジュール冒頭のコメント参照）であり、`Box<[u8]>` の生ポインタ化・
//! 復元にのみ `unsafe` を用いる。

/// Rust 確保のバイト列を Swift 側へ値渡しする ABI 表現。
///
/// バイト列が存在しない場合（エラー詳細なし・エンコード失敗時のフォールバック等）は
/// `ptr` が `NULL`・`len` が `0` の空バッファ（[`FandheBuffer::empty`]）を返す契約とする。
#[repr(C)]
pub struct FandheBuffer {
    /// バイト列の先頭ポインタ。バイト列が存在しない場合は `NULL`。
    pub ptr: *const u8,
    /// バイト列の長さ（バイト数）。`ptr` が `NULL` の場合は `0`。
    pub len: usize,
}

impl FandheBuffer {
    /// バイト列が存在しないことを表す空バッファ（`ptr` が `NULL`・`len` が `0`）を作る。
    pub fn empty() -> Self {
        Self {
            ptr: std::ptr::null(),
            len: 0,
        }
    }

    /// `bytes` の所有権を `FandheBuffer` へ移し、Swift 側へ値で返せる形にする。
    ///
    /// 空の `Vec` の場合は確保済みバッファを作らず [`Self::empty`] を返す（`ptr` を
    /// 常に有効なヒープポインタにする必要をなくし、`fandhe_buffer_free` 側の
    /// 「`NULL` なら何もしない」分岐と対称にする）。
    ///
    /// 返した `FandheBuffer` は必ず [`crate::ffi::fandhe_buffer_free`] で解放すること
    /// （二重解放・リーク対策、設計文書 §3.1 参照）。
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        if bytes.is_empty() {
            return Self::empty();
        }
        let boxed: Box<[u8]> = bytes.into_boxed_slice();
        let len = boxed.len();
        // SAFETY: `Box::into_raw` で得たポインタは、対応する `Box::from_raw`
        // （`fandhe_buffer_free` 側）でのみ復元・解放される契約とする。ここでは
        // 所有権を放棄するのみで、以降このバイト列に Rust 側から触れない。
        let ptr = Box::into_raw(boxed) as *const u8;
        Self { ptr, len }
    }

    /// [`Self::from_vec`] が生成したバッファを解放する。
    ///
    /// `ptr` が `NULL`（[`Self::empty`] 由来）の場合は何もしない。それ以外は
    /// `from_vec` が `Box::into_raw` した `Box<[u8]>` として復元し、スコープ末尾で
    /// `drop` する（Swift 側が `free()` 等を直接呼ばない契約の実体）。
    ///
    /// # Safety
    ///
    /// `self` は [`Self::from_vec`] または [`Self::empty`] が返した値そのものであり、
    /// 同一バッファに対して本関数を二度呼んではならない（呼び出し元 `ffi` 層の
    /// エクスポート関数が Swift 側との「1 回のみ解放」契約を保証する）。
    pub unsafe fn free(self) {
        if self.ptr.is_null() {
            return;
        }
        // SAFETY: 呼び出し元契約（本関数の doc）により、`self.ptr`/`self.len` は
        // `from_vec` が確保した `Box<[u8]>` の生ポインタ・長さと一致する。
        unsafe {
            let slice = std::slice::from_raw_parts_mut(self.ptr as *mut u8, self.len);
            drop(Box::from_raw(slice as *mut [u8]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空の `Vec` からは `NULL` ポインタ・長さ 0 の空バッファが得られる。
    #[test]
    fn from_vec_empty_yields_null_buffer() {
        let buffer = FandheBuffer::from_vec(Vec::new());
        assert!(buffer.ptr.is_null());
        assert_eq!(buffer.len, 0);
    }

    /// 非空の `Vec` は中身を保持したバッファになり、`free` で二重解放にならず解放できる。
    #[test]
    fn from_vec_roundtrips_and_frees_without_double_free() {
        let bytes = vec![1u8, 2, 3, 4];
        let buffer = FandheBuffer::from_vec(bytes.clone());
        assert!(!buffer.ptr.is_null());
        assert_eq!(buffer.len, bytes.len());

        // SAFETY: `buffer` は直前の `from_vec` の戻り値そのものであり、本テスト内で
        // 一度だけ `free` を呼ぶ。
        unsafe {
            buffer.free();
        }
    }

    /// 空バッファ（`ptr` が `NULL`）に対する `free` は何もせず安全に完了する。
    #[test]
    fn free_on_empty_buffer_is_noop() {
        let buffer = FandheBuffer::empty();
        // SAFETY: 空バッファは解放対象のヒープ領域を持たないため、`free` は
        // 早期 return する no-op になる。
        unsafe {
            buffer.free();
        }
    }
}
