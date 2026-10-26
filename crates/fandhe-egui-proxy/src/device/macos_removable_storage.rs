//! macOS 向け外部ストレージの着脱の実装（Issue #858、親 #175）。`macos/fandhe_macos_removable_storage.m`
//! （NSWorkspace の通知センターと `mountedVolumeURLs`。`build.rs` が macOS 分岐で feature `device` 有効時に
//! コンパイルする）を `unsafe extern "C"` 3 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で
//! 契約を明記する。`device/removable_storage.rs` の macOS 経路から UI スレッドで呼ばれ、契約は
//! `fandhe_device::removable_storage`。
//!
//! `.m` が返す固定長バッファは [`from_raw`] で契約の値へ写す。写像はホストでもテストできるよう
//! モジュールは `cfg(any(macos, test))`。
//!
//! # 方式
//!
//! OS 資源は `id` で `.m` が所有し、Rust へは生ポインタを渡さない。通知は `.m` が
//! `fandhe_rust_removable_storage_fire`（`device/removable_storage.rs`）経由で `id` の登録表を引いて
//! 出来事にする。停止（[`MacGuard`] の drop）は「登録解除 → `.m` の observer 解除」の順。
//!
//! # OS を正にする範囲
//!
//! マウント・取り外しの直前・取り外しの 3 種を知らせる（その他の変化は無く `support()` で false）。
//! 通知の `NSDevicePath` は読まず、一覧は毎回 `mountedVolumeURLs` から読み直す。種類は
//! `URLResourceKey` の属性の写し（[`kind_from_apple`]）。列挙されたボリュームはマウント済みなので
//! 状態は常にマウント中（読み取り専用の判定は取らない）。
//!
//! # 守らない範囲
//!
//! 強制抜去では `willUnmount` が届かず、`mountedVolumeURLs` は I/O で待つことがある（公式）。
//! 共通の守備範囲は trait doc を参照。

use std::ffi::CStr;
#[cfg(target_os = "macos")]
use std::sync::Arc;

use fandhe_device::removable_storage::{
    kind_from_apple, volumes, Volume, VolumeState, MAX_VOLUMES,
};
use fandhe_device::DeviceError;

const NAME_BYTES: usize = 256;

/// `.m` の `FandheVolumeItem` と同じレイアウト。
#[repr(C)]
#[derive(Clone, Copy)]
struct RawItem {
    name: [u8; NAME_BYTES],
    internal: i8,
    removable: i8,
    ejectable: i8,
    local: i8,
}

/// `.m` の `FandheVolumesRaw` と同じレイアウト。
#[repr(C)]
struct Raw {
    count: i32,
    items: [RawItem; MAX_VOLUMES],
}

impl Raw {
    #[cfg(target_os = "macos")]
    fn blank() -> Box<Raw> {
        Box::new(Raw {
            count: 0,
            items: [RawItem {
                name: [0; NAME_BYTES],
                internal: -1,
                removable: -1,
                ejectable: -1,
                local: -1,
            }; MAX_VOLUMES],
        })
    }
}

