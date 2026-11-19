//! macOS 向け画面のスポイト実装（Issue #762）。`driver` の macOS 経路から呼ばれ、
//! `macos/fandhe_macos_eye_dropper.m`（`NSColorSampler`。`build.rs` が
//! コンパイルする）を `unsafe extern "C"` 1 関数（[`present`]）で呼ぶ。
//! `unsafe` はこの呼び出しと受け口に閉じ `// SAFETY:` で契約を明記する
//! （`fandhe-cap-contacts` の `apple.rs` と同型、coding-rust.md の FFI 例外規定準拠）。
//!
//! `.m` 側のハンドラから [`fandhe_rust_eye_dropper_outcome`] が呼ばれ、
//! `driver` の `bridge` へ結果を push する（pull 型。画面 effect と公開入口が同じ slot を共有する）。コード → 結果の写像は
//! `fandhe_device::eye_dropper::result_from_macos_code` に一本化する。
//!
//! 拾った色をログへ出力しない（security.md A09）。

// SAFETY: `macos/fandhe_macos_eye_dropper.m` が定義する C ABI。契約は `.m` ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_macos_eye_dropper_present(request_id: u64) -> i32;
}

/// スポイトの起動を依頼する。`request_id` は `bridge` の `SlotOwner` が払い出した ID。
/// 戻り値は「依頼を受理できたか」（`false` は呼び出し元が `Internal` として畳む）。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64) -> bool {
    // SAFETY: 引数は値渡しの `u64` のみでポインタを渡さない。`.m` 側は main queue へ
    // dispatch してから即座に 0 を返す契約。
    let code = unsafe { fandhe_macos_eye_dropper_present(request_id) };
    code == 0
}

/// `.m` 側のハンドラから呼ばれる受け口。ポインタを受けず、色は整数 `rgb` で渡る。
/// macOS は event loop 側が待機中に `request_repaint_after` を出すため repaint は呼ばない。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "C" fn fandhe_rust_eye_dropper_outcome(request_id: u64, code: i32, rgb: i32) {
    crate::driver::bridge::deliver(
        request_id,
        fandhe_device::eye_dropper::result_from_macos_code(code, rgb)
            .map_err(crate::error::EyeDropperError::from_device),
    );
}
