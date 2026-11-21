//! 直接依存で選んだ機能も、初めて使われたときに終了処理の登録表へ載ることの検査（Issue #1453）。
//!
//! この結合テストは `fandhe` の feature を経ずに本 crate だけへ依存する利用者と同じ立場で入口を呼ぶ。
//! 登録表を回すのは描画層（`fandhe-render-egui` の `ProxyApp`・`Menu`）で、その経路は同 crate の `tests/menu.rs` が固定する。

#[test]
fn first_use_registers_exit_and_screen_drop() {
    assert!(!fandhe_runtime::teardown::registered("memory-pressure"));
    let _ = fandhe_cap_memory_pressure::platform_memory_pressure();
    assert!(fandhe_runtime::teardown::registered("memory-pressure"));
    // 登録した処理（`shutdown`）は購読が無くても安全に呼べる。
    fandhe_runtime::teardown::run_exit();
    fandhe_runtime::teardown::run_screen_drop();
}
