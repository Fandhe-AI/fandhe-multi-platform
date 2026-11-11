//! マウント破棄時に呼ぶ解放関数の登録表。
//!
//! 機能ごとの `release_mount(mount_id)` を共通層が名指しすると依存が 共通層 → 機能 になるため、
//! 機能側から登録する形にする。`mount` モジュール（破棄の検知）から [`run`] が呼ばれる。

use std::cell::RefCell;

/// 登録キーと解放関数の組。
type Hook = (&'static str, fn(u64));

thread_local! {
    static HOOKS: RefCell<Vec<Hook>> = const { RefCell::new(Vec::new()) };
}

/// `key` の解放関数を登録する。同じ `key` の再登録は無視する（effect の入口で毎回呼んでよい）。
pub(crate) fn register(key: &'static str, release: fn(u64)) {
    HOOKS.with(|h| {
        let mut h = h.borrow_mut();
        if !h.iter().any(|(k, _)| *k == key) {
            h.push((key, release));
        }
    });
}

/// 破棄された `mount_id` について、登録済みの全関数を登録順に 1 回ずつ呼ぶ。
/// 関数の中から [`register`] されても二重借用にならないよう、複製してから借用を落として呼ぶ。
pub(crate) fn run(mount_id: u64) {
    let hooks: Vec<fn(u64)> = HOOKS.with(|h| h.borrow().iter().map(|(_, f)| *f).collect());
    for f in hooks {
        f(mount_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static SEEN: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
    }

    fn a(id: u64) {
        SEEN.with(|s| s.set((s.get().0 + id, s.get().1)));
    }
    fn b(id: u64) {
        SEEN.with(|s| s.set((s.get().0, s.get().1 + id)));
    }
    fn reentrant(_: u64) {
        register("re-inner", a);
    }

    #[test]
    fn same_key_runs_once_and_ids_pass_through() {
        register("a", a);
        register("a", a);
        register("b", b);
        run(7);
        assert_eq!(SEEN.with(Cell::get), (7, 7));
    }

    #[test]
    fn register_from_inside_release_does_not_panic() {
        register("re", reentrant);
        run(1);
        run(1);
    }
}
