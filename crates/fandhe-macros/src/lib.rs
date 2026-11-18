//! `#[fandhe::main]`: 画面を作る関数 1 つから、各 OS の入口を生成する属性（Issue #1200、REQ-21 / TASK-21.5）。
//!
//! proc-macro は別 crate が必須のため本 crate に置き、入口 crate `fandhe` が `fandhe::main` として再公開する
//! （`docs/architecture/consumer-app-structure.md` §3）。生成するコードは `::fandhe::__private` の入口関数を呼ぶだけで、
//! 起動の中身（端末機能の登録・終了処理・ソフトキーボード・insets）は `fandhe` 側が持つ。
//! 利用者は `fandhe` にだけ依存すればよく、本 crate を直接依存に書かない。
#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, parse_macro_input};

/// 画面を作る関数から、各 OS の入口を生成する。
///
/// 付ける関数は `fn(&fandhe::egui::Context) -> Box<dyn fandhe::app::ProxyAppDyn>` の形にする。
/// 入口は起動時に 1 回だけこの関数を呼び、返した画面を最初に出す（画面の名前や環境変数では選ばない）。
///
/// 生成するもの:
///
/// - `pub fn main()`: デスクトップ（macOS・Windows・Linux）と iOS の起動。bin の `src/main.rs` に付けたときはそのまま
///   実行体の入口になる。`src/lib.rs` に付けたときは、`src/main.rs` から `<lib 名>::main()` を呼ぶ。
///   Android・Web では何もしない（bin を全ターゲットで同じ形に書けるようにするため）
/// - `android_main`（Android）: `android-activity` が cdylib から探す固定名の関数。`src/lib.rs`（`crate-type` に
///   `cdylib`）に付けたときに使われる。`#[unsafe(no_mangle)]` を伴うため、Android では crate に
///   `#![forbid(unsafe_code)]` を付けられない（`deny` にする）
/// - wasm-bindgen の `start`（Web）: 読み込み時に呼ばれる入口
///
/// ```ignore
/// #[fandhe::main]
/// fn app(ctx: &fandhe::egui::Context) -> Box<dyn fandhe::app::ProxyAppDyn> {
///     Box::new(fandhe::app::Menu::new(ctx, "メニュー"))
/// }
/// ```
#[proc_macro_attribute]
pub fn main(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(
            proc_macro2::TokenStream::from(attr)
                .into_iter()
                .next()
                .map_or_else(proc_macro2::Span::call_site, |t| t.span()),
            "#[fandhe::main] は引数を取りません",
        )
        .to_compile_error()
        .into();
    }
    let func = parse_macro_input!(item as ItemFn);
    if let Err(e) = check(&func) {
        return e.to_compile_error().into();
    }
    let name = &func.sig.ident;
    quote! {
        #func

        /// デスクトップ・iOS の起動（`#[fandhe::main]` が生成）。
        #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
        #[allow(dead_code)]
        pub fn main() -> ::core::result::Result<(), ::fandhe::app::LaunchError> {
            ::fandhe::__private::run(::core::env!("CARGO_PKG_NAME"), #name)
        }

        /// Android・Web では何もしない（入口は `android_main`・wasm-bindgen の `start`。`#[fandhe::main]` が生成）。
        #[cfg(any(target_os = "android", target_arch = "wasm32"))]
        #[allow(dead_code)]
        pub fn main() -> ::core::result::Result<(), ::fandhe::app::LaunchError> {
            ::core::result::Result::Ok(())
        }

        /// Android の入口（`#[fandhe::main]` が生成）。`android-activity` が固定名で探して呼ぶ。
        ///
        /// # Safety
        /// この関数自体は `unsafe` 操作を含まない。`no_mangle` は名前修飾を外すだけで、`android_main` は
        /// `android-activity` の規約上の固定名。cdylib は 1 つの Android 実行単位にだけリンクされる。
        #[cfg(target_os = "android")]
        #[allow(unsafe_code)]
        #[unsafe(no_mangle)]
        fn android_main(__fandhe_android_app: ::fandhe::__private::AndroidApp) {
            ::fandhe::__private::run_android(__fandhe_android_app, ::core::env!("CARGO_PKG_NAME"), #name)
        }

        /// Web の入口（`#[fandhe::main]` が生成）。wasm-bindgen の glue が読み込み時に呼ぶ。
        #[cfg(target_arch = "wasm32")]
        #[::fandhe::__private::wasm_bindgen::prelude::wasm_bindgen(
            start,
            wasm_bindgen = ::fandhe::__private::wasm_bindgen
        )]
        pub fn __fandhe_start() -> ::core::result::Result<(), ::fandhe::__private::wasm_bindgen::JsValue> {
            ::fandhe::__private::start_web(#name)
        }
    }
    .into()
}

/// 付けた関数の形を確かめる。型の誤り（戻り値など）は生成したコードの型検査に任せ、ここでは形だけを見る。
fn check(func: &ItemFn) -> syn::Result<()> {
    let sig = &func.sig;
    if sig.ident == "main" {
        return Err(syn::Error::new_spanned(
            &sig.ident,
            "#[fandhe::main] が main を生成するため、画面を作る関数は main 以外の名前にしてください",
        ));
    }
    if let Some(a) = &sig.asyncness {
        return Err(syn::Error::new_spanned(
            a,
            "#[fandhe::main] の関数は async にできません",
        ));
    }
    if !sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &sig.generics,
            "#[fandhe::main] の関数は型引数を取れません",
        ));
    }
    if sig.inputs.len() != 1 {
        return Err(syn::Error::new_spanned(
            &sig.inputs,
            "#[fandhe::main] の関数は引数を 1 つ（&fandhe::egui::Context）だけ取ります",
        ));
    }
    Ok(())
}
