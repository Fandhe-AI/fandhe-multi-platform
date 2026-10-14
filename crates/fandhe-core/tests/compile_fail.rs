//! クロージャ環境キャプチャ拒否の compile-fail 網羅検証スイート
//! （TASK-13.1-S3 / REQ-13 / NFR-9、親 issue #28、S1: #29 の後続）。
//!
//! `state::ReadFn` / `ReadAccessor`（`state/read.rs`）と `ui` モジュール
//! （`Screen::button` / `Screen::list` / `Node::TextField`）は、状態アクセス経路を
//! `fn(&S) -> T` / `fn(&mut S, T)` の関数ポインタに限定している。本スイートは
//! 「環境をキャプチャするクロージャがこれらの API へ渡された場合、実際にコンパイル段階で
//! 拒否される」ことを [trybuild](https://docs.rs/trybuild) で実行可能な形に固定化し、
//! 将来の API 変更（例: 引数を `impl Fn` へ緩めてしまう等）で安全網が静かに失われることを
//! 防ぐリグレッション検知手段とする。
//!
//! `docs/spec/03-poc/ai-optimization/experiments/error-injection/src/bin/case05_closure_capture_as_fnptr.rs`
//! （PoC-6）で実証済みの拒否パターンを、`fandhe_core` コードベース向けのケース群として踏襲する。
//!
//! `tests/compile_fail/*.rs` は拒否されるべきケース（`compile_fail`）、
//! `tests/compile_fail/pass/*.rs` は制約が過剰拒否していないことを固定化する
//! 受理されるべきケース（`pass`）として登録する。
//!
//! 対応する `.stderr` は `TRYBUILD=overwrite cargo test --test compile_fail` で生成し、
//! closure → fn ポインタ coercion 拒否の診断（E0308 系）が含まれることを確認したうえで
//! コミット対象に含める（診断文言は rustc バージョンに依存するため、CI 環境の toolchain と
//! 大きく異なる場合は再生成が必要になる。リスクは計画書 #31 の §8 を参照）。
#[test]
fn closure_capture_is_rejected_at_compile_time() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/compile_fail/*.rs");
    cases.pass("tests/compile_fail/pass/*.rs");
}