/// 1 / 0 / -1 → `Option<bool>`。それ以外の値は読めなかった扱い。
fn flag(v: i8) -> Option<bool> {
    match v {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// `.m` の生値を契約の値へ写す。件数が `0..=MAX_VOLUMES` の外なら信頼できないので `Err`。
/// 名前は NUL 終端と UTF-8 を検証し、満たさなければ `None`（件数には数える）。
fn from_raw(raw: &Raw) -> Result<Vec<Volume>, DeviceError> {
    let n = usize::try_from(raw.count)
        .ok()
        .filter(|&n| n <= MAX_VOLUMES)
        .ok_or(DeviceError::Internal("removable_storage read"))?;
    Ok(volumes(raw.items[..n].iter().map(|it| {
        let name = CStr::from_bytes_until_nul(&it.name)
            .ok()
            .and_then(|c| c.to_str().ok())
            .map(str::to_owned);
        let kind = kind_from_apple(
            flag(it.internal),
            flag(it.removable),
            flag(it.ejectable),
            flag(it.local),
        );
        (name, kind, VolumeState::Mounted)
    })))
}

#[cfg(target_os = "macos")]
mod ffi {
    use super::Raw;

    // SAFETY: `macos/fandhe_macos_removable_storage.m`（`build.rs` が macOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_removable_storage_start(id: u64) -> i32;
        fn fandhe_macos_removable_storage_stop(id: u64);
        fn fandhe_macos_removable_storage_list(out: *mut Raw) -> i32;
    }

    /// 開始に成功したか。
    #[allow(unsafe_code)]
    pub(super) fn start(id: u64) -> bool {
        // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。ポインタは渡さない。
        unsafe { fandhe_macos_removable_storage_start(id) == 0 }
    }

    /// 停止して解放する。未知の `id` は何もしない。
    #[allow(unsafe_code)]
    pub(super) fn stop(id: u64) {
        // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。未知の id は no-op。
        unsafe { fandhe_macos_removable_storage_stop(id) }
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn list(out: &mut Raw) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` は `Raw` と同じレイアウトの範囲だけを
        // 書き、ポインタを保持しない。
        unsafe { fandhe_macos_removable_storage_list(out) }
    }
}

/// 実行中の購読。drop で停止する。
#[cfg(target_os = "macos")]
struct MacGuard {
    id: u64,
}

#[cfg(target_os = "macos")]
impl fandhe_device::removable_storage::SubscriptionGuard for MacGuard {}

#[cfg(target_os = "macos")]
impl Drop for MacGuard {
    fn drop(&mut self) {
        // 先に登録を外し、以後の遅れた呼び出しを捨てさせてから OS の observer を外す。
        super::removable_storage::unregister(self.id);
        ffi::stop(self.id);
    }
}

#[cfg(target_os = "macos")]
struct MacRemovableStorage;

#[cfg(target_os = "macos")]
impl fandhe_device::removable_storage::RemovableStorage for MacRemovableStorage {
    fn support(&self) -> fandhe_device::removable_storage::EventSupport {
        // 順序は ALL_EVENTS: マウント・取り外しの直前・取り外しは取れる、その他の変化は取れない。
        fandhe_device::removable_storage::EventSupport {
            events: [true, true, true, false],
        }
    }

    fn start(
        &self,
        on_signal: fandhe_device::removable_storage::OnSignal,
    ) -> Result<Box<dyn fandhe_device::removable_storage::SubscriptionGuard>, DeviceError> {
        let id = super::removable_storage::register(on_signal);
        if !ffi::start(id) {
            super::removable_storage::unregister(id);
            return Err(DeviceError::Internal("removable_storage start"));
        }
        Ok(Box::new(MacGuard { id }))
    }

    fn read_volumes(&self) -> Result<Vec<Volume>, DeviceError> {
        // 約 8KB。UI スレッドのスタックでも足りるがヒープに置く。
        let mut raw = Raw::blank();
        if ffi::list(&mut raw) != 0 {
            return Err(DeviceError::Internal("removable_storage read"));
        }
        from_raw(&raw)
    }
}

/// macOS の `RemovableStorage` 実装を返す（`device/removable_storage.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Arc<dyn fandhe_device::removable_storage::RemovableStorage> {
    Arc::new(MacRemovableStorage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::removable_storage::VolumeKind;

    fn blank_item() -> RawItem {
        RawItem {
            name: [0; NAME_BYTES],
            internal: -1,
            removable: -1,
            ejectable: -1,
            local: -1,
        }
    }

    fn raw_with(count: i32, items: &[RawItem]) -> Box<Raw> {
        let mut raw = Box::new(Raw {
            count,
            items: [blank_item(); MAX_VOLUMES],
        });
        raw.items[..items.len()].copy_from_slice(items);
        raw
    }

    #[test]
    fn from_raw_rejects_out_of_range_counts() {
        assert!(from_raw(&raw_with(-1, &[])).is_err());
        assert!(from_raw(&raw_with(MAX_VOLUMES as i32 + 1, &[])).is_err());
        assert_eq!(from_raw(&raw_with(0, &[])).unwrap(), vec![]);
    }

    #[test]
    fn from_raw_maps_flags_and_drops_unknown_values() {
        let mut a = blank_item();
        a.name[..4].copy_from_slice(b"Boot");
        a.internal = 1;
        a.removable = 0;
        a.ejectable = 0;
        a.local = 1;
        let mut b = blank_item();
        // フラグ 2 は読めなかった扱い（None）。判定に要る値が欠けるので Unknown。
        b.internal = 2;
        b.removable = 0;
        b.ejectable = 0;
        b.local = 1;
        b.name[..2].copy_from_slice(&[0xff, 0xfe]); // 不正な UTF-8
        let v = from_raw(&raw_with(2, &[a, b])).unwrap();
        assert_eq!(v[0].name.as_deref(), Some("Boot"));
        assert_eq!(v[0].kind, VolumeKind::Internal);
        assert_eq!(v[0].state, VolumeState::Mounted);
        assert_eq!(v[1].name, None);
        assert_eq!(v[1].kind, VolumeKind::Unknown);
    }

    #[cfg(target_os = "macos")]
    mod mac {
        use super::super::*;

        #[test]
        fn start_stop_start_succeeds_and_leaves_nothing() {
            let imp = implementation();
            let g1 = imp.start(Arc::new(|_| {})).expect("開始できる");
            drop(g1);
            let g2 = imp.start(Arc::new(|_| {})).expect("再開始できる");
            drop(g2);
        }

        #[test]
        fn double_start_with_same_id_fails_without_side_effects() {
            let id = super::super::super::removable_storage::register(Arc::new(|_| {}));
            assert!(ffi::start(id));
            assert!(!ffi::start(id), "同じ id の二重登録は失敗");
            ffi::stop(id);
            // stop の後は同じ id で再登録できる（辞書が空になっている）。
            assert!(ffi::start(id));
            ffi::stop(id);
            super::super::super::removable_storage::unregister(id);
        }

        #[test]
        fn support_declares_other_as_unavailable() {
            let s = implementation().support();
            assert!(s.events[0] && s.events[1] && s.events[2]);
            assert!(!s.events[3]);
        }

        #[test]
        fn read_volumes_returns_at_least_the_boot_volume() {
            let v = implementation().read_volumes().expect("読める");
            assert!(!v.is_empty(), "起動ボリュームは常にある");
        }
    }
}
