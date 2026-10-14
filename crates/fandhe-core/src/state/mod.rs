//! 状態アクセス層の入口（REQ-13 / TASK-13.1、AI 型安全 DSL の一部）。
//!
//! `ui` モジュールが個別に持つ状態アクセス用の関数ポインタ表記を、状態管理レイヤ
//! 共通の型として集約する。読み取り側 `fn(&S) -> T`（[`ReadFn`] / [`ReadAccessor`]）と
//! 書き込み側 `fn(&mut S, T)`（[`WriteFn`] / [`WriteAccessor`]）を提供する
//! （TASK-13.1-S1/S2、#29・#30）。
//!
//! `ReadFn` / `ReadAccessor` / `WriteAccessor`（および `ui` モジュールの
//! `Screen::button` / `Screen::list` / `Node::TextField`）が、環境をキャプチャする
//! クロージャをコンパイル段階で拒否することは、`crates/fandhe-core/tests/compile_fail.rs`
//! の trybuild ベース compile-fail 網羅検証スイートで固定化している（TASK-13.1-S3、#31）。
//!
//! `ui` モジュール側の `Node::TextField::get` / `Node::List::expand` / `Node::TextField::set`
//! は [`ReadFn`] / [`WriteFn`] を関数ポインタ表記として使う（TASK-13.1-S4、#32・#307）。
//! PoC-6（`docs/spec/03-poc/ai-optimization/dsl/src/lib.rs`）との API 面・挙動の整合は
//! ユニットテスト（`tests/poc6_alignment.rs`）で確認している。
//!
//! 上記の型定義層に加えて、「イベント発生 → 状態 `S` の更新 → 再描画判定」の配線を
//! [`store::Event`] / [`store::StateStore`] として提供する（TASK-1.1-S5、#21）。
//! こちらは `ui` モジュールが提供する公開 API（`ui::click_button` 等）を呼び出す側であり、
//! 依存方向は `state` → `ui`（型定義のみを提供する `read` / `write` とは異なる関係）。
//!
//! `ui::Node::VirtualList::item`（`docs/architecture/list-virtualization-architecture.md`
//! §3.3）は「状態 `S` と index から値を読み取る」関数ポインタ表記として
//! [`read::ReadIndexedFn`] を使う（TASK-5.5-S2、#85）。[`ReadFn`] とは引数の個数が異なる
//! ため代入互換ではなく、表記を集約する別の型エイリアスとして存在する。

mod read;
mod store;
mod write;

pub use read::{ReadAccessor, ReadFn, ReadIndexedFn};
pub use store::{Event, StateStore, WidgetValue};
pub use write::{WriteAccessor, WriteFn};
