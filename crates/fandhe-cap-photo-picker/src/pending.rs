//! iOS の PHPicker 要求ごとの受け取り（`deliver`）の登録表。`ios.rs` の受け口 `fandhe_rust_photo_picker_outcome` が id で 1 件を取り出して
//! 呼ぶ。登録・取り出し・呼び出しは OS に触れない純粋なロジックのため、cfg を付けずホストでテストする（iOS 以外では未使用）。
//!
//! 写真選択・文字認識（`fandhe-cap-image-recognition`）・バーコード読み取り（`fandhe-cap-barcode-scan`）が同じ PHPicker の受け口を
//! 共有する。以前は要求 id の最上位ビットのタグで 3 機能へ振り分けていたが、要求ごとに `deliver` を登録する形にしたため、
//! 受け口は機能を知らない（タグの衝突・振り分けの順序への依存が無い）。
//!
//! # 守る範囲
//!
//! 1 プロセス内で、1 つの id の受け取りが 1 回だけ呼ばれること（取り出しと登録の消去を同じロックの内側で行う）。呼ぶのは
//! ロックを外してから（`deliver` の中で別の要求が登録されてもデッドロックしない）。知らない id・既に取り出した id は捨てる
//! （成功にしない）。守らない範囲: 提示したまま結果が来ない要求の回収（OS がコールバックしない場合は登録が残る。1 件 1 要求の小さな
//! 閉包で、次の要求を妨げない）。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use fandhe_device::file_picker::PickedFile;

use crate::error::PhotoPickerError;

/// 要求 1 件の受け取り。OS コールバックのスレッド（`NSItemProvider` のキュー）で呼ばれる。
pub type IosPickDeliver = Box<dyn FnOnce(Result<Option<PickedFile>, PhotoPickerError>) + Send>;

/// id と受け取りの対応表。同時に保留される要求は高々数件（1 画面 1 要求）のため、線形の探索で足りる。
// ponytail: 要求が数十件を超える使い方が出たら HashMap にする（`const` 構築のため今は Vec）。
pub(crate) struct PendingDelivers {
    next: AtomicU64,
    list: Mutex<Vec<(u64, IosPickDeliver)>>,
}

impl PendingDelivers {
    pub(crate) const fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
            list: Mutex::new(Vec::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(u64, IosPickDeliver)>> {
        // 毒されても表自体は壊れない（挿入・削除だけ）ため、そのまま使う。
        self.list.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `id` の登録を取り出して表から消す（同じロックの内側）。
    fn take(&self, id: u64) -> Option<IosPickDeliver> {
        let mut list = self.lock();
        let at = list.iter().position(|(k, _)| *k == id)?;
        Some(list.swap_remove(at).1)
    }

    /// 受け取りを登録し、OS へ渡す id を返す（0 は使わない）。
    pub(crate) fn register(&self, deliver: IosPickDeliver) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.lock().push((id, deliver));
        id
    }

    /// 提示の同期的な失敗時に登録を消す。まだ登録されていれば `true`。
    pub(crate) fn cancel(&self, id: u64) -> bool {
        self.take(id).is_some()
    }

    /// `id` の受け取りを取り出して `result` を渡す。知らない id・既に呼んだ id は捨てて `false`。
    pub(crate) fn dispatch(
        &self,
        id: u64,
        result: Result<Option<PickedFile>, PhotoPickerError>,
    ) -> bool {
        // 取り出しと消去を同じロックで行い、ロックを外してから呼ぶ。
        let deliver = self.take(id);
        match deliver {
            Some(deliver) => {
                deliver(result);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    type Seen = Arc<StdMutex<Vec<(u8, bool)>>>;

    fn recorder(tag: u8, seen: &Seen) -> IosPickDeliver {
        let seen = seen.clone();
        Box::new(move |r| seen.lock().unwrap().push((tag, matches!(r, Ok(None)))))
    }

    #[test]
    fn delivers_to_the_registered_request_once() {
        let table = PendingDelivers::new();
        let seen: Seen = Arc::default();
        let a = table.register(recorder(1, &seen));
        let b = table.register(recorder(2, &seen));
        assert_ne!(a, b);
        assert!(table.dispatch(b, Ok(None)));
        assert!(table.dispatch(a, Err(PhotoPickerError::Cancelled("x"))));
        assert_eq!(*seen.lock().unwrap(), vec![(2, true), (1, false)]);
        // 同じ id の 2 回目は捨てる（重複通知）。
        assert!(!table.dispatch(a, Ok(None)));
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn unknown_id_is_dropped_not_success() {
        let table = PendingDelivers::new();
        let seen: Seen = Arc::default();
        let id = table.register(recorder(1, &seen));
        assert!(!table.dispatch(id + 100, Ok(None)));
        assert!(seen.lock().unwrap().is_empty());
        // 登録した要求は残っていて、正しい id で届く。
        assert!(table.dispatch(id, Ok(None)));
    }

    #[test]
    fn cancel_removes_the_registration() {
        let table = PendingDelivers::new();
        let seen: Seen = Arc::default();
        let id = table.register(recorder(1, &seen));
        assert!(table.cancel(id));
        assert!(!table.cancel(id));
        assert!(!table.dispatch(id, Ok(None)));
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn deliver_may_register_another_request_without_deadlock() {
        let table: &'static PendingDelivers = Box::leak(Box::new(PendingDelivers::new()));
        let seen: Seen = Arc::default();
        let seen2 = seen.clone();
        let id = table.register(Box::new(move |_| {
            table.register(recorder(9, &seen2));
        }));
        assert!(table.dispatch(id, Ok(None)));
    }
}
