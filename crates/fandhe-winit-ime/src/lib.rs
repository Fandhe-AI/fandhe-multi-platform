//! `fandhe-core` の IME 境界型（[`fandhe_core::ui::ImeEvent`] / [`fandhe_core::ui::ImeRequest`]）と
//! winit（`WindowEvent::Ime(Ime::*)` / `Window::set_ime_*`）を接続するプラットフォーム統合レイヤー
//! （REQ-8 / TASK-8.5-S7、Issue #188）。
//!
//! `docs/architecture/ime-architecture.md` §4.4「winit マッピング（デスクトップ: macOS・
//! Windows・Linux、S7・S9〜S14）」の実装。二層アーキテクチャ（coding-rust.md: 依存方向は
//! 最適化レイヤ → コアの一方向）に従い、winit 依存コードは `fandhe-core`
//! （`#![forbid(unsafe_code)]`）に置けないため、本クレートへ切り出している。
//!
//! # スコープ
//!
//! 本クレートが提供するのは**純粋な変換レイヤ**（関数・トレイト）に加え、
//! [`session::ImeSessionTracker`]（Issue #587。winit にはネイティブなセッション概念が
//! ないため、`ImeRequest::BeginSession`/`ImeEvent` の観測列から帰属セッションを算出する
//! 小さな状態機械）である。
//! - [`ime_event_from_winit`]: winit → コア（`WindowEvent::Ime` 受信時に呼ぶ）
//! - [`ImeWindow`] / [`apply_ime_request`]: コア → winit（`ImeRequest` 発生時に呼ぶ）
//! - [`session::ImeSessionTracker`]: `ImeFocusCoordinator::dispatch_ime` へ渡す帰属
//!   セッションの算出（`session` モジュール doc「付け替え規則」参照）
//!
//! イベントループへの実際の配線（`winit::event_loop::ApplicationHandler` 実装からの
//! 呼び出し）・`TextFieldWidget<S>` との結線は本クレートには含まない
//! （TASK-8.5-S8・Issue #189、設計文書 §8「スコープ外」）。同様にコンポジション状態モデルの
//! 保持（Issue #184）、preedit 描画（Issue #186）、候補ウィンドウ位置計算（Issue #187）も対象外。
//!
//! macOS 向けの本レイヤ（winit 経由で `NSTextInputClient` へ委譲される契約）は
//! TASK-8.5-S9（#190）でソースレベル検証済み。
//! Windows 向けの本レイヤ（winit 経由で IMM32 API へ委譲される契約。TSF ベースの
//! IME は OS 側の IMM32 互換レイヤ経由で連携する）も TASK-8.5-S11（#192）で
//! ソースレベル検証済み。
//! Linux 向けの本レイヤ（winit 経由で X11 は XIM、Wayland は `zwp_text_input_v3` へ
//! 委譲される契約。ibus/fcitx はそれぞれ XIM サーバー・コンポジタ実装経由で連携する）
//! も TASK-8.5-S14（#195）でソースレベル + シーケンス再現検証済み。
//! X11・Wayland いずれも `Ime::Enabled`/`Disabled` が `SetEnabled` 要求・
//! フォーカス変化に同期して即時発火する点
//! （macOS・Windows の遅延発火とは異なる）、`set_ime_purpose` が Wayland でのみ実際に
//! 有効である点（X11 は macOS・Windows と同様に no-op）が判明している。macOS・Windows
//! での実機（日本語入力等）動作検証（Issue #191・#193）、Linux での実機検証
//! （ibus-mozc / fcitx5-mozc 等、S14 検証記録 §6 のチェックリスト対象）は、
//! いずれも本クレートには含まない。
//!
//! iOS 向けの本レイヤ（TASK-8.5-S16・Issue #197）は上記の `Ime::*` 経路とは異なる
//! **確定文字列のみの commit 型経路**を提供する（
//! ソースレベル検証済み）。
//! winit iOS バックエンドは `UITextInput`（marked text API）を実装せず `UIKeyInput`
//! （`insertText:`/`deleteBackward`）のみを実装するため、`Ime::*` イベントは iOS では
//! 一切発火しない。[`text_field_input_from_key`] は `insertText:`/`deleteBackward` 由来の
//! `KeyboardInput`（`ElementState`・`Key`・`text`）を、既存の編集入力契約
//! `fandhe_core::ui::TextFieldInput`（`InsertText`/`DeleteBackward`）へ変換する
//! （[`ime_event_from_winit`] とは排他に使うこと。iOS では前者のみ、デスクトップ
//! （macOS/Windows/Linux）では後者のみが実際にイベントを送出するため通常は競合しないが、
//! 呼び出し側が誤って両方へ同一イベントを流すと二重挿入になりうる）。
//! iOS 実機（日本語かな入力・候補バー確定等）での動作検証（Issue #198）は
//! 人間担当であり、本クレートには含まない。
//!
//! Android 向けの本レイヤ（TASK-8.5-S19・Issue #200）は iOS（S16）と同型の
//! **確定文字列のみの commit 型経路**を提供する（
//! ソースレベル検証済み）。
//! winit 0.30.13 の Android バックエンド（NativeActivity 方式）は
//! `InputEvent::KeyEvent` のみを `WindowEvent::KeyboardInput` へ変換し、
//! `InputEvent::TextEvent`（android-activity の GameActivity バックエンド限定で発火する
//! composing text）は `warn!` ログのみで無視するため、`WindowEvent::Ime(Ime::*)` は
//! Android では一切発火しない。したがって [`text_field_input_from_key`] を iOS と同一の
//! 変換規則のままそのまま再利用でき、`ImeRequest::SetEnabled` は
//! `AndroidApp::show_soft_input`/`hide_soft_input` に委譲されて**ソフトウェアキーボードの
//! 表示/非表示そのもの**を意味する（iOS の `becomeFirstResponder`/`resignFirstResponder`
//! と同型の意味論）。`SetCursorArea`/`SetPurpose` は winit Android バックエンドでは
//! no-op（`mod.rs` L916・L926）。インライン preedit（`setComposingText` 相当）は
//! NativeActivity 方式では実現不可であり、本クレートの対象外（§9 参照）。
//!
//! # セキュリティ考慮（§7、`.claude/rules/security.md` 準拠）
//!
//! - `Ime::Preedit` / `Ime::Commit` が運ぶ文字列は外部入力として扱い、本クレートでは
//!   一切加工しない（制御文字除去等の意味論は `TextFieldInput::InsertText` 契約側
//!   ―S8 の配線時― に委ねる）。
//! - byte offset → char offset の変換は `fandhe-core` の `char_offset_from_byte`
//!   （panic-free・境界クランプ）をそのまま利用し、本クレートで独自の境界判定を行わない
//!   （不正 UTF-8 境界・範囲外オフセットでの panic を排除、§7「サロゲートペア・不正
//!   UTF 境界」）。
//! - ログ出力・`println!` を一切行わない（`ImeEvent` の Debug マスクは `fandhe-core` 側の
//!   契約であり、本クレートはそれをさらに漏らさない）。
//!
//! # `ImePurpose::Password` の学習・予測変換抑止（Issue #586）
//!
//! [`apply_ime_request`] の `SetPurpose` doc のとおり、winit の `set_ime_purpose` は
//! Wayland 以外で no-op である。本クレートは [`sensitive_input`]（macOS の
//! SecureEventInput 経由）・[`android_editor_info`]（Android の `EditorInfo` 経由）で
//! 一部プラットフォームの代替担保を提供する。詳細は各モジュール doc・
//! `docs/architecture/ime-known-constraints.md` §3.3 を参照。
//!
//! # unsafe 境界
//!
//! クレートルートは `#![deny(unsafe_code)]` を採用する（`forbid` ではなく `deny` にする
//! 理由: [`macos_secure_input`] モジュールのみ Carbon FFI のため `unsafe` が不可避であり、
//! `forbid` はクレート内のどこからも `#[allow(unsafe_code)]` で局所的に上書きできないため。
//! `fandhe-swiftui-bridge` と同じ方式、`scripts/audit/unsafe-audit.sh` の
//! `FORBID_EXEMPT_CRATES` に登録済み）。`macos_secure_input` 以外の全モジュールは
//! 引き続き `unsafe` を一切含まない。
#![deny(unsafe_code)]

