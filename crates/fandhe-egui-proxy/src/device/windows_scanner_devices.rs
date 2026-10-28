//! Windows 向けスキャナー取り込み（Issue #984。骨格は #981 (a)）。WinRT
//! `Windows.Devices.Scanners.ImageScanner`（WIA ドライバーのスキャナーのみ。カメラ機器は範囲外）を呼ぶ。
//!
//! 呼び出し元: `device/scanner_devices.rs::platform` の Windows 経路が、ワーカースレッドから
//! `ScannerDevices::import_one` を同期に呼ぶ。世代・期限の判定と、表示前の復号（2 段検査の 2 段目）は
//! 呼び出し側が担い、本ファイルは `ImportedImage` か `ScannerImportFailure` だけを返す。
//! 契約（結果の種類・写像表・守る範囲）の SSOT は `fandhe_device::scanner_devices`。
//!
//! # 流れ
//!
//! 1. 探索: `DeviceInformation::FindAllAsyncAqsFilter(ImageScanner::GetDeviceSelector())` を
//!    `DISCOVERY_WINDOW` の間、短い間隔で繰り返す。0 台なら `NoDevice`。使うのは OS の列挙順の先頭
//! 2. 開く: `ImageScanner::FromIdAsync`
//! 3. 計画: Flatbed → Feeder、JPEG → PNG の順で、取り込み元と形式の両方が使えるものを選ぶ
//! 4. 取り込み: `ScanPreviewToStreamAsync` でメモリのストリームへ 1 枚。Pictures Library へは保存しない
//! 5. 結果: `Succeeded`・サイズを確かめ、読み出して `ImportedImage::from_os` へ渡す
//!
//! 失敗の写像（全経路で一貫。`-1` 等へ潰さない）: クラスなし・statics なし → `Unsupported`、
//! 取り込み結果が空 → `NoImage`、失敗の後に列挙し直して選んだ機器が消えていれば `Interrupted`
//! （分からなければ元の失敗）、それ以外の HRESULT（`E_ACCESSDENIED`・WIA のエラーを含む）→ `Failed`。
//!
//! 期限は `remaining` から求めた `Instant`（単調時計）。期限切れは `Cancel()` を呼び、OS の完了通知を
//! 上限付きで待ってから `TimedOut` を返す。ストリームは成功・失敗・期限切れの全経路で閉じる（契約表 9）。
//! 機器名・Id・HRESULT・画像の中身はログにも `Debug` にも出さない（契約表 8）。
//!
//! `RoInitialize` は呼ばず（MTA フォールバック）、`unsafe` は使わない。
//!
//! 既知の天井: 正常系の機材での実機確認が未了（#967）。WIA エラーの HRESULT を `Failed` に寄せている。
//! パッケージ無しの exe で `ImageScanner` が使えるかは未確認。`CANCEL_GRACE`・`RECHECK_WINDOW` は仮の値。
//! 型検査は `x86_64-pc-windows-gnu` のみ。

use std::sync::Arc;

use fandhe_device::scanner_devices::{ScannerDevices, ScannerImportFailure};

/// `REGDB_E_CLASSNOTREG`（この環境にクラスが無い）。
#[cfg(any(target_os = "windows", test))]
const REGDB_E_CLASSNOTREG: i32 = 0x8004_0154_u32 as i32;
/// `E_NOINTERFACE`（statics が無い古い OS）。
#[cfg(any(target_os = "windows", test))]
const E_NOINTERFACE: i32 = 0x8000_4002_u32 as i32;

/// HRESULT を契約上の失敗へ写す。非対応を示す 2 つだけ `Unsupported`、他は `Failed`。
#[cfg(any(target_os = "windows", test))]
fn failure_from_hresult(code: i32) -> ScannerImportFailure {
    if code == REGDB_E_CLASSNOTREG || code == E_NOINTERFACE {
        ScannerImportFailure::Unsupported
    } else {
        ScannerImportFailure::Failed
    }
}

/// 失敗の後に列挙し直した結果（`still_present`: 選んだ機器がまだ居るか。`None` は分からない）で
/// 失敗を確定する。居なくなっていれば `Interrupted`。根拠なしに `Interrupted` にしない。
#[cfg(any(target_os = "windows", test))]
fn after_failure(
    failure: ScannerImportFailure,
    still_present: Option<bool>,
) -> ScannerImportFailure {
    match (failure, still_present) {
        (ScannerImportFailure::Unsupported | ScannerImportFailure::TimedOut, _) => failure,
        (_, Some(false)) => ScannerImportFailure::Interrupted,
        _ => failure,
    }
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Flatbed,
    Feeder,
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Jpeg,
    Png,
}

