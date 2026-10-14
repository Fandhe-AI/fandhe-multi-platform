//! アクセシビリティ・SEO 要件の確認テスト（TASK-4.4-S4 / Issue #82 / REQ-4 L76・REQ-9 L133）。
//!
//! `dom_tree.rs` / `html.rs` の単体テストは既に h1/p/label+input/button/ul-li の
//! マッピング・エスケープを個々に固定化済みであり、本ファイルはそれらを重複して
//! 再検証しない。ここで固定化するのは、`docs/architecture/dom-output-architecture.md`
//! §2.2・§3 が検証観点として要求する「代表画面 1 つを通しで見たときの a11y/SEO 要件」
//! （見出しがちょうど 1 つであること・状態由来の全テキストが JS 実行なしで
//! `to_html` の出力に含まれること）であり、実ブラウザの Accessibility Tree 確認
//! （`scripts/web/verify-dom-a11y.sh`）の native 側対応物という位置づけ。
//!
//! 実ブラウザでの Accessibility Tree 確認結果（見出し・textbox・button・list・listitem
//! の意味情報が実際に公開されること、PoC-5 の 0 件との対比）は
//! 本テストの対象外で、実ブラウザで別途確認する。

use fandhe_core::sample_screen;
use fandhe_web_dom::{build_dom_tree, to_html};

/// `to_html` 出力中の `<h1` の出現回数を数える（文書構造上の見出しレベルが
/// 1 つに固定されていることの検証、SEO 上の見出し構造要件）。
fn count_h1_open_tags(html: &str) -> usize {
    html.matches("<h1>").count()
}

#[test]
fn sample_screen_html_has_exactly_one_h1_heading() {
    // REQ-4 L76: 「アクセシビリティ・SEO が重要な画面」の代表例として、
    // 見出し構造が文書中に 1 つのみであることを固定化する
    // （design §2.2「画面ルート要素の先頭子として 1 つのみ配置」）。
    let screen = sample_screen();
    let state = fandhe_core::SampleState {
        memo: "下書き".to_string(),
        items: vec!["item-1".to_string(), "item-2".to_string()],
        ..Default::default()
    };

    let dom = build_dom_tree(&screen, &state, 0);
    let html = to_html(&dom);

    assert_eq!(
        count_h1_open_tags(&html),
        1,
        "見出しは画面ルートの先頭子として 1 つのみ配置される想定（design §2.2）: {html}"
    );
    assert!(html.starts_with("<div><h1>"));
}

#[test]
fn sample_screen_html_contains_all_state_derived_text_without_js() {
    // SEO の実質要件（design §4「SSR/SSG 経路の実装はスコープ外」の前段）: `to_html` の
    // 出力（SSR 経路）は JS 実行なしで状態由来の全テキストを含む必要がある
    // （クローラビリティ）。ランタイムマウント経路（`mount`、wasm32 専用）は
    // JS 必須でありこの要件を満たさない旨は検証記録側に明記する。
    let screen = sample_screen();
    let state = fandhe_core::SampleState {
        memo: "下書きメモ".to_string(),
        items: vec!["item-1".to_string(), "item-2".to_string()],
        ..Default::default()
    };

    let dom = build_dom_tree(&screen, &state, 0);
    let html = to_html(&dom);

    for expected in [
        "サンプル画面",        // Screen タイトル → h1
        "代表 1 画面サンプル", // Node::Text → p
        "メモ",                // TextField::label → label
        "下書きメモ",          // TextField::get(state) → input value
        "カウントを増やす",    // Button::label
        "項目を追加",          // Button::label
        "item-1",              // List::expand(state) → li
        "item-2",
    ] {
        assert!(
            html.contains(expected),
            "state 由来のテキスト「{expected}」が to_html の出力に含まれていない（JS なしでクロール不可）: {html}"
        );
    }
}

