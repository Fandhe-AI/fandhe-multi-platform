//! 状態読み取りアクセサ `ReadFn<S, T>` / `ReadAccessor<S, T>`（TASK-13.1-S1 / REQ-13）。
//!
//! `ui` モジュールの `Node::TextField::get` や `Node::List::expand` は、状態 `S` への
//! アクセス経路をモノモーフィックな関数ポインタ `fn(&S) -> T` に限定している
//! （PoC-6: `docs/spec/03-poc/ai-optimization/dsl/src/lib.rs` で実証済みの設計）。
//! 本モジュールはその表記の SSOT（`ReadFn`）と、名前付き API・ドキュメントの置き場となる
//! newtype（`ReadAccessor`）を提供する。`Node::TextField::get` / `Node::List::expand` の
//! フィールド型は `ReadFn` を使う（TASK-13.1-S4、#32）。書き込み側 `fn(&mut S, T)`
//! （`Node::TextField::set` / `Node::Button::on_click`）は対称形の `WriteFn`（`write.rs`）が担う。
//! クロージャ拒否が実際にコンパイル段階で機能することは、`crates/fandhe-core/tests/compile_fail.rs`
//! （trybuild ベース、TASK-13.1-S3、#31）で網羅検証している。以下の doctest はその代表例を
//! rustdoc 上でも示す最小ケース。
//!
//! ```compile_fail
//! use fandhe_core::state::ReadAccessor;
//!
//! struct Counter { count: i32 }
//!
//! let bonus = 1;
//! // 環境をキャプチャするクロージャは `ReadFn<S, T> = fn(&S) -> T` へ coerce できず、
//! // コンパイル段階で拒否される（網羅検証は tests/compile_fail.rs を参照）。
//! let accessor: ReadAccessor<Counter, i32> = ReadAccessor::new(|state| state.count + bonus);
//! ```

/// 状態 `S` から値 `T` を読み取る関数ポインタ型。
///
/// `fn` ポインタに限定することで、環境をキャプチャするクロージャは代入できない
/// （キャプチャ付きクロージャは `fn` へ coerce 不能なため、コンパイル時に拒否される）。
/// これにより状態 `S` への暗黙の外部参照を型システムで遮断し、AI 生成コードに対する
/// 安全網とする設計（REQ-13 / NFR-9、PoC-6 踏襲）。
pub type ReadFn<S, T> = fn(&S) -> T;

/// 状態 `S` と index から値 `T` を読み取る関数ポインタ型（TASK-5.5-S2 / REQ-5）。
///
/// `Node::VirtualList::item`（`docs/architecture/list-virtualization-architecture.md`
/// §3.3 の契約）のように、状態に加えて「どの項目か」を表す `usize` index を
/// 追加で受け取る読み取りアクセサの SSOT 型エイリアス。[`ReadFn`] と同じ理由
/// （クロージャの環境キャプチャを型レベルで拒否する、REQ-13 / PoC-6 踏襲）で
/// `fn` ポインタに限定する。実体は生の関数ポインタであり、[`ReadFn`] との
/// 互換や自動変換は提供しない（引数の個数が異なるため代入互換ではない。
/// 本エイリアスは表記の集約のみを目的とする非破壊的な追加）。
pub type ReadIndexedFn<S, T> = fn(&S, usize) -> T;

/// `ReadFn<S, T>` を保持する名前付きアクセサ。
///
/// 生の関数ポインタ型 `ReadFn<S, T>` をそのまま各所へ渡すこともできるが、
/// 本型は状態アクセス層の入口として名前付き API・ドキュメントを集約する置き場となる。
/// 将来追加される書き込みアクセサ（S2 / #30）と対称な API 形状を保つ狙いもある。
///
/// `Debug` / `Clone` / `Copy` は手動実装する（[`crate::ui::Node`] と同じ理由）。
/// フィールドは関数ポインタ 1 つのみで状態型 `S` を所有しないため、`derive` マクロが
/// 自動付与する `S: Debug` / `S: Clone` という不要な境界を避ける。
///
/// # Examples
///
/// ```
/// use fandhe_core::state::ReadAccessor;
///
/// struct Counter {
///     count: i32,
/// }
///
/// fn read_count(state: &Counter) -> i32 {
///     state.count
/// }
///
/// let accessor: ReadAccessor<Counter, i32> = ReadAccessor::new(read_count);
/// let state = Counter { count: 3 };
/// assert_eq!(accessor.get(&state), 3);
/// ```
pub struct ReadAccessor<S, T> {
    read: ReadFn<S, T>,
}

