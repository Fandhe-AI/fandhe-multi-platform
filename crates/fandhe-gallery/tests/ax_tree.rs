//! AX ツリー公開の機械的検証（TASK-9.1-S3 / REQ-9 / Issue #216）。
//!
//! 親 TASK-9.1（Issue #213）の受け入れ基準は「VoiceOver 相当の読み上げが 1 要素以上
//! 成立すること」だが、本開発環境（Linux・x86_64）には macOS・VoiceOver が存在しない。
//! VoiceOver は NSAccessibility 経由で AccessKit の AX ツリーを読み上げる（accesskit の
//! macOS アダプタが NSAccessibility プロトコルへブリッジする構造、PoC-1 実証と同じ経路）ため、
//! **VoiceOver が実際に読み上げる情報源そのもの**である
//! AccessKit ツリー（role・accessible name）を `egui_kittest` 経由で Linux 上から機械的に
//! 検査することで、macOS 実機なしに検証できる範囲を最大化する
//! （「実機不可時は人間担当手順書 + 機械的代替検証」パターンを踏襲）。
//!
//! macOS 実機での VoiceOver 実読み上げ確認は人間担当に委ねる。
//!
//! ## 検証範囲
//!
//! egui/eframe 側の標準ウィジェット（`ui.heading` / `ui.label` / `ui.button` /
//! `ui.text_edit_singleline`）が `Cargo.toml` の dev-dependencies（`egui_kittest` の
//! `"wgpu"` feature が無条件に有効化する `egui`/`"accesskit"` feature、および
//! `eframe` 側の明示的な `"accesskit"` feature 有効化）により常時公開する AX ノード
//! のみを検証対象とする。独自ウィジェット（`Node::Widget`）由来の AX マッピング
//! （`src/accessibility.rs`、`render_screen`（`src/lib.rs`）の `Node::Widget` アーム）
//! は本テストの対象外とする。
//!
//! ## GPU アダプタ不要（`desktop_render.rs` / `prototype_render.rs` との相違点）
//!
//! `desktop_render.rs` / `prototype_render.rs` は PNG エビデンス保存のため
//! `egui_kittest::wgpu::WgpuTestRenderer` を明示的に指定しており、wgpu アダプタが
//! 存在しない環境（CI ランナー等）では `Harness` 構築時に panic するため、
//! アダプタ欠如を検出してテストをスキップする機構を備える。
//!
//! 本ファイルは AccessKit の role・accessible name のみを検査し、画素のラスタライズ
//! 結果は用いないため、`.renderer(...)` を指定せず既定の `LazyRenderer`
//! （wgpu セットアップを要求しない）に委ねる。AX ツリーは `egui::Context` が
//! `Harness::run` の中で（描画バックエンドと独立に）構築するため、wgpu アダプタの
//! 有無に関わらず全テストが実行される（`WGPU_ADAPTER_NAME=nonexistent-adapter-xyz
//! WGPU_BACKEND=noop` でアダプタ選択を強制的に失敗させても全 7 件成功することを
//! 実機確認済み）。したがって本ファイルには「スキップ」の概念がなく、
//! `cargo test` の `ok` は常に「実際に検査を実行して合格した」ことを意味する
//! （Issue #216 レビュー指摘対応: スキップと合格が区別できない問題の解消）。

use egui::accesskit::Role;
use egui_kittest::kittest::Queryable;
use fandhe_core::{sample_screen, SampleState, StateStore};
use fandhe_prototype::AppState;

/// `sample_screen()`（TASK-1.1-S6 の代表 1 画面サンプル）を描画する `Harness` を組み立てる。
/// AX ツリー検査のみが目的のため、レンダラは既定（`LazyRenderer`、wgpu 不要）のままとする。
fn build_sample_harness() -> egui_kittest::Harness<'static, StateStore<SampleState>> {
    let screen = sample_screen();

    egui_kittest::Harness::builder().build_ui_state(
        move |ui, store: &mut StateStore<SampleState>| {
            fandhe_egui_proxy::render_screen(ui, &screen, store);
        },
        StateStore::new(SampleState::default()),
    )
}

