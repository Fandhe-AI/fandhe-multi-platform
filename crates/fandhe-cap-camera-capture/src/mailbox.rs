//! 非同期のカメラ結果（Android の JNI コールバック）と、同期的に待つワーカースレッドの受け渡し窓口。
//!
//! 呼び出し元は [`crate::android`]（Android）のみ。ターゲット非依存の作りで、ホスト上の
//! ユニットテストでも検証する（`lib.rs` で `cfg(any(target_os = "android", test))` に限る）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::{CameraError, Image};

/// [`CaptureMailbox`] の内部状態。単一飛行（同時に 1 件の撮影のみ）を強制する。
/// `Pending`/`Ready` は要求 ID（`u64`）を保持し、`deliver` が古い要求への遅延コールバックを
/// 新しい要求の結果と取り違えないようにする（codex-review 指摘、PR #790。下記
/// `CaptureMailbox` doc 参照）。
enum MailboxSlot {
    /// 撮影要求なし。
    Idle,
    /// 撮影要求（ID 付き）を出し、コールバックを待っている。
    Pending(u64),
    /// コールバックが届いた（`wait` が消費すると `Idle` へ戻る）。
    Ready(u64, Result<Image, CameraError>),
}

/// 非同期カメラコールバック（Android の JNI）と、`Camera::capture` を同期的に呼ぶ
/// worker スレッド（`spawn_capture` 参照）の間を仲介する受け渡し窓口。
///
/// 単一飛行: `begin` が `None` を返した場合、呼び出し元は新規の撮影を開始せず
/// エラーを返す契約（`android.rs` の `AndroidCamera::capture` 参照）。
///
/// 要求 ID: `begin` が返す ID を呼び出し元が JNI 往復（`requestCameraCapture` /
/// `nativeOnCameraResult` 引数）に乗せて運び、`cancel`/`deliver`/`wait` はすべて同じ ID を
/// 要求する。これにより「要求 A が `wait` タイムアウトで `Idle` に戻った後、要求 B が `begin` して
/// から A の遅延コールバックが届く」順序でも、`deliver` は ID 不一致で無視され、B が A の古い
/// 結果を受け取らない（codex-review 指摘、PR #790）。
pub struct CaptureMailbox {
    slot: Mutex<MailboxSlot>,
    condvar: Condvar,
    next_id: AtomicU64,
}

impl CaptureMailbox {
    /// 空の mailbox（`Idle`）を構築する。`static` 初期化子から呼べるよう `const fn` にする
    /// （`android.rs` の `CAPTURE_MAILBOX` 参照）。
    pub const fn new() -> Self {
        Self {
            slot: Mutex::new(MailboxSlot::Idle),
            condvar: Condvar::new(),
            next_id: AtomicU64::new(0),
        }
    }

    /// `Idle` なら新しい要求 ID を発行して `Pending` へ遷移し、その ID を返す。
    /// 既に進行中なら `None`
    /// （ponytail: 待ち行列化はしない。撮影が同時多重に要る場合は Vec 化して拡張）。
    pub fn begin(&self) -> Option<u64> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, MailboxSlot::Idle) {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            *slot = MailboxSlot::Pending(id);
            Some(id)
        } else {
            None
        }
    }

    /// `begin` 後、JNI 呼び出し自体が失敗するなど、コールバックが来ないと分かった時点で
    /// `Pending(id)` を `Idle` へ戻す（`deliver` を経由しない早期中断）。`id` が現在の
    /// `Pending` と一致しない場合は無視する（別要求のスロットを誤って畳まない）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))] // ホストのテストでは呼ばれない
    pub fn cancel(&self, id: u64) {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, MailboxSlot::Pending(pending_id) if pending_id == id) {
            *slot = MailboxSlot::Idle;
        }
    }

    /// コールバックスレッドから結果を届ける。`Pending(id)` と ID が一致する場合のみ受理する。
    /// 一致しない（重複コールバック・タイムアウト後の遅延到着で `id` が既に古い）場合は
    /// 無視する（`wait` 側が既にタイムアウトで `Idle` に戻し、別要求が `begin` 済みの
    /// 可能性があるため）。
    pub fn deliver(&self, id: u64, result: Result<Image, CameraError>) {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, MailboxSlot::Pending(pending_id) if pending_id == id) {
            *slot = MailboxSlot::Ready(id, result);
            self.condvar.notify_all();
        }
    }

    /// `begin` を呼んだスレッドが、その ID の結果を待つ。`timeout` 内に届かない場合は
    /// スロットを `Idle` へ戻し（固着防止）、`CameraError::Unsupported` を返す
    /// （専用の Timeout バリアントは設けず、意味が近い既存バリアントを再利用する）。
    pub fn wait(&self, id: u64, timeout: Duration) -> Result<Image, CameraError> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + timeout;
        loop {
            if matches!(*slot, MailboxSlot::Ready(ready_id, _) if ready_id == id) {
                let ready = std::mem::replace(&mut *slot, MailboxSlot::Idle);
                return match ready {
                    MailboxSlot::Ready(_, result) => result,
                    _ => unreachable!("直前に Ready であることを確認済み"),
                };
            }
            let now = Instant::now();
            if now >= deadline {
                // 自分（id）の Pending のみ Idle へ戻す。ID 不一致（既に別要求が
                // 進行中）ならここでは何もしない（他要求のスロットを畳まない）。
                if matches!(*slot, MailboxSlot::Pending(pending_id) if pending_id == id) {
                    *slot = MailboxSlot::Idle;
                }
                return Err(CameraError::Unsupported("camera_timeout"));
            }
            let (guard, _timeout_result) = self
                .condvar
                .wait_timeout(slot, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            slot = guard;
        }
    }
}

