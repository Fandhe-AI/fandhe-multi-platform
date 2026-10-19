//! Android 向け PDF 表示の実装（Issue #472、親 #107 の b-android）。
//! `device/pdf_viewer.rs` の Android 経路から `implementation` が呼ばれ、ドライバのワーカースレッドから
//! `MainActivity.pdfRenderFirstPage` を同期 JNI（署名 `([BI)[B`）で 1 回呼ぶ。
//! Java 側は `PdfPageHelper`（`android.graphics.pdf.PdfRenderer`）。Rust への native コールバックは無い。
//!
//! # コード表（SSOT は `fandhe_device::pdf_viewer::decode_code`。Java は同じ値で持つ）
//!
//! 戻りの `byte[]`: 成功 = `[0][page_count:u32 BE][width:u32 BE][height:u32 BE][RGBA]`、
//! 失敗 = コード 1 バイトだけ（2 壊れている / 3 暗号化 / 4 0 ページ / 5 寸法・引数の不正）。
//! null・空・未知のコードは `Internal` に倒す。成功は [`check_page`] だけが判定する。
//!
//! # 期限・ライフサイクル
//!
//! 期限（10 秒）は Rust のドライバだけが持ち、Java は走り切る（期限後の成功はドライバが捨てる）。
//! Java 側は要求 id も保留状態も持たない。Activity が作り直されたときの保留記録は `android_main` の
//! 再実行で `pdf_viewer::reset_pending_tracking` が捨てる。要求 id は単調に増えるため、
//! 古いワーカーの完了が新しい要求を消費することはない。短い処理なので背景遷移では中断しない。
//!
//! 守る範囲: 失敗を成功値へ畳まないこと・資源の解放・FFI に渡す長さの上限。
//! 守らない範囲: 任意の PDF・`/Rotate`、暗号化 PDF の実 API での確認、期限後も OS 側の描画が走り切ること、
//! 色の正確さ・所要時間。PDF・画素・OS の生エラー値はログに出さない。実機確認は #468。

#[cfg(any(target_os = "android", test))]
use fandhe_device::pdf_viewer::{check_page, decode_code, PdfFirstPage};
#[cfg(any(target_os = "android", test))]
use fandhe_device::DeviceError;

/// 画面側へ見せる失敗の識別子（文言はドライバが固定文言にする）。
#[cfg(any(target_os = "android", test))]
const FAILED: &str = "pdf_viewer android";
/// 成功応答のヘッダ長（コード 1 + ページ数 4 + 幅 4 + 高さ 4）。
#[cfg(any(target_os = "android", test))]
const HEADER: usize = 13;

/// Java の戻りを契約へ写す。成功は `check_page` を通ったものだけ。
#[cfg(any(target_os = "android", test))]
fn decode_reply(reply: &[u8]) -> Result<PdfFirstPage, DeviceError> {
    let Some((&code, rest)) = reply.split_first() else {
        return Err(DeviceError::Internal(FAILED));
    };
    if code != 0 {
        // `decode_code` が `Ok` を返すのは 0 だけ。ここでは必ず `Err` になる。
        return Err(decode_code(i32::from(code))
            .err()
            .unwrap_or(DeviceError::Internal(FAILED)));
    }
    if reply.len() < HEADER {
        return Err(DeviceError::Internal(fandhe_device::pdf_viewer::SIZE));
    }
    let be = |i: usize| u32::from_be_bytes([rest[i], rest[i + 1], rest[i + 2], rest[i + 3]]);
    check_page(be(0), be(4), be(8), rest[12..].to_vec())
}

#[cfg(target_os = "android")]
mod imp {
    use fandhe_device::pdf_viewer::{validate_input, PdfFirstPage, PdfPageRenderer, MAX_SIDE};
    use fandhe_device::DeviceError;
    use jni::objects::{JByteArray, JObject, JValue};

    use super::{decode_reply, FAILED, HEADER};

    pub(super) struct AndroidPdfPageRenderer;

