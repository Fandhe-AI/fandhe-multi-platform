//! Phase 11（親 #1101）型サンプル「写真選択」（Issue #1103）の effect ドライバ（機能 crate `fandhe-cap-photo-picker` の中核、
//! Issue #1355）。`fandhe_core::screens::photo_pick`（端末依存ゼロ）の要求を観測し、[`PhotoPicker`] で画像を 1 枚選ばせ、
//! **実際に復号できることを確かめてから**（契約の 2 段目、`fandhe_device::photo_picker` モジュール doc）画素を
//! `fandhe_runtime::image_store` へ置き、`CompletionSender` 経由で成功を返す（`fandhe-gallery` の `screens/registry.rs` の
//! `effect` 区画から呼ばれる契約、`fandhe_runtime::EffectFn` 参照）。復号そのものは `fandhe_device::photo_picker::decode_for_display`
//! （写真選択・書類スキャン・スキャナー機器が共有する 1 か所）に任せ、ここは形式名・サイズ・失敗の文言への写像だけを持つ。
//!
//! Web（wasm32）は本経路を通らず `fandhe-web-dom` の `file_picker.rs` が `createImageBitmap` で復号を確かめる
//! （gallery は wasm32 では本 crate を依存に入れない）。
//!
//! # スレッド
//!
//! macOS の `NSOpenPanel` は main スレッド必須のため、`file_pick` と同じく呼ばれたスレッド（egui の update ループ = main）で
//! `pick()` を同期に呼ぶ。固定タイムアウトは掛けない。
//!
//! # 対応 OS
//!
//! macOS / Windows: [`RfdPhotoPicker`](crate::RfdPhotoPicker)。Linux は `UnsupportedPhotoPicker` 固定（ダミーの成功実装へは倒さない）。
//!
//! iOS / Android（Issue #1104）は `PhotoPicker` トレイトを通らない。OS のピッカー（`PHPickerViewController` / `PickVisualMedia`）は
//! コールバック型で、同期 `pick()` に直すとスレッド待ちが要るため、`biometric_auth` と同じ pull 型 `bridge`
//! （`fandhe_runtime::request_slot::RequestSlot`）で結果を受ける。復号確認は OS コールバックのスレッド側（`bridge::deliver`）で
//! 済ませ、egui の UI スレッドでは復号しない。保留中の要求は Activity 再生成で **復元せず破棄**する（契約: `fandhe_device::photo_picker`
//! モジュール doc。`file_pick` の復元方式は採らない。`fandhe-gallery` の `android_main` が `reset_pending_tracking` を呼ぶ）。
//! OS 側が JPEG / PNG 以外（HEIC・GIF・WebP 等）を JPEG へ変換して渡し、変換できなければ `Internal` として失敗表示にする。
//! 形式の最終判定は OS の名乗る MIME ではなく `validate_photo` の先頭バイトで行う。
//!
//! # 復号の上限（A04 リソース枯渇）
//!
//! 入力は 8 MiB 上限（1 段目）。復号の辺・確保の上限と表示用の縮小は `decode_for_display` が掛け、超えたら `Internal`
//! （成功にしない）。デスクトップ・モバイルとも PNG / JPEG のみ復号する（GIF / WebP は `Internal`）。

use fandhe_core::screens::photo_pick::{
    PhotoPickState, ShownPhoto, finish_pick, mark_picking, take_request,
};
use fandhe_device::photo_picker::{
    ImageFormat, MAX_PHOTO_BYTES, PickedPhoto, decode_for_display as decode_pixels,
};
use fandhe_runtime::{CompletionSender, image_store};

#[cfg(any(target_os = "ios", target_os = "android"))]
use fandhe_core::screens::photo_pick::PickPhase;
#[cfg(any(test, target_os = "ios", target_os = "android"))]
use fandhe_device::file_picker::PickedFile;
#[cfg(any(test, target_os = "ios", target_os = "android"))]
use fandhe_device::photo_picker::validate_photo;

use crate::contract::PhotoPicker;
#[cfg(test)]
use crate::contract::UnsupportedPhotoPicker;
use crate::error::PhotoPickerError;

use std::sync::Arc;

/// 現在のプラットフォームに応じた同期の [`PhotoPicker`] を返す（プロセスで 1 個）。状態型に依存しない入口で、
/// 文字認識・バーコード読み取りの機能 crate もデスクトップ経路でこれを使う。macOS・Windows は OS のダイアログ
/// （[`RfdPhotoPicker`](crate::RfdPhotoPicker)）、iOS・Android は結果が非同期に届き同期の trait を通らないため `Unsupported`
/// 固定（成功を偽装しない。画面は [`photo_pick_effect`] の経路を使う）、それ以外（Linux）も `Unsupported` 固定。
// ponytail: iOS・Android 向けの同期 trait は要求が出たら足す（begin / deliver の pull 型を同期に直すにはスレッド待ちが要る）。
pub fn platform_photo_picker() -> Arc<dyn PhotoPicker> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn PhotoPicker>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                Arc::new(crate::desktop::RfdPhotoPicker::new())
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                Arc::new(crate::contract::UnsupportedPhotoPicker)
            }
        })
        .clone()
}

