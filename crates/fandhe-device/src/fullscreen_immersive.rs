//! 全画面表示（システムバー非表示）（Phase 11 の拡充ループ、親 #1133、Issue #1312、対応表
//! `docs/device/capabilities/fullscreen-immersive.json`）。
//!
//! 扱う範囲は「ウィンドウ（画面）を全画面にする / 通常へ戻す」ことと、実際に全画面かの読み戻しだけ。
//! 対象は macOS / Windows（#1312）、iOS（#1313）、Android（#1314）、Web（#1315）。
//! `fandhe-egui-proxy` の橋渡し層が UI スレッドから [`Fullscreen::request`] で切り替えを依頼し、
//! [`Fullscreen::is_fullscreen`] を毎フレーム読み戻して画面
//! （`fandhe_core::screens::fullscreen_toggle`）へ返す。
//!
//! # 契約
//!
//! - **依頼と完了を分ける**: `request` は OS へ依頼するだけで完了を待たない。macOS の全画面遷移は
//!   アニメーションで 1 秒前後かかり、失敗すると元へ戻るため、依頼直後の読み戻しでは確定させない
//! - **表示は読み戻しで決める**: 要求ではなく `is_fullscreen` で決める（契約表 6「嘘の成功」）。
//!   `Ok` を返しても読み戻しが目標に一致しなければ失敗。期限は共通コアの `CONFIRM_TIMEOUT_SECS`（3 秒）
//! - **`is_fullscreen` の `None`**: 読めない（ウィンドウ未準備など）。`false` で代用しない
//! - **外部からの変化**（緑ボタン・OS ショートカット・Android のジェスチャー・Web の `fullscreenchange`）は
//!   読み戻しの差分として画面へ反映する
//! - **エラー種別**: 非対応 → `Err(Unsupported)`、OS 呼び出しの失敗 → `Err(Internal)`。権限は不要で
//!   利用者がキャンセルする経路も無いため `PermissionDenied` / `Cancelled` は返さない。契約表 10 の
//!   三値のうちキャンセルは該当なしで、成功 / 失敗の二値
//! - **スレッド**: UI スレッドから同期で呼ぶ。ワーカーは作らない（契約表 11）
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内で表示が実際の状態に追従すること、失敗・非対応・読めない状態を「全画面です」と
//! 見せないこと。守らない（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）: 遷移アニメーション中の
//! 一時的な表示ずれ、期限切れの後に OS が遅れて切り替えた場合の失敗文言（表示自体は追従する）、
//! 複数ウィンドウ、プロセス終了時の後片付け（OS がウィンドウごと片付ける）。いずれも表示が一時的に
//! 食い違うだけで、データ消失・資源漏れ・嘘の成功には至らない。
//!
//! # 後続 OS の目安
//!
//! - iOS: `prefersStatusBarHidden` / `prefersHomeIndicatorAutoHidden`
//! - Android: `WindowInsetsControllerCompat.hide(systemBars())`（ジェスチャーで一時表示される）
//! - Web: `requestFullscreen` を `fandhe-web-dom` から呼び、`fullscreenchange` で反映

use crate::DeviceError;

/// 全画面表示の契約（モジュール doc の「契約」節）。
pub trait Fullscreen: Send + Sync {
    /// `true` で全画面、`false` で通常へ戻す依頼を出す。完了は待たない。
    fn request(&self, on: bool) -> Result<(), DeviceError>;

    /// 実際に全画面か。`None` は読めない状態。
    fn is_fullscreen(&self) -> Option<bool>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedFullscreen;

impl Fullscreen for UnsupportedFullscreen {
    fn request(&self, _on: bool) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("fullscreen-immersive"))
    }

    fn is_fullscreen(&self) -> Option<bool> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds_nor_reads() {
        let f = UnsupportedFullscreen;
        assert!(matches!(
            f.request(true),
            Err(DeviceError::Unsupported("fullscreen-immersive"))
        ));
        assert_eq!(f.is_fullscreen(), None);
    }
}
