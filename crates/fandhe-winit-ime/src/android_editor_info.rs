//! Android: `ImePurpose::Password` を `AndroidApp::set_ime_editor_info`（`EditorInfo`）へ
//! 反映し、ソフトウェアキーボード（Gboard 等）の学習辞書・予測変換候補への残留を抑止する
//! （Issue #586）。winit の `Window::set_ime_purpose` は Android では no-op
//! （`window.rs` L926、`lib.rs` の `apply_ime_request` doc 参照）であり、winit を介さず
//! `android-activity`（winit が re-export する `winit::platform::android::activity::*`）の
//! API へ直接委譲する、`crate::lib` の変換経路とは独立した並列の経路。
//!
//! `docs/architecture/ime-android-input-connection.md` §4.6 で設計済みの方針の実装化。

use fandhe_core::ui::ImePurpose;

/// Android [`InputType`](https://developer.android.com/reference/android/text/InputType) の
/// ビット値をこのクレートの依存先を経由せず定義し直したもの（`android-activity` の
/// `input::InputType` bitflags と一致することをテストで検査する。プラットフォームに依存しない
/// 純粋写像 [`merge_editor_info_bits`] を非 Android ターゲットでもホストテストできるように
/// する狙い）。
mod bits {
    /// `TYPE_MASK_CLASS`（`inputType` の下位 4 bit。class は排他的な値であり独立フラグではない）。
    pub const TYPE_MASK_CLASS: u32 = 0x0000_000f;
    /// `TYPE_MASK_VARIATION`（`inputType` の bit 4-11。variation も class 同様に排他的な値
    /// であり、既存 variation を残したまま別 variation を OR すると未定義値になる。
    /// Password 合成時は本マスクで既存 variation を消してから置換する、Bugbot/codex-review
    /// 指摘対応）。
    pub const TYPE_MASK_VARIATION: u32 = 0x0000_0ff0;
    /// `TYPE_CLASS_TEXT`。
    pub const TYPE_CLASS_TEXT: u32 = 0x0000_0001;
    /// `TYPE_TEXT_VARIATION_PASSWORD`。
    pub const TYPE_TEXT_VARIATION_PASSWORD: u32 = 0x0000_0080;
    /// `TYPE_TEXT_FLAG_NO_SUGGESTIONS`。
    pub const TYPE_TEXT_FLAG_NO_SUGGESTIONS: u32 = 0x0008_0000;
    /// `IME_FLAG_NO_PERSONALIZED_LEARNING`（`ImeOptions` 側）。
    pub const IME_FLAG_NO_PERSONALIZED_LEARNING: u32 = 0x0100_0000;
}

/// [`ImePurpose`] を Android `EditorInfo` の `(inputType, imeOptions)` ビット値へ合成する
/// 純粋関数（ホスト上でもユニットテスト可能。`apply_ime_purpose_android` が
/// `android-activity` の bitflags 型へ変換して実際の呼び出しへ渡す）。
///
/// `existing_input_type` / `existing_ime_options` には呼び出し元が直前に
/// `set_ime_editor_info` へ渡した値（ホスト側で設定済みの capitalization・multiline 等）を
/// そのまま渡す契約とする。`android-activity` の `AndroidApp::set_ime_editor_info` は
/// 現在の `EditorInfo` を読み出す API を持たない一方向 set のみのため、本関数は全置換
/// ではなく `Password` 適用時のみ password/no-learning ビットを既存値へ追加する（Issue #586。
/// purpose のみから全ビットを再構築すると、purpose 切替のたびにホスト設定済みの
/// Enter アクション・capitalization・multi-line 等が失われる）。
///
/// - `Password`: `class`/`variation`（`TYPE_MASK_CLASS`/`TYPE_MASK_VARIATION`）を
///   `TYPE_CLASS_TEXT`/`TYPE_TEXT_VARIATION_PASSWORD` へ置換し、`TYPE_TEXT_FLAG_NO_SUGGESTIONS`
///   フラグと `IME_FLAG_NO_PERSONALIZED_LEARNING` を既存ビットへ追加する（学習辞書・予測変換
///   候補の双方を抑止する）。class/variation は Android の `InputType` 定義上、独立フラグでは
///   なく排他的な値（下位 4bit/次続 8bit の数値）のため、既存 email/URI 等の variation を
///   残したまま `TYPE_TEXT_VARIATION_PASSWORD` を OR すると未定義値になり Password として
///   認識されない（codex-review/Bugbot 指摘対応）。
/// - `Normal` / `Terminal`: 既存ビットを一切変更せずそのまま返す（受け取った値を Password
///   適用前の値として素通しするのみ）。password 関連ビット（`TYPE_TEXT_FLAG_NO_SUGGESTIONS`
///   / `IME_FLAG_NO_PERSONALIZED_LEARNING`）を無条件 AND-NOT で除去しないのは、本関数からは
///   これらのビットが「自分が Password 適用時に足したもの」なのか「ホストが元々別の目的で
///   設定していたもの」なのか区別できず、後者まで消してしまうため（Issue #586）。
///   呼び出し元は Password 適用前の
///   `existing_input_type` / `existing_ime_options`（ホストが本来望む値そのもの）を保持し、
///   `Normal` / `Terminal` 復帰時にその保持値をそのまま渡す契約とする（本関数側では一切
///   加工しない）。
pub fn merge_editor_info_bits(
    existing_input_type: u32,
    existing_ime_options: u32,
    purpose: ImePurpose,
) -> (u32, u32) {
    match purpose {
        ImePurpose::Normal | ImePurpose::Terminal => (existing_input_type, existing_ime_options),
        ImePurpose::Password => {
            let class_and_variation_mask = bits::TYPE_MASK_CLASS | bits::TYPE_MASK_VARIATION;
            let password_input_bits =
                bits::TYPE_TEXT_VARIATION_PASSWORD | bits::TYPE_TEXT_FLAG_NO_SUGGESTIONS;
            (
                (existing_input_type & !class_and_variation_mask)
                    | bits::TYPE_CLASS_TEXT
                    | password_input_bits,
                existing_ime_options | bits::IME_FLAG_NO_PERSONALIZED_LEARNING,
            )
        }
    }
}

