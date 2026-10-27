//! Android 向けファイル・動画のプレビューとサムネイル（Issue #920、親 #182 の b-android）。
//!
//! 呼び出し元: `device/file_preview.rs` の Android 経路が [`thumbnailer`]・[`present_preview`] を呼ぶ。
//! 期限（サムネイル 10 秒）・世代・`request_id` の照合・期限後の結果の破棄はドライバ側で済んでいる。
//! Java 側は `FilePreviewHelper`（コード表・流れ・置き場はそちらの先頭コメント）。
//!
//! - サムネイル: ワーカースレッドから `MainActivity.filePreviewThumbnail`（署名 `(I[BIJ)[B`）を同期 JNI で
//!   1 回呼ぶ。API 29 未満は Java が非対応コードを返し `Unsupported` になる（アプリ内で縮小して代用しない）。
//!   戻りは [`fandhe_device::file_preview::thumbnail_from_android_reply`] が契約へ写す（成功は `check_thumbnail`
//!   を通ったものだけ）。null・JNI 失敗・長さ超過は `Internal`。
//! - プレビュー: `MainActivity.requestFilePreview`（署名 `(JI[B)V`）で UI スレッドへ依頼し、Java が
//!   `nativeOnFilePreviewResult(id, code)` で返す → `preview_from_android_code` →
//!   `file_preview::bridge::deliver`（`RequestSlot` の id と照合）。入力が無いと egui が描画しないため
//!   native 受け口で `request_repaint` を呼ぶ。
//!
//! # ライフサイクル
//!
//! Activity が作り直されると `android_main` の再実行で `file_preview::reset_pending_tracking` が保留を捨てる。
//! 要求 id は単調に増えるため、旧 Activity の native コールバックが新しい要求を消費することはない。
//! サムネイルは短い同期処理なので背景遷移では中断しない。
//!
//! 守る範囲: 失敗を成功に畳まないこと・JNI に渡す長さの上限・資源の解放・Activity 再生成時の旧要求の破棄。
//! 守らない範囲: 受け手アプリが実際に表示したか、受け手が読む途中の同名ファイルの置き換え（中身は同じ）、
//! 色・アルファの正確さ、所要時間。`resolveActivity`・`<queries>`・権限は使わない。
//! パス・画素・id・結果コードはログに出さない。
//!
//! no-op スタブは置かない: native は Rust の `requestFilePreview` を経た後でしか呼ばれない
//! （external_app_launch と同じ）。JNI 署名の照合先は Java の宣言とこのファイルの 2 か所。

#[cfg(target_os = "android")]
use std::sync::Arc;

#[cfg(target_os = "android")]
use fandhe_device::file_preview::{
    thumbnail_from_android_reply, FileKind, FileThumbnailer, PreviewResult, ThumbnailResult,
    OUTCOME, SIZE, THUMBNAIL_MAX_SIDE,
};
#[cfg(target_os = "android")]
use fandhe_device::DeviceError;

/// kind コード（Java 側 `FilePreviewHelper.KIND_*` と対）。
#[cfg(any(target_os = "android", test))]
const KIND_SAMPLE_IMAGE: i32 = 0;
#[cfg(any(target_os = "android", test))]
const KIND_UNKNOWN: i32 = 1;

#[cfg(any(target_os = "android", test))]
fn kind_code(kind: fandhe_device::file_preview::FileKind) -> i32 {
    match kind {
        fandhe_device::file_preview::FileKind::SampleImage => KIND_SAMPLE_IMAGE,
        fandhe_device::file_preview::FileKind::Unknown => KIND_UNKNOWN,
    }
}

#[cfg(target_os = "android")]
mod imp {
    use jni::objects::{JByteArray, JObject};

