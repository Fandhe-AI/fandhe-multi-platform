//! 状態書き込みアクセサ `WriteFn<S, T>` / `WriteAccessor<S, T>`（TASK-13.1-S2 / REQ-13）。
//!
//! [`crate::state::read`] の読み取りアクセサと対称に、`ui` モジュールの
//! `Node::TextField::set` が採用する書き込み経路 `fn(&mut S, T)` の表記を集約する
//! （PoC-6: `docs/spec/03-poc/ai-optimization/dsl/src/lib.rs` で実証済みの設計）。
//! 本モジュールはその表記の SSOT（`WriteFn`）と、名前付き API・ドキュメントの
//! 置き場となる newtype（`WriteAccessor`）を提供する。クロージャ拒否の compile-fail
//! 網羅検証は `crates/fandhe-core/tests/compile_fail.rs`（TASK-13.1-S3、#31）が担当し、
//! `ui` モジュール（`Node::TextField::set`）は `WriteFn` を型として使う（#307）。

/// 状態 `S` へ値 `T` を書き込む関数ポインタ型。
///
/// [`crate::state::ReadFn`] と対称に `fn` ポインタへ限定することで、環境をキャプチャする
/// クロージャは代入できない（キャプチャ付きクロージャは `fn` へ coerce 不能なため、
/// コンパイル時に拒否される）。これにより状態 `S` への暗黙の外部参照を型システムで
/// 遮断し、AI 生成コードに対する安全網とする設計（REQ-13 / NFR-9、PoC-6 踏襲）。
pub type WriteFn<S, T> = fn(&mut S, T);

/// `WriteFn<S, T>` を保持する名前付きアクセサ。
///
/// 生の関数ポインタ型 `WriteFn<S, T>` をそのまま各所へ渡すこともできるが、
/// 本型は状態アクセス層の入口として名前付き API・ドキュメントを集約する置き場となる。
/// [`crate::state::ReadAccessor`]（S1・#29）と対称な API 形状を保つ（`get` に対する `set`）。
///
/// `Debug` / `Clone` / `Copy` は手動実装する（[`crate::state::ReadAccessor`] / [`crate::ui::Node`]
/// と同じ理由）。フィールドは関数ポインタ 1 つのみで状態型 `S` を所有しないため、
/// `derive` マクロが自動付与する `S: Debug` / `S: Clone` という不要な境界を避ける。
///
/// # Examples
///
/// ```
/// use fandhe_core::state::WriteAccessor;
///
/// struct Counter {
///     count: i32,
/// }
///
/// fn write_count(state: &mut Counter, value: i32) {
///     state.count = value;
/// }
///
/// let accessor: WriteAccessor<Counter, i32> = WriteAccessor::new(write_count);
/// let mut state = Counter { count: 0 };
/// accessor.set(&mut state, 5);
/// assert_eq!(state.count, 5);
/// ```
pub struct WriteAccessor<S, T> {
    write: WriteFn<S, T>,
}

impl<S, T> WriteAccessor<S, T> {
    /// 名前付き関数（または非キャプチャクロージャの `fn` への coercion）から構築する。
    pub fn new(write: WriteFn<S, T>) -> Self {
        Self { write }
    }

    /// 状態 `S` へ値 `T` を書き込む（[`crate::state::ReadAccessor::get`] と対称なメソッド名）。
    pub fn set(&self, state: &mut S, value: T) {
        (self.write)(state, value)
    }

    /// 内部の関数ポインタを取り出す。既存 `ui` モジュールの生 `fn` ポインタ API
    /// （`Node::TextField::set` 等）へ橋渡しする用途を想定する。
    pub fn as_fn(&self) -> WriteFn<S, T> {
        self.write
    }
}

impl<S, T> std::fmt::Debug for WriteAccessor<S, T> {
    /// デバッグ出力用。関数ポインタのアドレスではなく型名のみを表示する
    /// （[`crate::state::ReadAccessor`] の `Debug` 実装と同じ方針）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteAccessor").finish()
    }
}

