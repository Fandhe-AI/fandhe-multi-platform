//! Android 向けゲームコントローラ実装（Issue #935、親 #932、Windows 実装 #933 の横展開）。
//! `device/gamepad.rs` の `platform_gamepad()` Android 分岐から使われる。
//!
//! # 受け口が Java 側にある理由
//!
//! `android-activity`（GameActivity グルー）・`winit` の Android backend はいずれも
//! `SOURCE_TOUCHSCREEN` 以外の `MotionEvent` を native へ渡す前に捨てる／タッチとして
//! 扱ってしまうため、Rust 側だけではスティック・トリガの入力を受け取れない（#935
//! 実装計画 §2.1）。そのため既存構成（`MainActivity.java` + JNI push、傾き #831 と
//! 同型）で受ける。
//!
//! # 「観測のみ・消費しない」で TV フォーカス（#280）を守る
//!
//! `MainActivity` の `dispatchKeyEvent`/`dispatchGenericMotionEvent` はイベントを
//! 一切消費せず（`super.dispatch...` の戻り値をそのまま返す）、ゲームパッド由来なら
//! 追加で本モジュールの JNI エクスポートへ通知するだけに留める。リモコンの
//! `KEYCODE_DPAD_*` は従来どおり GameActivity → winit → `egui::Key::Arrow*` →
//! `tv_focus.rs` へ流れ続ける（#935 実装計画 §2.2）。
//!
//! # 接続状態はレベルトリガ（再走査）
//!
//! Android には軸の現在値を問い合わせる API が無い（イベント駆動のみ）ため、接続の
//! 有無は Java が `InputDevice.getDeviceIds()` を再走査して都度通知する
//! （`onResume`・`InputDeviceListener` の added/removed/changed・`onPause` では
//! 「無し」）。接続直後は「接続中 + 中立値」になる（実際に接続されており入力が無い
//! 状態の真値が中立であるため、`fandhe_device::gamepad` モジュール doc の
//! 「未接続を全 0 の接続中の代用にしない」契約には反しない）。
//! ponytail: 復帰時にスティックを倒したままだと次の `MotionEvent` まで中立値が
//! 天井になる。取りこぼしが問題になれば Java 側で最新値を都度問い合わせる経路へ
//! 上げる。
//!
//! # split pad（1 台のコントローラが複数 deviceId に分かれる機種）は非対応
//!
//! Joy-Con 等の split pad 機種は、Android 上ではボタン用・ジョイスティック用の
//! 2 つの `InputDevice`（deviceId）として別々に見える。Android は同一物理機器の
//! サブデバイスを OS 側で既に 1 つの `InputDevice` へ統合しており、2 つに見える
//! 時点で別機器という扱いになる。`InputDevice.getControllerNumber()` は機器ごとに
//! 固有の値であり、片割れ同士の共通識別には使えない（Android 公式仕様）。公開 API に
//! 複数 deviceId を再結合する手段が無いため、束ね機能は正しく作れない。よって本実装は
//! [`Tracked`] が deviceId を 1 つだけ保持し、走査で見つかった先頭 1 台のみを追跡する
//! （親 issue #932 の v1 範囲。PR #939 の codex-review 指摘 P0「Scan fuses unrelated
//! gamepad IDs」・P1「controllerNumber の前提が誤り」を受けて束ね機能自体を撤去した）。
//! split pad 機種は片側のみが操作対象になる（`docs/device/capabilities/gamepad.json`
//! の `android.note` にも明記）。
//!
//! # ロジックを Rust 側に寄せる理由
//!
//! `platforms/android/app/src/` に Java のテスト基盤が無いため、Java は生値
//! （deviceId・source・軸値・flat・keyCode）を primitive のみで渡し、ソース判定・
//! デッドゾーン・値域正規化・keyCode → ビット写像・状態遷移は本モジュールの
//! 純粋な状態機械（[`AndroidGamepadState`]）が担う。`device/tilt.rs::gravity_to_tilt`
//! と同じ「ロジックはホストの `cargo test` で検証する」方針。
//!
//! 非 Android ターゲットでは状態機械本体（`Gamepad` トレイト実装・JNI エクスポートを
//! 除く部分）は `#[cfg(test)]` のユニットテストのみが参照するため、`dead_code`
//! 誤検知を避けるためモジュール全体に `cfg_attr` で許可する
//! （`device/tilt.rs::GRAVITY_EARTH` と同じ判断）。

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

#[cfg(target_os = "android")]
use std::sync::Mutex;

use fandhe_device::gamepad::{
    normalize_axis, normalize_trigger, GamepadSnapshot, BUTTON_A, BUTTON_B, BUTTON_DPAD_DOWN,
    BUTTON_DPAD_LEFT, BUTTON_DPAD_RIGHT, BUTTON_DPAD_UP, BUTTON_LEFT_SHOULDER, BUTTON_LEFT_STICK,
    BUTTON_MENU, BUTTON_RIGHT_SHOULDER, BUTTON_RIGHT_STICK, BUTTON_VIEW, BUTTON_X, BUTTON_Y,
};

