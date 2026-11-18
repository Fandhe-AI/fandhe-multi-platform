//! egui_kittest による要素欠落 0 件の機械的検証（TASK-1.2 / REQ-1）。
//!
//! `fandhe_core::sample_screen()`（見出し・本文テキスト・「メモ」入力欄・
//! 「カウントを増やす」「項目を追加」ボタン・リスト項目の 4 種の要素）を
//! [`fandhe_egui_proxy::render_screen`] で描画し、AccessKit ツリー経由で全要素の
//! 存在を確認する（PoC-1 の「macOS AX ツリーで要素確認」と同型の検証を
//! クロスプラットフォームに再現する）。ボタン押下・テキスト入力による状態反映も
//! あわせて検証し、wgpu バックエンドでレンダ画像（PNG エビデンス）を
//! `CARGO_TARGET_TMPDIR` へ保存する。
//!
//! ## GPU アダプタが存在しない環境でのふるまい（Issue #23 の CI 指摘対応）
//!
//! `egui_kittest::wgpu::WgpuTestRenderer` は wgpu アダプタ（ソフトウェアラスタライザ
//! 含む）が 1 つも見つからない環境では `Harness` 構築時に panic する
//! （`CustomNativeAdapterSelectionError("No adapter found")`）。CI
//! ランナーは ci.md によりグローバル状態を汚す野良インストール（Mesa llvmpipe 導入等）を
//! workflow に書けないため、この crate 側でアダプタ欠如を検出し、
//! 各テストを（失敗ではなく）スキップする。スキップ判定は `catch_unwind` した panic
//! メッセージが実際にアダプタ欠如由来（`is_adapter_unavailable_panic` が判定する
//! 部分文字列に一致）である場合に限定し、それ以外の `Harness` 構築時 panic（API 不一致・
//! 初期化バグ等の regression）は再送出してテストを失敗させる。`Harness` 構築後の
//! 描画結果・アサーションもこれまで通り実行し、失敗ならテストを失敗させる
//! （いずれも fake-green 化を防ぐ。Issue #23 の Bugbot 指摘対応）。
//!
//! アダプタ欠如を「隠れた仕様」にしないため、環境変数 `FANDHE_REQUIRE_DESKTOP_RENDER=1`
//! を設定するとスキップせず本来の panic を再送出する（実機・ローカルでの GPU 前提の
//! 明示的な検証や、ランナーへソフトウェアラスタライザを導入した後の再有効化に使う）。

use egui_kittest::kittest::Queryable;
use fandhe_core::{sample_screen, SampleState, StateStore};

/// この検証環境の GPU アダプタ取得は、標準テストランナーの複数スレッドから
/// 同時に行うと初期化がハングすることを実測で確認したため（1 GPU を共有する
/// CI/検証環境での既知の制約）、テスト単位で直列化する。
///
/// `PoisonError` は `into_inner()` で回復する。あるテストが（アダプタ欠如以外の
/// 理由で）このロック保持中に panic しても、後続テストまで連鎖的に失敗させないため。
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// プロセス内で一度でも「アダプタなし」と判定したら記録し、以降の `build_harness_or_skip`
/// 呼び出しでは `Harness` の再構築（と再度の panic/catch_unwind）を省略する。
/// 5 テストのうち最初の 1 回だけ実際にプローブすれば十分なため。
static GPU_UNAVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `render_screen` を `build_ui_state` の状態（`StateStore<SampleState>`）越しに
/// 毎フレーム呼び出す `Harness` を組み立てる。`screen`（不変な DSL 記述）はクロージャに
/// 直接キャプチャし、可変な `StateStore` のみを kittest の状態として持たせる。
fn build_harness() -> egui_kittest::Harness<'static, StateStore<SampleState>> {
    let screen = sample_screen();

    egui_kittest::Harness::builder()
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
        .build_ui_state(
            move |ui, store: &mut StateStore<SampleState>| {
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            StateStore::new(SampleState::default()),
        )
}

/// `egui_wgpu::WgpuError::CustomNativeAdapterSelectionError` /
/// `egui_wgpu::wgpu::RequestAdapterError`（`egui_kittest` の
/// `default_wgpu_setup` がアダプタ 0 件時に返す `"No adapter found"`）に由来する
/// panic メッセージにのみ現れる部分文字列。`build_harness` 内の
/// `.expect("Failed to create render state")` は `WgpuError` を `{:?}`（Debug）で
/// 展開するため、アダプタ欠如以外の panic（API 不一致・初期化バグ等）ではこれらの
/// 文字列は現れない。ここに一致しない panic を「アダプタ欠如」として握りつぶすと
/// fake-green の原因になるため（Issue #23 の Bugbot 指摘）、判定はこの部分文字列
/// 一致に限定する。
const ADAPTER_UNAVAILABLE_PANIC_MARKERS: [&str; 2] =
    ["CustomNativeAdapterSelectionError", "No adapter found"];

