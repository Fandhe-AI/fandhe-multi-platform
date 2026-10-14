//! Phase 11（親 #1101）型サンプル「写真選択」（Issue #1103）の effect ドライバ。
//! `fandhe_core::screens::photo_pick`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::photo_picker::PhotoPicker` で画像を 1 枚選ばせ、**実際に復号できることを
//! 確かめてから**（契約の 2 段目、`photo_picker` モジュール doc）画素を
//! [`crate::image_store`] へ置き、`CompletionSender` 経由で成功を返す
//! （`crate::screens::registry` の `effect` 区画から呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。Web（wasm32）は
//! 本経路を通らず `fandhe-web-dom` の `file_picker.rs` が `createImageBitmap` で復号を確かめる。
//!
//! # スレッド
//!
//! macOS の `NSOpenPanel` は main スレッド必須のため、`file_pick` と同じく呼ばれたスレッド
//! （egui の update ループ = main）で `pick()` を同期に呼ぶ。固定タイムアウトは掛けない。
//!
//! # 対応 OS
//!
//! macOS / Windows: [`RfdPhotoPicker`]。Linux は `UnsupportedPhotoPicker` 固定
//! （ダミーの成功実装へは倒さない）。
//!
//! iOS / Android（Issue #1104）は `PhotoPicker` トレイトを通らない。OS のピッカー
//! （`PHPickerViewController` / `PickVisualMedia`）はコールバック型で、同期 `pick()` に
//! 直すとスレッド待ちが要るため、`biometric_auth` と同じ pull 型 [`bridge`]
//! （`device/request_slot.rs::RequestSlot`）で結果を受ける。復号確認は OS コールバックの
//! スレッド側（[`bridge::deliver`]）で済ませ、egui の UI スレッドでは復号しない。
//! 保留中の要求は Activity 再生成で **復元せず破棄**する（契約: `photo_picker` モジュール doc。
//! `file_pick` の復元方式は採らない。`android_main` が [`reset_pending_tracking`] を呼ぶ）。
//! OS 側が JPEG / PNG 以外（HEIC・GIF・WebP 等）を JPEG へ変換して渡し、変換できなければ
//! `Internal` として失敗表示にする。形式の最終判定は OS の名乗る MIME ではなく
//! `validate_photo` の先頭バイトで行う。
//!
//! # 復号の上限（A04 リソース枯渇）
//!
//! 入力は 8 MiB 上限（1 段目）。復号は辺 [`MAX_SIDE`]・確保 [`MAX_ALLOC`] の上限を `image::Limits`
//! で掛け、超えたら `Internal`（成功にしない）。表示は [`DISPLAY_SIDE`] 以下へ縮めた複製で行う
//! （入力を切り詰めて成功扱いにはしない。egui の最大テクスチャ辺も避けられる）。
//! デスクトップは PNG / JPEG のみ復号する（`image` の gif / webp feature は新しい依存を
//! 足すため採らない。GIF / WebP は `Internal`）。

use std::io::Cursor;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use std::sync::Arc;

#[cfg(any(target_os = "ios", target_os = "android"))]
use fandhe_core::screens::photo_pick::PickPhase;
use fandhe_core::screens::photo_pick::{
    finish_pick, mark_picking, take_request, PhotoPickState, ShownPhoto,
};
#[cfg(any(test, not(any(target_os = "ios", target_os = "android"))))]
use fandhe_device::photo_picker::PhotoPicker;
use fandhe_device::photo_picker::{ImageFormat, PickedPhoto, MAX_PHOTO_BYTES};
use fandhe_device::DeviceError;

#[cfg(any(test, target_os = "ios", target_os = "android"))]
use fandhe_device::file_picker::PickedFile;
#[cfg(any(test, target_os = "ios", target_os = "android"))]
use fandhe_device::photo_picker::validate_photo;

use crate::app::CompletionSender;
use crate::image_store;

/// 復号を許す辺の最大（画素）。
const MAX_SIDE: u32 = 16384;
/// 復号中の確保上限（バイト）。
const MAX_ALLOC: u64 = 256 * 1024 * 1024;
/// 表示用の複製の辺の上限（画素）。
const DISPLAY_SIDE: u32 = 1024;

/// 現在のプラットフォームに応じた `PhotoPicker` 実装を返す（プロセスで 1 個）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub(crate) fn implementation() -> Arc<dyn PhotoPicker> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn PhotoPicker>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                Arc::new(fandhe_device::photo_picker::RfdPhotoPicker::new())
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                Arc::new(fandhe_device::photo_picker::UnsupportedPhotoPicker)
            }
        })
        .clone()
}

