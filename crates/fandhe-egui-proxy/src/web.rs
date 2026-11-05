//! Web（wasm32-unknown-unknown）のブートストラップ（TASK-1.2）。
//!
//! `web/index.html` の `<canvas id="fandhe-proxy-canvas">` へ `fandhe_render_egui::launch::start_web`（eframe の WebRunner）を
//! 通してアタッチする。`scripts/web/build-wasm-proxy.sh` が `wasm-bindgen --target web` で
//! 生成した glue コードから本モジュールの [`start`] が呼ばれる想定
//! （`fandhe-gallery` の `#[wasm_bindgen(start)]` から呼ぶ）。
//!
//! CJK フォント（`sample_screen()` の日本語ラベル向け）は `fonts/` 配信ディレクトリから
//! `fetch` で取得し、取得できた場合のみ `font::apply_font_bytes` で差し替える。
//! TASK-10.1（Issue #218）でコピー元を `crates/fandhe-fonts` 同梱の Noto Sans JP へ
//! 変更し（`scripts/web/build-wasm-proxy.sh` 参照）、システムフォント非依存とした。
//! wasm32 向けバイナリへ直接 `include_bytes!` しないのは、配信サイズ（REQ-4 の 3MB 基準）
//! と衝突するため（`crates/fandhe-fonts/src/lib.rs` のモジュールコメント参照）。
//! 取得失敗時は既定フォントのまま続行する（非致命、`font.rs` のモジュールコメント参照）。

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// 配信ディレクトリ上のフォントパス（`scripts/web/build-wasm-proxy.sh` が
/// `crates/fandhe-fonts` 同梱の Noto Sans JP Regular をコピーする。存在しない場合は
/// 取得失敗として扱い、既定フォントで続行する）。
const CJK_FONT_URL: &str = "fonts/noto-sans-jp-regular.otf";

/// Web のブートストラップ本体。`fandhe-gallery` の `#[wasm_bindgen(start)]` から呼ばれる
/// （wasm の start 関数は最終成果物の cdylib が 1 つだけ持つ。Issue #1165）。`build_app` は画面の登録。
pub fn start(build_app: crate::BuildAppFn) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    // DOM 取得は `start()` の戻り値である `Result<(), JsValue>` へそのまま伝播できるよう、
    // `spawn_local` の非同期ブロックへ入る前に同期的に行う（coding-rust.md の
    // 「panic を API 契約にしない・回復可能なエラーは Result で返す」方針に沿い、
    // `.expect()` ではなく `?` で呼び出し元（wasm-bindgen glue）へエラーを返す）。
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("window が取得できない"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("document が取得できない"))?;
    let canvas = document
        .get_element_by_id("fandhe-proxy-canvas")
        .ok_or_else(|| {
            JsValue::from_str(
                "canvas 要素 #fandhe-proxy-canvas が見つからない（web/index.html 参照）",
            )
        })?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| JsValue::from_str("#fandhe-proxy-canvas は canvas 要素である必要がある"))?;

    // TASK-4.1（REQ-4 / M3、条件 4）: `WebRunner.start()`（`start_web`）の前に必ず通す構造的ガード。
    // 詳細は `ensure_canvas_css_size` のドキュメンテーションコメント参照。
    ensure_canvas_css_size(&canvas);

    wasm_bindgen_futures::spawn_local(async move {
        let font_bytes = fetch_font_bytes(CJK_FONT_URL).await;

        // WebGL2 固定・WebRunner の起動は `fandhe_render_egui::launch::start_web` が持つ。
        let start_result = fandhe_render_egui::launch::start_web(canvas, move |lc| {
            let ctx = lc.egui_ctx();
            if let Some(bytes) = font_bytes {
                crate::font::apply_font_bytes(ctx, bytes);
            }
            // Issue #715: `FANDHE_PROXY_APP`（wasm ではビルド時 `option_env!`
            // 経由、`screens::selected_app_name` 参照）で選んだ画面をレジストリ
            // （`build_app`）から解決する。未指定・未登録は既定の
            // sample 画面（`ProxyApp::new`）へフォールバックする。
            let app: Box<dyn fandhe_render_egui::EguiApp> =
                match crate::selected_app_name().and_then(|name| build_app(&name, ctx)) {
                    Some(app) => app,
                    None => Box::new(crate::ProxyApp::new(ctx)),
                };
            app
        })
        .await;

        if let Err(err) = start_result {
            web_sys::console::error_1(&format!("eframe の起動に失敗した: {err}").into());
        }
    });

    Ok(())
}