/// `PrototypeApp` と同型のループ（毎フレーム `current_screen` を引き直して描画）で
/// `Harness` を組み立てる（`prototype_render.rs::build_harness` と同一構成。
/// レンダラは既定のままとする理由は [`build_sample_harness`] と同じ）。
fn build_prototype_harness() -> egui_kittest::Harness<'static, StateStore<AppState>> {
    egui_kittest::Harness::builder().build_ui_state(
        move |ui, store: &mut StateStore<AppState>| {
            let screen = fandhe_prototype::current_screen(store.state());
            fandhe_egui_proxy::render_screen(ui, &screen, store);
        },
        StateStore::new(AppState::default()),
    )
}

/// `FANDHE_SAVE_AX_DUMP=1` の場合のみ、`node` の AX ツリー全体（role・label・value・
/// children を再帰的に含む `Debug` 出力）を `_/verification/task-9.1-s3/{name}.txt`
/// （git 管理外）へ保存する。既定では何も書き込まない opt-in（security.md A05 準拠）。
///
/// パスはこの関数が受け取る `name`（呼び出し元で固定したリテラル）のみから組み立て、
/// 外部入力（環境変数の値そのもの等）をパス生成に用いない（A03 インジェクション対策）。
fn maybe_save_ax_dump<N>(name: &str, node: &N)
where
    N: std::fmt::Debug,
{
    if std::env::var_os("FANDHE_SAVE_AX_DUMP").is_none() {
        return;
    }

    let out_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../_/verification/task-9.1-s3");
    std::fs::create_dir_all(&out_dir).expect("エビデンス出力ディレクトリの作成に失敗した");

    std::fs::write(out_dir.join(format!("{name}.txt")), format!("{node:#?}"))
        .expect("AX ツリーダンプの書き込みに失敗した");
}

/// `sample_screen()` の全要素（見出し・本文・メモ欄・ボタン 2 種）が、AccessKit の
/// role・accessible name（label/value）付きで公開されていることを検証する。
///
/// PoC-1（macOS 実機、8 要素）との対比はレポート側（§3）で行う。本テストは
/// 「role が意味のある値であること」までを固定化し、`get_by_label` のみ（文字列一致）
/// より一段厳しい検証にする（VoiceOver は role を読み上げに使うため、role が
/// `Role::Unknown` のままでは読み上げ内容が空になる）。
#[test]
fn sample_screen_exposes_role_and_label_for_all_widget_kinds() {
    let mut harness = build_sample_harness();
    harness.run();

    // 見出し・本文（`ui.heading` / `ui.label`）は egui 0.33 時点で専用の
    // `Role::Heading` ではなく `Role::Label` にマッピングされる
    // （`egui::Response::fill_accesskit_node_from_widget_info` の `WidgetType::Label`
    // 分岐、レポート §4 に実装上の根拠を記録）。
    harness.get_by_role_and_label(Role::Label, "サンプル画面");
    harness.get_by_role_and_label(Role::Label, "代表 1 画面サンプル");
    harness.get_by_role_and_label(Role::TextInput, "メモ");
    harness.get_by_role_and_label(Role::Button, "カウントを増やす");
    harness.get_by_role_and_label(Role::Button, "項目を追加");

    maybe_save_ax_dump("sample-screen", &harness.root());
}