#[cfg(target_os = "android")]
mod android_apply {
    use super::merge_editor_info_bits;
    use fandhe_core::ui::ImePurpose;
    use winit::platform::android::activity::input::{ImeOptions, InputType, TextInputAction};
    use winit::platform::android::activity::AndroidApp;

    // 本モジュール定義のビット値が android-activity 0.6.1 の bitflags 定義と一致することを
    // コンパイル時に検査する（定数の乖離は android-activity のバージョン更新時に検知できる）。
    const _: () = assert!(super::bits::TYPE_CLASS_TEXT == InputType::TYPE_CLASS_TEXT.bits());
    const _: () = assert!(
        super::bits::TYPE_TEXT_VARIATION_PASSWORD == InputType::TYPE_TEXT_VARIATION_PASSWORD.bits()
    );
    const _: () = assert!(
        super::bits::TYPE_TEXT_FLAG_NO_SUGGESTIONS
            == InputType::TYPE_TEXT_FLAG_NO_SUGGESTIONS.bits()
    );
    const _: () = assert!(
        super::bits::IME_FLAG_NO_PERSONALIZED_LEARNING
            == ImeOptions::IME_FLAG_NO_PERSONALIZED_LEARNING.bits()
    );

    /// [`ImePurpose`] を `AndroidApp::set_ime_editor_info` へ反映する（winit を介さない
    /// 直接呼び出し、`crate::sensitive_input` とは独立した並列経路）。
    ///
    /// `existing_input_type` / `existing_ime_options` は呼び出し元がホストの
    /// EditorInfo として維持したい既存設定（Enter アクション相当の capitalization・
    /// multiline 等）をそのまま渡す（`merge_editor_info_bits` の doc・codex-review P1
    /// 対応参照。`AndroidApp` は現在の EditorInfo を読み出す API を持たないため
    /// 呼び出し元が状態を保持する契約とする）。`existing_text_input_action` は
    /// purpose に関わらずそのまま素通しし、Enter キーの意味論を変更しない
    /// （`ImeRequest` はコアの契約上 Enter アクションを持たないため）。
    pub fn apply_ime_purpose_android(
        app: &AndroidApp,
        existing_input_type: InputType,
        existing_text_input_action: TextInputAction,
        existing_ime_options: ImeOptions,
        purpose: ImePurpose,
    ) {
        let (input_type, ime_options) = merge_editor_info_bits(
            existing_input_type.bits(),
            existing_ime_options.bits(),
            purpose,
        );
        app.set_ime_editor_info(
            InputType::from_bits_retain(input_type),
            existing_text_input_action,
            ImeOptions::from_bits_retain(ime_options),
        );
    }
}