/// 取り込み元 1 つぶんの機能。
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, Default)]
struct Caps {
    source_supported: bool,
    preview_supported: bool,
    jpeg: bool,
    png: bool,
}

/// Flatbed → Feeder、JPEG → PNG の順で選ぶ。取り込み元はスキャンとプレビューの両方が使えるものだけ。
/// 候補が無ければ `None`（呼び出し側は `Failed`。成功にも「機器なし」にもしない）。
#[cfg(any(target_os = "windows", test))]
fn pick_plan(flatbed: Caps, feeder: Caps) -> Option<(Source, Format)> {
    [(Source::Flatbed, flatbed), (Source::Feeder, feeder)]
        .into_iter()
        .filter(|(_, c)| c.source_supported && c.preview_supported)
        .find_map(|(s, c)| {
            if c.jpeg {
                Some((s, Format::Jpeg))
            } else if c.png {
                Some((s, Format::Png))
            } else {
                None
            }
        })
}

/// `ScanPreviewToStreamAsync` の結果を読む前に確かめる。
#[cfg(any(target_os = "windows", test))]
fn check_preview(succeeded: bool, size: u64) -> Result<(), ScannerImportFailure> {
    if !succeeded {
        Err(ScannerImportFailure::Failed)
    } else if size == 0 {
        Err(ScannerImportFailure::NoImage)
    } else if size > fandhe_device::scanner_devices::MAX_PAGE_BYTES as u64 {
        Err(ScannerImportFailure::TooLarge)
    } else {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use fandhe_device::scanner_devices::{DeviceKind, ImportedImage, DISCOVERY_WINDOW};
    use windows::core::HSTRING;
    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Devices::Scanners::{ImageScanner, ImageScannerFormat, ImageScannerScanSource};
    use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};
    use windows_future::{AsyncOperationCompletedHandler, AsyncStatus, IAsyncOperation};

    use super::*;

    /// `Cancel()` 後に OS の完了通知を待つ上限。仮の値で #967 で見直す。
    /// ponytail: 上限を超えたら待たずに `TimedOut` を返す。ドライバの `os_active` がワーカーの
    /// 復帰まで次の要求を止める。天井はこの値で、実機で長引くなら見直す。
    const CANCEL_GRACE: Duration = Duration::from_secs(5);
    /// 探索窓内の再列挙の間隔。
    const POLL: Duration = Duration::from_millis(200);
    /// 失敗の後に機器の有無を確かめ直す上限。仮の値。
    const RECHECK_WINDOW: Duration = Duration::from_secs(2);

    fn os_failure(e: &windows::core::Error) -> ScannerImportFailure {
        failure_from_hresult(e.code().0)
    }

    /// `IAsyncOperation` を期限つきで待って結果を取る。期限切れは `Cancel()` の完了を待ってから `TimedOut`。
    fn wait_op<T: windows::core::RuntimeType + 'static>(
        deadline: Instant,
        op: &IAsyncOperation<T>,
    ) -> Result<T, ScannerImportFailure> {
        let (tx, rx) = mpsc::channel();
        let handler = AsyncOperationCompletedHandler::new(move |_, status| {
            let _ = tx.send(status);
            Ok(())
        });
        if op.SetCompleted(&handler).is_err() {
            let _ = op.Cancel();
            return Err(ScannerImportFailure::Failed);
        }
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(AsyncStatus::Completed | AsyncStatus::Error) => {
                op.GetResults().map_err(|e| os_failure(&e))
            }
            Ok(_) => Err(ScannerImportFailure::Failed),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = op.Cancel();
                // 猶予内に Completed が届いても成功にはしない。
                let _ = rx.recv_timeout(CANCEL_GRACE);
                Err(ScannerImportFailure::TimedOut)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(ScannerImportFailure::Failed),
        }
    }

    /// スキャナーの Id を OS の列挙順で返す。
    fn enumerate(deadline: Instant) -> Result<Vec<HSTRING>, ScannerImportFailure> {
        let e = |e: windows::core::Error| os_failure(&e);
        let selector = ImageScanner::GetDeviceSelector().map_err(e)?;
        let op = DeviceInformation::FindAllAsyncAqsFilter(&selector).map_err(e)?;
        let all = wait_op(deadline, &op)?;
        (0..all.Size().map_err(e)?)
            .map(|i| all.GetAt(i).and_then(|d| d.Id()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(e)
    }

    /// 失敗を確定する。列挙し直して選んだ機器が消えていれば `Interrupted`。
    fn settle(failure: ScannerImportFailure, id: &HSTRING) -> ScannerImportFailure {
        if matches!(
            failure,
            ScannerImportFailure::Unsupported | ScannerImportFailure::TimedOut
        ) {
            return failure;
        }
        let present = enumerate(Instant::now() + RECHECK_WINDOW)
            .ok()
            .map(|ids| ids.contains(id));
        after_failure(failure, present)
    }

    /// 取り込み先のストリーム。Drop で必ず閉じる（全経路）。
    struct Stream(InMemoryRandomAccessStream);

    impl Drop for Stream {
        fn drop(&mut self) {
            let _ = self.0.Close();
        }
    }

    pub(super) struct WindowsScannerDevices;

    impl ScannerDevices for WindowsScannerDevices {
        fn import_one(&self, remaining: Duration) -> Result<ImportedImage, ScannerImportFailure> {
            let deadline = Instant::now() + remaining;
            let (id, found) = discover(deadline)?;
            scan(&id, found, deadline).map_err(|f| settle(f, &id))
        }
    }

    /// 探索窓の間に先頭の機器の Id と台数を得る。
    fn discover(deadline: Instant) -> Result<(HSTRING, u32), ScannerImportFailure> {
        let window_end = (Instant::now() + DISCOVERY_WINDOW).min(deadline);
        // 探索窓だけが尽きた（全体の期限は残っている）なら「見つからなかった」。
        let exhausted = if window_end < deadline {
            ScannerImportFailure::NoDevice
        } else {
            ScannerImportFailure::TimedOut
        };
        loop {
            match enumerate(window_end) {
                Ok(ids) if !ids.is_empty() => return Ok((ids[0].clone(), ids.len() as u32)),
                Ok(_) => {}
                Err(ScannerImportFailure::TimedOut) => return Err(exhausted),
                Err(f) => return Err(f),
            }
            let left = window_end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(exhausted);
            }
            std::thread::sleep(POLL.min(left));
        }
    }

    fn format_ok(c: &windows::core::Result<impl FormatConfig>, f: ImageScannerFormat) -> bool {
        c.as_ref().is_ok_and(|c| c.supported(f))
    }

    /// Flatbed・Feeder の configuration を同じ形で扱う。
    trait FormatConfig {
        fn supported(&self, f: ImageScannerFormat) -> bool;
    }

    impl FormatConfig for windows::Devices::Scanners::ImageScannerFlatbedConfiguration {
        fn supported(&self, f: ImageScannerFormat) -> bool {
            self.IsFormatSupported(f).unwrap_or(false)
        }
    }

    impl FormatConfig for windows::Devices::Scanners::ImageScannerFeederConfiguration {
        fn supported(&self, f: ImageScannerFormat) -> bool {
            self.IsFormatSupported(f).unwrap_or(false)
        }
    }

    fn scan(
        id: &HSTRING,
        found: u32,
        deadline: Instant,
    ) -> Result<ImportedImage, ScannerImportFailure> {
        let e = |e: windows::core::Error| os_failure(&e);
        let scanner = wait_op(deadline, &ImageScanner::FromIdAsync(id).map_err(e)?)?;

        // 読み取りに失敗した項目は false（候補から外れるだけで、成功にはならない）。
        let flatbed_cfg = scanner.FlatbedConfiguration();
        let feeder_cfg = scanner.FeederConfiguration();
        let source_caps = |src: ImageScannerScanSource, jpeg: bool, png: bool| Caps {
            source_supported: scanner.IsScanSourceSupported(src).unwrap_or(false),
            preview_supported: scanner.IsPreviewSupported(src).unwrap_or(false),
            jpeg,
            png,
        };
        let flatbed = source_caps(
            ImageScannerScanSource::Flatbed,
            format_ok(&flatbed_cfg, ImageScannerFormat::Jpeg),
            format_ok(&flatbed_cfg, ImageScannerFormat::Png),
        );
        let feeder = source_caps(
            ImageScannerScanSource::Feeder,
            format_ok(&feeder_cfg, ImageScannerFormat::Jpeg),
            format_ok(&feeder_cfg, ImageScannerFormat::Png),
        );
        let (source, format) = pick_plan(flatbed, feeder).ok_or(ScannerImportFailure::Failed)?;
        let os_format = match format {
            Format::Jpeg => ImageScannerFormat::Jpeg,
            Format::Png => ImageScannerFormat::Png,
        };
        let os_source = match source {
            Source::Flatbed => {
                flatbed_cfg.map_err(e)?.SetFormat(os_format).map_err(e)?;
                ImageScannerScanSource::Flatbed
            }
            Source::Feeder => {
                feeder_cfg.map_err(e)?.SetFormat(os_format).map_err(e)?;
                ImageScannerScanSource::Feeder
            }
        };

        let stream = Stream(InMemoryRandomAccessStream::new().map_err(e)?);
        let op = scanner
            .ScanPreviewToStreamAsync(os_source, &stream.0)
            .map_err(e)?;
        let result = wait_op(deadline, &op)?;
        let size = stream.0.Size().map_err(e)?;
        check_preview(result.Succeeded().unwrap_or(false), size)?;

        let input = stream.0.GetInputStreamAt(0).map_err(e)?;
        let reader = DataReader::CreateDataReader(&input).map_err(e)?;
        // メモリ上のストリームの読み出しで機器には触れない（`windows_media_transcode.rs::read_all` と同じ）。
        let loaded = reader.LoadAsync(size as u32).map_err(e)?.get().map_err(e)?;
        if u64::from(loaded) != size {
            return Err(ScannerImportFailure::Failed);
        }
        let mut bytes = vec![0u8; size as usize];
        reader.ReadBytes(&mut bytes).map_err(e)?;
        ImportedImage::from_os(bytes, DeviceKind::Scanner, found)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 機器の無い環境では成功しない（`NoDevice` か `Unsupported`）。
        #[test]
        fn no_scanner_does_not_succeed() {
            assert!(WindowsScannerDevices
                .import_one(Duration::from_secs(30))
                .is_err());
        }

        #[test]
        fn zero_deadline_does_not_succeed() {
            assert!(WindowsScannerDevices.import_one(Duration::ZERO).is_err());
        }
    }
}

