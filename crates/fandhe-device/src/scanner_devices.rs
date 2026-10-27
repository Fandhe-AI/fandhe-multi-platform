//! 接続したスキャナー・カメラ機器からの取り込みの抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/scanner-devices.json`、親 #192、Issue #981）。
//!
//! 扱う範囲は「操作 1 回で、探索窓 [`DISCOVERY_WINDOW`] の間に機器を探し、先頭の 1 台から最新の
//! 画像 1 枚を取り込んで表示する」ことまで。機器の選択 UI・複数枚・RAW・テザー撮影・ネットワーク
//! 接続の機器・着脱の継続監視は扱わない。OS ごとに扱える機器が違う（macOS・iOS・Android は
//! カメラ機器、Windows は WIA のスキャナー、macOS のスキャナーと Windows のカメラ機器は範囲外、
//! Web は対象外）。`fandhe-egui-proxy` の effect ドライバ（`device/scanner_devices.rs`）が本モジュールの
//! trait をワーカースレッドから呼び、OS 層が実装する。
//!
//! # 契約
//!
//! - **結果の種類**: 成功 1 値（[`ImportedImage`]）と失敗（[`ScannerImportFailure`]）だけ。
//!   キャンセルという結果は無い（契約表 10）。Android の USB 許可ダイアログを利用者が閉じた
//!   場合は拒否と区別できないため `PermissionDenied`
//! - **失敗の理由**（写像表。OS 列は後続の子 #982〜#984 への指示を兼ねる）:
//!
//! | 状況 | macOS | iOS | Android | Windows | 返す値 |
//! | --- | --- | --- | --- | --- | --- |
//! | 非対応 | OS・SDK が非対応 | 同左 | `UsbManager` 無し | 同左・fallback の OS | `Unsupported` |
//! | 機器が無い | 探索窓内に 0 台 | 同左 | Still Image 機器 0 件 | スキャナー 0 件 | `NoDevice` |
//! | 画像が無い | 画像 0 件 | 同左 | 画像オブジェクト 0 件 | 取り込み結果が空 | `NoImage` |
//! | 機器がロック中 | アクセス制限・ロック | 同左 | 判別できない（`Failed`） | — | `DeviceLocked` |
//! | 権限の拒否・制限 | — | `contentsAuthorizationStatus` が denied / restricted | `hasPermission` が偽 | — | `PermissionDenied` |
//! | 期限切れ | Rust の期限 | 同左 | 同左 | 同左 | `TimedOut` |
//! | 取り込み中に機器が外れた・背景遷移 | `didRemoveDevice:` | 同左・背景遷移 | `ACTION_USB_DEVICE_DETACHED` | 機器消失 | `Interrupted` |
//! | 画像が上限超過 | — | — | — | — | `TooLarge` |
//! | 画像が 0 バイト・形式不明・復号不可 | — | — | — | — | `Undecodable` |
//! | OS の失敗・未知のコード | `NSError` | 同左 | 例外・`null` | HRESULT | `Failed` |
//!
//! - **OS を正にする**: 機器の有無は OS の列挙結果で決める。許可は通知の extras を信じず OS の
//!   状態を読み直す。最新の 1 枚は OS が持つ作成日時で選ぶ。`null` や日時の欠けた要素を飛ばして
//!   成功を作らない（`Failed`）。OS のコードを `-1` 等へ潰さない
//! - **期限**（契約表 3）: [`IMPORT_DEADLINE`]。起点は要求を観測した時の `Instant`（単調時計）で、
//!   `import_one` へは残り時間を渡す。期限切れの時は、OS 側の非同期操作（セッション・ダウンロード・
//!   探索）を解除し終えてから `TimedOut` を返す。UI キューへ解除を post しただけで戻らない。
//!   期限後に届いた成功は成功として受け取らない。値は仮で、実機確認（#967）で見直す
//! - **同時 1 件と世代**（契約表 2）: 処理中の再押下は無視する。古い要求の完了は要求 id と画面の
//!   世代の両方で捨てる
//! - **成功値の条件**（契約表 6）: 成功として見せてよいのは 2 段の検査（[`ImportedImage::from_os`] と
//!   ドライバでの復号）を通り、見つかった機器が 1 台以上の画像だけ
//! - **秘密値**（契約表 8）: 機器名・シリアル・画像の中身・OS のエラー文をログ・画面・`Debug` に出さない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 並行経路は「画面 → OS 層 1 本 → 完了」の直列で、経路ごとに別のロックを足さない。
//!
//! 守る範囲: 1 プロセス内での要求と結果の順序、嘘の成功の禁止（0 台・0 件・拒否・ロック・
//! 復号できない画像を成功にしない）、セッション・一時ファイル・接続の解放を成功・失敗・期限切れ・
//! 中断の全経路で行うこと。守らない範囲: 探索窓の後に現れた機器、複数台のうち先頭以外、他アプリが
//! 同じ機器を同時に開いていることによる失敗（`Failed`）、複数プロセス、プロセスが落ちた後の結果。

use std::fmt;
use std::time::Duration;

use crate::photo_picker::{PickedPhoto, validate_photo};

/// 機器を探す時間。仮の値で、実機確認（#967）で見直す。
pub const DISCOVERY_WINDOW: Duration = Duration::from_secs(3);

/// 要求を観測してから結果を得るまでの期限（Rust のドライバだけが持つ）。仮の値で、#967 で見直す。
pub const IMPORT_DEADLINE: Duration = Duration::from_secs(60);

/// 取り込んだ画像のバイト数上限。写真選択の上限（SSOT）を参照し、値を複製しない。
pub const MAX_PAGE_BYTES: usize = crate::photo_picker::MAX_PHOTO_BYTES;

