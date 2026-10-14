//! 型サンプル「音声デバイス」の Web 経路（Issue #1333、親 #1136）。
//!
//! `fandhe_core::screens::audio_devices_show`（端末依存ゼロ）を
//! [`super::mount::mount_with_effect`] の effect として観測し、
//! `navigator.mediaDevices.enumerateDevices()` の結果を `finish_read` へ書き戻す。
//! `fandhe-device` の `AudioDevices` トレイトは経由しない（`fandhe-device/src/audio_devices.rs`
//! の module doc どおり、Web は本クレートの担当）。`fandhe-egui-proxy` の wasm canvas 経路は対象外。
//! 範囲は一覧のみ。出力先の選択（`selectAudioOutput` / `setSinkId`）は扱わない。
//!
//! # 読み取り方
//!
//! - 各要素から `kind` と `label` だけを `Reflect` で読む。`deviceId` / `groupId` は読まない
//!   （フィンガープリントに使える識別子を取らない）
//! - `getUserMedia` は呼ばない。マイクを開かず、許可ダイアログも出さない
//! - 許可前は仕様上 `label` が空になる。`audioinput` / `audiooutput` のどれにも名前が無く
//!   要素があるなら「許可前」とみなし、件数も信用せず `AfterMicPermission` にする
//! - 出力を列挙できるかは `HTMLMediaElement.prototype.setSinkId` の有無で判定する。
//!   無ければ出力は `Unavailable`（嘘の 0 件を出さない）
//! - Permissions Policy（`microphone` / `speaker-selection`）で禁止された方向は、ブラウザが結果から
//!   除外するため `Unavailable`。判定 API（`document.permissionsPolicy`）が無く一覧が空の方向も、
//!   禁止と区別できないので `Unavailable`（確定の 0 件にしない）
//! - 両方向とも `Unavailable` なら読み取りは成功にせず、固定文言の `Failed` で完了する
//! - 非 secure context・`mediaDevices` 無し・`enumerateDevices` 無しは `Unsupported`。
//!   同期例外・reject は固定文言の `Failed`（例外の `message` は画面にも `console` にも出さない。
//!   security.md A09）
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 0 件・取得不可・許可前の区別、失敗を成功値へ畳まないこと、ID を取らないこと、
//! 名前の加工と件数上限、1 マウント内の取得順序（世代照合）。
//! 守らない: ブラウザ間の表記・粒度の一致、Chrome が並べる「既定」「通信」の擬似エントリ
//! （除外に `deviceId` が要る）、読み取り後の接続・取り外しの反映（`devicechange` は購読せず
//! 再取得まで）、複数タブ間の一致、非表示タブで `enumerateDevices` が解決しない場合の期限
//! （device_info と同じ扱い）、入力 0 台でマイクを許可できない環境の出力が「許可後に表示」のまま残ること。

use fandhe_core::screens::audio_devices_show::{AudioDevicesReading, DeviceGroup};
use fandhe_device::audio_devices::{listed, AudioDirection};

/// `kind` と `label` の組から表示用の読み取り結果を作る純関数。
///
/// `*_policy` は Permissions Policy が方向ごとに列挙を許すか（`microphone` / `speaker-selection`）。
/// `Some(false)` は禁止で、ブラウザが該当方向を結果から除外するため `Unavailable` にする。
/// `None`（判定 API が無い）で一覧が空のときも、禁止による除外と区別できないので
/// 確定の 0 件にせず `Unavailable` にする。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn reading_from_entries(
    entries: &[(String, String)],
    output_supported: bool,
    input_policy: Option<bool>,
    output_policy: Option<bool>,
) -> AudioDevicesReading {
    let labels = |kind: &str| -> Vec<Option<String>> {
        entries
            .iter()
            .filter(|(k, _)| k == kind)
            .map(|(_, l)| Some(l.clone()))
            .collect()
    };
    let inputs = labels("audioinput");
    let outputs = labels("audiooutput");
    let granted = inputs
        .iter()
        .chain(&outputs)
        .flatten()
        .any(|l| !l.trim().is_empty());
    let group = |d: AudioDirection| match d {
        AudioDirection::Listed(v) => DeviceGroup::Listed(v.into_iter().map(|d| d.name).collect()),
        AudioDirection::Unavailable => DeviceGroup::Unavailable,
        AudioDirection::OnlyWhileMicActive => DeviceGroup::OnlyWhileMicActive,
    };
    // 禁止、または空で許可を確認できない方向は「0 件」と言えない。
    let unverifiable =
        |policy: Option<bool>, empty: bool| policy == Some(false) || (policy.is_none() && empty);
    AudioDevicesReading {
        inputs: if unverifiable(input_policy, inputs.is_empty()) {
            DeviceGroup::Unavailable
        } else if !granted && !inputs.is_empty() {
            DeviceGroup::AfterMicPermission
        } else {
            group(listed(inputs))
        },
        outputs: if !output_supported || unverifiable(output_policy, outputs.is_empty()) {
            DeviceGroup::Unavailable
        } else if !granted {
            DeviceGroup::AfterMicPermission
        } else {
            group(listed(outputs))
        },
    }
}