pub mod android_editor_info;
#[cfg(target_os = "macos")]
pub mod macos_secure_input;
pub mod sensitive_input;

pub mod session;
pub use session::ImeSessionTracker;

use fandhe_core::ui::{ImeEvent, ImePurpose, ImeRequest, TextFieldInput};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime};
use winit::keyboard::{Key, NamedKey};
use winit::window::{ImePurpose as WinitImePurpose, Window};

/// winit の `WindowEvent::Ime(Ime::*)` を、プラットフォーム非依存の [`ImeEvent`] へ変換する
/// （`ime-architecture.md` §4.4 対応表）。
///
/// `Ime::Preedit` が運ぶカーソル位置は winit の契約上 byte offset（UTF-8）であり
/// （[`winit::event::Ime`](https://docs.rs/winit/latest/winit/event/enum.Ime.html) 参照）、
/// `fandhe-core` 契約の char offset（§4.6）へ `char_offset_from_byte` で変換する。
/// この変換は非境界・範囲外オフセットに対しても panic せずクランプする
/// （`fandhe-core` 側の既存契約をそのまま継承）。
///
/// winit の既知の挙動（呼び出し元が前提としてよい契約）:
/// - `Commit` の直前には必ず空文字列の `Preedit` が届く。
/// - `Disabled` を受け取ったらコンポジションは終了しており、次の `Enabled` まで
///   `Preedit` / `Commit` は発生しない。
///
/// macOS 固有の挙動（TASK-8.5-S9 ソース検証）:
/// - `Enabled` は `ImeRequest::SetEnabled(true)` への即時応答ではなく、最初の
///   `setMarkedText`（コンポジション開始）時に遅延発火する。フォーカス獲得直後に
///   即座に届くことを前提にしない。
/// - 入力ソース切替（かな→ABC 等）でフォーカスが継続していても `Disabled` が発火し、
///   次のコンポジション開始まで `Enabled` は来ない（`Disabled`→`Enabled` の再サイクルを
///   呼び出し元の状態モデルが許容する必要がある）。
/// - IME allowed 中でも非コンポジション入力（ASCII 直接入力・デッドキー等）は
///   `Ime::Commit` ではなく通常の `KeyboardInput` 経路で届く。二重挿入は winit 側で
///   防止されるが、呼び出し元は両経路の統合を前提にする必要がある（TASK-8.5-S8 配線）。
///
/// Windows 固有の挙動（TASK-8.5-S11 ソース検証）:
/// - `Enabled` は macOS と同様に `ImeRequest::SetEnabled(true)` への即時応答ではなく、
///   `WM_IME_STARTCOMPOSITION`（コンポジション開始）時に遅延発火する。
/// - **`Disabled` はコンポジションが確定するたびに `WM_IME_ENDCOMPOSITION` で発火する。**
///   macOS（入力ソース切替時のみ）より高頻度であり、フォーカス継続中でも変換確定の
///   たびに `Disabled`→（次回変換開始で）`Enabled` の再サイクルが起こりうる。呼び出し元の
///   コンポジション状態モデルはこの高頻度な再サイクルを許容する必要がある
///   （S3・#184 / S8・#189 への申し送り）。
/// - 韓国語（Hangul）IME が `WM_IME_ENDCOMPOSITION` 後に `WM_IME_COMPOSITION` を送出する
///   既知の quirk は winit 側（`ime_state` 判定）で吸収済みであり、本関数が受け取る
///   `Ime::*` イベント列には影響しない（S25・#206 は実機での実発火順序確認）。
///
/// Linux 固有の挙動（TASK-8.5-S14 ソース検証）:
/// - `Enabled`/`Disabled` は macOS・Windows（いずれもコンポジション開始/終了まで
///   遅延発火）と異なり、X11・Wayland 共通で `ImeRequest::SetEnabled` 要求・フォーカス
///   変化に**同期して即時発火**する（第三のパターン）。IM コンテキスト再生成（X11の
///   `set_ime_allowed`）・Wayland の `Enter`/`Leave` いずれも該当する。
/// - X11 の `Preedit` カーソルは常に `start == end` の単一点であり、Windows・Wayland の
///   ような選択範囲（`start != end`）を表現しない。
/// - X11 は `preedit_done_callback` 由来の空 `Preedit` とキー入力（`lookup_utf8`）由来の
///   空 `Preedit` が連続送出されうる（本関数は 1 イベントずつ独立処理するため問題ない）。
/// - Wayland は `zwp_text_input_v3` の `done` イベントで「空 Preedit → Commit → 次の
///   Preedit」が 1 バッチとして連続送出されうる。
/// - ibus/fcitx デーモンの再起動（IM サーバー切断）時、X11 は `xim_destroy_callback` →
///   フォールバック IM への `replace_im` で全コンテキストが再生成され、変換中の
///   composition は `Commit` を経由せず破棄される（再接続後に新規 `Enabled` で再開）。
///
/// 韓国語・中国語固有の挙動（TASK-8.5-S25 検証）:
/// - 韓国語 2 ボル式は 1 打鍵ごとに音節が完成しうるため、上記 Windows の
///   `Disabled`→`Enabled` 再サイクルが日本語（文節単位）よりさらに高頻度になりうるが、
///   本関数・呼び出し元の状態モデルに追加の分岐は不要（既存契約のまま吸収できる）。
/// - 中国語拼音は `Preedit` が ASCII 文字列（"nihao"）である一方 `Commit` は複数文字の
///   CJK 文字列（"你好"）になり、文字種・長さが乖離するが、`char_offset_from_byte` は
///   文字種に依存しないため変換契約に影響しない。
///
/// これらの状態遷移そのものの処理（コンポジション状態モデルの更新）はコア側
/// （TASK-8.5-S3・S8）の責務であり、本関数は変換のみを行うパススルーである。
pub fn ime_event_from_winit(ime: Ime) -> ImeEvent {
    match ime {
        Ime::Enabled => ImeEvent::Enabled,
        Ime::Preedit(text, cursor) => {
            let cursor = cursor.map(|(start, end)| {
                let start = fandhe_core::ui::char_offset_from_byte(&text, start);
                let end = fandhe_core::ui::char_offset_from_byte(&text, end);
                (start, end)
            });
            ImeEvent::Preedit { text, cursor }
        }
        Ime::Commit(text) => ImeEvent::Commit(text),
        Ime::Disabled => ImeEvent::Disabled,
    }
}

