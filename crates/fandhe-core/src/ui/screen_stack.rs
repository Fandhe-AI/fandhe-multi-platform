//! 複数 `Screen<S>` を重ねて戻る画面遷移スタック（`ScreenStack<S>`、TASK-1.1 系列 /
//! Issue #726）。
//!
//! `fandhe-prototype` は `AppState::route` を書き換え、プラットフォーム側が毎フレーム
//! `current_screen(state)` を引き直す方式（`crates/fandhe-prototype/src/nav.rs`）で
//! 画面遷移を実証済みである。本型はこのパターンを共通コアの型として一般化したもので、
//! 状態 `S` は単一共有のまま、画面ごとに `fn(&S) -> Screen<S>` を積み重ねて管理する。
//!
//! 構築関数は現在の状態を読み取り専用（`&S`）で受け取るため、ボタンの文言など状態で変わる
//! 要素を持つ画面を積める（Issue #1289）。REQ-13（状態書き込みは `fn(&mut S)` 限定）とは、
//! 構築側に書き込み口（`&mut S`）を渡さないこと、保持するのが関数ポインタのみで
//! クロージャの環境キャプチャが型レベルで拒否されることで両立する。状態の更新は従来どおり
//! ボタンハンドラ等の `fn(&mut S)` だけが行い、構築関数は更新後の状態から毎フレーム
//! 組み直される（描画側の `fandhe-render-egui` が毎フレーム呼び直すため、キャッシュしない）。
//! `ScreenStack<S>` は状態型 `S` のフィールドとして保持する想定（例:
//! `crate::screens::list_detail::ListDetailState::nav`）。ボタンハンドラ
//! （`fn(&mut S)`）が `state.nav.push(..)` / `state.nav.pop()` を呼ぶことで、
//! [`Screen::button`] の関数ポインタ制約を満たしたまま画面遷移を表現できる。
//!
//! 呼び出し元（プラットフォームレイヤ・`render_headless`）は [`ScreenStack::current`]
//! （状態を渡して呼ぶ）が返す `Screen<S>` のみを描く契約とし、スタック全体を意識しない
//! （`docs/ai/SCHEMA.md` パターン 8 参照）。

use super::screen::Screen;

/// 画面ビルダー（`fn(&S) -> Screen<S>`）の積み重ね。常に 1 枚以上を保持する
/// （[`Self::new`] で root を必ず積むため、[`Self::current`] が空になることはない）。
///
/// `Debug` / `Clone` は手動実装する（[`Screen<S>`] と同様、状態型 `S` を所有しないため
/// `derive` マクロが不要な `S: Debug` / `S: Clone` 境界を課すのを避ける）。
pub struct ScreenStack<S> {
    stack: Vec<fn(&S) -> Screen<S>>,
}

impl<S> std::fmt::Debug for ScreenStack<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenStack")
            .field("depth", &self.stack.len())
            .finish()
    }
}

impl<S> Clone for ScreenStack<S> {
    fn clone(&self) -> Self {
        Self {
            stack: self.stack.clone(),
        }
    }
}

impl<S> ScreenStack<S> {
    /// `root` を唯一の画面として積んだスタックを作る。`root` は [`Self::pop`] で
    /// 取り除かれない（最後の 1 枚として常に残る、モジュール冒頭の不変条件）。
    pub fn new(root: fn(&S) -> Screen<S>) -> Self {
        Self { stack: vec![root] }
    }

    /// 画面を積む（末尾追加）。積んだ直後に [`Self::current`] を呼ぶと `screen`
    /// が返る。
    pub fn push(&mut self, screen: fn(&S) -> Screen<S>) {
        self.stack.push(screen);
    }

    /// 直前の画面へ戻る（末尾を取り除く）。root（スタックが 1 枚のみ）の場合は
    /// 取り除かず `false` を返す（panic を API 契約にしない、coding-rust.md 準拠）。
    /// 取り除けた場合は `true` を返す。
    pub fn pop(&mut self) -> bool {
        if self.stack.len() <= 1 {
            return false;
        }
        self.stack.pop();
        true
    }

    /// 現在表示すべき画面を、末尾のビルダーに `state` を読み取り専用で渡して構築する。
    /// 呼び出しのたびに都度構築するため、状態が変われば表示も変わる
    /// （ビルダーは環境キャプチャを持たない `fn(&S)` で、書き込めない、REQ-13）。
    ///
    /// `state` は本スタックを保持する状態そのもの（`state.nav.current(state)`）を渡す想定。
    pub fn current(&self, state: &S) -> Screen<S> {
        // `new` が root を必ず積むため、`stack` が空になることはない
        // （`expect` は初期化不変条件の保証であり、実装コードの `unwrap` 禁止の
        // 例外に当たる、coding-rust.md 参照）。
        let top = self
            .stack
            .last()
            .expect("ScreenStack は常に 1 枚以上の画面を保持する");
        top(state)
    }

    /// 現在の積み枚数（root を含む）。
    pub fn depth(&self) -> usize {
        self.stack.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyState {
        count: u32,
    }

    const S0: DummyState = DummyState { count: 0 };

    fn root_screen(_: &DummyState) -> Screen<DummyState> {
        Screen::new("root")
    }

    fn detail_screen(_: &DummyState) -> Screen<DummyState> {
        Screen::new("detail")
    }

    fn counter_screen(state: &DummyState) -> Screen<DummyState> {
        fn inc(state: &mut DummyState) {
            state.count += 1;
        }
        Screen::new("counter").button(
            crate::ui::WidgetId::from_raw(1),
            if state.count == 0 { "開始" } else { "続行" },
            inc,
        )
    }

    #[test]
    fn new_holds_only_root_at_depth_one() {
        let stack: ScreenStack<DummyState> = ScreenStack::new(root_screen);
        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.current(&S0).title(), "root");
    }

    #[test]
    fn push_switches_current_and_increases_depth() {
        let mut stack: ScreenStack<DummyState> = ScreenStack::new(root_screen);
        stack.push(detail_screen);
        assert_eq!(stack.depth(), 2);
        assert_eq!(stack.current(&S0).title(), "detail");
    }

    #[test]
    fn pop_returns_to_previous_screen() {
        let mut stack: ScreenStack<DummyState> = ScreenStack::new(root_screen);
        stack.push(detail_screen);

        let popped = stack.pop();

        assert!(popped);
        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.current(&S0).title(), "root");
    }

    #[test]
    fn pop_on_root_is_no_op_and_returns_false() {
        let mut stack: ScreenStack<DummyState> = ScreenStack::new(root_screen);

        let popped = stack.pop();

        assert!(!popped);
        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.current(&S0).title(), "root");
    }

    #[test]
    fn current_rebuilds_from_state_and_changes_after_update() {
        use crate::ui::Node;
        let mut stack: ScreenStack<DummyState> = ScreenStack::new(root_screen);
        stack.push(counter_screen);
        let mut state = DummyState { count: 0 };

        let label = |screen: &Screen<DummyState>| match screen.iter().next() {
            Some(Node::Button { label, .. }) => label.clone(),
            _ => panic!("Button 以外"),
        };
        assert_eq!(label(&stack.current(&state)), "開始");

        // 状態の書き込みは `fn(&mut S)` のハンドラ経由のみ（REQ-13）。更新後は同じスタックから
        // 組み直した画面の文言が変わる。
        assert!(crate::ui::click_button(
            &stack.current(&state),
            "開始",
            &mut state
        ));
        assert_eq!(label(&stack.current(&state)), "続行");
        assert_eq!(stack.depth(), 2);
    }
}
