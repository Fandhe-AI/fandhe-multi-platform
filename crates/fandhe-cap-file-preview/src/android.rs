//! Android 向けファイル・動画のプレビューとサムネイル（Issue #920、親 #182 の b-android）。
//!
//! 呼び出し元: `driver.rs` の Android 経路が [`thumbnailer`]・[`present_preview`] を呼ぶ。
//! 期限（サムネイル 10 秒）・世代・`request_id` の照合・期限後の結果の破棄はドライバ側で済んでいる。
//! Java 側は `FilePreviewHelper`（コード表・流れ・置き場はそちらの先頭コメント）。
//!
//! - サムネイル: ワーカースレッドから static の `FilePreviewHelper.thumbnail`（署名 `(Landroid/app/Activity;I[BIJ)[B`）を同期 JNI で
//!   1 回呼ぶ。API 29 未満は Java が非対応コードを返し `Unsupported` になる（アプリ内で縮小して代用しない）。
//!   戻りは [`crate::contract::thumbnail_from_android_reply`] が契約へ写す（成功は `check_thumbnail`
//!   を通ったものだけ）。null・JNI 失敗・長さ超過は `Internal`。
//! - プレビュー: static の `FilePreviewHelper.preview`（署名 `(Landroid/app/Activity;JI[B)Z`）で UI スレッドへ依頼し、Java が
//!   `nativeOnFilePreviewResult(id, code)` で返す → `preview_from_android_code` →
//!   `driver::bridge::deliver`（`RequestSlot` の id と照合）。入力が無いと egui が描画しないため
//!   native 受け口で `request_repaint` を呼ぶ。
//!
//! # ライフサイクル
//!
//! Activity が作り直されると gallery の `android_main` の再実行で `reset_pending_tracking` が保留を捨てる。
//! 要求 id は単調に増えるため、旧 Activity の native コールバックが新しい要求を消費することはない。
//! サムネイルは短い同期処理なので背景遷移では中断しない。
//!
//! 守る範囲: 失敗を成功に畳まないこと・JNI に渡す長さの上限・資源の解放・Activity 再生成時の旧要求の破棄。
//! 守らない範囲: 受け手アプリが実際に表示したか、受け手が読む途中の同名ファイルの置き換え（中身は同じ）、
//! 色・アルファの正確さ、所要時間。`resolveActivity`・`<queries>`・権限は使わない。
//! パス・画素・id・結果コードはログに出さない。
//!
//! no-op スタブは置かない: native は Rust の `preview` の呼び出しを経た後でしか呼ばれない
//! （external_app_launch と同じ）。JNI 署名の照合先は Java の宣言とこのファイルの 2 か所。

#[cfg(target_os = "android")]
use std::sync::Arc;

#[cfg(target_os = "android")]
use crate::contract::{
    FileKind, FileThumbnailer, OUTCOME, PreviewResult, SIZE, THUMBNAIL_MAX_SIDE, ThumbnailResult,
    thumbnail_from_android_reply,
};
#[cfg(target_os = "android")]
use crate::error::FilePreviewError;

/// kind コード（Java 側 `FilePreviewHelper.KIND_*` と対）。
#[cfg(any(target_os = "android", test))]
const KIND_SAMPLE_IMAGE: i32 = 0;
#[cfg(any(target_os = "android", test))]
const KIND_UNKNOWN: i32 = 1;

#[cfg(any(target_os = "android", test))]
fn kind_code(kind: crate::contract::FileKind) -> i32 {
    match kind {
        crate::contract::FileKind::SampleImage => KIND_SAMPLE_IMAGE,
        crate::contract::FileKind::Unknown => KIND_UNKNOWN,
    }
}

#[cfg(target_os = "android")]
mod imp {
    use jni::objects::{JByteArray, JObject};

    /// `FilePreviewHelper` の static メソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`（例外は消す）。
    /// クラスは Activity の class loader で解決する（`fandhe_platform_android::with_activity_class`）。
    pub(super) fn call_helper<T>(
        f: impl FnOnce(
            &mut jni::Env,
            &JObject,
            &jni::objects::JClass,
        ) -> Result<T, fandhe_platform_android::JniCallFailed>,
    ) -> Option<T> {
        let app = fandhe_platform_android::android_app()?;
        fandhe_platform_android::with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.FilePreviewHelper"),
            |env, activity, class| {
                let r = f(env, activity, class);
                if r.is_err() && env.exception_check() {
                    env.exception_clear();
                }
                r
            },
        )
        .ok()
    }

    /// 戻りの `byte[]` を長さの上限を確かめてからコピーする。null は `None`、上限超過は空配列。
    pub(super) fn read_reply(
        env: &mut jni::Env,
        arr: JByteArray,
        max: usize,
    ) -> Result<Option<Vec<u8>>, fandhe_platform_android::JniCallFailed> {
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
    ) -> Result<ThumbnailResult, FilePreviewError> {
        use jni::objects::{JByteArray, JValue};
        if !(1..=THUMBNAIL_MAX_SIDE).contains(&max_side) {
            return Err(FilePreviewError::InvalidInput("file_preview max_side"));
        }
        // 上限は Java 側の MAX_TIMEOUT_MS と同じ。
        let timeout_ms = timeout.as_millis().min(60_000) as i64;
        let reply = imp::call_helper(|env, a, class| {
            let j_bytes = env.byte_array_from_slice(bytes)?;
            let ret = env.call_static_method(
                class,
                jni::jni_str!("thumbnail"),
                jni::jni_sig!((android.app.Activity, int, byte[], int, long) -> byte[]),
                &[
                    JValue::Object(a),
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
        .ok_or(FilePreviewError::Internal(OUTCOME))?;
        if reply.is_empty() {
            return Err(FilePreviewError::Internal(SIZE));
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
) -> Result<Option<PreviewResult>, FilePreviewError> {
    use jni::objects::JValue;
    // helper が無い（`false`）・JNI 失敗は受理されなかったものとして失敗で確定する（結果が来ないまま待たせない）。
    let accepted = imp::call_helper(|env, a, class| {
        let j_bytes = env.byte_array_from_slice(&bytes)?;
        Ok(env
            .call_static_method(
                class,
                jni::jni_str!("preview"),
                jni::jni_sig!((android.app.Activity, long, int, byte[]) -> boolean),
                &[
                    JValue::Object(a),
                    JValue::Long(request_id as jni::sys::jlong),
                    JValue::Int(kind_code(kind)),
                    JValue::Object(&j_bytes),
                ],
            )?
            .z()?)
    });
    match accepted {
        Some(true) => Ok(None),
        _ => Err(FilePreviewError::Internal(OUTCOME)),
    }
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
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_FilePreviewHelper_nativeOnFilePreviewResult<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    crate::driver::bridge::deliver(
        request_id as u64,
        crate::contract::preview_from_android_code(code),
    );
    // 入力が無いと egui が描画せず「プレビュー中」で止まるため次フレームを起こす。
    fandhe_runtime::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::FileKind;

    #[test]
    fn kind_code_matches_java() {
        assert_eq!(kind_code(FileKind::SampleImage), 0);
        assert_eq!(kind_code(FileKind::Unknown), 1);
    }
}
