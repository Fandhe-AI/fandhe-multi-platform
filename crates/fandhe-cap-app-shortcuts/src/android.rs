//! Android 向けアプリショートカット実装（Issue #426、親 #101）。ランチャーのアイコン長押しに
//! 「デモを開く」を出す。`driver` の Android 経路からワーカースレッド（`ON_WORKER`）で呼ばれ、
//! 契約は `crate::contract`。Java 側 `AppShortcutsHelper.install`（static）を JNI で同期に呼ぶ。
//! 選ばれたショートカットは url-scheme の経路（`UrlSchemeLifecycle` → `UrlInbox`）に乗る（機能 crate 同士は Java でも結ばない）。
//!
//! # ライフサイクル
//!
//! 要求 id・コールバック・保留状態を持たない同期 pull 型（`android_focus_mode_status.rs` と同型）。
//! 一度だけの登録は呼び出し側の static `InstallCell` が持つので、Activity 再生成後に旧結果が
//! 新しい要求を消費する経路は構造上ない。
//!
//! # 結果コード表（JNI 署名 `(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)I`）
//!
//! SSOT は Java 側 `AppShortcutsHelper.RESULT_*`。0 = 登録した、1 = API 25 未満（`Unsupported`）、
//! それ以外（2・未知値・負値）と Activity 未確立・JNI 失敗は `Internal`。成功には倒さない。
//!
//! # OS API
//!
//! `ShortcutManager.setDynamicShortcuts`（API 25+）。静的ショートカットは `CLEAR_TASK` 固定で
//! MainActivity を作り直す（#259）ため使わない。
//! <https://developer.android.com/develop/ui/views/launch/shortcuts/creating-shortcuts>
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: API 24・登録失敗を成功にしない、1 プロセス 1 回の登録、起動中の選択で Activity を
//! 作り直さない、URL・例外の文言をログに出さない。
//! 守らない: 画面を一度開くまで出ない、他アプリからの同じ `fandhe://` URL との区別、
//! ランチャーごとの表示差、失敗後のプロセス内再試行、ポーリング間隔ぶんの遅れ。

use crate::contract::{AppShortcuts, Registration, ShortcutSpec};
use crate::error::AppShortcutsError;

/// Java `AppShortcutsHelper.RESULT_INSTALLED`。
const RESULT_INSTALLED: i32 = 0;
/// Java `AppShortcutsHelper.RESULT_UNSUPPORTED`（API 25 未満）。
const RESULT_UNSUPPORTED: i32 = 1;

/// Java の結果コードを契約上の結果へ写像する。未知のコードは失敗。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn registration_from_code(code: i32) -> Result<Registration, AppShortcutsError> {
    match code {
        RESULT_INSTALLED => Ok(Registration::Installed),
        RESULT_UNSUPPORTED => Err(AppShortcutsError::Unsupported("app_shortcuts")),
        _ => Err(AppShortcutsError::Internal("app_shortcuts install")),
    }
}

/// Android 実装（`AppShortcutsHelper.install` を同期で呼ぶ）。
#[cfg(target_os = "android")]
struct AndroidAppShortcuts;

#[cfg(target_os = "android")]
impl AndroidAppShortcuts {
    /// Java の static メソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`。
    /// `unsafe` は共通層 `fandhe_platform_android::with_activity_class` に閉じており、本ファイルには無い。
    fn call_install(&self, spec: &ShortcutSpec<'_>) -> Option<i32> {
        use fandhe_platform_android::{JniCallFailed, with_activity_class};
        let app = fandhe_platform_android::android_app()?;
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.AppShortcutsHelper"),
            |env, activity, class| -> Result<i32, JniCallFailed> {
                let label = env.new_string(spec.title)?;
                let url = env.new_string(spec.url)?;
                let ret = env.call_static_method(
                    class,
                    jni::jni_str!("install"),
                    jni::jni_sig!((android.content.Context, java.lang.String, java.lang.String) -> int),
                    &[
                        jni::objects::JValue::Object(activity),
                        jni::objects::JValue::Object(&label),
                        jni::objects::JValue::Object(&url),
                    ],
                );
                let value = match ret {
                    Ok(v) => v,
                    Err(e) => {
                        // 例外は自動クリアされない（jni 0.22）。文言は読まない。
                        if env.exception_check() {
                            env.exception_clear();
                        }
                        return Err(e.into());
                    }
                };
                Ok(value.i()?)
            },
        )
        .ok()
    }
}

#[cfg(target_os = "android")]
impl AppShortcuts for AndroidAppShortcuts {
    fn install(&self, spec: &ShortcutSpec<'_>) -> Result<Registration, AppShortcutsError> {
        match self.call_install(spec) {
            Some(code) => registration_from_code(code),
            None => Err(AppShortcutsError::Internal("app_shortcuts install")),
        }
    }
}

/// Android の実装を返す。ホストのテストビルドでは `Unsupported`。
pub(crate) fn implementation() -> &'static dyn AppShortcuts {
    #[cfg(target_os = "android")]
    {
        &AndroidAppShortcuts
    }
    #[cfg(not(target_os = "android"))]
    {
        &crate::contract::UnsupportedAppShortcuts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_implementation_is_unsupported() {
        assert!(matches!(
            implementation().install(&ShortcutSpec {
                title: "t",
                url: "x://y"
            }),
            Err(AppShortcutsError::Unsupported(_))
        ));
    }

    #[test]
    fn code_mapping_never_fakes_success() {
        assert!(matches!(
            registration_from_code(0),
            Ok(Registration::Installed)
        ));
        assert!(matches!(
            registration_from_code(1),
            Err(AppShortcutsError::Unsupported(_))
        ));
        for c in [2, 99, -1, i32::MIN] {
            assert!(matches!(
                registration_from_code(c),
                Err(AppShortcutsError::Internal(_))
            ));
        }
    }
}