/// `MotionEvent`/`KeyEvent` の `source`（Android `InputDevice`）が
/// ゲームパッド／ジョイスティック由来かどうかを判定する。`SOURCE_GAMEPAD`
/// （`0x0401`）・`SOURCE_JOYSTICK`（`0x0100_0010`）いずれかのビットが立って
/// いれば真（#935 実装計画 §3、Android 公式定数値）。
///
/// HAT 軸を持たず十字キーを `SOURCE_DPAD`（`0x0201`）の `KeyEvent` のみで通知する
/// 機種があるため、`SOURCE_DPAD` が立ち `SOURCE_KEYBOARD`（`0x0101`）を含まない
/// 場合も真とする。TV リモコンの `KEYCODE_DPAD_*` は `SOURCE_KEYBOARD | SOURCE_DPAD`
/// （`0x0301`）で来るため、`SOURCE_KEYBOARD` ビットで区別すれば TV フォーカス
/// （#280）の経路を誤って「接続中」にしない（codex-review 指摘、PR #939）。
fn is_gamepad_source(source: i32) -> bool {
    const SOURCE_GAMEPAD: i32 = 0x0000_0401;
    const SOURCE_JOYSTICK: i32 = 0x0100_0010;
    const SOURCE_DPAD: i32 = 0x0000_0201;
    const SOURCE_KEYBOARD: i32 = 0x0000_0101;
    (source & SOURCE_GAMEPAD) == SOURCE_GAMEPAD
        || (source & SOURCE_JOYSTICK) == SOURCE_JOYSTICK
        || ((source & SOURCE_DPAD) == SOURCE_DPAD && (source & SOURCE_KEYBOARD) != SOURCE_KEYBOARD)
}

/// スティック軸のデッドゾーン処理。`|value| <= flat` はノイズとして `0.0` へ丸め、
/// それ以外は素通しする。NaN・負の `flat` は `0.0` 扱いにして panic せず安全側へ倒す
/// （security.md A03、OS からの想定外入力を検証せず展開しない）。値域クランプ自体は
/// 呼び出し元が [`normalize_axis`] で行う（責務を分離し、本関数は閾値判定のみに絞る）。
fn apply_flat(value: f32, flat: f32) -> f32 {
    let flat = if flat.is_nan() || flat < 0.0 {
        0.0
    } else {
        flat
    };
    if value.is_nan() || value.abs() <= flat {
        0.0
    } else {
        value
    }
}

/// `KeyEvent#getKeyCode()`（Android `KeyEvent.KEYCODE_BUTTON_*`/`KEYCODE_DPAD_*`）を
/// [`fandhe_device::gamepad`] のビット定数へ写像する。未知の keyCode は `None`
/// （panic せず無視する、security.md A03）。`L2`/`R2` のデジタルのみ報告する機種の
/// トリガ反映はスコープ外（#935 実装計画 §7）。
fn map_key_code(key_code: i32) -> Option<u16> {
    Some(match key_code {
        96 => BUTTON_A,
        97 => BUTTON_B,
        99 => BUTTON_X,
        100 => BUTTON_Y,
        102 => BUTTON_LEFT_SHOULDER,
        103 => BUTTON_RIGHT_SHOULDER,
        106 => BUTTON_LEFT_STICK,
        107 => BUTTON_RIGHT_STICK,
        108 => BUTTON_MENU,
        109 => BUTTON_VIEW,
        19 => BUTTON_DPAD_UP,
        20 => BUTTON_DPAD_DOWN,
        21 => BUTTON_DPAD_LEFT,
        22 => BUTTON_DPAD_RIGHT,
        _ => return None,
    })
}

/// `AXIS_HAT_X`/`AXIS_HAT_Y` の生値を十字キービットへ写像する（閾値 0.5）。
/// Android の `AXIS_HAT_Y` は下方向が正（`fandhe_core::screens::tilt_ball` と同じ
/// 画面座標系、`gamepad.rs` モジュール doc の軸の向き契約参照）のため、負が UP。
fn hat_to_buttons(hat_x: f32, hat_y: f32) -> u16 {
    const HAT_THRESHOLD: f32 = 0.5;
    let mut buttons = 0u16;
    if hat_y <= -HAT_THRESHOLD {
        buttons |= BUTTON_DPAD_UP;
    }
    if hat_y >= HAT_THRESHOLD {
        buttons |= BUTTON_DPAD_DOWN;
    }
    if hat_x <= -HAT_THRESHOLD {
        buttons |= BUTTON_DPAD_LEFT;
    }
    if hat_x >= HAT_THRESHOLD {
        buttons |= BUTTON_DPAD_RIGHT;
    }
    buttons
}