/// `catch_unwind` で捕捉した panic ペイロードが、アダプタ欠如に由来するものかを
/// 判定する。`panic!`/`.expect()` のペイロードは通常 `&'static str` か `String` の
/// いずれかで渡ってくるため、両方を試す。
fn is_adapter_unavailable_panic(payload: &(dyn std::any::Any + Send)) -> bool {
    let message: Option<&str> = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));

    match message {
        Some(message) => ADAPTER_UNAVAILABLE_PANIC_MARKERS
            .iter()
            .any(|marker| message.contains(marker)),
        // メッセージを文字列として取り出せない panic は判別不能なため、
        // 安全側に倒してアダプタ欠如とは扱わない（= スキップせず失敗させる）。
        None => false,
    }
}

/// `build_harness` を呼び出し、wgpu アダプタが存在しないことによる構築時 panic のみを
/// 捕捉して `None` を返す（テストをスキップさせる）。`Harness` 構築後に起きる不具合
/// （描画・アサーションの失敗）はここでは扱わず、呼び出し元のテスト本体でそのまま
/// 失敗させる（構築の成否だけをスキップ判定の境界にする）。
///
/// 捕捉した panic が [`is_adapter_unavailable_panic`] に一致しない場合（API 不一致・
/// 初期化バグ等、アダプタ欠如以外の regression）は `std::panic::resume_unwind` で
/// そのまま再送出し、テストを失敗させる（fake-green 化を防ぐ。Issue #23 の
/// Bugbot 指摘対応）。
///
/// `FANDHE_REQUIRE_DESKTOP_RENDER=1` が設定されている場合はスキップせず、
/// 捕捉した panic をそのまま再送出する（アダプタ欠如を隠さない明示的な検証用）。
fn build_harness_or_skip() -> Option<egui_kittest::Harness<'static, StateStore<SampleState>>> {
    if std::env::var_os("FANDHE_REQUIRE_DESKTOP_RENDER").is_some() {
        return Some(build_harness());
    }

    if GPU_UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "notice: wgpu アダプタが取得できない環境のため、デスクトップ実描画テストをスキップします（既知・別テストで確認済み）"
        );
        return None;
    }

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(build_harness)) {
        Ok(harness) => Some(harness),
        Err(payload) => {
            if !is_adapter_unavailable_panic(payload.as_ref()) {
                // アダプタ欠如と判別できない panic はスキップ扱いにせず、そのまま
                // 再送出してテストを失敗させる（fake-green 化を防ぐ）。
                std::panic::resume_unwind(payload);
            }

            GPU_UNAVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
            eprintln!(
                "notice: wgpu アダプタが取得できない環境のため、デスクトップ実描画テストをスキップします（CI ランナーに GPU/ソフトウェアラスタライザ未整備の既知の制約。強制検証するには FANDHE_REQUIRE_DESKTOP_RENDER=1 を設定する）"
            );
            None
        }
    }
}

/// `sample_screen()` の全要素（見出し・本文・メモ欄・ボタン 2 種。初期リスト項目は
/// 0 件のため描画されない）が AccessKit ツリーに存在することを検証する。
#[test]
fn all_declared_elements_are_present_with_no_missing_widgets() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness.get_by_label("サンプル画面");
    harness.get_by_label("代表 1 画面サンプル");
    harness.get_by_label("メモ");
    harness.get_by_label("カウントを増やす");
    harness.get_by_label("項目を追加");
}

/// 「カウントを増やす」ボタン押下で `StateStore` の `count` が増え、`version` が
/// 進むことを検証する（`render_screen` → `Event::ButtonClicked` → `dispatch` の配線確認）。
#[test]
fn clicking_increment_button_updates_state_via_dispatch() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness.get_by_label("カウントを増やす").click();
    harness.run();

    assert_eq!(harness.state().state().count, 1);
    assert_eq!(harness.state().version(), 1);
}

/// 「項目を追加」ボタンを 2 回押下すると `items` が 2 件に増え、追加された項目が
/// リスト展開（`Node::List` → `Screen::visit_expanded`）を通じて描画されることを検証する。
#[test]
fn clicking_add_item_twice_renders_two_expanded_list_items() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness.get_by_label("項目を追加").click();
    harness.run();
    harness.get_by_label("項目を追加").click();
    harness.run();

    assert_eq!(harness.state().state().items.len(), 2);
    harness.get_by_label("item-1");
    harness.get_by_label("item-2");
}

