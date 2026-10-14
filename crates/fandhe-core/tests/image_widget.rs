//! `Image<S>`（画像ウィジェット）の結合テスト（TASK-8.3-S36 / REQ-8、Issue #131）。
//!
//! 親 issue #95（TASK-8.3）の一部。`Screen::image()` / `Screen::image_bound()` が
//! 生成する `Node::Widget` が、既存の `WidgetHarness`（TASK-8.3-S5 / #100）を
//! 通じてもフレームループ・バージョン再利用の意味論（S5 契約）を壊さないこと、および
//! `render_headless`（`Node::Widget` プレースホルダ契約、`widget.rs` 参照）に対して
//! 未配線のまま安全側（無出力）であることを検証する。`tests/toggle_switch_widget.rs`
//! のパターンを画像向けに踏襲する。
//!
//! ロード状態遷移は自己完結の別型 `ImageWidget<S>`（`image_widget.rs`）が担い、本ファイルの
//! 対象外（Issue #130 のスコープ）。本テストは `Image::fixed` / `Image::bound` /
//! `Widget` トレイト直接呼び出しの範囲でのみ検証する。

use fandhe_core::{
    Image, LifecyclePhase, Measurement, Node, PaintCommand, Screen, Widget, WidgetHarness,
    WidgetId, render_headless,
};

/// プロフィール画面の状態。`avatar_url` フィールドを画像ウィジェットが読み取る想定。
#[derive(Default)]
struct ProfileState {
    avatar_url: String,
}

fn get_avatar_url(state: &ProfileState) -> String {
    state.avatar_url.clone()
}

fn image_screen() -> Screen<ProfileState> {
    Screen::new("プロフィール画面").image_bound(WidgetId::from_raw(1), get_avatar_url, "アバター")
}

/// (1) `.image_bound()` を含む `Screen` を `WidgetHarness` で駆動しても、S5 契約
/// （バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_image_screen() {
    let mut harness = WidgetHarness::new(image_screen(), ProfileState::default());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (2) 初回 `run_frame` は `Declared` → `Mounted` → `Layout` → `Paint` の順で
/// ライフサイクルフェーズを記録することを検証する（`toggle_switch_widget.rs` と同じ
/// 検証軸を、`Image` を含む画面に対して確認する）。
#[test]
fn image_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(image_screen(), ProfileState::default());
    harness.run_frame();

    assert_eq!(
        harness.phases(),
        &[
            LifecyclePhase::Declared,
            LifecyclePhase::Mounted,
            LifecyclePhase::Layout,
            LifecyclePhase::Paint
        ]
    );
}

/// (3) `render_headless` スナップショットに `Node::Widget`（`Image`）の行が現れないことを
/// 固定化する（未配線契約、`widget.rs` / `render.rs` 参照）。
///
/// `sync_from_state`（`&mut self` 要求）を呼べる経路が `render_headless` に存在しないため、
/// `paint()` への委譲は行わず安全側（無出力）に倒す契約を、画像についても回帰検知できる
/// ようにする。
#[test]
fn render_headless_snapshot_does_not_contain_image_line() {
    let screen = image_screen();
    let state = ProfileState {
        avatar_url: "https://example.com/avatar.png".to_string(),
    };

    let snapshot = render_headless(&screen, &state);

    // タイトル行のみで、画像の行（alt テキスト・プレースホルダのいずれも）は出力されない。
    assert_eq!(snapshot, vec!["# プロフィール画面".to_string()]);
}

/// `.image()` / `.image_bound()` 混在時も他バリアント（`.text()` 等）の描画・走査に
/// 影響しないことを確認する（既存バリアントとの後方互換性、`Node::Widget` 追加時の
/// 回帰防止）。
#[test]
fn image_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<ProfileState> = Screen::new("混在画面").text("見出し").image(
        WidgetId::from_raw(1),
        "assets/logo.png",
        "ロゴ",
    );
    let state = ProfileState {
        avatar_url: String::new(),
    };

    let snapshot = render_headless(&screen, &state);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// (4) `sync_from_state` 経由のソース識別子キャッシュ更新と `paint` コマンド列の内容
/// （alt テキストのみが `Text` に現れ、ソース識別子は現れないこと）を、`Widget` トレイトを
/// 直接呼び出して検証する。`Screen` から取り出した `Node::Widget` に対し
/// `&mut dyn Widget<ProfileState>` を得るため、`Node::Widget` の中身をテスト内で直接構築する
/// （`render_headless` は `&mut` を届けない契約のため、この経路は本テスト固有の直接駆動で
/// ある）。
#[test]
fn sync_from_state_updates_cached_source_and_paint_reflects_alt_only() {
    let mut image: Image<ProfileState> =
        Image::bound(WidgetId::from_raw(1), get_avatar_url, "アバター画像");
    let available = Measurement {
        width: 200.0,
        height: 200.0,
    };

    let state = ProfileState {
        avatar_url: "https://example.com/secret-token.png".to_string(),
    };
    image.sync_from_state(&state);
    assert_eq!(image.source_text(), "https://example.com/secret-token.png");

    let commands = image.paint((0.0, 0.0), image.measure(available));
    assert_eq!(
        commands,
        vec![
            PaintCommand::Rect,
            PaintCommand::Text("アバター画像".to_string()),
        ]
    );
    for command in &commands {
        if let PaintCommand::Text(text) = command {
            assert!(!text.contains("secret-token"));
        }
    }
}

/// `Node::Widget` 経由で組み立てた画像が 1 ノード分正しく追加されることを確認する
/// （`toggle_switch_widget.rs` の `toggle_switch_screen_builds_one_widget_node` と同じ観点）。
#[test]
fn image_screen_builds_one_widget_node() {
    let screen = image_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(_) = node {
            count += 1;
        }
    });

    assert_eq!(count, 1);
}