/// 先頭 1 台ぶんの最新値（[`fandhe_device::gamepad`] モジュール doc の「複数台接続」
/// 契約: 2 台目以降は無視する。split pad 機種の非対応についてはモジュール doc
/// 「split pad は非対応」参照、PR #939）。
struct Tracked {
    device_id: i32,
    left: (f32, f32),
    right: (f32, f32),
    left_trigger: f32,
    right_trigger: f32,
    /// `KeyEvent` 由来の押下ビット。
    key_buttons: u16,
    /// `AXIS_HAT_*` 由来の十字キービット（`key_buttons` とは別に持つ。十字キーが
    /// `KeyEvent`/`MotionEvent` どちらで来るかは機種依存のため）。
    hat_buttons: u16,
}

impl Tracked {
    /// 接続直後・別デバイスへの切替直後の中立値（モジュール doc「接続状態は
    /// レベルトリガ」参照。実際に接続されており入力が無い状態の真値であり、
    /// 未接続を偽装するものではない）。
    fn neutral(device_id: i32) -> Self {
        Self {
            device_id,
            left: (0.0, 0.0),
            right: (0.0, 0.0),
            left_trigger: 0.0,
            right_trigger: 0.0,
            key_buttons: 0,
            hat_buttons: 0,
        }
    }

    fn buttons(&self) -> u16 {
        self.key_buttons | self.hat_buttons
    }
}

/// 走査（[`AndroidGamepadState::presence_scan_begin`]〜`presence_scan_end`）で 1 件
/// 記録できる deviceId 数の上限。Java 側の異常（バグ・大量デバイス接続）で際限なく
/// 増え続けないよう安全側の上限を設ける（security.md A03）。
const MAX_SCAN_ENTRIES: usize = 16;

/// 走査中に見つかった deviceId の一覧。
#[derive(Default)]
struct PendingScan {
    device_ids: Vec<i32>,
}

impl PendingScan {
    /// `device_id` を記録する。重複・上限超過時は無視する
    /// （panic せず安全側へ倒す、security.md A03）。
    fn push(&mut self, device_id: i32) {
        if self.device_ids.contains(&device_id) {
            return;
        }
        if self.device_ids.len() < MAX_SCAN_ENTRIES {
            self.device_ids.push(device_id);
        }
    }

    fn is_empty(&self) -> bool {
        self.device_ids.is_empty()
    }

    fn contains(&self, device_id: i32) -> bool {
        self.device_ids.contains(&device_id)
    }

    /// 走査で最初に見つかった deviceId。
    fn first(&self) -> Option<i32> {
        self.device_ids.first().copied()
    }
}

/// Android ゲームコントローラ入力の状態機械（純粋・ターゲット非依存）。JNI 越しに
/// Java から届く生値を受け取り、[`fandhe_device::gamepad::Gamepad::read`] が返す
/// スナップショットへ写像する。ホストの `cargo test` から `cfg` 抜きで検証できる
/// （モジュール doc「ロジックを Rust 側に寄せる理由」参照）。
pub(crate) struct AndroidGamepadState {
    current: Option<Tracked>,
    /// `presence()` が一度でも呼ばれたか（Bugbot 指摘 PR #939）。`false` の間だけ
    /// `accept_or_adopt` の「先頭 1 台の走査漏れの保険」を有効にする。一度
    /// `presence()` を経た後は、切断（`presence(false, _)`）後に遅延到着した
    /// `MotionEvent`/`KeyEvent` が `current == None` を「新規デバイス」と誤認して
    /// 再採用し、disconnect を無かったことにするのを防ぐ（再接続は Java 側が
    /// `InputDeviceListener` の added イベントで必ず `presence(true, _)` を送るため、
    /// この保険が無くても再採用できる）。
    presence_seen: bool,
    /// [`Self::presence_scan_begin`]〜[`Self::presence_scan_end`] の間だけ `Some`
    /// （walk中に見つかった deviceId を蓄積する）。
    pending_scan: Option<PendingScan>,
}

impl AndroidGamepadState {
    /// 空状態（未接続）で構築する。
    pub(crate) const fn new() -> Self {
        Self {
            current: None,
            presence_seen: false,
            pending_scan: None,
        }
    }

    /// `current` が `device_id` を追跡中ならその可変参照を返す。未追跡（`None`）かつ
    /// `presence()`/scan がまだ一度も呼ばれていなければ、中立値で新規に採用する
    /// （先頭 1 台の走査漏れの保険、`presence_seen` doc 参照）。別デバイスを追跡中、
    /// または走査を経た後に切断済みで `current` が `None` の場合は `None`
    /// （先頭 1 台のみを扱う契約、2 台目以降は無視する。切断後の遅延イベントで
    /// 再採用しない）。
    fn accept_or_adopt(&mut self, device_id: i32) -> Option<&mut Tracked> {
        if self.current.is_none() && !self.presence_seen {
            self.current = Some(Tracked::neutral(device_id));
        }
        match &mut self.current {
            Some(tracked) if tracked.device_id == device_id => Some(tracked),
            _ => None,
        }
    }

