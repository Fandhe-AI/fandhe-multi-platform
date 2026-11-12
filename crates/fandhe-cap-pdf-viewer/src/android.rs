//! Android 向け PDF 表示の実装（Issue #472、親 #107 の b-android）。
//! `driver.rs` の Android 経路から `implementation` が呼ばれ、ドライバのワーカースレッドから
//! `PdfPageHelper.renderFirstPage(Context, byte[], int)` を同期 JNI（署名 `(Landroid/content/Context;[BI)[B`）で 1 回呼ぶ
//! （`MainActivity` には何も書かない）。Java 側は `PdfPageHelper`（`android.graphics.pdf.PdfRenderer`）。Rust への native コールバックは無い。
//!
//! # コード表（SSOT は `crate::contract::decode_code`。Java は同じ値で持つ）
//!
//! 戻りの `byte[]`: 成功 = `[0][page_count:u32 BE][width:u32 BE][height:u32 BE][RGBA]`、
//! 失敗 = コード 1 バイトだけ（2 壊れている / 3 暗号化 / 4 0 ページ / 5 寸法・引数の不正）。
//! null・空・未知のコードは `Internal` に倒す。成功は [`check_page`] だけが判定する。
//!
//! # 期限・ライフサイクル
//!
//! 期限（10 秒）は Rust のドライバだけが持ち、Java は走り切る（期限後の成功はドライバが捨てる）。
//! Java 側は要求 id も保留状態も持たない。Activity が作り直されたときの保留記録は `android_main` の
//! 再実行で [`crate::reset_pending_tracking`] が捨てる（gallery の `android_main` が呼ぶ）。要求 id は単調に増えるため、
//! 古いワーカーの完了が新しい要求を消費することはない。短い処理なので背景遷移では中断しない。
//!
//! 守る範囲: 失敗を成功値へ畳まないこと・資源の解放・FFI に渡す長さの上限。
//! 守らない範囲: 任意の PDF・`/Rotate`、暗号化 PDF の実 API での確認、期限後も OS 側の描画が走り切ること、
//! 色の正確さ・所要時間。PDF・画素・OS の生エラー値はログに出さない。実機確認は #468。

#[cfg(any(target_os = "android", test))]
use crate::contract::{PdfFirstPage, check_page, decode_code};
#[cfg(any(target_os = "android", test))]
use crate::error::PdfViewerError;

/// 画面側へ見せる失敗の識別子（文言はドライバが固定文言にする）。
#[cfg(any(target_os = "android", test))]
const FAILED: &str = "pdf_viewer android";
/// 成功応答のヘッダ長（コード 1 + ページ数 4 + 幅 4 + 高さ 4）。
#[cfg(any(target_os = "android", test))]
const HEADER: usize = 13;

/// Java の戻りを契約へ写す。成功は `check_page` を通ったものだけ。
#[cfg(any(target_os = "android", test))]
fn decode_reply(reply: &[u8]) -> Result<PdfFirstPage, PdfViewerError> {
    let Some((&code, rest)) = reply.split_first() else {
        return Err(PdfViewerError::Internal(FAILED));
    };
    if code != 0 {
        // `decode_code` が `Ok` を返すのは 0 だけ。ここでは必ず `Err` になる。
        return Err(decode_code(i32::from(code))
            .err()
            .unwrap_or(PdfViewerError::Internal(FAILED)));
    }
    if reply.len() < HEADER {
        return Err(PdfViewerError::Internal(crate::contract::SIZE));
    }
    let be = |i: usize| u32::from_be_bytes([rest[i], rest[i + 1], rest[i + 2], rest[i + 3]]);
    check_page(be(0), be(4), be(8), rest[12..].to_vec())
}

#[cfg(target_os = "android")]
mod imp {
    use crate::contract::{MAX_SIDE, PdfFirstPage, PdfPageRenderer, validate_input};
    use crate::error::PdfViewerError;
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    use jni::objects::{JByteArray, JValue};

    use super::{FAILED, HEADER, decode_reply};

    pub(super) struct AndroidPdfPageRenderer;

    impl PdfPageRenderer for AndroidPdfPageRenderer {
        fn render_first_page(
            &self,
            pdf: &[u8],
            max_side: u32,
        ) -> Result<PdfFirstPage, PdfViewerError> {
            validate_input(pdf)?;
            if !(1..=MAX_SIDE).contains(&max_side) {
                return Err(PdfViewerError::InvalidInput);
            }
            let app = android_app().ok_or(PdfViewerError::Internal(FAILED))?;
            let reply = with_activity_class(
                &app,
                jni::jni_str!("com.fandhe.platform.PdfPageHelper"),
                |env, activity, class| -> Result<Option<Vec<u8>>, JniCallFailed> {
                    let j_pdf = env.byte_array_from_slice(pdf)?;
                    let ret = env.call_static_method(
                        class,
                        jni::jni_str!("renderFirstPage"),
                        jni::jni_sig!((android.content.Context, byte[], int) -> byte[]),
                        &[
                            JValue::Object(activity),
                            JValue::Object(&j_pdf),
                            JValue::Int(max_side as i32),
                        ],
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
                },
            )
            .ok()
            .flatten()
            .ok_or(PdfViewerError::Internal(FAILED))?;
            decode_reply(&reply)
        }
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn crate::contract::PdfPageRenderer> {
    std::sync::Arc::new(imp::AndroidPdfPageRenderer)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn crate::contract::PdfPageRenderer> {
    std::sync::Arc::new(crate::contract::UnsupportedPdfPageRenderer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{BROKEN, EMPTY, ENCRYPTED, MAX_SIDE, OUTCOME, SIZE};

    fn reply(pages: u32, w: u32, h: u32, body: usize) -> Vec<u8> {
        let mut v = vec![0];
        for n in [pages, w, h] {
            v.extend(n.to_be_bytes());
        }
        v.extend(vec![7; body]);
        v
    }

    fn internal(r: Result<PdfFirstPage, PdfViewerError>) -> &'static str {
        match r {
            Err(PdfViewerError::Internal(s)) => s,
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
            Err(PdfViewerError::Unsupported)
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