    /// `MainActivity` の同期メソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`（例外は消す）。
    /// 2 箇所の `unsafe` は `android_media_transcode.rs::call_activity` と同一の根拠。
    #[allow(unsafe_code)]
    fn call_activity<T>(
        f: impl FnOnce(&mut jni::Env, &JObject) -> Result<T, crate::device::android::JniCallFailed>,
    ) -> Option<T> {
        let app = crate::android::android_app()?;
        // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
        // `app` は本呼び出しの間生存する。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(|env: &mut jni::Env| {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
            let activity = unsafe { env.as_cast_raw::<JObject>(&raw_activity)? };
            let r = f(env, &activity);
            if r.is_err() && env.exception_check() {
                env.exception_clear();
            }
            r
        })
        .ok()
    }

    impl PdfPageRenderer for AndroidPdfPageRenderer {
        fn render_first_page(
            &self,
            pdf: &[u8],
            max_side: u32,
        ) -> Result<PdfFirstPage, DeviceError> {
            validate_input(pdf)?;
            if !(1..=MAX_SIDE).contains(&max_side) {
                return Err(DeviceError::InvalidInput("pdf_viewer max_side"));
            }
            let reply = call_activity(|env, a| {
                let j_pdf = env.byte_array_from_slice(pdf)?;
                let ret = env.call_method(
                    a,
                    jni::jni_str!("pdfRenderFirstPage"),
                    jni::jni_sig!((byte[], int) -> byte[]),
                    &[JValue::Object(&j_pdf), JValue::Int(max_side as i32)],
                )?;
                let arr = env.cast_local::<JByteArray>(ret.l()?)?;
                if arr.is_null() {
                    return Ok(None);
                }
                // 長さを検証してから確保・コピーする。
                let max = HEADER + (MAX_SIDE as usize) * (MAX_SIDE as usize) * 4;
                if arr.len(env)? > max {
                    return Ok(Some(Vec::new()));
                }
                Ok(Some(env.convert_byte_array(&arr)?))
            })
            .flatten()
            .ok_or(DeviceError::Internal(FAILED))?;
            decode_reply(&reply)
        }
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::pdf_viewer::PdfPageRenderer> {
    std::sync::Arc::new(imp::AndroidPdfPageRenderer)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::pdf_viewer::PdfPageRenderer> {
    std::sync::Arc::new(fandhe_device::pdf_viewer::UnsupportedPdfPageRenderer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::pdf_viewer::{BROKEN, EMPTY, ENCRYPTED, MAX_SIDE, OUTCOME, SIZE};

    fn reply(pages: u32, w: u32, h: u32, body: usize) -> Vec<u8> {
        let mut v = vec![0];
        for n in [pages, w, h] {
            v.extend(n.to_be_bytes());
        }
        v.extend(vec![7; body]);
        v
    }

    fn internal(r: Result<PdfFirstPage, DeviceError>) -> &'static str {
        match r {
            Err(DeviceError::Internal(s)) => s,
            other => panic!("失敗のはずが {other:?}"),
        }
    }

    #[test]
    fn success_only_when_everything_is_consistent() {
        let p = decode_reply(&reply(3, 2, 2, 16)).unwrap();
        assert_eq!((p.page_count, p.image.width, p.image.height), (3, 2, 2));
    }

    #[test]
    fn failure_codes_are_distinguished() {
        assert_eq!(internal(decode_reply(&[2])), BROKEN);
        assert_eq!(internal(decode_reply(&[3])), ENCRYPTED);
        assert_eq!(internal(decode_reply(&[4])), EMPTY);
        assert_eq!(internal(decode_reply(&[5])), SIZE);
        assert!(matches!(
            decode_reply(&[1]),
            Err(DeviceError::Unsupported("pdf_viewer"))
        ));
        assert_eq!(internal(decode_reply(&[99])), OUTCOME);
        assert_eq!(internal(decode_reply(&[])), FAILED);
    }

    #[test]
    fn malformed_success_never_passes() {
        assert_eq!(internal(decode_reply(&[0])), SIZE);
        assert_eq!(internal(decode_reply(&reply(3, 2, 2, 16)[..12])), SIZE);
        assert_eq!(internal(decode_reply(&reply(3, 2, 2, 15))), SIZE);
        assert_eq!(internal(decode_reply(&reply(3, 2, 2, 17))), SIZE);
        assert_eq!(internal(decode_reply(&reply(3, 0, 2, 0))), SIZE);
        let over = MAX_SIDE + 1;
        assert_eq!(internal(decode_reply(&reply(3, over, 1, 0))), SIZE);
        assert_eq!(internal(decode_reply(&reply(0, 2, 2, 16))), EMPTY);
    }
}
