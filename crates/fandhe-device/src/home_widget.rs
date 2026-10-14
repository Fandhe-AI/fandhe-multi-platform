//! ホーム画面ウィジェットへ表示する 1 行テキストの抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/home-widget.json`）。
//!
//! 扱う範囲は「アプリが保存した 1 行のテキストを、ホーム画面（macOS は通知センター /
//! デスクトップ）のウィジェットに表示する」ことだけ。対象は iOS / Android / macOS。
//! Windows・Web・Linux は [`UnsupportedHomeWidget`]（`Unsupported`）。Windows は
//! Widgets Board が package identity 必須のため今回対象外。複数ウィジェット・画像・操作（タップ）・
//! 定期更新は扱わない。
//!
//! 共有保存先は Apple が App Group コンテナのファイル（macOS は XcodeGen の小さな
//! ホストアプリ + WidgetKit 拡張を併設）、Android が `SharedPreferences`。ウィジェット側
//! （Swift / Java）はそこを読んで描画する。実装は後続の機能 PR（`fandhe-egui-proxy` の
//! `device/`）で入れる。
//!
//! # 契約
//!
//! - **`set_text`**: 共有保存先へ書けたら `Ok(())`。保存後にウィジェットの再描画要求
//!   （`reloadAllTimelines` / `AppWidgetManager` の更新通知）を出すが、これはベスト
//!   エフォートで、要求の失敗・空振りは `Ok` のまま（`Ok` が約束するのは「保存できた」
//!   ことで、ウィジェットへの反映ではない。反映は OS 側で非同期・予算制限があり、
//!   要求が失敗した場合は次に再描画要求が通るまで古い表示のままになりうる＝既知の制約）。保存に失敗したら `Err`
//!   （`Internal`）で、成功を装わない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **入力検証**: 1 行・[`MAX_TEXT_CHARS`] 文字以内（[`validate_text`]）。改行
//!   （`\n` / `\r` / NEL などの制御文字と U+2028 / U+2029）を含む・長すぎる場合は `Err(InvalidInput)`。黙って切り詰めない。
//!   前後の空白は入力どおり保存する（trim しない、同「8. 秘密値」）。空文字は有効な
//!   入力で、「表示を空にする」ことを意味する
//! - **`current_text`**: 保存済みの値を読む。未保存は `Ok(None)`（空文字を番兵にしない）。
//!   読み出しに失敗したら `Err`（未保存と混同しない）
//! - **ウィジェット 0 個でも保存は成功**: ユーザーがウィジェットを置いていなくても
//!   `set_text` は成功し、`current_text` で読める（再描画要求は空振りしてよい）
//! - **対象外 OS**: 全メソッドが `Err(Unsupported("home_widget"))`。初期化に失敗した実装も
//!   [`UnsupportedHomeWidget`] へ倒す（ダミーの成功実装へは倒さない）
//!
//! # 守る範囲と既知の制約
//!
//! 守るのは 1 プロセス内の呼び出し順での保存結果と、失敗を成功と見せないこと。
//! 既知の制約（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）として、
//! **ウィジェットへの実際の反映は OS 側で非同期**であり保証しない（Apple の reload 予算・
//! Android の更新タイミングで遅れる / 間引かれる）。`Ok` は「保存した」
//! までの意味で、ウィジェットが今その文字を表示していることではない。複数プロセス
//! （ウィジェット拡張プロセスを含む）が同時に保存先を触ったときの食い違いは扱わない。

use crate::DeviceError;

/// 保存できるテキストの最大文字数（`char` 数）。ウィジェットの 1 行表示の目安。
pub const MAX_TEXT_CHARS: usize = 40;

/// ホーム画面ウィジェットの契約（モジュール doc の「契約」節）。
pub trait HomeWidget: Send + Sync {
    /// 表示テキストを保存して再描画を要求する。入力は [`validate_text`] で検証済みの
    /// ものだけを保存する。
    fn set_text(&self, text: &str) -> Result<(), DeviceError>;
    /// 保存済みのテキストを返す。未保存は `Ok(None)`。
    fn current_text(&self) -> Result<Option<String>, DeviceError>;
}

/// 対象外 OS（Windows・Web・Linux）・初期化失敗時のフォールバック。
pub struct UnsupportedHomeWidget;

impl HomeWidget for UnsupportedHomeWidget {
    fn set_text(&self, _text: &str) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("home_widget"))
    }

    fn current_text(&self) -> Result<Option<String>, DeviceError> {
        Err(DeviceError::Unsupported("home_widget"))
    }
}

/// 入力が 1 行かつ [`MAX_TEXT_CHARS`] 文字以内かを検証する（各 OS 実装が保存前に呼ぶ）。
pub fn validate_text(text: &str) -> Result<(), DeviceError> {
    if text
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err(DeviceError::InvalidInput("home_widget text has newline"));
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(DeviceError::InvalidInput("home_widget text too long"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_empty_and_keeps_surrounding_spaces_valid() {
        assert!(validate_text("").is_ok());
        assert!(validate_text("  hi  ").is_ok());
    }

    #[test]
    fn rejects_newline_and_too_long() {
        assert!(matches!(
            validate_text("a\nb"),
            Err(DeviceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_text("a\rb"),
            Err(DeviceError::InvalidInput(_))
        ));
        for c in ['\u{85}', '\u{2028}', '\u{2029}'] {
            assert!(matches!(
                validate_text(&format!("a{c}b")),
                Err(DeviceError::InvalidInput(_))
            ));
        }
        assert!(validate_text(&"あ".repeat(MAX_TEXT_CHARS)).is_ok());
        assert!(matches!(
            validate_text(&"あ".repeat(MAX_TEXT_CHARS + 1)),
            Err(DeviceError::InvalidInput(_))
        ));
    }

    #[test]
    fn unsupported_is_error_for_both_methods() {
        assert!(matches!(
            UnsupportedHomeWidget.set_text("x"),
            Err(DeviceError::Unsupported("home_widget"))
        ));
        assert!(matches!(
            UnsupportedHomeWidget.current_text(),
            Err(DeviceError::Unsupported("home_widget"))
        ));
    }
}