/// winit の `WindowEvent::KeyboardInput`（`ElementState`・論理キー・確定済みテキスト）を、
/// `fandhe-core` の編集入力契約 [`TextFieldInput`] へ変換する（TASK-8.5-S16・Issue #197）。
///
/// 主対象は iOS（`UIKeyInput::insertText:`/`deleteBackward` 経由で 1 文字ずつ送出される
/// `KeyboardInput`。§2 の 2・3）だが、コンポジションを伴わない `KeyboardInput` 経路
/// （デスクトップの ASCII 直接入力等、S9 申し送り）にも同様に適用できる。
///
/// Android（TASK-8.5-S19・Issue #200）も
/// 主対象に含む。winit 0.30.13 Android バックエンドは `InputEvent::KeyEvent` を
/// `character_map_and_combine_key`（JNI `KeyCharacterMap` 照会・dead-key 結合）経由で
/// `WindowEvent::KeyboardInput` へ変換し、`Ime::*` は一切発火しないため、iOS と同一の
/// 変換規則がそのまま適用できる（`KEYCODE_DEL` → `NamedKey::Backspace`、dead-key は
/// winit 側で結合済みの文字列が `text` に届く）。
///
/// 変換規則:
/// - `ElementState::Pressed` かつ `text` が `Some` → `TextFieldInput::InsertText`
///   （文字列は加工しない。制御文字除去は `TextFieldInput::InsertText` 契約側
///   ―`fandhe-core` 側― に委ねる、OWASP A03 対策の継承）。
/// - `ElementState::Pressed` かつ `logical_key` が `Key::Named(NamedKey::Backspace)`
///   → `TextFieldInput::DeleteBackward`（`text` の有無によらず優先）。
/// - `ElementState::Released`、修飾キー単体、上記以外の `Key::Named` 等 → `None`
///   （iOS の `insertText:`/`deleteBackward` は Pressed/Released の対を送出するが、
///   Released 側を変換すると二重挿入になるため無視する。§2 の 2・3 参照）。
///
/// [`ime_event_from_winit`] とは排他に使うこと（呼び出し側契約）。iOS では
/// `Ime::*` が一切発火しないため通常は競合しないが、同一 `KeyboardInput` を両関数へ
/// 二重に流さない。panic-free（`unwrap`/`expect` 不使用）。
pub fn text_field_input_from_key(
    state: ElementState,
    logical_key: &Key,
    text: Option<&str>,
) -> Option<TextFieldInput> {
    if state != ElementState::Pressed {
        return None;
    }
    if matches!(logical_key, Key::Named(NamedKey::Backspace)) {
        return Some(TextFieldInput::DeleteBackward);
    }
    text.map(|text| TextFieldInput::InsertText(text.to_string()))
}

