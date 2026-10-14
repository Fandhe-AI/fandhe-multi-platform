//! 型付きトークン参照のカテゴリ取り違え拒否を固定化する compile-fail 検証スイート
//! （TASK-8.4-S19、#175）。
//!
//! `tests/compile_fail.rs`（TASK-13.1-S3）はクロージャ環境キャプチャ拒否専用の
//! スイートであり、本ファイルは `token::typed::TypedTokenRef<K>` の型パラメータ
//! `K` によるカテゴリ不一致検出専用として分離する（対象とする安全網の性質が
//! 異なるため、`tests/compile_fail.rs` へは混在させない）。
//!
//! `docs/architecture/design-token-architecture.md` §5.3 が定める「カテゴリ
//! ごとに閉じた型を引数に取る型付きアクセサ」の契約が、実際にコンパイル段階で
//! 機能する（型パラメータ・型エイリアスの異なる `TypedTokenRef` 同士を取り違えた
//! 場合に拒否される）ことを [trybuild](https://docs.rs/trybuild) で固定化し、
//! 将来の API 変更で安全網が静かに失われることを防ぐリグレッション検知手段とする。
//!
//! `tests/token_typed_compile_fail/*.rs` は拒否されるべきケース（`compile_fail`）、
//! `tests/token_typed_compile_fail/pass/*.rs` は制約が過剰拒否していないことを
//! 固定化する受理されるべきケース（`pass`）として登録する。
//!
//! 対応する `.stderr` は
//! `TRYBUILD=overwrite cargo test -p fandhe-core --test token_typed_compile_fail`
//! で生成し、型不一致の診断（E0308 系）が含まれることを確認したうえでコミット
//! 対象に含める（診断文言は rustc バージョンに依存するため、`rust-toolchain.toml`
//! が固定する 1.88.0 と CI 環境の toolchain が一致することを前提とする）。
#[test]
fn typed_token_ref_category_mismatch_is_rejected_at_compile_time() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/token_typed_compile_fail/*.rs");
    cases.pass("tests/token_typed_compile_fail/pass/*.rs");
}
