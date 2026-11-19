//! CJK フォントの実行時読み込み（TASK-1.2 / TASK-10.1、フォント同梱化込み）。
//!
//! `sample_screen()` はラベル・項目名に日本語文字列（「メモ」「カウントを増やす」等）を
//! 含むが、egui は自前ラスタライザのため既定フォントに日本語グリフを持たない
//! （未対応文字は "tofu" 表示になる。PoC-1 で「要素欠落ではなくフォント課題」と
//! 記録済みの既知事項、REQ-10）。
//!
//! TASK-10.1（Issue #218）でシステムフォント依存を解消し、`fandhe-fonts` クレートが
//! `include_bytes!` で同梱する Noto Sans JP Regular を native 版の既定フォントとして
//! 使う（システムに CJK フォントが存在しない環境・iOS/Android でも tofu にならない）。
//! 検証用に環境変数 `FANDHE_PROXY_CJK_FONT` で任意のフォントファイルへ上書きできる
//! 経路のみ残す（読み込み失敗時は同梱フォントへフォールバックする）。

/// native 版: 環境変数 `FANDHE_PROXY_CJK_FONT`（検証用の上書き経路）が指すフォントを
/// 優先し、未設定または読み込み失敗時は `fandhe-fonts` 同梱の Noto Sans JP Regular を
/// egui のフォント定義に追加する。同梱フォントは常にバイナリへ埋め込まれているため、
/// 「見つからず tofu のまま継続」という失敗分岐は同梱バイト列自体の登録が失敗しない限り
/// 発生しない（panic を API 契約にしない、coding-rust.md のエラーハンドリング方針）。
#[cfg(not(target_arch = "wasm32"))]
pub fn install_cjk_font(ctx: &egui::Context) {
    let bytes = std::env::var("FANDHE_PROXY_CJK_FONT")
        .ok()
        .and_then(|path| match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(err) => {
                eprintln!(
                    "FANDHE_PROXY_CJK_FONT（{path}）の読み込みに失敗したため同梱フォントへフォールバックする: {err}"
                );
                None
            }
        })
        .unwrap_or_else(|| fandhe_fonts::NOTO_SANS_JP_REGULAR.to_vec());

    apply_font_bytes(ctx, bytes);
}

/// wasm 版: フォント読み込みは `fandhe` の `src/entry/web.rs` 側で `fetch` により非同期に行うため、
/// 同期呼び出しであるここでは何もしない（起動時は既定フォントで開始し、
/// 取得完了後（失敗時も記号専用 family のために）[`apply_fonts`] で差し替える）。
#[cfg(target_arch = "wasm32")]
pub fn install_cjk_font(_ctx: &egui::Context) {}

/// 読み込み済みのフォントバイト列を egui のフォント定義へ登録し、
/// 全 `FontFamily` の先頭（最優先）に挿入する。アイコンの記号専用 family も登録する
/// （[`apply_fonts`] の `Some(bytes)`）。
pub fn apply_font_bytes(ctx: &egui::Context, bytes: Vec<u8>) {
    apply_fonts(ctx, Some(bytes));
}

/// フォント定義を組み立てて登録する。`cjk` が `Some` なら全 `FontFamily` の先頭へ挿入する。
/// `None`（Web でフォントの取得に失敗した経路）でも、アイコンの記号専用 family は必ず登録する
/// （登録が無いと `Proportional` に戻り、NotoEmoji → emoji-icon-font の逆順で引いて字形が変わるため。Issue #1437）。
pub fn apply_fonts(ctx: &egui::Context, cjk: Option<Vec<u8>>) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(bytes) = cjk {
        let font_name = "fandhe-proxy-cjk".to_string();
        fonts.font_data.insert(
            font_name.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        for family in fonts.families.values_mut() {
            family.insert(0, font_name.clone());
        }
    }
    // Issue #1437: アイコンの記号専用 family。CJK を先頭に入れた上の family のままだと、
    // Noto Sans JP が ⬇ ⬆ ⚠ ★ ♥ ▶ を取って字形がばらつくため、CJK を入れずに egui 同梱の
    // 2 つのフォント（emoji-icon-font を先、Noto Emoji を後）だけで引く。上のループの後で登録する。
    fonts.families.insert(
        icon_family(),
        vec!["emoji-icon-font".to_owned(), "NotoEmoji-Regular".to_owned()],
    );
    ctx.set_fonts(fonts);
}

/// アイコンの記号専用 family の名前（[`apply_fonts`] が登録する）。
const ICON_FAMILY: &str = "fandhe-icon";

fn icon_family() -> egui::FontFamily {
    egui::FontFamily::Name(ICON_FAMILY.into())
}

/// アイコンの記号を描く family を返す。[`apply_fonts`] を通っていない `ctx`（Web の起動直後〜フォント取得の完了前や、
/// 既定フォントのままのテスト）では専用 family が未登録で、未登録の `Name` を使うと egui が panic
/// するため `Proportional` に戻す。ここでは優先の順が専用 family と逆（NotoEmoji が先）なので、
/// 製品の経路（ネイティブは `install_cjk_font`、Web は取得の成否にかかわらず `apply_fonts`）は必ず登録する。
pub(crate) fn icon_font_family(ctx: &egui::Context) -> egui::FontFamily {
    let family = icon_family();
    if ctx.fonts(|f| f.definitions().families.contains_key(&family)) {
        family
    } else {
        egui::FontFamily::Proportional
    }
}
