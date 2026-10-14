//! レイアウトプロトコル（サイズ計測・配置）基盤。
//!
//! `docs/architecture/widget-architecture.md`（S1、PR #351）が規定する「`measure` 契約
//! の具体化」（§4.3）・「`Widget<S>::measure` の呼び出し元の実装」（§5）を担う
//! （TASK-8.3-S4 / REQ-8 / M3、Issue #99。親 issue #95 の分解 sub-issue 4/60）。
//!
//! `taffy`（REQ-8 明示採用のレイアウトエンジン）を内部実装として使うが、taffy の型は
//! 本モジュールの公開 API に一切露出しない。エンジン差し替え・バージョン更新の影響を
//! ここへ閉じ込めるための境界であり、呼び出し元（プラットフォームレイヤの描画走査・
//! 将来の `Widget<S>` 実装層）は本モジュールの型のみを参照すればよい。
//!
//! - [`geometry::LayoutSize`][] / [`geometry::LayoutPoint`][] / [`geometry::LayoutRect`][]:
//!   レイアウト専用の最小ジオメトリ。`ui::geometry`（Issue #98）との名前衝突を避けて
//!   `Layout` 接頭辞を付けている。両者の型統合は後続作業とする
//!   （out-of-scope-tracking.md 対象）
//! - [`style::LayoutStyle`][] / [`style::Dimension`][] / [`style::FlexDirection`][]:
//!   宣言層へ見せるスタイル語彙（4h スコープの最小サブセット）
//! - [`measure::Measure`][] / [`measure::LayoutConstraints`][] / [`measure::AvailableSpace`][]:
//!   サイズ計測プロトコル。`Widget<S>` トレイト（S2 / Issue #97）とはまだ未接続であり、
//!   `Widget<S>::measure` はこのトレイトの実装として接続される想定
//! - [`tree::LayoutTree`][] / [`tree::LayoutNodeId`][]: レイアウトツリー構築・制約解決
//!   （taffy の `TaffyTree` をラップする、本モジュールの中核）
//! - [`error::LayoutError`][]: 未登録ノード参照・不正な制約値（NaN・無限大・負値）を
//!   `Result` で返すためのエラー型

mod error;
mod geometry;
mod measure;
mod style;
mod tree;

pub use error::LayoutError;
pub use geometry::{LayoutPoint, LayoutRect, LayoutSize};
pub use measure::{AvailableSpace, LayoutConstraints, Measure};
pub use style::{Dimension, FlexDirection, LayoutStyle};
pub use tree::{LayoutNodeId, LayoutTree};