    /// 単一デバイスの接続/切断を通知する簡易 API。内部的には
    /// [`Self::presence_scan_begin`]/[`Self::presence_scan_id`]/[`Self::presence_scan_end`]
    /// を組み合わせる（テスト専用のショートハンド。Android ターゲットの JNI 経路は
    /// 常に scan 3 関数を直接呼ぶため、`target_os = "android"` では未使用になる）。
    #[cfg_attr(target_os = "android", allow(dead_code))]
    pub(crate) fn presence(&mut self, present: bool, device_id: i32) {
        self.presence_scan_begin();
        if present {
            self.presence_scan_id(device_id);
        }
        self.presence_scan_end();
    }

    /// Java 側の再走査（`reportGamepadPresence()`）の開始を通知する。走査中に見つかる
    /// deviceId は [`Self::presence_scan_id`] で 1 件ずつ通知される。
    pub(crate) fn presence_scan_begin(&mut self) {
        self.presence_seen = true;
        self.pending_scan = Some(PendingScan::default());
    }

    /// 走査中に見つかったゲームパッド/ジョイスティックソースの `device_id` を 1 件
    /// 通知する。[`Self::presence_scan_begin`] より前の呼び出しは無視する
    /// （防御的、security.md A03。呼び出し順序は Java 側の実装が保証するが panic
    /// はしない）。
    pub(crate) fn presence_scan_id(&mut self, device_id: i32) {
        if let Some(scan) = &mut self.pending_scan {
            scan.push(device_id);
        }
    }

    /// Java 側の再走査結果を確定する。走査で 1 件も見つからなければ `None`（切断後に
    /// 古い値を返さない契約）。
    ///
    /// 現在の追跡対象の deviceId が走査結果に残っていればそのまま値を保持する。
    /// 残っていなければ未接続へ戻すのではなく、走査で最初に見つかった deviceId を
    /// 新たな追跡対象として中立値で採用し直す（別デバイスへの切り替え、または
    /// 初回接続。split pad 等の複数 deviceId 束ねは行わない。モジュール doc
    /// 「split pad は非対応」参照、PR #939）。
    pub(crate) fn presence_scan_end(&mut self) {
        let scan = self.pending_scan.take().unwrap_or_default();
        if scan.is_empty() {
            self.current = None;
            return;
        }

        let still_present = self
            .current
            .as_ref()
            .is_some_and(|tracked| scan.contains(tracked.device_id));

        if !still_present {
            // `is_empty()` を確認済みのため `first()` は必ず `Some`。
            if let Some(first_id) = scan.first() {
                self.current = Some(Tracked::neutral(first_id));
            }
        }
    }

    /// `dispatchGenericMotionEvent` から届いたスティック・トリガ・十字キー軸の最新
    /// サンプルを反映する。非ゲームパッドソース・別デバイス（先頭 1 台以外）は無視する
    /// （モジュール doc「split pad は非対応」参照。追跡中でない deviceId からの
    /// 入力は他コントローラのものとして混ぜない）。戻り値は実際に追跡中デバイスへ
    /// 反映し、かつスナップショットが変化したか（`ACTION_MOVE` は静止中も連続発火
    /// するため、`apply_flat` 適用後の値が前回と同一なら `false` を返し、呼び出し元の
    /// `request_repaint` 連打を防ぐ。`key` が変化時のみ `true` を返すのと同じ契約）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn motion(
        &mut self,
        device_id: i32,
        source: i32,
        x: f32,
        y: f32,
        z: f32,
        rz: f32,
        left_trigger: f32,
        right_trigger: f32,
        hat_x: f32,
        hat_y: f32,
        flat_x: f32,
        flat_y: f32,
        flat_z: f32,
        flat_rz: f32,
    ) -> bool {
        if !is_gamepad_source(source) {
            return false;
        }
        let Some(tracked) = self.accept_or_adopt(device_id) else {
            return false;
        };
        let left = (
            normalize_axis(apply_flat(x, flat_x)),
            normalize_axis(apply_flat(y, flat_y)),
        );
        let right = (
            normalize_axis(apply_flat(z, flat_z)),
            normalize_axis(apply_flat(rz, flat_rz)),
        );
        let left_trigger = normalize_trigger(left_trigger);
        let right_trigger = normalize_trigger(right_trigger);
        let hat_buttons = hat_to_buttons(hat_x, hat_y);
        let changed = tracked.left != left
            || tracked.right != right
            || tracked.left_trigger != left_trigger
            || tracked.right_trigger != right_trigger
            || tracked.hat_buttons != hat_buttons;
        tracked.left = left;
        tracked.right = right;
        tracked.left_trigger = left_trigger;
        tracked.right_trigger = right_trigger;
        tracked.hat_buttons = hat_buttons;
        changed
    }

    /// `dispatchKeyEvent` から届いたボタンの押下/解放を反映する。非ゲームパッド
    /// ソース・未知の keyCode・別デバイスは無視する。状態が実際に変化した場合のみ
    /// `true` を返す（ボタン長押し中の ACTION_DOWN 連打で毎回 repaint させないため、
    /// 呼び出し元はこの戻り値で repaint の要否を判断する）。
    pub(crate) fn key(&mut self, device_id: i32, source: i32, key_code: i32, down: bool) -> bool {
        if !is_gamepad_source(source) {
            return false;
        }
        let Some(bit) = map_key_code(key_code) else {
            return false;
        };
        let Some(tracked) = self.accept_or_adopt(device_id) else {
            return false;
        };
        let before = tracked.key_buttons;
        if down {
            tracked.key_buttons |= bit;
        } else {
            tracked.key_buttons &= !bit;
        }
        tracked.key_buttons != before
    }

    /// [`fandhe_device::gamepad::Gamepad::read`] が返すスナップショットを組み立てる。
    /// 未接続（`current` が `None`）は `None`（全 0 の接続中と混同しない契約）。
    pub(crate) fn snapshot(&self) -> Option<GamepadSnapshot> {
        self.current.as_ref().map(|tracked| GamepadSnapshot {
            left_stick: tracked.left,
            right_stick: tracked.right,
            left_trigger: tracked.left_trigger,
            right_trigger: tracked.right_trigger,
            buttons: tracked.buttons(),
        })
    }
}