impl<S, T> ReadAccessor<S, T> {
    /// 名前付き関数（または非キャプチャクロージャの `fn` への coercion）から構築する。
    pub fn new(read: ReadFn<S, T>) -> Self {
        Self { read }
    }

    /// 状態 `S` から値 `T` を読み取る。
    pub fn get(&self, state: &S) -> T {
        (self.read)(state)
    }

    /// 内部の関数ポインタを取り出す。既存 `ui` モジュールの生 `fn` ポインタ API
    /// （`Node::TextField::get` 等）へ橋渡しする用途を想定する。
    pub fn as_fn(&self) -> ReadFn<S, T> {
        self.read
    }
}

impl<S, T> std::fmt::Debug for ReadAccessor<S, T> {
    /// デバッグ出力用。関数ポインタのアドレスではなく型名のみを表示する
    /// （[`crate::ui::Node`] の `Debug` 実装と同じ方針）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadAccessor").finish()
    }
}

impl<S, T> Clone for ReadAccessor<S, T> {
    /// 関数ポインタは `S` / `T` に依らず複製可能なため、状態型 `S` に境界を課さず複製できる。
    fn clone(&self) -> Self {
        *self
    }
}

impl<S, T> Copy for ReadAccessor<S, T> {}

#[cfg(test)]
mod tests {
    use super::{ReadAccessor, ReadFn};

    /// テスト専用の状態型。`S: Clone` を実装しないことで、`ReadAccessor` の
    /// `Debug` / `Clone` / `Copy` が `S` に不要な境界を課していないことを確認する。
    struct CounterState {
        count: i32,
        note: String,
    }

    fn read_count(state: &CounterState) -> i32 {
        state.count
    }

    fn read_note(state: &CounterState) -> String {
        state.note.clone()
    }

    #[test]
    fn get_reads_i32_from_named_function() {
        let accessor: ReadAccessor<CounterState, i32> = ReadAccessor::new(read_count);
        let state = CounterState {
            count: 42,
            note: String::new(),
        };

        assert_eq!(accessor.get(&state), 42);
    }

    #[test]
    fn get_reads_string_from_named_function() {
        let accessor: ReadAccessor<CounterState, String> = ReadAccessor::new(read_note);
        let state = CounterState {
            count: 0,
            note: "初期値".to_string(),
        };

        assert_eq!(accessor.get(&state), "初期値");
    }

    #[test]
    fn new_accepts_non_capturing_closure_coerced_to_fn_pointer() {
        // 環境をキャプチャしないクロージャは `fn` ポインタへ coerce 可能であり、
        // `new` の引数として受理される（受理側の境界確認）。
        let accessor: ReadAccessor<CounterState, i32> = ReadAccessor::new(|state| state.count);
        let state = CounterState {
            count: 7,
            note: String::new(),
        };

        assert_eq!(accessor.get(&state), 7);
    }

    #[test]
    fn copy_and_clone_work_without_state_bounds() {
        // CounterState は Clone を実装しないため、この呼び出しが成立すること自体が
        // ReadAccessor の Clone/Copy が S に不要な境界を課していない証拠になる。
        let accessor: ReadAccessor<CounterState, i32> = ReadAccessor::new(read_count);
        let copied = accessor;
        let cloned = Clone::clone(&accessor);

        let state = CounterState {
            count: 5,
            note: String::new(),
        };
        assert_eq!(copied.get(&state), 5);
        assert_eq!(cloned.get(&state), 5);
    }

    #[test]
    fn debug_output_does_not_panic() {
        let accessor: ReadAccessor<CounterState, i32> = ReadAccessor::new(read_count);
        let output = format!("{accessor:?}");
        assert_eq!(output, "ReadAccessor");
    }

    #[test]
    fn read_fn_is_assignment_compatible_with_existing_ui_get_signature() {
        // Node::TextField::get の型は `fn(&S) -> String` であり、ReadFn<S, String> と
        // 同一表記であることを型レベルで確認する（既存 ui モジュールとの整合性）。
        let f: ReadFn<CounterState, String> = read_note;
        let state = CounterState {
            count: 0,
            note: "値".to_string(),
        };
        assert_eq!(f(&state), "値");
    }
}