/// 復号確認を通った画像。
pub(crate) struct Decoded {
    image: egui::ColorImage,
    format: &'static str,
    size: usize,
    width: u32,
    height: u32,
}

fn format_name(f: ImageFormat) -> &'static str {
    match f {
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Png => "PNG",
        ImageFormat::Gif => "GIF",
        ImageFormat::Webp => "WebP",
    }
}

/// 2 段目: 実際に復号できるかを確かめ、表示用の画素を作る。失敗は `Internal`。
/// ファイル名・パス・バイト列はエラーに含めない（A09）。
fn decode_for_display(photo: &PickedPhoto) -> Result<Decoded, DeviceError> {
    let fail = DeviceError::Internal("photo_picker decode");
    let format = match photo.format {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        // feature を持たない（モジュール doc）。成功にせず失敗として返す。
        ImageFormat::Gif | ImageFormat::Webp => return Err(fail),
    };
    let mut reader = image::ImageReader::with_format(Cursor::new(&photo.bytes[..]), format);
    // `Limits` は non_exhaustive のため、既定値から書き換える。
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| fail)?;
    let (width, height) = (decoded.width(), decoded.height());
    // thumbnail は小さい画像を拡大しうるため、大きいときだけ縮める。
    let shown = if width > DISPLAY_SIDE || height > DISPLAY_SIDE {
        decoded.thumbnail(DISPLAY_SIDE, DISPLAY_SIDE)
    } else {
        decoded
    };
    let rgba = shown.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(Decoded {
        image: egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
        format: format_name(photo.format),
        size: photo.bytes.len(),
        width,
        height,
    })
}

/// `DeviceError` を画面の失敗文言へ写す（`Display` の内部識別子は出さない）。
fn failure_message(e: &DeviceError) -> String {
    match e {
        DeviceError::InvalidInput(_) => format!(
            "{} MiB を超える画像は選べません",
            MAX_PHOTO_BYTES / (1024 * 1024)
        ),
        DeviceError::Unsupported(_) => "この環境では写真選択を使えません".to_string(),
        DeviceError::Internal(s) if s.contains("decode") => {
            "画像を表示できませんでした".to_string()
        }
        // 内部識別子（`photo_picker format` 等）を利用者向け文言へ出さない。
        _ => "写真を選べませんでした".to_string(),
    }
}

/// 選択 → 復号確認までを行い 3 値へ写す。キャンセルは `Ok(None)`。
#[cfg(any(test, not(any(target_os = "ios", target_os = "android"))))]
fn pick_and_decode(picker: &dyn PhotoPicker) -> Result<Option<Decoded>, String> {
    match picker.pick() {
        Ok(photo) => decode_for_display(&photo)
            .map(Some)
            .map_err(|e| failure_message(&e)),
        Err(DeviceError::Cancelled(_)) => Ok(None),
        Err(e) => Err(failure_message(&e)),
    }
}

/// OS から届いた選択結果（iOS / Android）を 2 段検査に通して 3 値へ写す。
/// キャンセル・選択なしは `Ok(None)`（失敗にしない）、0 バイト・形式不明・上限超・復号不能・
/// 変換失敗は利用者向け文言の `Err`（内部識別子・パス・名前は出さない、A09）。
#[cfg(any(test, target_os = "ios", target_os = "android"))]
fn mobile_outcome(
    result: Result<Option<PickedFile>, DeviceError>,
) -> Result<Option<Decoded>, String> {
    match result {
        Ok(None) | Err(DeviceError::Cancelled(_)) => Ok(None),
        Ok(Some(file)) => validate_photo(file.bytes)
            .and_then(|photo| decode_for_display(&photo))
            .map(Some)
            .map_err(|e| failure_message(&e)),
        Err(e) => Err(failure_message(&e)),
    }
}