impl Default for AndroidGamepadState {
    fn default() -> Self {
        Self::new()
    }
}

// --------------------------------------------------------------------------
// Android ターゲット限定（`Gamepad` トレイト実装・JNI エクスポート）
// --------------------------------------------------------------------------

/// プロセス内で共有する状態（Java 側 JNI コールバックはどのスレッドからでも
/// 届き得るため `Mutex`、`device/tilt.rs::MOTION` と同じ考え方）。
#[cfg(target_os = "android")]
static STATE: Mutex<AndroidGamepadState> = Mutex::new(AndroidGamepadState::new());

/// Android 実ゲームコントローラ実装（Issue #935）。`device/gamepad.rs` の
/// `platform_gamepad()` Android 分岐から構築される。
#[cfg(target_os = "android")]
pub struct AndroidGamepad;

#[cfg(target_os = "android")]
impl fandhe_device::gamepad::Gamepad for AndroidGamepad {
    fn read(&self) -> Result<Option<GamepadSnapshot>, fandhe_device::DeviceError> {
        let state = STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(state.snapshot())
    }
}

/// `MainActivity.nativeOnGamepadPresenceScanBegin`（Java 側）から呼ばれる JNI
/// エクスポート関数（Issue #935）。`reportGamepadPresence()` が `InputDevice.getDeviceIds()`
/// の再走査を開始したことを通知する。以降の deviceId は
/// [`Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanId`] で 1 件ずつ
/// 届き、[`Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanEnd`] で
/// 確定する（モジュール doc「接続状態はレベルトリガ」参照）。
///
/// `JNIEnv`/`jclass` は本関数内で一切使わず、生ポインタへのアクセスを行わないため
/// 関数本体に `unsafe` 操作は含まない。`#[no_mangle]` はこの 1 関数に閉じ、`unsafe`
/// を公開 API 側へ漏らさない（`.claude/rules/coding-rust.md` の FFI 例外規定準拠）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` により Rust の名前修飾を
/// 無効化するが、シンボル名は JNI 命名規約上の固定名であり、本クレートは cdylib
/// として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない
/// （`device/android.rs::nativeOnMotionChanged` と同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanBegin(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
) {
    let mut state = STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.presence_scan_begin();
}

/// `MainActivity.nativeOnGamepadPresenceScanId`（Java 側）から呼ばれる JNI
/// エクスポート関数（Issue #935）。走査中に見つかったゲームパッド/ジョイスティック
/// ソースの `device_id` を 1 件通知する。安全性根拠は
/// [`Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanBegin`] と同じ。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanId(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    device_id: i32,
) {
    let mut state = STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.presence_scan_id(device_id);
}

/// `MainActivity.nativeOnGamepadPresenceScanEnd`（Java 側）から呼ばれる JNI
/// エクスポート関数（Issue #935）。走査結果を確定する（`AndroidGamepadState::presence_scan_end`
/// 参照）。安全性根拠は
/// [`Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanBegin`] と同じ。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanEnd(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
) {
    {
        let mut state = STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.presence_scan_end();
    }
    // ユーザー操作（コントローラの接続/切断）起点のみのため、センサのような
    // 画面ゲート（`device/android.rs::active_screen_uses_tilt` 相当）は不要。
    crate::android::request_repaint();
}