#[test]
fn sample_screen_html_escapes_malicious_state_end_to_end() {
    // A03 リグレッション: html.rs 単体では既にエスケープを検証済みだが、
    // sample_screen() 経由（実際にアプリが使う組み立て経路）でも状態由来の
    // 悪意ある文字列がエスケープされることを end-to-end で固定化する
    // （design §2.6「状態由来の文字列がマークアップとして解釈されることはない」）。
    let screen = sample_screen();
    let state = fandhe_core::SampleState {
        memo: "<script>alert(1)</script>".to_string(),
        items: vec!["<img src=x onerror=alert(2)>".to_string()],
        ..Default::default()
    };

    let dom = build_dom_tree(&screen, &state, 0);
    let html = to_html(&dom);

    assert!(!html.contains("<script>"));
    assert!(!html.contains("<img src=x"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("&lt;img src=x"));
}

#[test]
fn sample_screen_html_escapes_attribute_breakout_payload_end_to_end() {
    // レビュー指摘: 上記の script タグペイロードはダブルクォートを含まず、
    // input value 属性（TextField::get(state) が書き込む先）のクォート境界を
    // 破る攻撃（attribute-breakout: `"` で属性値を閉じて任意属性を注入する）を
    // 経由した end-to-end 検証になっていなかった。html.rs 単体では `"` の
    // エスケープを確認済みだが、sample_screen() 経由の組み立て経路でも
    // 属性値コンテキストで安全であることをここで固定化する。
    let screen = sample_screen();
    let state = fandhe_core::SampleState {
        memo: "\" onfocus=alert(1) autofocus x=\"".to_string(),
        items: vec!["item-1".to_string()],
        ..Default::default()
    };

    let dom = build_dom_tree(&screen, &state, 0);
    let html = to_html(&dom);

    // エスケープ後は生の `"` が現れず `&quot;` に置換されているため、
    // value 属性値のコンテキストを脱出して onfocus 属性を新設することはできない。
    // 脱出に成功していれば raw な `"` の直後に ` onfocus=alert(1)` が続くはずだが、
    // 実際にはその `"` 自体が `&quot;` に置換されている。
    assert!(
        !html.contains("\" onfocus=alert(1)"),
        "attribute-breakout ペイロードが value 属性を脱出できている: {html}"
    );
    assert!(
        html.contains("&quot; onfocus=alert(1) autofocus x=&quot;"),
        "ペイロードの `\"` が &quot; へエスケープされた状態で保持されていない: {html}"
    );
}

#[test]
fn empty_list_state_produces_ul_with_no_listitem() {
    // demo.rs / verify-dom-a11y.sh は実ブラウザの Accessibility Tree で list/listitem
    // ロールを観測するため、事前に項目を投入した状態でマウントする
    // （空リストだと listitem ロールが AX ツリーに現れない可能性があるため）。
    // 本テストはその前提（空リストでは li が 0 件になること）を明示的に固定化し、
    // demo.rs 側の「項目を事前投入する」という設計判断の根拠を裏付ける。
    let screen = sample_screen();
    let state = fandhe_core::SampleState::default();

    let dom = build_dom_tree(&screen, &state, 0);
    let html = to_html(&dom);

    assert!(html.contains("<ul></ul>"));
}

#[test]
fn form_screen_error_text_renders_as_alert_role_p() {
    // #814: form の検証エラー 3 種は `Screen::error_text`（`StyledText { role: Error, .. }`）
    // で表示され、Web DOM 出力で `role="alert"`（暗黙 `aria-live="assertive"`）を持つ
    // `<p>` に変換される想定（`dom_tree.rs` の変換規則自体の単体検証は別ファイルで
    // 済ませているため、ここでは form 画面を通しで使う経路のみ確認する）。
    use fandhe_core::click_button;
    use fandhe_core::screens::form::{
        form_screen, FormState, AGREE_ERROR, EMAIL_ERROR, NAME_ERROR,
    };

    let screen = form_screen();
    let mut state = FormState::default();
    assert!(click_button(&screen, "送信", &mut state));

    let dom = build_dom_tree(&screen, &state, 0);
    let html = to_html(&dom);

    for expected in [NAME_ERROR, EMAIL_ERROR, AGREE_ERROR] {
        let alert_p = format!(r#"<p role="alert" class="fandhe-text-error">{expected}</p>"#);
        assert!(
            html.contains(&alert_p),
            "エラー文「{expected}」が role=\"alert\" の p 要素として出力されていない: {html}"
        );
    }
    assert_eq!(
        html.matches(r#"role="alert""#).count(),
        3,
        "エラー文は 3 件のみのはず: {html}"
    );
}

#[test]
fn form_screen_default_state_has_three_empty_alert_paragraphs() {
    // 送信前（`FormState::default()`）は 3 つの `error_text` がすべて空文字列を返す
    // （form.rs 冒頭のトレードオフ: 空 `<p>` は支援技術が実害なく無視できるため許容する）。
    use fandhe_core::screens::form::{form_screen, FormState};

    let screen = form_screen();
    let state = FormState::default();

    let dom = build_dom_tree(&screen, &state, 0);
    let html = to_html(&dom);

    assert_eq!(
        html.matches(r#"<p role="alert" class="fandhe-text-error"></p>"#)
            .count(),
        3
    );
}