/// [`apply_ime_request`] が要求する winit ウィンドウ操作の最小集合を切り出したトレイト。
///
/// `winit::window::Window` は実ウィンドウハンドルを要求しヘッドレステスト環境で
/// 構築できないため、本トレイトでモックできるようにする（`fandhe-core` の
/// `coding-rust.md` テスト規約「ユニットテストは同一ファイルの `#[cfg(test)] mod tests`」
/// に従い、本クレート内で Mock 実装によるユニットテストを可能にする）。
pub trait ImeWindow {
    /// [`winit::window::Window::set_ime_allowed`] への委譲窓口。
    fn set_ime_allowed(&self, allowed: bool);
    /// [`winit::window::Window::set_ime_cursor_area`] への委譲窓口
    /// （物理ピクセル座標、論理→物理変換は呼び出し側の [`apply_ime_request`] が担う）。
    fn set_ime_cursor_area(&self, position: PhysicalPosition<f64>, size: PhysicalSize<u32>);
    /// [`winit::window::Window::set_ime_purpose`] への委譲窓口。
    fn set_ime_purpose(&self, purpose: WinitImePurpose);
    /// 論理ピクセル → 物理ピクセル変換に使うウィンドウのスケールファクタ
    /// （[`winit::window::Window::scale_factor`] への委譲窓口）。
    fn scale_factor(&self) -> f64;
}