impl<S, T> Clone for WriteAccessor<S, T> {
    /// 関数ポインタは `S` / `T` に依らず複製可能なため、状態型 `S` に境界を課さず複製できる。
    fn clone(&self) -> Self {
        *self
    }
}

impl<S, T> Copy for WriteAccessor<S, T> {}

#[cfg(test)]
mod tests {
    use super::{WriteAccessor, WriteFn};

    /// テスト専用の状態型。`S: Clone` を実装しないことで、`WriteAccessor` の
    /// `Debug` / `Clone` / `Copy` が `S` に不要な境界を課していないことを確認する。
    struct CounterState {
        count: i32,
        note: String,
    }

    fn write_count(state: &mut CounterState, value: i32) {
        state.count = value;
    }

    fn write_note(state: &mut CounterState, value: String) {
        state.note = value;
    }

    #[test]
    fn set_writes_i32_via_named_function() {
        let accessor: WriteAccessor<CounterState, i32> = WriteAccessor::new(write_count);
        let mut state = CounterState {
            count: 0,
            note: String::new(),
        };

        accessor.set(&mut state, 42);

        assert_eq!(state.count, 42);
    }

    #[test]
    fn set_writes_string_via_named_function() {
        let accessor: WriteAccessor<CounterState, String> = WriteAccessor::new(write_note);
        let mut state = CounterState {
            count: 0,
            note: String::new(),
        };

        accessor.set(&mut state, "更新後".to_string());

        assert_eq!(state.note, "更新後");
    }

    #[test]
    fn new_accepts_non_capturing_closure_coerced_to_fn_pointer() {
        // 環境をキャプチャしないクロージャは `fn` ポインタへ coerce 可能であり、
        // `new` の引数として受理される（受理側の境界確認）。
        let accessor: WriteAccessor<CounterState, i32> =
            WriteAccessor::new(|state, value| state.count = value);
        let mut state = CounterState {
            count: 0,
            note: String::new(),
        };

        accessor.set(&mut state, 7);

        assert_eq!(state.count, 7);
    }

    #[test]
    fn copy_and_clone_work_without_state_bounds() {
        // CounterState は Clone を実装しないため、この呼び出しが成立すること自体が
        // WriteAccessor の Clone/Copy が S に不要な境界を課していない証拠になる。
        let accessor: WriteAccessor<CounterState, i32> = WriteAccessor::new(write_count);
        let copied = accessor;
        let cloned = Clone::clone(&accessor);

        let mut state = CounterState {
            count: 0,
            note: String::new(),
        };
        copied.set(&mut state, 5);
        assert_eq!(state.count, 5);

        cloned.set(&mut state, 9);
        assert_eq!(state.count, 9);
    }

    #[test]
    fn debug_output_does_not_panic() {
        let accessor: WriteAccessor<CounterState, i32> = WriteAccessor::new(write_count);
        let output = format!("{accessor:?}");
        assert_eq!(output, "WriteAccessor");
    }

    #[test]
    fn write_fn_is_assignment_compatible_with_existing_ui_set_signature() {
        // Node::TextField::set は #307 で WriteFn<S, String> 宣言へ統合済み。
        // 統合後も代入互換であることを型レベルで確認する（既存 ui モジュールとの整合性）。
        let f: WriteFn<CounterState, String> = write_note;
        let mut state = CounterState {
            count: 0,
            note: String::new(),
        };
        f(&mut state, "値".to_string());
        assert_eq!(state.note, "値");
    }

    #[test]
    fn as_fn_returns_working_function_pointer() {
        let accessor: WriteAccessor<CounterState, i32> = WriteAccessor::new(write_count);
        let raw: WriteFn<CounterState, i32> = accessor.as_fn();
        let mut state = CounterState {
            count: 0,
            note: String::new(),
        };

        raw(&mut state, 3);

        assert_eq!(state.count, 3);
    }
}