#[cfg(target_os = "android")]
pub use android_apply::apply_ime_purpose_android;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_sets_variation_and_no_suggestions_and_no_learning() {
        let (input_type, ime_options) = merge_editor_info_bits(0, 0, ImePurpose::Password);
        assert_eq!(
            input_type,
            bits::TYPE_CLASS_TEXT
                | bits::TYPE_TEXT_VARIATION_PASSWORD
                | bits::TYPE_TEXT_FLAG_NO_SUGGESTIONS
        );
        assert_eq!(ime_options, bits::IME_FLAG_NO_PERSONALIZED_LEARNING);
    }

    #[test]
    fn normal_and_terminal_use_plain_text_class_with_no_options() {
        assert_eq!(merge_editor_info_bits(0, 0, ImePurpose::Normal), (0, 0));
        assert_eq!(merge_editor_info_bits(0, 0, ImePurpose::Terminal), (0, 0));
    }

    /// ホスト側が設定済みの capitalization/multiline 相当のビット（password 関連以外）は
    /// purpose 切替をまたいで保持されなければならないことの検証（Issue #586）。
    #[test]
    fn password_preserves_unrelated_existing_input_type_bits() {
        const HOST_MULTILINE_LIKE_BIT: u32 = 0x0002_0000; // TYPE_TEXT_FLAG_MULTI_LINE 相当
        let (input_type, _) =
            merge_editor_info_bits(HOST_MULTILINE_LIKE_BIT, 0, ImePurpose::Password);
        assert_eq!(
            input_type,
            HOST_MULTILINE_LIKE_BIT
                | bits::TYPE_CLASS_TEXT
                | bits::TYPE_TEXT_VARIATION_PASSWORD
                | bits::TYPE_TEXT_FLAG_NO_SUGGESTIONS
        );
    }

    /// 既存 variation（例: EMAIL_ADDRESS）が設定済みの状態から Password へ切り替えると、
    /// 既存 variation を OR で汚さず PASSWORD variation へ排他的に置換されなければならない
    /// ことの検証（OR すると `TYPE_TEXT_VARIATION_EMAIL_ADDRESS | TYPE_TEXT_VARIATION_PASSWORD`
    /// の未定義値になり Password として認識されない）。
    #[test]
    fn password_replaces_existing_variation_exclusively() {
        const TYPE_TEXT_VARIATION_EMAIL_ADDRESS: u32 = 0x0000_0020;
        let existing_input_type = bits::TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_EMAIL_ADDRESS;
        let (input_type, _) = merge_editor_info_bits(existing_input_type, 0, ImePurpose::Password);
        assert_eq!(
            input_type,
            bits::TYPE_CLASS_TEXT
                | bits::TYPE_TEXT_VARIATION_PASSWORD
                | bits::TYPE_TEXT_FLAG_NO_SUGGESTIONS,
            "既存 variation が排他的に置換されず OR で残存している"
        );
        assert_eq!(
            input_type & TYPE_TEXT_VARIATION_EMAIL_ADDRESS,
            0,
            "既存の EMAIL_ADDRESS variation ビットが除去されていない"
        );
    }

    /// Password → Normal/Terminal 復帰時、本関数はビットを一切加工せず素通しすることの検証
    /// （Issue #586）。password 関連ビット（`TYPE_TEXT_FLAG_NO_SUGGESTIONS` /
    /// `IME_FLAG_NO_PERSONALIZED_LEARNING`）を AND-NOT で除去しないのは、それが
    /// 「自分が Password 適用時に足したもの」なのか「ホストが元々別目的で設定していたもの」
    /// なのか本関数からは判別できず、後者まで誤って消してしまうため。呼び出し元が
    /// Password 適用前の値を保持しそのまま渡す契約になっているため、本関数は
    /// 渡された値をそのまま返すだけでよい。
    #[test]
    fn normal_and_terminal_pass_existing_bits_through_unchanged() {
        const HOST_MULTILINE_LIKE_BIT: u32 = 0x0002_0000;
        const HOST_NO_SUGGESTIONS_BIT: u32 = bits::TYPE_TEXT_FLAG_NO_SUGGESTIONS;
        const HOST_NO_LEARNING_BIT: u32 = bits::IME_FLAG_NO_PERSONALIZED_LEARNING;
        // ホストが Password とは無関係に、通常フィールドで意図的に予測変換抑止・学習抑止
        // ビットを立てているケースを模す。
        let existing_input_type =
            HOST_MULTILINE_LIKE_BIT | bits::TYPE_CLASS_TEXT | HOST_NO_SUGGESTIONS_BIT;
        let existing_ime_options = HOST_NO_LEARNING_BIT;
        let (input_type, ime_options) = merge_editor_info_bits(
            existing_input_type,
            existing_ime_options,
            ImePurpose::Normal,
        );
        assert_eq!(input_type, existing_input_type);
        assert_eq!(ime_options, existing_ime_options);
    }
}
