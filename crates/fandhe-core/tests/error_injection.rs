//! 型/参照エラー検知安全網の拡張検証ハーネス（TASK-13.3 / REQ-13 / NFR-9、親 issue #6、issue #37）。
//!
//! PoC-6（`docs/spec/03-poc/ai-optimization/experiments/error-injection/src/bin/case01〜12`）が
//! 実証した「AI が生成しがちな型/参照エラー系の誤り 12 種」を、`fandhe_core` の
//! 公開 API（`Screen<S>` / `Node<S>` / `state::{ReadFn, WriteFn, ReadAccessor, WriteAccessor}` /
//! `StateStore`）に対して注入し、Rust コンパイラがビルド段階で実際に検知・拒否することを
//! [trybuild](https://docs.rs/trybuild) で固定化する。
//!
//! ## 既存 `tests/compile_fail.rs`（TASK-13.1-S3）との責務分界
//!
//! `tests/compile_fail.rs` は「クロージャ環境キャプチャ拒否」という 1 種類の誤りを
//! `ReadAccessor` / `Screen::button` / `Screen::list` / `Node::TextField` の各経路で
//! 9 バリエーション網羅する特化スイートである。本スイート（`tests/error_injection/`）は
//! それとは独立に、PoC-6 の誤り分類 12 種全域（未定義フィールド・型不整合・所有権・借用・
//! 引数個数・トレイト境界・未定義メソッド・戻り値型・ジェネリック状態型不一致等）を
//! カバーする。両スイートは `tests/compile_fail/*.rs` と `tests/error_injection/*.rs` で
//! ディレクトリを分離しており、trybuild の glob 登録が重複・衝突しないようにしている
//! （`case05_closure_capture_as_fnptr.rs` は分類上クロージャキャプチャ拒否と重なるが、
//! これは PoC-6 の 12 分類のうち 1 項目として本スイートに含める設計判断であり、
//! 既存スイートとの重複起票ではない）。
//!
//! ## ケースの設計方針
//!
//! 各ケースは「単一の誤りのみを含み、その誤りを直せばコンパイルが通る」ことを保証する
//! （複合エラーは検知理由の特定を曖昧にするため避ける、実装計画 §3 手順 2 の方針）。
//! PoC-6 のケースのうち、素の Rust だけで完結していたもの（所有権・借用エラー系の
//! 02/03/04/06/09/10/12）は、可能な範囲で `fandhe_core` の実 API を経由する形に置き換えている。
//! ただし `case12`（クロージャへのムーブ後の再使用）は、`fandhe_core` の関数ポインタ限定 API
//! （`ReadFn`/`WriteFn`）へ move クロージャを渡すと「fn ポインタへ coerce 不能」という
//! 別種のエラー（E0308、`case05` と同じ分類）が先に発生し、狙った use-after-move
//! （E0382）を検証できなくなるため、意図的に素の Rust のまま据え置く
//! （PoC-6 の元ケースからの差分）。
//!
//! ## `.stderr` の再生成手順
//!
//! `TRYBUILD=overwrite cargo test -p fandhe-core --test error_injection` で生成する。
//! trybuild の診断文言は rustc バージョンに依存するため（`tests/compile_fail.rs` と同様の
//! 既知の制約）、CI 環境の toolchain と手元の環境が大きく異なる場合は再生成が必要になる。
//! 生成後は各 `.stderr` に想定エラーコード（ケースファイル冒頭コメント参照）が含まれることを
//! 確認したうえでコミット対象に含める。
//!
//! `tests/error_injection/*.rs` は拒否されるべきケース（`compile_fail`）、
//! `tests/error_injection/pass/*.rs` は制約が過剰拒否していないことを固定化する
//! 受理されるべきケース（`pass`）として登録する。
#[test]
fn injected_type_and_reference_errors_are_rejected_at_compile_time() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/error_injection/*.rs");
    cases.pass("tests/error_injection/pass/*.rs");
}