/// `MainActivity.nativeOnGamepadMotion`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #935）。`dispatchGenericMotionEvent` が最新サンプルのみ（履歴サンプルは
/// 読まない）を中継する。安全性根拠は [`Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanBegin`]
/// と同じ。
///
/// `request_repaint` は (1) `gamepad_show` 画面を表示中（[`crate::android::active_screen_uses_gamepad`]）
/// かつ (2) 追跡中デバイスへ反映しスナップショットが実際に変化した（[`AndroidGamepadState::motion`]
/// の戻り値）場合に限って呼ぶ（Bugbot 指摘 PR #939。無視対象デバイス・非表示画面でのスティック
/// ドリフトが毎サンプル UI ループを起こすのを防ぐ、`ACTIVE_SCREEN_USES_GAMEPAD` doc と同じ理由。
/// `ACTION_MOVE` は静止中も連続発火するため、変化判定が無いと静止スティックでも
/// 毎サンプル repaint し続ける、二次指摘参照）。
#[cfg(target_os = "android")]
#[allow(unsafe_code, clippy::too_many_arguments)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadMotion(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    device_id: i32,
    source: i32,
    x: f32,
    y: f32,
    z: f32,
    rz: f32,
    left_trigger: f32,
    right_trigger: f32,
    hat_x: f32,
    hat_y: f32,
    flat_x: f32,
    flat_y: f32,
    flat_z: f32,
    flat_rz: f32,
) {
    let updated = {
        let mut state = STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.motion(
            device_id,
            source,
            x,
            y,
            z,
            rz,
            left_trigger,
            right_trigger,
            hat_x,
            hat_y,
            flat_x,
            flat_y,
            flat_z,
            flat_rz,
        )
    };
    if updated && crate::android::active_screen_uses_gamepad() {
        crate::android::request_repaint();
    }
}