/// 復号確認を通った画像。
pub(crate) struct Decoded {
    pub(crate) image: fandhe_core::RgbaPixels,
    pub(crate) format: &'static str,
    pub(crate) size: usize,
    pub(crate) width: u32,
    pub(crate) height: u32,
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
pub(crate) fn decode_for_display(photo: &PickedPhoto) -> Result<Decoded, PhotoPickerError> {
    let d = decode_pixels(&photo.bytes, photo.format)
        .ok_or(PhotoPickerError::Internal("photo_picker decode"))?;
    Ok(Decoded {
        image: d.image,
        format: format_name(photo.format),
        size: photo.bytes.len(),
        width: d.width,
        height: d.height,
    })
}

/// `PhotoPickerError` を画面の失敗文言へ写す（`Display` の内部識別子は出さない）。
fn failure_message(e: &PhotoPickerError) -> String {
    match e {
        PhotoPickerError::InvalidInput(_) => format!(
            "{} MiB を超える画像は選べません",
            MAX_PHOTO_BYTES / (1024 * 1024)
        ),
        PhotoPickerError::Unsupported(_) => "この環境では写真選択を使えません".to_string(),
        PhotoPickerError::Internal(s) if s.contains("decode") => {
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
        Err(PhotoPickerError::Cancelled(_)) => Ok(None),
        Err(e) => Err(failure_message(&e)),
    }
}

/// OS から届いた選択結果（iOS / Android）を 2 段検査に通して 3 値へ写す。
/// キャンセル・選択なしは `Ok(None)`（失敗にしない）、0 バイト・形式不明・上限超・復号不能・
/// 変換失敗は利用者向け文言の `Err`（内部識別子・パス・名前は出さない、A09）。
#[cfg(any(test, target_os = "ios", target_os = "android"))]
fn mobile_outcome(
    result: Result<Option<PickedFile>, PhotoPickerError>,
) -> Result<Option<Decoded>, String> {
    match result {
        Ok(None) | Err(PhotoPickerError::Cancelled(_)) => Ok(None),
        Ok(Some(file)) => validate_photo(file.bytes)
            .map_err(PhotoPickerError::from)
            .and_then(|photo| decode_for_display(&photo))
            .map(Some)
            .map_err(|e| failure_message(&e)),
        Err(e) => Err(failure_message(&e)),
    }
}

/// iOS / Android の OS コールバックとの橋渡し（pull 型、`fandhe_runtime::request_slot::RequestSlot` の facade。モジュール doc 参照）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use std::sync::Arc;

    use fandhe_device::file_picker::PickedFile;
    use fandhe_runtime::request_slot::RequestSlot;

    use super::Decoded;
    use crate::error::PhotoPickerError;

    /// 画面へ反映する完了結果。毎フレームの `snapshot` クローンが画素を複製しないよう `Arc`。
    pub(crate) type Outcome = Result<Option<Arc<Decoded>>, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 提示直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を 2 段検査（復号確認込み）に通してから置く。呼び出しスレッドは OS コールバック側
    /// （iOS は NSItemProvider のキュー、Android はワーカースレッド）で、UI スレッドではない。`request_id` 不一致・
    /// 未登録・重複は捨てて `false`（検査は結果を置く前に済ませるため、捨てられる結果でも復号は走る。1 要求 1 回なので許容）。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<Option<PickedFile>, PhotoPickerError>,
    ) -> bool {
        let outcome = super::mobile_outcome(result).map(|opt| opt.map(Arc::new));
        SLOT.deliver(request_id, outcome)
    }

    /// Activity 再生成直後に `fandhe-gallery` の `android_main` から呼ぶ。保留中の要求を捨て、以後届く旧 `request_id` の
    /// [`deliver`] を不一致で無視させる（契約は「復元せず破棄」）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// Activity 再生成直後に `fandhe-gallery` の `android_main` が呼ぶ。保留中の写真選択の要求を捨てる（契約は「復元せず破棄」。
/// `bridge::SLOT` がプロセス static のため再生成をまたいで残るのを、新しい `Idle` の画面状態を作る前に断つ）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
}

/// 提示を依頼する。結果は非同期に `bridge::deliver` へ届く。`false` は提示依頼自体の同期的な失敗（コールバックは二度と来ない）。
#[cfg(target_os = "ios")]
fn present(request_id: u64) -> bool {
    crate::ios::present_ios_photo_picker(Box::new(move |result| {
        bridge::deliver(request_id, result);
    }))
}

