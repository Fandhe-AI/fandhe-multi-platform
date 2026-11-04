//! Apple（iOS / macOS）向けの共通層。機能ごとの crate が ObjC / Swift 薄層を自分の中に持てるように、
//! 全機能で共有する部品だけをここに置く（TASK-21.2 / Issue #1168、親 #1159）。
//!
//! - `include/fandhe_ios_common.h`: 全 `.m` が `#import` する共有ヘッダ。`build` モジュールの
//!   `compile_objc` が `-I` へ常に足すため、利用側は何も指定しなくてよい。
//! - `build`（feature `build`）: build script 用の `.m` / Swift コンパイルと framework リンクの helper。
//!   build 依存として使う。Linux・Windows のホストでもコンパイルされるため、本 crate 全体は
//!   `cfg` で囲わない。
//! - [`is_main_thread`]: main thread 限定の OS 呼び出しの前に使う判定（iOS・macOS のみ）。
//!
//! 起動経路（`ios_scene`・`ios_text_input`）は winit の補完であり、本 crate には置かない。

#![deny(unsafe_code)]

#[cfg(feature = "build")]
pub mod build;

#[cfg(any(target_os = "ios", target_os = "macos"))]
#[allow(unsafe_code)]
mod main_thread {
    // SAFETY: `pthread_main_np` は libSystem（Darwin の全バイナリに常時リンク済み）が
    // 提供する引数無しの単純な問い合わせ関数で、呼び出しスレッドを変更しない
    // （呼び出し元スレッドが main スレッドなら非 0 を返す）。
    unsafe extern "C" {
        fn pthread_main_np() -> i32;
    }

    pub(super) fn is_main_thread() -> bool {
        // SAFETY: 上記 `pthread_main_np` 宣言のコメント参照。副作用も前提条件も無く
        // 常に安全に呼び出せる。
        unsafe { pthread_main_np() != 0 }
    }
}

/// 呼び出しスレッドが main スレッドかどうかを返す。
///
/// UIKit / AppKit を触る ObjC 実装は main スレッド専用の呼び出し契約を持つ。公開型は誰でも構築でき
/// ワーカースレッドからの誤用をコンパイラは検出できないため、FFI 呼び出しの前にこれで検査し、
/// main 以外ならエラーを返して unsafe な前提を呼び出し側へ漏らさない（coding-rust.md の FFI 例外規定）。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub fn is_main_thread() -> bool {
    main_thread::is_main_thread()
}