impl ImeWindow for Window {
    fn set_ime_allowed(&self, allowed: bool) {
        Window::set_ime_allowed(self, allowed);
    }

    fn set_ime_cursor_area(&self, position: PhysicalPosition<f64>, size: PhysicalSize<u32>) {
        Window::set_ime_cursor_area(self, position, size);
    }

    fn set_ime_purpose(&self, purpose: WinitImePurpose) {
        Window::set_ime_purpose(self, purpose);
    }

    fn scale_factor(&self) -> f64 {
        Window::scale_factor(self)
    }
}

/// コア（`fandhe-core`）からの [`ImeRequest`] を、winit ウィンドウ操作へ変換して適用する
/// （`ime-architecture.md` §4.2・§4.4 対応表）。
///
/// `SetCursorArea` はコア契約上の論理ピクセル座標（§4.2「コアはウィンドウのスケール
/// ファクタを知らない」契約）を、`window.scale_factor()` を用いて winit の
/// `PhysicalPosition<f64>` / `PhysicalSize<u32>` へ変換する（論理→物理変換はプラット
/// フォーム層の責務、§4.2）。負値・非有限値（`NaN`/`Infinity`）は 0 へクランプし、
/// サイズは切り上げる（`PhysicalSize<u32>` が符号なし整数のため）。panic-free。
///
/// `ImeRequest::SetPurpose` は winit 0.30 系では Wayland 以外（macOS・Windows・**X11**
/// 含む）で no-op であることが TASK-8.5-S9（macOS）・TASK-8.5-S11（Windows）・
/// TASK-8.5-S13（Linux X11）のソース検証で判明している（`Window::set_ime_purpose` の
/// 公式 doc 「Unsupported: iOS / Android / Web / Windows / X11 / macOS / Orbital」。
/// X11 側実装も `window.rs` L1754 `pub fn set_ime_purpose(&self, _purpose: ImePurpose) {}`）。
/// **Wayland のみ唯一の対応プラットフォームであり**、`ContentHint`/`ContentPurpose`
/// へマップされコンポジタへ実際に伝わる（ただし実際に IME が学習抑止へ
/// 反映するかは保証ではなくヒントに留まる）。本関数自体は契約どおり `SetPurpose` を
/// 委譲するのみで、no-op プラットフォームでの代替担保は行わない（Issue #586 で
/// macOS・Android 向けに追加した [`sensitive_input`] / [`android_editor_info`] は、
/// 本関数と並列に呼ぶ独立した経路であり、本関数の実装・契約には影響しない）。
///
/// `ImePurpose::Password` 時も `SetEnabled(false)` を追加発行しない（Issue #204、
/// PR #457 のレビュー指摘）。下記 iOS/Android 固有挙動のとおり、この 2 プラットフォーム
/// では `SetEnabled` が IME の学習・予測変換ではなく**ソフトウェアキーボードの
/// 表示/非表示そのもの**に委譲されるため、`SetEnabled(false)` を送ると
/// パスワードフィールドにフォーカスした瞬間にキーボードごと隠れて入力できなくなる。
/// `fandhe-core` 側の `TextFieldWidget::set_ime_purpose` / `apply_input` は
/// `Password` でも `SetPurpose` のみを発行し `SetEnabled` は他の purpose と同様に扱う
/// （`crates/fandhe-core/src/ui/text_field.rs` モジュール doc「Password 時の
/// ポリシー」参照）。no-op プラットフォームでの学習・予測変換抑止の代替担保は
/// 未解決のまま `fandhe-core` 側からプラットフォーム層へ委譲されており、
/// 複数フィールド間の調停（Issue #205）とあわせて申し送り事項とする
/// （設計文書 `ime-architecture.md` §9 参照）。
///
/// iOS 固有の挙動（TASK-8.5-S16 ソース検証）:
/// - `SetEnabled` は `view.becomeFirstResponder()`/`resignFirstResponder()` に委譲され、
///   **ソフトウェアキーボードの表示/非表示そのもの**を意味する（デスクトップの IME
///   有効/無効切替とは異なる意味論だが、既存の呼び出し契約はそのまま流用できる）。
/// - `SetCursorArea`/`SetPurpose` はいずれも no-op（`warn!` ログのみ、`window.rs`
///   L366-368・L386-388）。iOS の変換候補はキーボード上部の候補バーに表示されるため
///   カーソル矩形通知は不要（macOS・Windows・Linux X11 と同様の既知差異）。
///
/// Android 固有の挙動（TASK-8.5-S19 ソース検証）:
/// - `SetEnabled` は `AndroidApp::show_soft_input(true)`/`hide_soft_input(true)`
///   （`mod.rs` L918-924）に委譲され、iOS と同様に**ソフトウェアキーボードの
///   表示/非表示そのもの**を意味する。
/// - `SetCursorArea`（`mod.rs` L916）・`SetPurpose`（L926）はいずれも空実装（no-op）。
///   Android の変換候補もソフトウェアキーボード側 UI（候補バー）に表示されるため
///   カーソル矩形通知は不要（iOS と同型の既知差異）。
pub fn apply_ime_request(window: &impl ImeWindow, request: &ImeRequest) {
    match request {
        ImeRequest::SetEnabled(allowed) => window.set_ime_allowed(*allowed),
        ImeRequest::SetCursorArea { origin, size } => {
            let scale = window.scale_factor();
            let (x, y) = *origin;
            let (w, h) = *size;
            let position = PhysicalPosition::new(
                to_physical_component(x, scale),
                to_physical_component(y, scale),
            );
            let physical_size =
                PhysicalSize::new(to_physical_extent(w, scale), to_physical_extent(h, scale));
            window.set_ime_cursor_area(position, physical_size);
        }
        ImeRequest::SetPurpose(purpose) => window.set_ime_purpose(winit_purpose_from(*purpose)),
        ImeRequest::BeginSession(_) => {
            // winit にセッション開始を通知する native API は存在しない（Issue #587）。
            // `ImeSessionTracker::observe_request` が別途この要求を解釈し、以降の
            // `ImeEvent` の帰属セッションを算出する（`session` モジュール doc 参照）。
        }
    }
}