    /// `MainActivity` のメソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`（例外は消す）。
    /// 2 箇所の `unsafe` は `android_pdf_viewer.rs::call_activity` と同一の根拠。
    #[allow(unsafe_code)]
    pub(super) fn call_activity<T>(
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

    /// 戻りの `byte[]` を長さの上限を確かめてからコピーする。null は `None`、上限超過は空配列。
    pub(super) fn read_reply(
        env: &mut jni::Env,
        arr: JByteArray,
        max: usize,
    ) -> Result<Option<Vec<u8>>, crate::device::android::JniCallFailed> {
        if arr.is_null() {
            return Ok(None);
        }
        if arr.len(env)? > max {
            return Ok(Some(Vec::new()));
        }
        Ok(Some(env.convert_byte_array(&arr)?))
    }
}

/// Android のサムネイル実装。
#[cfg(target_os = "android")]
struct AndroidFileThumbnailer;

#[cfg(target_os = "android")]
impl FileThumbnailer for AndroidFileThumbnailer {
    fn thumbnail(
        &self,
        kind: FileKind,
        bytes: &[u8],
        max_side: u32,
        timeout: std::time::Duration,
    ) -> Result<ThumbnailResult, DeviceError> {
        use jni::objects::{JByteArray, JValue};
        if !(1..=THUMBNAIL_MAX_SIDE).contains(&max_side) {
            return Err(DeviceError::InvalidInput("file_preview max_side"));
        }
        // 上限は Java 側の MAX_TIMEOUT_MS と同じ。
        let timeout_ms = timeout.as_millis().min(60_000) as i64;
        let reply = imp::call_activity(|env, a| {
            let j_bytes = env.byte_array_from_slice(bytes)?;
            let ret = env.call_method(
                a,
                jni::jni_str!("filePreviewThumbnail"),
                jni::jni_sig!((int, byte[], int, long) -> byte[]),
                &[
                    JValue::Int(kind_code(kind)),
                    JValue::Object(&j_bytes),
                    JValue::Int(max_side as i32),
                    JValue::Long(timeout_ms),
                ],
            )?;
            let arr = env.cast_local::<JByteArray>(ret.l()?)?;
            // 長さを検証してから確保・コピーする（ヘッダ 9 バイト + 最大寸法の RGBA）。
            let max = 9 + (THUMBNAIL_MAX_SIDE as usize) * (THUMBNAIL_MAX_SIDE as usize) * 4;
            imp::read_reply(env, arr, max)
        })
        .flatten()
        .ok_or(DeviceError::Internal(OUTCOME))?;
        if reply.is_empty() {
            return Err(DeviceError::Internal(SIZE));
        }
        thumbnail_from_android_reply(&reply)
    }
}

/// Android のサムネイル実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn thumbnailer() -> Arc<dyn FileThumbnailer> {
    Arc::new(AndroidFileThumbnailer)
}

/// 他アプリへのプレビュー依頼。`Ok(None)` は受理済みで、結果は後から native コールバックで届く。
#[cfg(target_os = "android")]
pub(crate) fn present_preview(
    request_id: u64,
    kind: FileKind,
    bytes: Vec<u8>,
) -> Result<Option<PreviewResult>, DeviceError> {
    use jni::objects::JValue;
    imp::call_activity(|env, a| {
        let j_bytes = env.byte_array_from_slice(&bytes)?;
        env.call_method(
            a,
            jni::jni_str!("requestFilePreview"),
            jni::jni_sig!((long, int, byte[]) -> ()),
            &[
                JValue::Long(request_id as jni::sys::jlong),
                JValue::Int(kind_code(kind)),
                JValue::Object(&j_bytes),
            ],
        )?;
        Ok(())
    })
    .map(|()| None)
    .ok_or(DeviceError::Internal(OUTCOME))
}

/// `FilePreviewHelper.nativeOnFilePreviewResult`（Java 側）から呼ばれる JNI エクスポート。
/// id・結果コードはログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_FilePreviewHelper_nativeOnFilePreviewResult<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    super::file_preview::bridge::deliver(
        request_id as u64,
        fandhe_device::file_preview::preview_from_android_code(code),
    );
    // 入力が無いと egui が描画せず「プレビュー中」で止まるため次フレームを起こす。
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::file_preview::FileKind;

    #[test]
    fn kind_code_matches_java() {
        assert_eq!(kind_code(FileKind::SampleImage), 0);
        assert_eq!(kind_code(FileKind::Unknown), 1);
    }
}