#[cfg(target_os = "android")]
fn present(request_id: u64) -> bool {
    crate::android::request_photo_pick(request_id)
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
                Err(PhotoPickerError::Internal("photo_picker present")),
            );
        }
        return;
    }
    if state.phase != PickPhase::Picking {
        return;
    }
    let generation = state.generation;
    fandhe_runtime::request_slot::poll_and_finish(
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
#[cfg(any(test, not(any(target_os = "ios", target_os = "android"))))]
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
    drive(state, sender, platform_photo_picker().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::photo_picker::MAX_PHOTO_SIDE;
    use std::io::Cursor;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn photo(bytes: Vec<u8>) -> PickedPhoto {
        validate_photo(bytes).unwrap()
    }

    #[test]
    fn valid_png_decodes_and_reports_format_and_size() {
        let bytes = png(8, 4);
        let len = bytes.len();
        let d = decode_for_display(&photo(bytes)).unwrap();
        assert_eq!((d.width, d.height), (8, 4));
        assert_eq!((d.image.width(), d.image.height()), (8, 4));
        assert_eq!(d.format, "PNG");
        assert_eq!(d.size, len);
    }

    #[test]
    fn undecodable_inputs_fail_stage_two() {
        // 途中で切れた PNG・ヘッダーだけの JPEG・GIF・WebP・辺の上限超え。
        let mut cut = png(64, 64);
        cut.truncate(cut.len() / 2);
        for bytes in [
            cut,
            vec![0xFF, 0xD8, 0xFF],
            b"GIF89a....".to_vec(),
            b"RIFF\0\0\0\0WEBPVP8 ".to_vec(),
            png(MAX_PHOTO_SIDE + 1, 1),
        ] {
            assert!(matches!(
                decode_for_display(&photo(bytes)),
                Err(PhotoPickerError::Internal(_))
            ));
        }
    }

    struct Fake(fn() -> Result<PickedPhoto, PhotoPickerError>);
    impl PhotoPicker for Fake {
        fn pick(&self) -> Result<PickedPhoto, PhotoPickerError> {
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
        let cancel = mobile_outcome(Err(PhotoPickerError::Cancelled("photo_picker")));
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
        let e = mobile_outcome(Err(PhotoPickerError::Internal("photo_picker pick")))
            .err()
            .unwrap();
        assert_eq!(e, "写真を選べませんでした");
    }

    #[test]
    fn outcome_mapping_distinguishes_cancel_and_failures() {
        let cancel = pick_and_decode(&Fake(|| Err(PhotoPickerError::Cancelled("photo_picker"))));
        assert!(matches!(cancel, Ok(None)));
        let unsupported = pick_and_decode(&UnsupportedPhotoPicker).err().unwrap();
        assert_eq!(unsupported, "この環境では写真選択を使えません");
        let big = pick_and_decode(&Fake(|| {
            Err(PhotoPickerError::InvalidInput("photo_picker size"))
        }))
        .err()
        .unwrap();
        assert!(big.contains("8 MiB"), "{big}");
        let undecodable = pick_and_decode(&Fake(|| Ok(photo(vec![0xFF, 0xD8, 0xFF]))))
            .err()
            .unwrap();
        assert_eq!(undecodable, "画像を表示できませんでした");
        // 内部識別子（`photo_picker format` 等）は利用者向け文言へ漏らさない。
        let internal = pick_and_decode(&Fake(|| {
            Err(PhotoPickerError::Internal("photo_picker format"))
        }))
        .err()
        .unwrap();
        assert_eq!(internal, "写真を選べませんでした");
    }

    #[test]
    fn drive_without_request_is_noop_and_request_completes_with_picked_image() {
        use fandhe_core::screens::photo_pick::request_pick;
        use fandhe_runtime::{Completion, RepaintWaker};
        /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
        struct NoopWaker;
        impl RepaintWaker for NoopWaker {
            fn wake(&self) {}
            fn wake_after(&self, _delay: std::time::Duration) {}
        }
        let (tx, rx) = std::sync::mpsc::channel::<Completion<PhotoPickState>>();
        let sender = CompletionSender::new(tx, Arc::new(NoopWaker));
        let mut state = PhotoPickState::default();
        drive(
            &state,
            &sender,
            &Fake(|| Err(PhotoPickerError::Unsupported("x"))),
        );
        assert!(rx.try_recv().is_err());

        request_pick(&mut state);
        drive(&state, &sender, &Fake(|| Ok(photo(png(8, 4)))));
        for _ in 0..2 {
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .expect("完了が届く")(&mut state);
        }
        let shown = state.shown.as_ref().expect("成功が反映される");
        assert_eq!((shown.width, shown.height), (8, 4));
        image_store::clear();
    }
}