/// `MainActivity.nativeOnGamepadKey`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #935）。`dispatchKeyEvent` の DOWN/UP を中継する。安全性根拠は
/// [`Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanBegin`] と同じ。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadKey(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    device_id: i32,
    source: i32,
    key_code: i32,
    down: u8,
) {
    let updated = {
        let mut state = STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.key(device_id, source, key_code, down != 0)
    };
    if updated && crate::android::active_screen_uses_gamepad() {
        crate::android::request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE_GAMEPAD: i32 = 0x0000_0401;
    const SOURCE_JOYSTICK: i32 = 0x0100_0010;
    /// リモコン（キーボード + DPAD）のソース値（`SOURCE_KEYBOARD | SOURCE_DPAD`）。
    const SOURCE_DPAD_KEYBOARD: i32 = 0x0301;
    /// タッチスクリーンのソース値。
    const SOURCE_TOUCHSCREEN: i32 = 0x1002;

    #[test]
    fn initial_snapshot_is_none() {
        let state = AndroidGamepadState::new();
        assert!(state.snapshot().is_none());
    }

    #[test]
    fn presence_true_gives_neutral_snapshot_then_motion_fills_it() {
        let mut state = AndroidGamepadState::new();
        state.presence(true, 1);
        let neutral = state.snapshot().expect("接続直後は Some");
        assert_eq!(neutral.left_stick, (0.0, 0.0));

        state.motion(
            1,
            SOURCE_JOYSTICK,
            0.5,
            -0.5,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        let after = state.snapshot().unwrap();
        assert_eq!(after.left_stick, (0.5, -0.5));
    }

    /// Bugbot 二次指摘（PR #939）: `ACTION_MOVE` は静止中も連続発火するため、
    /// 同一サンプルの再送では `false`（変化なし）を返し、呼び出し元の
    /// `request_repaint` 連打を防ぐことを確認する。
    #[test]
    fn motion_returns_false_when_snapshot_unchanged() {
        let mut state = AndroidGamepadState::new();
        state.presence(true, 1);
        let sample = (
            1,
            SOURCE_JOYSTICK,
            0.5,
            -0.5,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        assert!(state.motion(
            sample.0, sample.1, sample.2, sample.3, sample.4, sample.5, sample.6, sample.7,
            sample.8, sample.9, sample.10, sample.11, sample.12, sample.13,
        ));
        assert!(!state.motion(
            sample.0, sample.1, sample.2, sample.3, sample.4, sample.5, sample.6, sample.7,
            sample.8, sample.9, sample.10, sample.11, sample.12, sample.13,
        ));
    }

    #[test]
    fn presence_false_resets_to_none_and_does_not_keep_stale_value() {
        let mut state = AndroidGamepadState::new();
        state.presence(true, 1);
        state.motion(
            1,
            SOURCE_JOYSTICK,
            1.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        state.presence(false, 1);
        assert!(state.snapshot().is_none());

        // 再接続時は前回の値を引き継がず中立値になる。
        state.presence(true, 1);
        assert_eq!(state.snapshot().unwrap().left_stick, (0.0, 0.0));
    }

    #[test]
    fn switching_device_id_drops_stale_value_and_buttons() {
        let mut state = AndroidGamepadState::new();
        state.presence(true, 1);
        state.motion(
            1,
            SOURCE_JOYSTICK,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        state.key(1, SOURCE_GAMEPAD, 96, true); // A

        state.presence(true, 2);
        let snapshot = state.snapshot().unwrap();
        assert_eq!(snapshot.left_stick, (0.0, 0.0));
        assert_eq!(snapshot.buttons, 0);
    }

    #[test]
    fn non_gamepad_source_motion_and_key_are_ignored() {
        let mut state = AndroidGamepadState::new();
        // 未接続のまま（`presence` を呼んでいない）リモコン/タッチのイベントが
        // 来ても「接続中」にならない。
        state.motion(
            1,
            SOURCE_DPAD_KEYBOARD,
            1.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        state.key(1, SOURCE_TOUCHSCREEN, 19, true);
        assert!(state.snapshot().is_none());
    }

    /// HAT 軸を持たず十字キーを `SOURCE_DPAD`（`SOURCE_KEYBOARD` を含まない）で
    /// 通知する機種でも DOWN が反映されることを確認する（codex-review 指摘 PR #939）。
    #[test]
    fn dpad_only_source_key_without_keyboard_bit_is_accepted() {
        const SOURCE_DPAD_ONLY: i32 = 0x0000_0201;
        let mut state = AndroidGamepadState::new();
        state.presence(true, 1);
        state.key(1, SOURCE_DPAD_ONLY, 19, true); // KEYCODE_DPAD_UP
        assert_eq!(state.snapshot().unwrap().buttons, BUTTON_DPAD_UP);
    }

    /// TV リモコン（`SOURCE_KEYBOARD | SOURCE_DPAD`）は `SOURCE_KEYBOARD` ビットを
    /// 含むため引き続き無視される（#280 のリモコン経路を誤って奪わない）。
    #[test]
    fn dpad_with_keyboard_bit_is_still_ignored() {
        let mut state = AndroidGamepadState::new();
        state.presence(true, 1);
        state.key(1, SOURCE_DPAD_KEYBOARD, 19, true);
        assert_eq!(state.snapshot().unwrap().buttons, 0);
    }

    /// Bugbot 指摘（PR #939）: `presence(false)` 後に同一デバイス由来の遅延到着
    /// `MotionEvent`/`KeyEvent` が届いても、`disconnect` が undone されない
    /// （`current` が `None` のまま）ことを確認する。
    #[test]
    fn late_event_after_presence_false_does_not_resurrect_device() {
        let mut state = AndroidGamepadState::new();
        state.presence(true, 1);
        state.presence(false, 1);
        assert!(state.snapshot().is_none());

        // 切断済みデバイスからの遅延到着イベント。
        state.motion(
            1,
            SOURCE_JOYSTICK,
            1.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        state.key(1, SOURCE_GAMEPAD, 96, true);
        assert!(
            state.snapshot().is_none(),
            "presence(false) 後の遅延イベントで再採用されてはならない"
        );

        // 正規の再接続（presence(true)）では改めて追跡が始まる。
        state.presence(true, 1);
        assert_eq!(state.snapshot().unwrap().left_stick, (0.0, 0.0));
    }

    /// `presence()` が一度も呼ばれていない起動直後は、従来どおり最初のイベントで
    /// 走査漏れの保険が働くことを確認する（回帰防止）。
    #[test]
    fn motion_before_first_presence_call_still_adopts_as_safety_net() {
        let mut state = AndroidGamepadState::new();
        state.motion(
            1,
            SOURCE_JOYSTICK,
            0.5,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        assert!(state.snapshot().is_some());
    }

    #[test]
    fn second_device_id_events_are_ignored_while_first_is_tracked() {
        let mut state = AndroidGamepadState::new();
        state.presence(true, 1);
        state.motion(
            2,
            SOURCE_JOYSTICK,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        assert_eq!(state.snapshot().unwrap().left_stick, (0.0, 0.0));
    }

    #[test]
    fn axis_y_positive_maps_to_left_stick_y_positive_without_sign_flip() {
        // Windows 実装（`WindowsGamepad`）とは異なり符号反転しない（モジュール doc 参照）。
        let mut state = AndroidGamepadState::new();
        state.presence(true, 1);
        state.motion(
            1,
            SOURCE_JOYSTICK,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        assert_eq!(state.snapshot().unwrap().left_stick.1, 1.0);
    }

    #[test]
    fn out_of_range_and_nan_axis_values_are_rounded() {
        let mut state = AndroidGamepadState::new();
        state.presence(true, 1);
        state.motion(
            1,
            SOURCE_JOYSTICK,
            2.0,
            f32::NAN,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        let snapshot = state.snapshot().unwrap();
        assert_eq!(snapshot.left_stick, (1.0, 0.0));
    }

    #[test]
    fn apply_flat_zeroes_within_flat_and_passes_through_outside() {
        assert_eq!(apply_flat(0.1, 0.2), 0.0);
        assert_eq!(apply_flat(0.5, 0.2), 0.5);
        assert_eq!(apply_flat(0.5, f32::NAN), 0.5);
        assert_eq!(apply_flat(0.1, -1.0), 0.1);
        assert_eq!(apply_flat(f32::NAN, 0.1), 0.0);
    }

    #[test]
    fn key_down_and_up_toggle_bits_and_repeat_down_is_idempotent() {
        let mut state = AndroidGamepadState::new();
        state.presence(true, 1);
        assert!(state.key(1, SOURCE_GAMEPAD, 96, true)); // A down: 変化あり
        assert!(!state.key(1, SOURCE_GAMEPAD, 96, true)); // repeat down: 変化なし
        assert_eq!(state.snapshot().unwrap().buttons, BUTTON_A);

        assert!(state.key(1, SOURCE_GAMEPAD, 96, false)); // A up: 変化あり
        assert_eq!(state.snapshot().unwrap().buttons, 0);
    }

    #[test]
    fn unknown_key_code_is_ignored() {
        let mut state = AndroidGamepadState::new();
        state.presence(true, 1);
        state.key(1, SOURCE_GAMEPAD, 4242, true);
        assert_eq!(state.snapshot().unwrap().buttons, 0);
    }

    #[test]
    fn hat_axis_maps_to_dpad_buttons_and_ors_with_key_buttons() {
        let mut state = AndroidGamepadState::new();
        state.presence(true, 1);
        state.key(1, SOURCE_GAMEPAD, 96, true); // A
        state.motion(
            1,
            SOURCE_JOYSTICK,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            -1.0,
            -1.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        let buttons = state.snapshot().unwrap().buttons;
        assert_eq!(buttons, BUTTON_A | BUTTON_DPAD_UP | BUTTON_DPAD_LEFT);
    }

    /// 走査で 2 台見つかった場合（split pad の 2 つの deviceId、または無関係な
    /// 独立した 2 台のコントローラ、いずれにせよ Rust 側からは区別できない）は
    /// 先頭 1 台のみを追跡し、2 台目からの入力は混入しないことを確認する
    /// （モジュール doc「split pad は非対応」参照、PR #939 の設計変更）。
    #[test]
    fn scan_with_two_devices_tracks_only_the_first_and_ignores_the_second() {
        let mut state = AndroidGamepadState::new();
        state.presence_scan_begin();
        state.presence_scan_id(1);
        state.presence_scan_id(2);
        state.presence_scan_end();

        assert!(state.key(1, SOURCE_GAMEPAD, 96, true)); // A（id=1、先頭）
        assert!(!state.key(2, SOURCE_GAMEPAD, 96, true)); // A（id=2、無視されるべき）
        assert!(!state.motion(
            2,
            SOURCE_JOYSTICK,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        ));

        let snapshot = state.snapshot().unwrap();
        assert_eq!(snapshot.buttons, BUTTON_A, "2 台目の入力は無視されるべき");
        assert_eq!(
            snapshot.left_stick,
            (0.0, 0.0),
            "2 台目の入力は無視されるべき"
        );
    }

    /// 追跡中の deviceId が再走査結果から消えたら未接続へ戻り、値が残らないことを
    /// 確認する（codex-review 指摘 P1 対応、PR #939）。
    #[test]
    fn tracked_device_missing_from_rescan_reverts_to_disconnected() {
        let mut state = AndroidGamepadState::new();
        state.presence_scan_begin();
        state.presence_scan_id(1);
        state.presence_scan_end();
        state.key(1, SOURCE_GAMEPAD, 96, true); // A

        // 再走査で id=1 が見つからなくなる（切断）。
        state.presence_scan_begin();
        state.presence_scan_end();

        assert!(
            state.snapshot().is_none(),
            "走査から消えたら未接続へ戻り、A が張り付いてはならない"
        );
    }

    /// 切断後に別の deviceId が走査で現れたら、新たな追跡対象として中立値から
    /// 採用し直すことを確認する（旧デバイスの押下状態を引き継がない）。
    #[test]
    fn new_device_after_disconnect_is_adopted_fresh() {
        let mut state = AndroidGamepadState::new();
        state.presence_scan_begin();
        state.presence_scan_id(1);
        state.presence_scan_end();
        state.key(1, SOURCE_GAMEPAD, 96, true); // A

        // id=1 が消え、別の id=2 が新たに現れる。
        state.presence_scan_begin();
        state.presence_scan_id(2);
        state.presence_scan_end();

        assert_eq!(
            state.snapshot().unwrap().buttons,
            0,
            "新たな deviceId は中立値で採用され、旧デバイスの A を引き継がない"
        );
        assert!(state.key(2, SOURCE_GAMEPAD, 97, true)); // B（id=2 から受理される）
        assert_eq!(
            state.snapshot().unwrap().buttons,
            fandhe_device::gamepad::BUTTON_B
        );
    }

    #[test]
    #[cfg(target_os = "android")]
    fn read_returns_ok_none_when_unconnected() {
        use fandhe_device::gamepad::Gamepad;
        let gamepad = AndroidGamepad;
        assert_eq!(gamepad.read().unwrap(), None);
    }
}
