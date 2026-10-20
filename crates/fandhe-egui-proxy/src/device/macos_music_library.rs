//! macOS 向け音楽ライブラリ実装（Issue #482、親 #108）。`macos/fandhe_macos_music_library.m`
//! （iTunesLibrary `ITLibrary`。`build.rs` が macOS 分岐で feature `device` 有効時にコンパイルし
//! iTunesLibrary をリンクする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ
//! `// SAFETY:` で契約を明記する。`device/music_library.rs` の macOS 経路からワーカースレッドで
//! 呼ばれ、契約は `fandhe_device::music_library`。
//!
//! バッファの解釈は `device/music_library.rs` の `tracks_from_nul_buffer`（iOS と共用）、戻りコードの
//! 写像は `fandhe_device::music_library::decode_code`。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: タイトルとアーティスト以外を取らない。曲名をログに出さない。壊れたバッファ・未知の
//!   戻りコードは成功にせず `Failed`。期限は引数 1 つで、期限切れでは `buf` に触れずに戻る
//! - 守らない: ad-hoc 署名で `ITLibrary` が nil になる場合の拒否との区別（どちらも `Unsupported`）、
//!   iCloud ミュージック由来の曲が載るか、読み取り後の変化
//!
//! # 未確認事項（Mac 目視 #480 で確認）
//!
//! - ad-hoc 署名の実行体で `ITLibrary` が nil にならないか（nil なら「この端末では使えません」と
//!   出る。動かなければ対応表を `deferred` に戻す）
//! - TCC の許可ダイアログがワーカー上の呼び出しで出るか
//!
//! 実物の `ITLibrary` はホストのテストから呼ばない。テスト実行体には `Info.plist` が埋め込まれず、
//! `NSAppleMusicUsageDescription` が無いと OS がプロセスを終了させるため。

use std::sync::Arc;
use std::time::Duration;

use fandhe_device::music_library::{decode_code, MusicLibrary, MusicLibraryFailure, TrackList};

use super::music_library::tracks_from_nul_buffer;

/// 受け取りバッファ。20 件 × 2 要素 × 1024 バイトが収まる。
const BUF_CAP: usize = 64 * 1024;

mod ffi {
    // SAFETY: `macos/fandhe_macos_music_library.m`（`build.rs` が macOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_music_library_list(
            timeout_ms: u64,
            buf: *mut u8,
            cap: usize,
            out_len: *mut usize,
            out_count: *mut u32,
            out_overflowed: *mut i32,
        ) -> i32;
    }

    /// `(戻りコード, 書いたバイト数, 組の数, 溢れたか)`。コード 0 以外のとき他の値は無効。
    #[allow(unsafe_code)]
    pub(super) fn list(timeout_ms: u64, buf: &mut [u8]) -> (i32, usize, u32, bool) {
        let (mut len, mut count, mut overflowed) = (0usize, 0u32, 0i32);
        // SAFETY: `buf` は `cap` バイトの有効な排他領域、残りの 3 つは呼び出しの間有効なローカル変数への
        // 排他ポインタ。`.m` は期限切れ後に遅れて走る処理から、これらのどれにも触れず保持もしない。
        let rc = unsafe {
            fandhe_macos_music_library_list(
                timeout_ms,
                buf.as_mut_ptr(),
                buf.len(),
                &mut len,
                &mut count,
                &mut overflowed,
            )
        };
        (rc, len, count, overflowed != 0)
    }
}

struct MacMusicLibrary;

impl MusicLibrary for MacMusicLibrary {
    fn list_tracks(&self, timeout: Duration) -> Result<TrackList, MusicLibraryFailure> {
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let mut buf = vec![0u8; BUF_CAP];
        let (rc, len, count, overflowed) = ffi::list(timeout_ms, &mut buf);
        if let Some(failure) = decode_code(rc) {
            return Err(failure);
        }
        // 範囲の検証は解釈の前に行う。
        if len > buf.len() {
            return Err(MusicLibraryFailure::Failed);
        }
        tracks_from_nul_buffer(&buf[..len], count, overflowed)
    }
}

/// macOS の `MusicLibrary` 実装を返す（`device/music_library.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn MusicLibrary> {
    Arc::new(MacMusicLibrary)
}