impl Default for CaptureMailbox {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// `deliver` → `wait` が別スレッド間で値を運ぶこと
    /// （Android の JNI コールバックスレッドと worker スレッドの関係を模す）。
    #[test]
    fn mailbox_delivers_across_threads() {
        let mailbox = Arc::new(CaptureMailbox::new());
        let id = mailbox.begin().expect("Idle からは begin できる");

        let deliverer = Arc::clone(&mailbox);
        std::thread::spawn(move || {
            deliverer.deliver(
                id,
                Ok(Image {
                    width: 1,
                    height: 1,
                    rgba: vec![0, 0, 0, 255],
                }),
            );
        });

        let image = mailbox
            .wait(id, Duration::from_secs(5))
            .expect("deliver された結果を受け取る");
        assert_eq!(image.width, 1);
    }

    /// `begin` 中（`Pending`）の再 `begin` は単一飛行を守るため `None` を返す。
    #[test]
    fn mailbox_rejects_concurrent_begin() {
        let mailbox = CaptureMailbox::new();
        assert!(mailbox.begin().is_some());
        assert!(mailbox.begin().is_none(), "Pending 中の再 begin は拒否する");
    }

    /// `deliver` が来ないまま `timeout` を超えると `Err` を返し、スロットを `Idle` に
    /// 戻して固着させない（次の `begin` が成功することも合わせて確認する）。
    #[test]
    fn mailbox_wait_times_out_and_resets_to_idle() {
        let mailbox = CaptureMailbox::new();
        let id = mailbox.begin().expect("Idle からは begin できる");
        let result = mailbox.wait(id, Duration::from_millis(10));
        assert!(result.is_err(), "deliver が来ないままタイムアウトする");
        assert!(
            mailbox.begin().is_some(),
            "タイムアウト後は Idle に戻り再度 begin できる"
        );
    }

    /// codex-review 指摘（PR #790）の再現: 要求 A が `wait` タイムアウトで `Idle` に
    /// 戻った後、要求 B が `begin` してから A の遅延コールバックが `deliver` されても、
    /// ID 不一致のため無視され、B は自分の結果が届くまで `wait` で待ち続ける
    /// （A の古い結果を受け取らない）。
    #[test]
    fn mailbox_ignores_stale_deliver_after_timeout() {
        let mailbox = CaptureMailbox::new();
        let id_a = mailbox.begin().expect("A は begin できる");
        let timeout_result = mailbox.wait(id_a, Duration::from_millis(10));
        assert!(timeout_result.is_err(), "A はタイムアウトする");

        let id_b = mailbox
            .begin()
            .expect("A のタイムアウト後 B は begin できる");
        assert_ne!(id_a, id_b, "A と B の要求 ID は異なる");

        // A の遅延コールバックが今ごろ届く（B の Pending 中）。
        mailbox.deliver(
            id_a,
            Ok(Image {
                width: 999,
                height: 999,
                rgba: vec![0, 0, 0, 255],
            }),
        );

        // B 自身の結果を届けると、A の値ではなく B の値を受け取れる。
        mailbox.deliver(
            id_b,
            Ok(Image {
                width: 2,
                height: 2,
                rgba: vec![1, 1, 1, 255],
            }),
        );
        let image = mailbox
            .wait(id_b, Duration::from_secs(1))
            .expect("B の結果を受け取る");
        assert_eq!(
            image.width, 2,
            "A の古い結果ではなく B 自身の結果を受け取る"
        );
    }
}
