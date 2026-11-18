//! iOS の入力欄の橋渡し（`ios/fandhe_ios_text_input.m`、`src/entry/ios_text_input.rs` の相手）をコンパイルする。
//!
//! 入口（feature `egui`）を使うときだけ要る。iOS 以外・feature `egui` なしでは何もしない。
//! `compile_objc` は `rustc-link-lib` で静的ライブラリを出すため、rlib を経て利用者の最終成果物へ伝わる
//! （`rustc-link-arg` と違い、利用者の build.rs で出し直さなくてよい）。

fn main() {
    if !fandhe_platform_apple::build::target_is_ios()
        || std::env::var_os("CARGO_FEATURE_EGUI").is_none()
    {
        return;
    }
    fandhe_platform_apple::build::compile_objc(
        "ios/fandhe_ios_text_input.m",
        &["Foundation", "UIKit"],
    );
}
