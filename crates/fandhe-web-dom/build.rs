//! Apple のホストでテスト実行体をリンクするためのリンク指定の受け取り。
//!
//! 本クレートは Web（wasm32）向けだが、ホストの `cargo test` では機能 crate の契約（純関数）を使うテストが
//! 実行体になる。Swift の薄層・弱リンクを持つ機能 crate（on-device-llm・screen-capture）が出す
//! `cargo:rustc-link-arg`（弱リンク・`-rpath`）はその crate 自身の成果物にしか効かないため、`links` メタデータ
//! （`export_link_args`）経由で受け取って出し直す。Apple 以外（wasm32 を含む）では何もしない
//! （`fandhe-platform-apple` の `build` モジュール doc 参照）。

fn main() {
    fandhe_platform_apple::build::import_link_args("fandhe_cap_on_device_llm");
    fandhe_platform_apple::build::import_link_args("fandhe_cap_screen_capture");
}