/// 両方向とも取得不可なら読み取りは成功と言えない（嘘の成功を避け、失敗で完了させる）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn nothing_readable(r: &AudioDevicesReading) -> bool {
    r.inputs == DeviceGroup::Unavailable && r.outputs == DeviceGroup::Unavailable
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::audio_devices_show::{
        finish_read, mark_reading, take_request, AudioDevicesShowState, ReadFailure,
    };
    use js_sys::{Array, Function, Reflect};
    use wasm_bindgen::prelude::*;

    use super::reading_from_entries;
    use crate::mount::Applier;

    /// 例外・reject 時の固定文言（egui-proxy 側と同じ）。
    const READ_FAILED: &str = "音声デバイスの読み取り";

    thread_local! {
        /// マウントごとの発送済み世代（同じ世代を二重に読まない）。
        static DISPATCHED: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に発送記録を解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        DISPATCHED.with(|d| d.borrow_mut().remove(&mount_id));
    }

    fn get(target: &JsValue, name: &str) -> JsValue {
        Reflect::get(target, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
    }

    /// `enumerateDevices()` を呼んで Promise を得る。呼べない環境は `Err(Unsupported)`、
    /// 同期例外は `Err(Failed)`。
    fn start_enumerate() -> Result<js_sys::Promise, ReadFailure> {
        let window = web_sys::window().ok_or(ReadFailure::Unsupported)?;
        if !window.is_secure_context() {
            return Err(ReadFailure::Unsupported);
        }
        let md: JsValue = window
            .navigator()
            .media_devices()
            .map_err(|_| ReadFailure::Unsupported)?
            .into();
        let f = get(&md, "enumerateDevices");
        if !f.is_function() {
            return Err(ReadFailure::Unsupported);
        }
        f.unchecked_into::<Function>()
            .call0(&md)
            .ok()
            .and_then(|p| p.dyn_into::<js_sys::Promise>().ok())
            .ok_or_else(|| ReadFailure::Failed(READ_FAILED.to_string()))
    }

    /// `HTMLMediaElement.prototype.setSinkId` があるか（出力を列挙するブラウザの目印）。
    fn output_supported() -> bool {
        let ctor = get(&js_sys::global(), "HTMLMediaElement");
        let proto = get(&ctor, "prototype");
        get(&proto, "setSinkId").is_function()
    }

    /// `document.permissionsPolicy`（旧 `featurePolicy`）で機能が許可されているか。
    /// 判定 API が無い・呼べないときは `None`。
    fn policy_allows(feature: &str) -> Option<bool> {
        let doc: JsValue = web_sys::window()?.document()?.into();
        let mut policy = get(&doc, "permissionsPolicy");
        if policy.is_undefined() || policy.is_null() {
            policy = get(&doc, "featurePolicy");
        }
        let f = get(&policy, "allowsFeature");
        if !f.is_function() {
            return None;
        }
        f.unchecked_into::<Function>()
            .call1(&policy, &JsValue::from_str(feature))
            .ok()?
            .as_bool()
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ読む。
    pub fn audio_devices_show_effect(
        state: &AudioDevicesShowState,
        applier: &Applier<AudioDevicesShowState>,
    ) {
        let Some(req) = take_request(state) else {
            return;
        };
        let generation = req.generation;
        let mount_id = applier.mount_id();
        let fresh =
            DISPATCHED.with(|d| d.borrow_mut().insert(mount_id, generation) != Some(generation));
        if !fresh {
            return;
        }
        applier.apply(mark_reading);
        let promise = match start_enumerate() {
            Ok(p) => p,
            Err(e) => {
                applier.apply(move |s| finish_read(s, generation, Err(e)));
                return;
            }
        };
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(list) => {
                    let entries: Vec<(String, String)> = Array::from(&list)
                        .iter()
                        .map(|d| {
                            (
                                get(&d, "kind").as_string().unwrap_or_default(),
                                get(&d, "label").as_string().unwrap_or_default(),
                            )
                        })
                        .collect();
                    let r = reading_from_entries(
                        &entries,
                        output_supported(),
                        policy_allows("microphone"),
                        policy_allows("speaker-selection"),
                    );
                    if super::nothing_readable(&r) {
                        Err(ReadFailure::Failed(READ_FAILED.to_string()))
                    } else {
                        Ok(r)
                    }
                }
                Err(_) => Err(ReadFailure::Failed(READ_FAILED.to_string())),
            };
            applier.apply(move |s| finish_read(s, generation, result));
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::audio_devices_show_effect;

/// マウント破棄時に発送記録を解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;

    fn e(kind: &str, label: &str) -> (String, String) {
        (kind.to_string(), label.to_string())
    }

    fn names(g: &DeviceGroup) -> Vec<Option<&str>> {
        match g {
            DeviceGroup::Listed(v) => v.iter().map(|n| n.as_deref()).collect(),
            other => panic!("Listed ではない: {other:?}"),
        }
    }

    #[test]
    fn granted_lists_both_and_keeps_duplicates() {
        let r = reading_from_entries(
            &[
                e("audioinput", " Mic\u{0}1 "),
                e("audioinput", "Mic1"),
                e("audiooutput", "Speaker"),
                e("videoinput", "Cam"),
            ],
            true,
            Some(true),
            Some(true),
        );
        assert_eq!(names(&r.inputs), [Some("Mic1"), Some("Mic1")]);
        assert_eq!(names(&r.outputs), [Some("Speaker")]);
    }

    #[test]
    fn before_permission_hides_names_and_counts() {
        let r = reading_from_entries(
            &[e("audioinput", ""), e("audiooutput", "")],
            true,
            Some(true),
            Some(true),
        );
        assert_eq!(r.inputs, DeviceGroup::AfterMicPermission);
        assert_eq!(r.outputs, DeviceGroup::AfterMicPermission);
    }

    #[test]
    fn no_inputs_before_permission_is_empty_input_and_pending_output() {
        let r = reading_from_entries(&[e("audiooutput", "")], true, Some(true), Some(true));
        assert_eq!(names(&r.inputs), Vec::<Option<&str>>::new());
        assert_eq!(r.outputs, DeviceGroup::AfterMicPermission);
    }

    #[test]
    fn output_unsupported_is_unavailable_not_zero() {
        let r = reading_from_entries(&[e("audioinput", "Mic")], false, Some(true), Some(true));
        assert_eq!(r.outputs, DeviceGroup::Unavailable);
        assert_eq!(names(&r.inputs), [Some("Mic")]);
    }

    #[test]
    fn granted_with_no_outputs_is_listed_zero() {
        let r = reading_from_entries(&[e("audioinput", "Mic")], true, Some(true), Some(true));
        assert_eq!(names(&r.outputs), Vec::<Option<&str>>::new());
    }

    #[test]
    fn caps_at_max() {
        let many: Vec<_> = (0..70).map(|i| e("audioinput", &format!("d{i}"))).collect();
        let r = reading_from_entries(&many, true, Some(true), Some(true));
        assert_eq!(
            names(&r.inputs).len(),
            fandhe_device::audio_devices::MAX_DEVICES
        );
    }

    #[test]
    fn policy_denied_direction_is_unavailable_not_zero() {
        let r = reading_from_entries(&[e("audioinput", "Mic")], true, Some(true), Some(false));
        assert_eq!(r.outputs, DeviceGroup::Unavailable);
        let r = reading_from_entries(&[e("audiooutput", "Spk")], true, Some(false), Some(true));
        assert_eq!(r.inputs, DeviceGroup::Unavailable);
    }

    #[test]
    fn both_directions_denied_is_nothing_readable() {
        let r = reading_from_entries(&[], true, Some(false), Some(false));
        assert!(super::nothing_readable(&r));
        let r = reading_from_entries(&[e("audioinput", "")], true, Some(true), Some(true));
        assert!(!super::nothing_readable(&r));
    }

    #[test]
    fn unknown_policy_with_empty_list_is_unavailable() {
        let r = reading_from_entries(&[e("audioinput", "Mic")], true, None, None);
        assert_eq!(r.outputs, DeviceGroup::Unavailable);
        assert_eq!(names(&r.inputs), [Some("Mic")]);
    }
}
