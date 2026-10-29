//! 壁紙・ロック画面の設定の抽象（Phase 11 の拡充ループ、Issue #970）。
//!
//! 範囲は「アプリが作った PNG 1 枚を端末の壁紙に設定するだけ」。利用者が選んだ画像、元の壁紙へ
//! 戻す操作、ホームとロックの同時指定、画面ごとの指定、ライブ壁紙は扱わない。
//! macOS は `NSWorkspace.setDesktopImageURL`（全画面へ設定して読み戻す）、Windows は
//! `SystemParametersInfoW(SPI_SETDESKWALLPAPER)`、Android（#971）は `WallpaperManager`。
//! macOS / Windows は同期の [`Wallpaper`] 実装、Android はコールバック型のため pull 型 `bridge`
//! で `fandhe_core::screens::wallpaper_set`（端末依存ゼロの状態機械）へ書き戻す
//! （`photo_save` と同じ判断）。それ以外は [`UnsupportedWallpaper`]。
//!
//! # 結果の種類
//!
//! 成功は「OS が受け付けた」の 1 種類だけ（[`WallpaperTarget`] で設定先を返す）。キャンセルする
//! UI が無いため `Cancelled` は返さない。毎回 OS を呼び、アプリ内の記憶で結果を作らない。
//!
//! | 状況 | 返す値 |
//! |---|---|
//! | macOS: 全画面で設定し読み戻しも一致 / Windows: `SystemParametersInfoW` が成功 | `Ok(Home)` |
//! | Android のロック画面（#971） | `Ok(Lock)` |
//! | 管理者設定で禁止（Android `isSetWallpaperAllowed == false`、#971） | `PermissionDenied("wallpaper")` |
//! | 入力不正（PNG でない・空・上限超過） | `InvalidInput("photo_library png")` |
//! | 元ファイルの書き込み失敗、OS のエラー、読み戻しの不一致、画面が 0 枚 | `Internal("wallpaper …")` |
//! | 非対応（Linux・iOS、macOS / Windows のロック画面、まだスタブの OS） | `Unsupported("wallpaper")` |
//!
//! # 元ファイル
//!
//! macOS / Windows は設定したパスを OS が参照し続けるため、`$HOME/.fandhe/wallpaper/` に
//! 置いて**消さない**。同じディレクトリの一時ファイルへ書いてから rename し、書きかけを
//! OS に参照させない（[`write_source_png`]）。書けなければ OS を呼ばない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の要求と完了の順序（世代の照合）、受け付けた以外を「設定しました」と
//! 見せないこと、完了を 1 回だけ反映すること、検査を通らない値を OS へ渡さないこと、
//! 元ファイルを書けなければ OS を呼ばないこと。
//!
//! 守らない: 複数プロセスからの同時設定、設定後に利用者や他アプリが壁紙を変えること、元ファイルを
//! 利用者が消すこと、macOS の Space ごとの壁紙、Windows でグループポリシー等により反映されない
//! のに TRUE が返る場合（公式は「エラーが無い限り TRUE」とだけ書く）、Android 12 以降の
//! Activity 再生成による結果表示の消失。
//!
//! エラーコード・パス・バイト列はログにも画面にも出さない（security.md A09）。

use crate::DeviceError;
use crate::photo_library::PngImage;

/// 設定先。画面の完了文言の出し分けに使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallpaperTarget {
    /// ホーム画面（デスクトップ）の壁紙。
    Home,
    /// ロック画面の壁紙。
    Lock,
}

/// 同期に設定できる実装向けの窓口（macOS / Windows）。コールバック型 OS は bridge を使う。
pub trait Wallpaper: Send + Sync {
    /// PNG を `target` の壁紙に設定し、確定した設定先を返す。契約はモジュール doc の表。
    fn set_wallpaper(
        &self,
        image: &PngImage,
        target: WallpaperTarget,
    ) -> Result<WallpaperTarget, DeviceError>;
}

/// 非対応プラットフォーム用。常に `Unsupported("wallpaper")`。
pub struct UnsupportedWallpaper;

impl Wallpaper for UnsupportedWallpaper {
    fn set_wallpaper(
        &self,
        _image: &PngImage,
        _target: WallpaperTarget,
    ) -> Result<WallpaperTarget, DeviceError> {
        Err(DeviceError::Unsupported("wallpaper"))
    }
}

/// `dir/wallpaper/sample-wallpaper.png` へ PNG を書いてパスを返す（macOS / Windows 共通。
/// OS 別のコードに複製しない）。一時ファイル経由の rename で、書きかけを OS に見せない。
/// 失敗はすべて `Internal("wallpaper source")`（パスは載せない）。
pub fn write_source_png(
    dir: &std::path::Path,
    image: &PngImage,
) -> Result<std::path::PathBuf, DeviceError> {
    let fail = |_| DeviceError::Internal("wallpaper source");
    let sub = dir.join("wallpaper");
    std::fs::create_dir_all(&sub).map_err(fail)?;
    let tmp = sub.join("sample-wallpaper.png.tmp");
    let dest = sub.join("sample-wallpaper.png");
    if let Err(e) = std::fs::write(&tmp, image.bytes()).and_then(|()| std::fs::rename(&tmp, &dest))
    {
        let _ = std::fs::remove_file(&tmp);
        return Err(fail(e));
    }
    Ok(dest)
}

/// 既定の置き場（`$HOME/.fandhe`）へ [`write_source_png`] する。ホームが無ければ `Unsupported`。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn write_source_png_default(image: &PngImage) -> Result<std::path::PathBuf, DeviceError> {
    let dir = crate::storage::FsStorage::desktop_default()?;
    write_source_png(&dir.0, image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> PngImage {
        let mut b = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        b.extend_from_slice(&[1, 2, 3, 4]);
        PngImage::new(b).expect("valid signature")
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("fandhe-wp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("mkdir");
        p
    }

    #[test]
    fn unsupported_never_succeeds() {
        for t in [WallpaperTarget::Home, WallpaperTarget::Lock] {
            assert!(matches!(
                UnsupportedWallpaper.set_wallpaper(&png(), t),
                Err(DeviceError::Unsupported("wallpaper"))
            ));
        }
    }

    #[test]
    fn write_source_overwrites_and_leaves_no_tmp() {
        let dir = scratch("ok");
        let first = write_source_png(&dir, &png()).expect("write");
        let again = write_source_png(&dir, &png()).expect("overwrite");
        assert_eq!(first, again);
        assert_eq!(std::fs::read(&again).expect("read"), png().bytes());
        assert!(!dir.join("wallpaper/sample-wallpaper.png.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_source_fails_when_dir_is_a_file() {
        let dir = scratch("bad");
        let file = dir.join("f");
        std::fs::write(&file, b"x").expect("file");
        assert!(matches!(
            write_source_png(&file, &png()),
            Err(DeviceError::Internal("wallpaper source"))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
