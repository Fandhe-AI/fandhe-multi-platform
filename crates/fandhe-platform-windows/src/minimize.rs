//! メインウィンドウの最小化・復元の切り替わりを配る口（TASK-21.4 / Issue #1331）。
//!
//! 最小化中は背景のフレームが再描画を予約せず、センサーのポーリングが止まる。そのため端末機能のうち
//! 「最小化で保持を手放し、復元で取り直す」ものが同じ判定を必要とする（environment-sensors・motion-raw-sensors）。
//! 以前は機能どうしが互いの OS 実装を直接呼び合っていたが、機能 crate 同士を依存させないため、判定と配信をここへ集める。
//!
//! - ウォッチャーはプロセスに 1 本。`IsIconic` を 200 ms 間隔で見て、立ち上がり・立ち下がりで登録された受け口を
//!   登録順に呼ぶ。受け口の一覧はロックの外へ写してから呼ぶ（受け口の中で別のロックを取ってもデッドロックしない）
//! - 復元のときは受け口を呼んだ後に `InvalidateRect` で 1 フレーム起こす（背景のまま走るフレームが再描画を予約せず、
//!   ポーリングが止まったままになるのを防ぐ。実機での確認は人間担当）
//! - 同じ受け口（関数のアドレス）は 1 度だけ登録する。登録した時点で最小化中なら、その受け口へ `Minimized` を 1 回届ける
//!   （後から使い始めた機能が、すでに起きた立ち上がりを見逃して最小化中に資源を取らないため）
//!
//! 守る範囲: 立ち上がり・立ち下がりを 1 回ずつ配ること、初期状態の再送を後続の配信より先に済ませること、受け口を呼ぶ間はロックを握らないこと。
//! 守らない範囲: 200 ms 未満の最小化・復元の取りこぼし、ウィンドウが無い間（`window()` が `None`）の判定。

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::WindowsAndMessaging::IsIconic;

/// 最小化を調べる間隔。
const POLL_MS: u64 = 200;

/// 最小化の立ち上がり・立ち下がり。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinimizeEdge {
    /// 最小化された。
    Minimized,
    /// 最小化から復元された。
    Restored,
}

/// 最小化状態の変化検出。同じ状態の連続では何も返さない。
#[derive(Debug, Default)]
struct EdgeTracker {
    was: bool,
}

impl EdgeTracker {
    fn update(&mut self, minimized: bool) -> Option<MinimizeEdge> {
        let prev = std::mem::replace(&mut self.was, minimized);
        match (prev, minimized) {
            (false, true) => Some(MinimizeEdge::Minimized),
            (true, false) => Some(MinimizeEdge::Restored),
            _ => None,
        }
    }
}

struct Shared {
    listeners: Vec<fn(MinimizeEdge)>,
    started: bool,
    minimized: bool,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    listeners: Vec::new(),
    started: false,
    minimized: false,
});

/// 初期状態の再送と、ウォッチャーの立ち上がり・立ち下がりの配信を直列にする（ロック順は常に `DELIVER` → `SHARED`）。
/// 再送を取った後に復元が先に配られ、新しい受け口へ `Restored` → `Minimized` の順で届くのを防ぐ。
/// 受け口の中から `watch_minimize` を呼ぶとデッドロックするため呼ばない。
static DELIVER: Mutex<()> = Mutex::new(());

fn lock_deliver() -> MutexGuard<'static, ()> {
    DELIVER.lock().unwrap_or_else(|p| p.into_inner())
}

fn lock() -> MutexGuard<'static, Shared> {
    SHARED.lock().unwrap_or_else(|p| p.into_inner())
}

/// 最小化・復元の受け口を登録し、ウォッチャーが動いていなければ起動する。ウィンドウ未取得・起動失敗は `false`
/// （受け口は登録せず、呼び出し側が次の機会に再試行する）。登録済みの受け口を渡しても何も増えず `true`。
pub fn watch_minimize(listener: fn(MinimizeEdge)) -> bool {
    if crate::window().is_none() {
        return false;
    }
    // 受け口を足してからウォッチャーを起こす（起動直後の最初の立ち上がりを落とさない）。
    // 再送の判定から再送の完了まで `DELIVER` を握り、その間はウォッチャーの配信を待たせる。
    let _deliver = lock_deliver();
    let (added, replay) = {
        let mut st = lock();
        if st
            .listeners
            .iter()
            .any(|l| std::ptr::fn_addr_eq(*l, listener))
        {
            (false, false)
        } else {
            st.listeners.push(listener);
            (true, st.minimized)
        }
    };
    if !ensure_started() {
        if added {
            lock()
                .listeners
                .retain(|l| !std::ptr::fn_addr_eq(*l, listener));
        }
        return false;
    }
    if replay {
        listener(MinimizeEdge::Minimized);
    }
    true
}

/// ウォッチャーを 1 回だけ起動する。起動に失敗したら次回に再試行できるよう印を戻す。
fn ensure_started() -> bool {
    {
        let mut st = lock();
        if st.started {
            return true;
        }
        st.started = true;
    }
    let spawned = std::thread::Builder::new()
        .name("fandhe-minimize-watcher".into())
        .spawn(watch);
    if spawned.is_err() {
        lock().started = false;
    }
    spawned.is_ok()
}

fn watch() {
    let mut tracker = EdgeTracker::default();
    loop {
        if let Some(hwnd) = crate::window()
            && let Some(edge) = tracker.update(is_minimized(hwnd))
        {
            // `DELIVER` で再送と直列化する。状態と受け口の一覧は `SHARED` の内側で更新・複製し、
            // 受け口の呼び出しは `SHARED` の外で行う。
            let _deliver = lock_deliver();
            let listeners = {
                let mut st = lock();
                st.minimized = edge == MinimizeEdge::Minimized;
                st.listeners.clone()
            };
            for listener in listeners {
                listener(edge);
            }
            if edge == MinimizeEdge::Restored {
                wake_frame(hwnd);
            }
        }
        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

#[allow(unsafe_code)]
fn is_minimized(hwnd: HWND) -> bool {
    // SAFETY: `hwnd` は `remember_window` が控えたメインウィンドウで、プロセス生存中は有効。
    // `IsIconic` は無効な HWND でも FALSE を返すだけで副作用がない。
    unsafe { IsIconic(hwnd) }.as_bool()
}

#[allow(unsafe_code)]
fn wake_frame(hwnd: HWND) {
    // SAFETY: 同上。`InvalidateRect` は無効な HWND なら失敗を返すだけで、矩形に NULL を渡すとクライアント領域全体を
    // 無効化する（次の描画を起こすだけ）。
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_tracker_reports_each_transition_once() {
        let mut m = EdgeTracker::default();
        assert_eq!(m.update(false), None);
        assert_eq!(m.update(true), Some(MinimizeEdge::Minimized));
        assert_eq!(m.update(true), None);
        assert_eq!(m.update(false), Some(MinimizeEdge::Restored));
        assert_eq!(m.update(false), None);
        assert_eq!(POLL_MS, 200);
    }
}