/// iOS / Android の OS コールバックとの橋渡し（pull 型、`device/request_slot.rs::RequestSlot`
/// の facade。モジュール doc 参照）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use std::sync::Arc;

    use fandhe_device::file_picker::PickedFile;
    use fandhe_device::DeviceError;

    use super::Decoded;
    use crate::device::request_slot::RequestSlot;

    /// 画面へ反映する完了結果。毎フレームの `snapshot` クローンが画素を複製しないよう `Arc`。
    pub(crate) type Outcome = Result<Option<Arc<Decoded>>, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 提示直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を 2 段検査（復号確認込み）に通してから置く。呼び出しスレッドは
    /// OS コールバック側（iOS は NSItemProvider のキュー、Android はワーカースレッド）で、
    /// UI スレッドではない。`request_id` 不一致・未登録・重複は捨てて `false`
    /// （検査は結果を置く前に済ませるため、捨てられる結果でも復号は走る。1 要求 1 回なので許容）。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<Option<PickedFile>, DeviceError>,
    ) -> bool {
        let outcome = super::mobile_outcome(result).map(|opt| opt.map(Arc::new));
        SLOT.deliver(request_id, outcome)
    }

    /// Activity 再生成直後に `android_main` から呼ぶ。保留中の要求を捨て、以後届く旧
    /// `request_id` の [`deliver`] を不一致で無視させる（契約は「復元せず破棄」）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    bridge::reset();
}

#[cfg(target_os = "ios")]
fn present(request_id: u64) -> bool {
    crate::device::ios::present_photo_picker(request_id)
}

#[cfg(target_os = "android")]
fn present(request_id: u64) -> bool {
    crate::device::android::request_photo_pick(request_id)
}

/// iOS / Android 版 effect 本体（毎フレーム・UI スレッドから呼ばれる。待機でブロックしない）。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn drive_mobile(state: &PhotoPickState, sender: &CompletionSender<PhotoPickState>) {
    if take_request(state).is_some() {
        // 前回の画素を先に解放する（失敗・キャンセルの横に前の画像を残さない）。
        image_store::clear();
        let _ = sender.send(Box::new(mark_picking));
        let request_id = bridge::begin();
        if !present(request_id) {
            // 提示依頼自体が同期的に失敗した（コールバックは二度と来ない）。
            bridge::deliver(
                request_id,
                Err(DeviceError::Internal("photo_picker present")),
            );
        }
        return;
    }
    if state.phase != PickPhase::Picking {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_| {},
        move |s: &mut PhotoPickState, outcome| {
            // 既に `Idle` へ戻っている・別世代なら反映せず、結果だけ畳む。
            if s.phase == PickPhase::Idle {
                return false;
            }
            if s.generation != generation {
                return true;
            }
            let result = outcome.map(|opt| {
                opt.map(|d| {
                    // 画素を先に置いてから成功を届ける（届いた時点で描ける）。
                    // ponytail: slot が参照を持つため表示用複製（<= 1024px、約 4 MiB）を 1 回 clone する。
                    // 気になれば `Arc<ColorImage>` を `image_store` が受けるようにする。
                    let source = format!("photo:{generation}");
                    image_store::put(source.clone(), d.image.clone());
                    ShownPhoto {
                        source,
                        format: d.format.to_string(),
                        size: d.size,
                        width: d.width,
                        height: d.height,
                    }
                })
            });
            finish_pick(s, generation, result);
            true
        },
    );
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn drive(
    state: &PhotoPickState,
    sender: &CompletionSender<PhotoPickState>,
    picker: &dyn PhotoPicker,
) {
    let Some(generation) = take_request(state) else {
        return;
    };
    // 前回の画素を先に解放する（失敗・キャンセルの横に前の画像を残さない）。
    image_store::clear();
    let _ = sender.send(Box::new(mark_picking));

    let result = pick_and_decode(picker).map(|opt| {
        opt.map(|d| {
            // 画素を先に置いてから成功を届ける（届いた時点で描ける）。
            let source = format!("photo:{generation}");
            image_store::put(source.clone(), d.image);
            ShownPhoto {
                source,
                format: d.format.to_string(),
                size: d.size,
                width: d.width,
                height: d.height,
            }
        })
    });
    let _ = sender.send(Box::new(move |s: &mut PhotoPickState| {
        finish_pick(s, generation, result);
    }));
}