/// `url` を `fetch` し、成功すればバイト列を返す。失敗（未配置・ネットワークエラー等）
/// は `None` を返し、呼び出し元で既定フォント継続にフォールバックする
/// （panic を API 契約にしない、coding-rust.md のエラーハンドリング方針）。
async fn fetch_font_bytes(url: &str) -> Option<Vec<u8>> {
    let window = web_sys::window()?;
    let resp_value = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .ok()?;
    let resp: web_sys::Response = resp_value.dyn_into().ok()?;
    if !resp.ok() {
        return None;
    }
    let buffer_promise = resp.array_buffer().ok()?;
    let buffer_value = wasm_bindgen_futures::JsFuture::from(buffer_promise)
        .await
        .ok()?;
    let array = js_sys::Uint8Array::new(&buffer_value);
    Some(array.to_vec())
}

/// TASK-4.1（REQ-4 / M3、条件 4）: Safari の canvas リサイズ無限ループ対策の
/// 構造的ガード。`start()` が `WebRunner.start()` を呼ぶ前に必ず通す。
///
/// 背景: PoC-5（`docs/spec/03-poc/web-wasm/README.md` 発見事項1）で、
/// canvas 要素に CSS 上の明示サイズが無い場合、egui/eframe の ResizeObserver
/// ベース自動リサイズが無限フィードバックループに陥り、`canvas.width`/`height`
/// が 2^26（67,108,864）まで異常膨張して描画不能になる Safari 26.5 のバグを
/// 確認済み（`emilk/egui` #4699「Web canvas resize loop」と同種）。対策の本体は
/// `web/index.html` 側のインライン `style` によるサイズ明示
/// （`docs/architecture/web-canvas-css-sizing.md` の規約。`<style>` ブロック・
/// 外部スタイルシートでは本関数が判定できないためインラインのみを対象とする）
/// だが、ホストページが規約を守り忘れた場合に無防備のままにしないため、
/// ここでも二重に対策する。
///
/// インラインスタイルに `width`/`height` が未設定の場合、canvas の
/// `width()`/`height()` 数値属性から `"{n}px"` を組み立てて設定する
/// （外部入力を style へ展開しないため、他クレート・他ページからの文脈は
/// 一切関与しない。A03 インジェクション対策として値は常に own 属性由来の数値）。
/// style 取得自体に失敗した場合は panic させず `console::warn_1` で警告するのみ
/// とし、続行する（coding-rust.md「panic を API 契約にしない」方針）。
fn ensure_canvas_css_size(canvas: &web_sys::HtmlCanvasElement) {
    let style = canvas.style();

    let width_unset = style
        .get_property_value("width")
        .is_ok_and(|v| v.is_empty());
    let height_unset = style
        .get_property_value("height")
        .is_ok_and(|v| v.is_empty());

    if !width_unset && !height_unset {
        return;
    }

    web_sys::console::warn_1(
        &"#fandhe-proxy-canvas に CSS サイズ明示が無い（web-canvas-css-sizing.md 違反）。\
          Safari のリサイズループ対策として canvas 属性値から補完する。"
            .into(),
    );

    if width_unset {
        let width_px = format!("{}px", canvas.width());
        if style.set_property("width", &width_px).is_err() {
            web_sys::console::warn_1(&"canvas の CSS width 設定に失敗した".into());
        }
    }
    if height_unset {
        let height_px = format!("{}px", canvas.height());
        if style.set_property("height", &height_px).is_err() {
            web_sys::console::warn_1(&"canvas の CSS height 設定に失敗した".into());
        }
    }
}
