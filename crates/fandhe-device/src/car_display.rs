//! 車載ディスプレイ（Android Auto）に出す 1 行テキストの抽象（Phase 11 の拡充ループ、親 #152、
//! 対応表 `docs/device/capabilities/car-display.json`）。
//!
//! 扱う範囲は「アプリが渡した 1 行のテキストを、車載画面（Car App Library の 1 画面）に出す」
//! ことと、「その車載画面が今出ているか」を読むことだけ。対象は Android（Android Auto / DHU）。
//! iOS（CarPlay）は Apple の entitlement の承認待ちのため dispatch に入れず、他 OS は
//! [`UnsupportedCarDisplay`]（`Unsupported`）。Android Automotive OS・複数画面・操作（タップ）・
//! 地図やメディアなど車載向けの別カテゴリは扱わない。
//!
//! 実装は後続の機能 PR（`fandhe-egui-proxy` の `device/android_car_display.rs`）で入れる。
//! 文言はプロセス内の表示元（Java の static）へ置き、車載画面（`CarAppService` が作る `Screen`）が
//! そこを読んで描画する。共通コアの画面は文言を添字の候補としてだけ扱い、ログ・`Debug` に出さない。
//!
//! # 契約
//!
//! - **`set_text`**: [`validate_text`] を通した文言を表示元へ置く。車載画面が出ていれば再描画
//!   （`invalidate`）を main へ post して `Ok(())`。`Ok` が約束するのは「受け付けた」ことまでで、
//!   車載画面への反映ではない。出ていない・未接続のときも `Ok`（次に開かれたとき反映される）。
//!   再描画要求の失敗は post 先で握る。JNI の呼び出し失敗は `Err(Internal)` で、成功を装わない
//!   （`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **入力検証**: 空でない 1 行・[`MAX_TEXT_CHARS`] 文字以内。空・制御文字（`\n` `\r` NEL を含む）・
//!   U+2028 / U+2029・長すぎる入力は `Err(InvalidInput)`。切り詰めない、trim しない（前後の空白は
//!   入力どおり）。エラー文言に入力値を含めない。定数は Rust だけを正とし Java へ複製しない
//! - **`state`**: 車載画面（Screen）が STARTED 以上なら `Showing`、そうでなければ `NotShowing`。
//!   `NotShowing` は失敗ではなく通常の状態で、失敗（`Err`）を `NotShowing` へ畳まない。JNI の失敗・
//!   Activity の不在は `Err(Internal)`
//! - **未設定**: `set_text` が一度も成功していない間、車載画面は固定の文言「未設定」を出す（OS 側）
//! - **`Unsupported`**: Android 以外だけ。Activity の一時的な不在は `Internal`（`Unsupported` にすると
//!   ドライバがポーリングをやめ、復帰しても戻らないため）。初期化不能もダミーの成功へ倒さない
//!
//! # キャンセル・権限・三値（契約表 10）
//!
//! 本機能にキャンセルは無い（要求は即時に受け付けるだけ）。権限ダイアログも無い
//! （`uses-permission` 不要）ため、拒否も存在しない。成功 / `NotShowing`（正常）/ 失敗（`Err`）の
//! 3 つを混ぜない。
//!
//! # 守る範囲と既知の制約
//!
//! 守るのは、1 プロセス内で後から渡した文言が出ること、失敗を成功と見せないこと、文言をログ・
//! `Debug`・エラー文言に出さないこと、古い Screen の `onStop` が新しい Screen の表示中を消さないこと。
//! 守らないのは、プロセス終了後の文言（未設定に戻る）、反映の遅れ（ポーリング 1 秒・ホストの間引き）、
//! DHU と実車の差、Android Auto の接続そのもの、Android Automotive OS、iOS（承認待ち）。
//!
//! 並行性（契約表 11）: `set` は画面側で高々 1 件、`state` は Poller が高々 1 件で世代により古い
//! 完了を捨てる。どちらも OS の応答を待たずに UI を止めない。

use crate::DeviceError;

/// 渡せるテキストの最大文字数（`char` 数）。車載画面の 1 行表示の目安。Rust 側だけが正。
pub const MAX_TEXT_CHARS: usize = 40;

/// 車載画面が今出ているか。`NotShowing` は失敗ではない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarDisplayState {
    /// 車載画面は出ていない（未接続・別の画面・停止中）。
    NotShowing,
    /// 車載画面が出ている。
    Showing,
}

/// 車載ディスプレイの契約（モジュール doc の「契約」節）。
pub trait CarDisplay: Send + Sync {
    /// 文言を表示元へ置き、出ていれば再描画を要求する。入力は [`validate_text`] を通す。
    fn set_text(&self, text: &str) -> Result<(), DeviceError>;
    /// 車載画面が出ているかを返す。
    fn state(&self) -> Result<CarDisplayState, DeviceError>;
}

/// 対象外 OS・初期化不能時のフォールバック。
pub struct UnsupportedCarDisplay;

impl CarDisplay for UnsupportedCarDisplay {
    fn set_text(&self, _text: &str) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("car_display"))
    }

    fn state(&self) -> Result<CarDisplayState, DeviceError> {
        Err(DeviceError::Unsupported("car_display"))
    }
}

/// 入力が空でない 1 行かつ [`MAX_TEXT_CHARS`] 文字以内かを検証する（各 OS 実装が JNI の手前で呼ぶ）。
pub fn validate_text(text: &str) -> Result<(), DeviceError> {
    if text.is_empty() {
        return Err(DeviceError::InvalidInput("car_display text is empty"));
    }
    if text
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err(DeviceError::InvalidInput("car_display text has newline"));
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(DeviceError::InvalidInput("car_display text too long"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invalid(s: &str) -> bool {
        matches!(validate_text(s), Err(DeviceError::InvalidInput(_)))
    }

    #[test]
    fn length_boundary() {
        assert!(validate_text(&"あ".repeat(MAX_TEXT_CHARS)).is_ok());
        assert!(invalid(&"あ".repeat(MAX_TEXT_CHARS + 1)));
    }

    #[test]
    fn rejects_empty_and_control_chars() {
        assert!(invalid(""));
        for c in ['\n', '\r', '\t', '\u{85}', '\u{2028}', '\u{2029}'] {
            assert!(invalid(&format!("a{c}b")), "{c:?}");
        }
    }

    #[test]
    fn keeps_surrounding_spaces_valid() {
        assert!(validate_text("  hi  ").is_ok());
    }

    #[test]
    fn error_does_not_echo_input() {
        let Err(e) = validate_text("secret\n") else {
            panic!("must fail");
        };
        assert!(!format!("{e:?}{e}").contains("secret"));
    }

    #[test]
    fn unsupported_is_error_for_both_methods() {
        assert!(matches!(
            UnsupportedCarDisplay.set_text("x"),
            Err(DeviceError::Unsupported("car_display"))
        ));
        assert!(matches!(
            UnsupportedCarDisplay.state(),
            Err(DeviceError::Unsupported("car_display"))
        ));
    }
}