/// 「項目を追加」ボタンを 2 回押下すると、追加された各項目（`Node::List` 展開）を含む
/// 全 8 要素（見出し・本文・「メモ」ラベル・メモ入力欄・ボタン 2 種・追加項目 2 件）が
/// role・accessible name 付きで AX ツリーへ公開されていることを検証する
/// （レポート §3.3 の PoC-1 対比表「8 要素」の内訳を、この 1 テストのアサーションで
/// 網羅的に固定化する）。
#[test]
fn sample_screen_list_items_are_exposed_as_labels_after_adding() {
    let mut harness = build_sample_harness();
    harness.run();

    harness.get_by_label("項目を追加").click();
    harness.run();
    harness.get_by_label("項目を追加").click();
    harness.run();

    harness.get_by_role_and_label(Role::Label, "サンプル画面");
    harness.get_by_role_and_label(Role::Label, "代表 1 画面サンプル");
    // `Node::TextField` は `ui.label(label)`（別ノード）＋ `labelled_by` で
    // アクセシブルネームを共有する `ui.text_edit_singleline`（本稿）の 2 ノードを
    // 生成する（`fandhe_egui_proxy::render_screen` の `Node::TextField` アーム参照）。
    // 「メモ」ラベル自体（`Role::Label`）も 8 要素の内訳に含まれるため、
    // `Role::TextInput` の入力欄と分けて両方を検証する
    // （Issue #216 レビュー指摘対応: メモの `Role::Label` ノードが未検証だった問題の解消）。
    harness.get_by_role_and_label(Role::Label, "メモ");
    harness.get_by_role_and_label(Role::TextInput, "メモ");
    harness.get_by_role_and_label(Role::Button, "カウントを増やす");
    harness.get_by_role_and_label(Role::Button, "項目を追加");
    harness.get_by_role_and_label(Role::Label, "item-1");
    harness.get_by_role_and_label(Role::Label, "item-2");

    maybe_save_ax_dump("sample-screen-with-items", &harness.root());
}

/// プロトタイプ一覧画面の要素（見出し・本文・5 ボタン）が role・label 付きで
/// 公開されていることを検証する。
#[test]
fn prototype_list_screen_exposes_role_and_label_for_all_widget_kinds() {
    let mut harness = build_prototype_harness();
    harness.run();

    harness.get_by_role_and_label(Role::Label, "一覧");
    harness.get_by_role_and_label(Role::Label, "タスクメモ一覧");
    harness.get_by_role_and_label(Role::Button, "項目を追加");
    harness.get_by_role_and_label(Role::Button, "次へ");
    harness.get_by_role_and_label(Role::Button, "前へ");
    harness.get_by_role_and_label(Role::Button, "詳細を開く");
    harness.get_by_role_and_label(Role::Button, "設定");

    maybe_save_ax_dump("prototype-list-screen", &harness.root());
}

/// 「設定」ボタン押下で設定画面へ遷移した後、`TextField`（ユーザー名）・
/// ボタン（一覧へ戻る）が role・label 付きで公開されていることを検証する
/// （`RenderMode::Dom` 指定画面。本テストは native/egui 描画経路のみを対象とし、
/// DOM 出力側の a11y 確認は TASK-4.4-S4（Issue #82）が別途担当済み）。
#[test]
fn prototype_settings_screen_exposes_role_and_label_after_navigation() {
    let mut harness = build_prototype_harness();
    harness.run();

    harness.get_by_label("設定").click();
    harness.run();

    harness.get_by_role_and_label(Role::TextInput, "ユーザー名");
    harness.get_by_role_and_label(Role::Button, "一覧へ戻る");

    maybe_save_ax_dump("prototype-settings-screen", &harness.root());
}

/// 「詳細を開く」ボタン押下で詳細画面へ遷移した後、選択中項目タイトル（`Role::Label`）・
/// メモ欄（`Role::TextInput`）・ボタン（一覧へ戻る）が role・label 付きで
/// 公開されていることを検証する。
///
/// `AppState::default()` は `items` が空のため、事前に「項目を追加」ボタンで
/// 実項目を 1 件作ってから選択・遷移させる（`fandhe_prototype::screens::list::add_item`
/// が新規項目を追加すると同時に `selected` をその項目へ合わせるため、詳細画面は
/// 空状態フォールバック文言（`detail.rs::expand_selected_title` の `None` 分岐）
/// ではなく実データのタイトルを展開する。空リストのまま検証すると選択タイトルの
/// AX パスが実質未検証になる問題の解消、Issue #216 レビュー指摘対応）。
#[test]
fn prototype_detail_screen_exposes_role_and_label_after_navigation() {
    let mut harness = build_prototype_harness();
    harness.run();

    harness.get_by_label("項目を追加").click();
    harness.run();

    harness.get_by_label("詳細を開く").click();
    harness.run();

    harness.get_by_role_and_label(Role::Label, "新規項目 1");
    harness.get_by_role_and_label(Role::TextInput, "メモ");
    harness.get_by_role_and_label(Role::Button, "一覧へ戻る");

    maybe_save_ax_dump("prototype-detail-screen", &harness.root());
}