/// 論理ピクセル座標成分（位置）を物理ピクセルへ変換する。負値・非有限値は 0 へクランプする
/// （`PhysicalPosition<f64>` は符号付きだが、IME カーソル矩形の原点が負になる契約はコア側に
/// 存在しないため、不正値の伝播を防ぐ意図で本レイヤでもクランプする）。
fn to_physical_component(logical: f32, scale: f64) -> f64 {
    let value = f64::from(logical) * scale;
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

/// 論理ピクセルのサイズ成分を物理ピクセルへ変換する。`PhysicalSize<u32>` が符号なし整数の
/// ため、負値・非有限値は 0 へクランプし、端数は切り上げる（矩形がカーソルを内包しない
/// 縮小方向の誤差を避ける）。
fn to_physical_extent(logical: f32, scale: f64) -> u32 {
    let value = f64::from(logical) * scale;
    if value.is_finite() && value > 0.0 {
        value.ceil() as u32
    } else {
        0
    }
}

/// [`ImePurpose`]（コア契約）を winit `window::ImePurpose` へ 1:1 マッピングする。
fn winit_purpose_from(purpose: ImePurpose) -> WinitImePurpose {
    match purpose {
        ImePurpose::Normal => WinitImePurpose::Normal,
        ImePurpose::Password => WinitImePurpose::Password,
        ImePurpose::Terminal => WinitImePurpose::Terminal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// ヘッドレステスト用の `ImeWindow` モック。呼び出し引数を記録するのみで
    /// 実ウィンドウ・実 IME には触れない。
    #[derive(Default)]
    struct MockWindow {
        scale_factor: f64,
        set_ime_allowed_calls: RefCell<Vec<bool>>,
        set_ime_cursor_area_calls: RefCell<Vec<(PhysicalPosition<f64>, PhysicalSize<u32>)>>,
        set_ime_purpose_calls: RefCell<Vec<WinitImePurpose>>,
    }

    impl MockWindow {
        fn with_scale_factor(scale_factor: f64) -> Self {
            Self {
                scale_factor,
                ..Default::default()
            }
        }
    }

    impl ImeWindow for MockWindow {
        fn set_ime_allowed(&self, allowed: bool) {
            self.set_ime_allowed_calls.borrow_mut().push(allowed);
        }

        fn set_ime_cursor_area(&self, position: PhysicalPosition<f64>, size: PhysicalSize<u32>) {
            self.set_ime_cursor_area_calls
                .borrow_mut()
                .push((position, size));
        }

        fn set_ime_purpose(&self, purpose: WinitImePurpose) {
            self.set_ime_purpose_calls.borrow_mut().push(purpose);
        }

        fn scale_factor(&self) -> f64 {
            self.scale_factor
        }
    }

    #[test]
    fn ime_event_from_winit_maps_enabled_disabled_commit() {
        assert_eq!(ime_event_from_winit(Ime::Enabled), ImeEvent::Enabled);
        assert_eq!(ime_event_from_winit(Ime::Disabled), ImeEvent::Disabled);
        assert_eq!(
            ime_event_from_winit(Ime::Commit("abc".to_string())),
            ImeEvent::Commit("abc".to_string())
        );
    }

    #[test]
    fn ime_event_from_winit_maps_preedit_without_cursor() {
        let event = ime_event_from_winit(Ime::Preedit("abc".to_string(), None));
        assert_eq!(
            event,
            ImeEvent::Preedit {
                text: "abc".to_string(),
                cursor: None,
            }
        );
    }

    #[test]
    fn ime_event_from_winit_converts_multibyte_cursor_byte_to_char_offset() {
        // "こんにちは" は 1 文字 3 byte（UTF-8）。byte cursor (3, 6) は
        // char offset (1, 2) に対応する（設計文書 §4.6 の変換例）。
        let event = ime_event_from_winit(Ime::Preedit("こんにちは".to_string(), Some((3, 6))));
        assert_eq!(
            event,
            ImeEvent::Preedit {
                text: "こんにちは".to_string(),
                cursor: Some((1, 2)),
            }
        );
    }

    #[test]
    fn ime_event_from_winit_clamps_non_boundary_cursor_without_panicking() {
        // "こんにちは" の byte offset 1・2 は先頭文字（3 byte）の中間点であり
        // char 境界ではないが、`char_offset_from_byte` の契約により panic せず
        // 含む char の先頭側（0）へクランプされる。
        let event = ime_event_from_winit(Ime::Preedit("こんにちは".to_string(), Some((1, 2))));
        assert_eq!(
            event,
            ImeEvent::Preedit {
                text: "こんにちは".to_string(),
                cursor: Some((0, 0)),
            }
        );
    }

    #[test]
    fn ime_event_from_winit_maps_empty_preedit() {
        let event = ime_event_from_winit(Ime::Preedit(String::new(), None));
        assert_eq!(
            event,
            ImeEvent::Preedit {
                text: String::new(),
                cursor: None,
            }
        );
    }

    #[test]
    fn apply_ime_request_set_enabled_delegates_to_window() {
        let window = MockWindow::with_scale_factor(1.0);
        apply_ime_request(&window, &ImeRequest::SetEnabled(true));
        assert_eq!(window.set_ime_allowed_calls.borrow().as_slice(), &[true]);
    }

    #[test]
    fn apply_ime_request_set_cursor_area_converts_logical_to_physical_at_scale_2() {
        let window = MockWindow::with_scale_factor(2.0);
        apply_ime_request(
            &window,
            &ImeRequest::SetCursorArea {
                origin: (10.0, 20.0),
                size: (30.0, 40.0),
            },
        );
        let calls = window.set_ime_cursor_area_calls.borrow();
        assert_eq!(calls.len(), 1);
        let (position, size) = calls[0];
        assert_eq!(position, PhysicalPosition::new(20.0, 40.0));
        assert_eq!(size, PhysicalSize::new(60, 80));
    }

    #[test]
    fn apply_ime_request_set_cursor_area_ceils_fractional_size() {
        let window = MockWindow::with_scale_factor(1.5);
        apply_ime_request(
            &window,
            &ImeRequest::SetCursorArea {
                origin: (0.0, 0.0),
                size: (1.0, 1.0),
            },
        );
        let calls = window.set_ime_cursor_area_calls.borrow();
        // 1.0 * 1.5 = 1.5 -> 切り上げで 2。
        assert_eq!(calls[0].1, PhysicalSize::new(2, 2));
    }

    #[test]
    fn apply_ime_request_set_cursor_area_clamps_negative_values_to_zero() {
        let window = MockWindow::with_scale_factor(1.0);
        apply_ime_request(
            &window,
            &ImeRequest::SetCursorArea {
                origin: (-5.0, -5.0),
                size: (-10.0, -10.0),
            },
        );
        let calls = window.set_ime_cursor_area_calls.borrow();
        assert_eq!(calls[0].0, PhysicalPosition::new(0.0, 0.0));
        assert_eq!(calls[0].1, PhysicalSize::new(0, 0));
    }

    #[test]
    fn text_field_input_from_key_maps_pressed_character_to_insert_text() {
        let key = Key::Character("あ".into());
        assert_eq!(
            text_field_input_from_key(ElementState::Pressed, &key, Some("あ")),
            Some(TextFieldInput::InsertText("あ".to_string()))
        );
    }

    #[test]
    fn text_field_input_from_key_ignores_released_character() {
        // iOS の `insertText:` は Pressed（text: Some）→ Released（text: None）の対を
        // 送出する（S16 §2 #2）。Released 側を変換すると二重挿入になるため無視する。
        let key = Key::Character("あ".into());
        assert_eq!(
            text_field_input_from_key(ElementState::Released, &key, None),
            None
        );
    }

    #[test]
    fn text_field_input_from_key_maps_pressed_backspace_to_delete_backward() {
        let key = Key::Named(NamedKey::Backspace);
        assert_eq!(
            text_field_input_from_key(ElementState::Pressed, &key, None),
            Some(TextFieldInput::DeleteBackward)
        );
    }

    #[test]
    fn text_field_input_from_key_ignores_released_backspace() {
        let key = Key::Named(NamedKey::Backspace);
        assert_eq!(
            text_field_input_from_key(ElementState::Released, &key, None),
            None
        );
    }

    #[test]
    fn text_field_input_from_key_ignores_other_named_keys_without_text() {
        let key = Key::Named(NamedKey::Enter);
        assert_eq!(
            text_field_input_from_key(ElementState::Pressed, &key, None),
            None
        );
    }

    #[test]
    fn text_field_input_from_key_passes_through_multi_char_string() {
        // 複数文字（合字・複数コードポイントの絵文字等）が 1 回で渡されても
        // 加工せずそのまま InsertText へ渡す（サロゲートペア等の破壊防止）。
        let key = Key::Character("🎉".into());
        assert_eq!(
            text_field_input_from_key(ElementState::Pressed, &key, Some("🎉")),
            Some(TextFieldInput::InsertText("🎉".to_string()))
        );
    }

    // --- Android（TASK-8.5-S19・Issue #200）変換規則のユニットテスト ------------
    // `text_field_input_from_key`/`apply_ime_request` は iOS（S16）と同一の関数を
    // そのまま再利用する契約（`Ime::*` が発火せず KeyEvent 経路のみが届く点が共通）。
    // ここでは Android 固有の入力パターン（KEYCODE_DEL・日本語文字・二重挿入防止）を
    // 明示的に確認する。

    #[test]
    fn text_field_input_from_key_maps_android_keycode_del_to_delete_backward() {
        // Android の KEYCODE_DEL は winit 側で `NamedKey::Backspace` へ正規化される
        // （`character_map_and_combine_key`、s19 検証記録 §2 対応表）。
        let key = Key::Named(NamedKey::Backspace);
        assert_eq!(
            text_field_input_from_key(ElementState::Pressed, &key, None),
            Some(TextFieldInput::DeleteBackward)
        );
    }

    #[test]
    fn text_field_input_from_key_maps_android_key_event_japanese_character() {
        // Android ソフトウェアキーボードが KeyEvent 経路（dead-key 結合済み）で
        // 届けた日本語文字も、iOS と同じ規則で InsertText へ変換される。
        let key = Key::Character("あ".into());
        assert_eq!(
            text_field_input_from_key(ElementState::Pressed, &key, Some("あ")),
            Some(TextFieldInput::InsertText("あ".to_string()))
        );
    }

    #[test]
    fn text_field_input_from_key_android_released_key_event_does_not_double_insert() {
        // Android の `InputEvent::KeyEvent`（`KeyAction::Up`）は winit で
        // `ElementState::Released`（`text: None`）として届く。Pressed 側で既に
        // 挿入済みのため、Released 側は None のまま二重挿入・二重削除にならない
        // （s19 検証記録 §4 対応表）。
        let key = Key::Character("a".into());
        assert_eq!(
            text_field_input_from_key(ElementState::Released, &key, None),
            None
        );
        let backspace = Key::Named(NamedKey::Backspace);
        assert_eq!(
            text_field_input_from_key(ElementState::Released, &backspace, None),
            None
        );
    }

    #[test]
    fn apply_ime_request_set_enabled_delegates_as_android_soft_keyboard_visibility() {
        // `ImeRequest::SetEnabled` は Android では `show_soft_input`/`hide_soft_input`
        // （ソフトウェアキーボードの表示/非表示）に委譲される契約（iOS の
        // becomeFirstResponder/resignFirstResponder と同型の意味論）。
        let window = MockWindow::with_scale_factor(1.0);
        apply_ime_request(&window, &ImeRequest::SetEnabled(true));
        apply_ime_request(&window, &ImeRequest::SetEnabled(false));
        assert_eq!(
            window.set_ime_allowed_calls.borrow().as_slice(),
            &[true, false]
        );
    }

    #[test]
    fn apply_ime_request_set_purpose_maps_all_variants() {
        let window = MockWindow::with_scale_factor(1.0);
        apply_ime_request(&window, &ImeRequest::SetPurpose(ImePurpose::Normal));
        apply_ime_request(&window, &ImeRequest::SetPurpose(ImePurpose::Password));
        apply_ime_request(&window, &ImeRequest::SetPurpose(ImePurpose::Terminal));
        assert_eq!(
            window.set_ime_purpose_calls.borrow().as_slice(),
            &[
                WinitImePurpose::Normal,
                WinitImePurpose::Password,
                WinitImePurpose::Terminal,
            ]
        );
    }
}
