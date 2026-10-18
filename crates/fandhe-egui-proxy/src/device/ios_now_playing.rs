//! iOS 向けロック画面・OS のメディア操作実装のスタブ（Issue #381 の骨格、#382 が埋める）。
//! `device/now_playing.rs::platform_now_playing` の iOS 経路から呼ばれ、契約は `fandhe_device::now_playing`。
//!
//! 埋めるときの注意（親 #98 の契約表）: OS 側のコールバックは `device::now_playing::commands()` へ
//! `push` するだけにする（11）。`clear` / `shutdown` は登録の解放を済ませてから戻る（9）。
//! 失敗・非対応を成功値へ畳まない（6）。それまでは非対応を返し、ダミーの成功へ倒さない。

use std::sync::Arc;

use fandhe_device::now_playing::{NowPlaying, UnsupportedNowPlaying};

/// 現在の OS 向け実装を返す。
pub(crate) fn implementation() -> Arc<dyn NowPlaying> {
    Arc::new(UnsupportedNowPlaying)
}