/// 取り込み元の機器の種類（機器名は持たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// カメラ機器（macOS・iOS・Android）。
    Camera,
    /// スキャナー（Windows）。
    Scanner,
}

/// 取り込んだ画像。[`ImportedImage::from_os`] を通ったものだけが作れる。
/// `Debug` は画像のバイト列を出さない。
#[derive(Clone, PartialEq, Eq)]
pub struct ImportedImage {
    photo: PickedPhoto,
    kind: DeviceKind,
    found: u32,
}

impl ImportedImage {
    /// OS から届いたバイト列と見つかった機器の台数を検査する（2 段検査の 1 段目）。
    ///
    /// - `found == 0` は画像の有無にかかわらず `NoDevice`
    /// - 上限超過は `TooLarge`
    /// - 0 バイト・形式不明は `Undecodable`
    ///
    /// `Ok` は「復号できる」ことを意味しない。ドライバが表示前に復号して確かめる。
    pub fn from_os(
        bytes: Vec<u8>,
        kind: DeviceKind,
        found: u32,
    ) -> Result<ImportedImage, ScannerImportFailure> {
        if found == 0 {
            return Err(ScannerImportFailure::NoDevice);
        }
        if bytes.len() > MAX_PAGE_BYTES {
            return Err(ScannerImportFailure::TooLarge);
        }
        match validate_photo(bytes) {
            Ok(photo) => Ok(ImportedImage { photo, kind, found }),
            Err(_) => Err(ScannerImportFailure::Undecodable),
        }
    }

    /// 画像。
    pub fn photo(&self) -> &PickedPhoto {
        &self.photo
    }

    /// 取り込み元の機器の種類。
    pub fn kind(&self) -> DeviceKind {
        self.kind
    }

    /// 探索窓の間に見つかった機器の台数（1 以上）。
    pub fn found(&self) -> u32 {
        self.found
    }
}

impl fmt::Debug for ImportedImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportedImage")
            .field("format", &self.photo.format)
            .field("len", &self.photo.bytes.len())
            .field("kind", &self.kind)
            .field("found", &self.found)
            .finish()
    }
}

/// 取り込めなかった理由（機器名・OS の文言・コードは持たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScannerImportFailure {
    /// 非対応 OS・端末。
    Unsupported,
    /// 探索窓の間に機器が見つからなかった。
    NoDevice,
    /// 機器はあるが画像が無かった。
    NoImage,
    /// 機器がロック中・アクセス制限。
    DeviceLocked,
    /// 権限の拒否・制限。
    PermissionDenied,
    /// 期限切れ。
    TimedOut,
    /// 取り込みの途中で機器が外れた・背景へ遷移した。
    Interrupted,
    /// 画像が上限を超えた。
    TooLarge,
    /// 画像が 0 バイト・形式不明・復号できない。
    Undecodable,
    /// OS の失敗・未知のコード。
    Failed,
}

/// 機器からの取り込みの OS 実装。ブロッキング呼び出しで、ワーカースレッドから呼ぶ。
pub trait ScannerDevices: Send + Sync {
    /// 探索 → 先頭の 1 台 → 最新の画像 1 枚の取り込みを 1 回行う。`remaining` は期限までの残り時間。
    fn import_one(&self, remaining: Duration) -> Result<ImportedImage, ScannerImportFailure>;
}

/// 非対応 OS 向け。常に `Unsupported`（成功を返さない）。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedScannerDevices;

impl ScannerDevices for UnsupportedScannerDevices {
    fn import_one(&self, _remaining: Duration) -> Result<ImportedImage, ScannerImportFailure> {
        Err(ScannerImportFailure::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_HEAD: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(
            UnsupportedScannerDevices.import_one(Duration::from_secs(1)),
            Err(ScannerImportFailure::Unsupported)
        );
    }

    #[test]
    fn zero_devices_is_no_device_even_with_valid_image() {
        assert_eq!(
            ImportedImage::from_os(PNG_HEAD.to_vec(), DeviceKind::Camera, 0),
            Err(ScannerImportFailure::NoDevice)
        );
    }

    #[test]
    fn empty_and_unknown_bytes_are_undecodable() {
        for bytes in [Vec::new(), vec![1, 2, 3, 4], vec![0u8; 16]] {
            assert_eq!(
                ImportedImage::from_os(bytes, DeviceKind::Camera, 1),
                Err(ScannerImportFailure::Undecodable)
            );
        }
    }

    #[test]
    fn oversize_is_too_large() {
        let mut big = PNG_HEAD.to_vec();
        big.resize(MAX_PAGE_BYTES + 1, 0);
        assert_eq!(
            ImportedImage::from_os(big, DeviceKind::Camera, 1),
            Err(ScannerImportFailure::TooLarge)
        );
    }

    #[test]
    fn png_and_jpeg_heads_pass_stage_one() {
        let i = ImportedImage::from_os(PNG_HEAD.to_vec(), DeviceKind::Scanner, 2).unwrap();
        assert_eq!((i.kind(), i.found()), (DeviceKind::Scanner, 2));
        assert_eq!(i.photo().bytes, PNG_HEAD.to_vec());
        assert!(ImportedImage::from_os(vec![0xFF, 0xD8, 0xFF], DeviceKind::Camera, 1).is_ok());
    }

    #[test]
    fn debug_hides_bytes() {
        let mut bytes = PNG_HEAD.to_vec();
        bytes.extend_from_slice(b"SecretPixels");
        let i = ImportedImage::from_os(bytes, DeviceKind::Camera, 1).unwrap();
        let d = format!("{i:?}");
        assert!(!d.contains("Secret") && !d.contains("137"), "{d}");
    }
}