/// Windows の `ScannerDevices` 実装を返す。Windows 以外（ホストテスト）は `Unsupported`。
pub(crate) fn implementation() -> Arc<dyn ScannerDevices> {
    #[cfg(target_os = "windows")]
    {
        Arc::new(imp::WindowsScannerDevices)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Arc::new(fandhe_device::scanner_devices::UnsupportedScannerDevices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::scanner_devices::MAX_PAGE_BYTES;
    use ScannerImportFailure as F;

    #[test]
    fn hresult_mapping() {
        assert_eq!(failure_from_hresult(REGDB_E_CLASSNOTREG), F::Unsupported);
        assert_eq!(failure_from_hresult(E_NOINTERFACE), F::Unsupported);
        assert_eq!(failure_from_hresult(0x8007_0005_u32 as i32), F::Failed);
        assert_eq!(failure_from_hresult(0x8021_0006_u32 as i32), F::Failed);
        assert_eq!(failure_from_hresult(-1), F::Failed);
    }

    #[test]
    fn after_failure_needs_evidence_for_interrupted() {
        assert_eq!(after_failure(F::Failed, Some(false)), F::Interrupted);
        assert_eq!(after_failure(F::Failed, Some(true)), F::Failed);
        assert_eq!(after_failure(F::Failed, None), F::Failed);
        assert_eq!(after_failure(F::Unsupported, Some(false)), F::Unsupported);
        assert_eq!(after_failure(F::TimedOut, Some(false)), F::TimedOut);
    }

    #[test]
    fn plan_prefers_flatbed_and_jpeg() {
        let ok = Caps {
            source_supported: true,
            preview_supported: true,
            jpeg: true,
            png: true,
        };
        let png_only = Caps { jpeg: false, ..ok };
        let no_preview = Caps {
            preview_supported: false,
            ..ok
        };
        assert_eq!(pick_plan(ok, ok), Some((Source::Flatbed, Format::Jpeg)));
        assert_eq!(
            pick_plan(png_only, ok),
            Some((Source::Flatbed, Format::Png))
        );
        assert_eq!(
            pick_plan(no_preview, ok),
            Some((Source::Feeder, Format::Jpeg))
        );
        let none = Caps {
            jpeg: false,
            png: false,
            ..ok
        };
        assert_eq!(pick_plan(none, Caps::default()), None);
        assert_eq!(pick_plan(no_preview, no_preview), None);
    }

    #[test]
    fn preview_check() {
        assert_eq!(check_preview(false, 10), Err(F::Failed));
        assert_eq!(check_preview(true, 0), Err(F::NoImage));
        assert_eq!(
            check_preview(true, MAX_PAGE_BYTES as u64 + 1),
            Err(F::TooLarge)
        );
        assert_eq!(check_preview(true, 1), Ok(()));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn host_implementation_is_unsupported() {
        let r = implementation().import_one(std::time::Duration::from_secs(1));
        assert_eq!(r.unwrap_err(), F::Unsupported);
    }
}