/// `registry.rs` の `effect` 区画から `photo_pick` 画面用に配線される effect ドライバ。
pub fn photo_pick_effect(state: &PhotoPickState, sender: &CompletionSender<PhotoPickState>) {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    drive_mobile(state, sender);
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    drive(state, sender, implementation().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::photo_picker::UnsupportedPhotoPicker;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn photo(bytes: Vec<u8>) -> PickedPhoto {
        fandhe_device::photo_picker::validate_photo(bytes).unwrap()
    }

    #[test]
    fn valid_png_decodes_within_display_side() {
        let d = decode_for_display(&photo(png(8, 4))).unwrap();
        assert_eq!((d.width, d.height), (8, 4));
        assert_eq!(d.image.size, [8, 4]);
        assert_eq!(d.format, "PNG");
    }

    #[test]
    fn large_image_is_shrunk_for_display_but_reports_original_size() {
        let d = decode_for_display(&photo(png(2048, 16))).unwrap();
        assert_eq!((d.width, d.height), (2048, 16));
        assert!(d.image.size[0] <= DISPLAY_SIDE as usize);
    }

    #[test]
    fn truncated_png_fails_stage_two() {
        let mut b = png(64, 64);
        b.truncate(b.len() / 2);
        assert!(matches!(
            decode_for_display(&photo(b)),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn header_only_jpeg_passes_stage_one_but_fails_stage_two() {
        let p = photo(vec![0xFF, 0xD8, 0xFF]);
        assert!(matches!(
            decode_for_display(&p),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn gif_and_webp_are_not_success_on_desktop() {
        for bytes in [b"GIF89a....".to_vec(), b"RIFF\0\0\0\0WEBPVP8 ".to_vec()] {
            assert!(matches!(
                decode_for_display(&photo(bytes)),
                Err(DeviceError::Internal(_))
            ));
        }
    }

    #[test]
    fn side_over_limit_is_rejected() {
        // 1 辺 16385 の PNG（ほぼ空なので小さい）。上限超えは復号前に弾かれる。
        let b = png(MAX_SIDE + 1, 1);
        assert!(matches!(
            decode_for_display(&photo(b)),
            Err(DeviceError::Internal(_))
        ));
    }

    struct Fake(fn() -> Result<PickedPhoto, DeviceError>);
    impl PhotoPicker for Fake {
        fn pick(&self) -> Result<PickedPhoto, DeviceError> {
            (self.0)()
        }
    }

    fn file(bytes: Vec<u8>) -> PickedFile {
        PickedFile {
            name: "photo".to_string(),
            bytes,
        }
    }

    #[test]
    fn mobile_outcome_maps_cancel_success_and_failures() {
        assert!(matches!(mobile_outcome(Ok(None)), Ok(None)));
        let cancel = mobile_outcome(Err(DeviceError::Cancelled("photo_picker")));
        assert!(matches!(cancel, Ok(None)));
        let ok = mobile_outcome(Ok(Some(file(png(8, 4))))).unwrap().unwrap();
        assert_eq!((ok.width, ok.height), (8, 4));
        // 0 バイト・形式不明は成功にしない。内部識別子も文言へ出さない。
        for bytes in [vec![], b"not an image at all".to_vec()] {
            let e = mobile_outcome(Ok(Some(file(bytes)))).err().unwrap();
            assert_eq!(e, "写真を選べませんでした");
        }
        let e = mobile_outcome(Ok(Some(file(vec![0xFF, 0xD8, 0xFF]))))
            .err()
            .unwrap();
        assert_eq!(e, "画像を表示できませんでした");
        let e = mobile_outcome(Ok(Some(file(vec![0; MAX_PHOTO_BYTES + 1]))))
            .err()
            .unwrap();
        assert!(e.contains("8 MiB"), "{e}");
        // OS 側の変換失敗（code 2）は失敗として見せる。
        let e = mobile_outcome(Err(DeviceError::Internal("photo_picker pick")))
            .err()
            .unwrap();
        assert_eq!(e, "写真を選べませんでした");
    }

    #[test]
    fn outcome_mapping_distinguishes_cancel_and_failures() {
        let cancel = pick_and_decode(&Fake(|| Err(DeviceError::Cancelled("photo_picker"))));
        assert!(matches!(cancel, Ok(None)));
        let unsupported = pick_and_decode(&UnsupportedPhotoPicker).err().unwrap();
        assert_eq!(unsupported, "この環境では写真選択を使えません");
        let big = pick_and_decode(&Fake(|| {
            Err(DeviceError::InvalidInput("photo_picker size"))
        }))
        .err()
        .unwrap();
        assert!(big.contains("8 MiB"), "{big}");
        let undecodable = pick_and_decode(&Fake(|| Ok(photo(vec![0xFF, 0xD8, 0xFF]))))
            .err()
            .unwrap();
        assert_eq!(undecodable, "画像を表示できませんでした");
        // 内部識別子（`photo_picker format` 等）は利用者向け文言へ漏らさない。
        let internal = pick_and_decode(&Fake(|| Err(DeviceError::Internal("photo_picker format"))))
            .err()
            .unwrap();
        assert_eq!(internal, "写真を選べませんでした");
    }
}