/// テキスト入力欄（「メモ」）へ入力すると `StateStore` の `memo` へ反映されることを
/// 検証する（`Node::TextField` → `Event::TextFieldChanged` → `dispatch` の配線確認）。
#[test]
fn typing_into_memo_field_updates_state() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    // TextEdit はフォーカスを持つ間のみテキストイベントを受理するため、
    // 先にクリックしてフォーカスを与えてから入力する。
    harness.get_by_label("メモ").click();
    harness.run();
    harness.get_by_label("メモ").type_text("こんにちは");
    harness.run();

    assert_eq!(harness.state().state().memo, "こんにちは");
}

/// Issue #614: 単一行 TextField で Enter（IME 確定操作と同じキー）を押しても
/// フォーカスが外れず、クリックし直さず続けて入力できることを検証する。
/// 修正前は `text_edit_singleline` の既定挙動（`return_key` に一致すると
/// `surrender_focus` する）により 2 回目の `type_text` が無視されていた。
#[test]
fn pressing_enter_in_memo_field_keeps_focus_for_continuous_input() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness.get_by_label("メモ").click();
    harness.run();
    harness.get_by_label("メモ").type_text("こんにちは");
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    // 再クリックせずに続けて入力できることがフォーカス維持の証拠になる。
    harness.get_by_label("メモ").type_text("世界");
    harness.run();

    assert_eq!(harness.state().state().memo, "こんにちは世界");
}

/// wgpu バックエンドでの実レンダリング結果を PNG として保存できることを検証する。
///
/// 出力先は `CARGO_TARGET_TMPDIR`（`target/` 配下の使い捨てディレクトリ）とする
/// （CI ランナー上でジョブ間・並列実行間に VCS 管理下のファイルを
/// 書き換えると作業ツリー汚染の要因になるため）。エビデンス画像が必要な場合は
/// 実行後に `CARGO_TARGET_TMPDIR` 配下の出力を各自で控える。
#[test]
fn renders_to_png_evidence_for_verification_report() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    let image = harness
        .render()
        .expect("wgpu レンダリングが失敗した（デスクトップ相当環境の GPU/Vulkan 前提が未整備）");

    let out_dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(out_dir).expect("エビデンス出力ディレクトリの作成に失敗した");

    image
        .save(out_dir.join("desktop-kittest-wgpu.png"))
        .expect("PNG 保存に失敗した");
}

/// `is_adapter_unavailable_panic` の判定ロジック単体の検証（GPU 非依存）。
/// Issue #23 の Bugbot 指摘（アダプタ欠如以外の panic をスキップとして握りつぶす）が
/// 再発していないことを、実際の `Harness` 構築を伴わずに保証する。
#[cfg(test)]
mod is_adapter_unavailable_panic_tests {
    use super::is_adapter_unavailable_panic;

    #[test]
    fn matches_custom_native_adapter_selection_error_message() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(
            "Failed to create render state: CustomNativeAdapterSelectionError(\"No adapter found\")"
                .to_string(),
        );
        assert!(is_adapter_unavailable_panic(payload.as_ref()));
    }

    #[test]
    fn matches_static_str_payload_containing_no_adapter_found() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("No adapter found");
        assert!(is_adapter_unavailable_panic(payload.as_ref()));
    }

    #[test]
    fn does_not_match_unrelated_panic_message() {
        // API 不一致・初期化バグ等、アダプタ欠如とは無関係な panic の例。
        let payload: Box<dyn std::any::Any + Send> =
            Box::new("index out of bounds: the len is 0 but the index is 3".to_string());
        assert!(!is_adapter_unavailable_panic(payload.as_ref()));
    }

    #[test]
    fn does_not_match_non_string_payload() {
        // 文字列として取り出せない panic ペイロード（例: 独自エラー型）は
        // 安全側に倒し、アダプタ欠如としては扱わない。
        let payload: Box<dyn std::any::Any + Send> = Box::new(42_i32);
        assert!(!is_adapter_unavailable_panic(payload.as_ref()));
    }

    /// `build_harness_or_skip` そのもの（アダプタ欠如以外の panic は再送出してテストを
    /// 失敗させる）を、実際の `Harness` 構築を介さずに再現して確認する。
    #[test]
    #[should_panic(expected = "intentional non-adapter panic for regression coverage")]
    fn build_harness_or_skip_style_dispatch_repropagates_non_adapter_panics() {
        let result: Result<(), Box<dyn std::any::Any + Send>> =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                panic!("intentional non-adapter panic for regression coverage");
            }));

        if let Err(payload) = result {
            if !is_adapter_unavailable_panic(payload.as_ref()) {
                std::panic::resume_unwind(payload);
            }
        }
    }
}
