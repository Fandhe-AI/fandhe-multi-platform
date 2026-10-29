//! macOS / iOS 向け端末内での画像生成実装（Issue #975 macOS、#976 iOS）。
//! `device/image_generation.rs` の macOS 経路と iOS 経路（`ios_image_generation`）から呼ばれ、
//! `apple/fandhe_apple_image_generation.swift`（Image Playground の `ImagePlaygroundViewController`。
//! `build.rs` が feature `device` 有効時に swiftc で静的ライブラリ化）を `unsafe extern "C"` 1 関数で呼ぶ。
//! Swift からは受け口 2 本（[`on_begin`]・[`on_done`]）の関数ポインタ経由で戻る。`unsafe` はこの
//! FFI 宣言・呼び出し・受け口に閉じ、各所に `// SAFETY:` を書く（coding-rust.md の FFI 例外規定）。
//! `build.rs` のコンパイル条件・`device.rs` の mod cfg と同じ OS 集合に保つ。
//!
//! 契約の SSOT は `fandhe_device::image_generation`。戻りコード → 結果の写像の SSOT は
//! `outcome_from_apple_code`。期限・世代照合・二重完了の破棄・表示開始の記録は effect ドライバの
//! bridge が担い、ここは生成画面の表示依頼と結果の受け取りだけをする。
//! 画素はコピーしてから渡し、ポインタを保持しない。画素・プロンプト・OS のエラー文はログに出さない。

use std::sync::Arc;

use fandhe_device::image_generation::{
    outcome_from_apple_code, ImageGenerationFailure, ImageGenerator, MAX_SIDE, PROMPT,
};

use super::image_generation::{complete_from_os, try_begin_present};

// SAFETY: `apple/fandhe_apple_image_generation.swift` が `@_cdecl` で定義する C ABI（`build.rs` が
// macOS・iOS ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は swift 冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_image_generation_present(
        request_id: u64,
        max_side: u32,
        prompt: *const u8,
        prompt_len: usize,
        begin: extern "C" fn(u64) -> bool,
        done: extern "C" fn(u64, i32, *const u8, usize, u32, u32),
    ) -> i32;
}

/// Image Playground の生成画面で画像を得る実装。
pub(crate) struct AppleImageGenerator;

/// macOS・iOS 共用の `ImageGenerator` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn ImageGenerator> {
    Arc::new(AppleImageGenerator)
}

impl ImageGenerator for AppleImageGenerator {
    #[allow(unsafe_code)]
    fn generate(&self, request_id: u64) -> Result<(), ImageGenerationFailure> {
        // SAFETY: `PROMPT` は静的な UTF-8 文字列で、Swift は呼び出しの間だけ参照し String へコピーしてから
        // main queue へ回す（保持しない）。他の引数は値渡しのプリミティブと、このクレートが定義する
        // `extern "C"` 関数のポインタ（プロセスの寿命の間有効）。Swift は即時に戻り、`on_begin`・`on_done` を
        // 後から main queue で呼ぶ。0 以外の戻りコードのときは `on_done` を呼ばない契約。
        let code = unsafe {
            fandhe_apple_image_generation_present(
                request_id,
                MAX_SIDE,
                PROMPT.as_ptr(),
                PROMPT.len(),
                on_begin,
                on_done,
            )
        };
        if code == 0 {
            return Ok(());
        }
        // 即時の失敗コード。画素は無いので成功は写像が失敗にする。
        match outcome_from_apple_code(code, None, 0, 0) {
            Err(f) => Err(f),
            // 1（キャンセル）は即時には起きない契約。起きたら嘘の成功にせず内部エラーにする。
            Ok(_) => Err(ImageGenerationFailure::Internal),
        }
    }
}

/// Swift が生成画面を出す直前に呼ぶ。`false` なら出さない（期限切れ・古い要求）。
extern "C" fn on_begin(request_id: u64) -> bool {
    try_begin_present(request_id)
}

/// Swift が画面を閉じた後に 1 要求につき 1 回だけ呼ぶ。`ptr` は呼び出しの間だけ有効で、所有権は
/// 移らない。画素は検査を通ったときだけ呼び出しの中でコピーする。
#[allow(unsafe_code)]
extern "C" fn on_done(
    request_id: u64,
    code: i32,
    ptr: *const u8,
    len: usize,
    width: u32,
    height: u32,
) {
    let side = MAX_SIDE as usize;
    let valid = code == 0
        && !ptr.is_null()
        && (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            == Some(len)
        && len <= side * side * 4;
    let rgba = if valid {
        // SAFETY: Swift 側が `len` バイトの有効なバッファを渡す契約。上で NULL でないこと・
        // `len == width*height*4`・上限以下を確かめた。呼び出しが返るまでは生存し、
        // ここで `Vec` へコピーするため返却後は参照しない。
        Some(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec())
    } else {
        None
    };
    complete_from_os(
        request_id,
        outcome_from_apple_code(code, rgba, width, height),
    );
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}
