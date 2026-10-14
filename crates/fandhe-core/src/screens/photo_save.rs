//! Phase 11（端末機能の拡充ループ、親 #1121）型サンプル「写真の保存」（Issue #1254）。
//! 説明文と「画像を 1 枚保存」ボタンだけの 1 画面。`FANDHE_PROXY_APP=photo_save` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod photo_save;` と対）。入力欄を持たないのは、保存する画像を
//! effect ドライバ（`device/photo_save.rs`）が自前で生成するため（OS 固有の操作 UI も無い）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。PNG の検査と OS 呼び出しは effect ドライバが
//! [`take_request`] で要求を観測して行い、[`finish_save`] で結果を注入する。本画面は
//! 二重押し防止と文言だけを持つ。保存先ごとの完了文言（iOS / macOS・Android・Windows）は
//! (a) で全 OS 分を確定しておき、後続の OS 別 PR は共通ファイルに触れない。
//!
//! # 結果を混同しない理由
//!
//! 権限拒否・非対応・失敗を黙って「保存しました」に丸めると、利用者は保存されたと誤解する。
//! 保存済みの文言は [`Ok(SaveOutcome::Saved)`] のときだけ出す。OS が判定した正常な結果は
//! `status`、失敗は `error`（`TextRole::Error`）へ書き分ける（`calendar_add` と同型）。
//! 世代番号（`generation`）で、古い完了や `Idle` で届いた完了を無視する。

use crate::{Screen, WidgetId};

/// 「画像を 1 枚保存」ボタンの `WidgetId`。
const SAVE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// iOS / macOS で保存できたときの案内。
pub const SAVED_PHOTO_LIBRARY_STATUS: &str = "写真ライブラリへ保存しました";
/// Android で保存できたときの案内。
pub const SAVED_GALLERY_STATUS: &str = "ギャラリー（Pictures フォルダ）へ保存しました";
/// Windows で保存できたときの案内。
pub const SAVED_PICTURES_STATUS: &str = "ピクチャ フォルダへ保存しました";
/// 権限が無いときの案内。
pub const DENIED_STATUS: &str = "写真への追加が許可されていません（設定で変更できます）";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この環境では写真を保存できません";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "保存できませんでした: ";

/// 保存操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SavePhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_saving`] 済み）。
    Saving,
}

/// 保存先の種類（`fandhe_device::photo_library::SavedTo` の写し。core は device に依存しない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavedPlace {
    /// iOS / macOS の写真ライブラリ。
    PhotoLibrary,
    /// Android のギャラリー（MediaStore）。
    Gallery,
    /// Windows の Pictures フォルダ。
    PicturesFolder,
}

/// OS が判定した結果（写像は `fandhe-egui-proxy` 側の責務）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    /// 保存が確定した。
    Saved(SavedPlace),
    /// 権限拒否・制限。
    Denied,
    /// この環境では非対応。
    Unsupported,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveRequest {
    /// [`finish_save`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PhotoSaveState {
    /// 進行状態。
    pub phase: SavePhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// OS が判定した結果の案内（通常色）。
    pub status: String,
    /// 失敗（`TextRole::Error`）。
    pub error: String,
}

/// 「画像を 1 枚保存」ボタンのハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn save(state: &mut PhotoSaveState) {
    if state.phase != SavePhase::Idle {
        return;
    }
    state.phase = SavePhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &PhotoSaveState) -> Option<SaveRequest> {
    (state.phase == SavePhase::Requested).then_some(SaveRequest {
        generation: state.generation,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_saving(state: &mut PhotoSaveState) {
    if state.phase == SavePhase::Requested {
        state.phase = SavePhase::Saving;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish_save(
    state: &mut PhotoSaveState,
    generation: u64,
    result: Result<SaveOutcome, String>,
) {
    if state.phase == SavePhase::Idle || state.generation != generation {
        return;
    }
    state.phase = SavePhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(SaveOutcome::Saved(SavedPlace::PhotoLibrary)) => {
            state.status = SAVED_PHOTO_LIBRARY_STATUS.to_string()
        }
        Ok(SaveOutcome::Saved(SavedPlace::Gallery)) => {
            state.status = SAVED_GALLERY_STATUS.to_string()
        }
        Ok(SaveOutcome::Saved(SavedPlace::PicturesFolder)) => {
            state.status = SAVED_PICTURES_STATUS.to_string()
        }
        Ok(SaveOutcome::Denied) => state.status = DENIED_STATUS.to_string(),
        Ok(SaveOutcome::Unsupported) => state.status = UNSUPPORTED_STATUS.to_string(),
        Err(m) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &PhotoSaveState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &PhotoSaveState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。処理中はボタンを外して二重押しを防ぐ。
pub fn current_screen(state: &PhotoSaveState) -> Screen<PhotoSaveState> {
    let screen = Screen::new("写真の保存")
        .text("アプリが作った画像を 1 枚、写真ライブラリへ保存します")
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != SavePhase::Idle {
        return screen;
    }
    screen.button(SAVE_BUTTON_ID, "画像を 1 枚保存", save)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saving() -> PhotoSaveState {
        let mut s = PhotoSaveState::default();
        save(&mut s);
        mark_saving(&mut s);
        s
    }

    fn shown(r: Result<SaveOutcome, String>) -> (String, String) {
        let mut s = saving();
        let g = s.generation;
        finish_save(&mut s, g, r);
        assert_eq!(s.phase, SavePhase::Idle);
        (s.status, s.error)
    }

    #[test]
    fn results_are_distinct_and_only_saved_says_saved() {
        let all = [
            shown(Ok(SaveOutcome::Saved(SavedPlace::PhotoLibrary))),
            shown(Ok(SaveOutcome::Saved(SavedPlace::Gallery))),
            shown(Ok(SaveOutcome::Saved(SavedPlace::PicturesFolder))),
            shown(Ok(SaveOutcome::Denied)),
            shown(Ok(SaveOutcome::Unsupported)),
            shown(Err("x".into())),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
            let said_saved = format!("{}{}", a.0, a.1).contains("保存しました");
            assert_eq!(said_saved, i < 3, "index {i}");
        }
        assert!(!FAILED_PREFIX.contains("保存しました"));
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = saving();
        let g = s.generation + 1;
        finish_save(&mut s, g, Ok(SaveOutcome::Saved(SavedPlace::PhotoLibrary)));
        assert_eq!(s.phase, SavePhase::Saving);
        assert!(s.status.is_empty());

        let mut idle = PhotoSaveState::default();
        finish_save(
            &mut idle,
            0,
            Ok(SaveOutcome::Saved(SavedPlace::PhotoLibrary)),
        );
        assert!(idle.status.is_empty());
    }

    #[test]
    fn save_is_noop_while_in_flight_and_clears_previous() {
        let mut s = saving();
        let g = s.generation;
        save(&mut s);
        assert_eq!((s.phase, s.generation), (SavePhase::Saving, g));

        let mut s = PhotoSaveState {
            status: SAVED_PHOTO_LIBRARY_STATUS.into(),
            error: "e".into(),
            ..Default::default()
        };
        save(&mut s);
        assert!(s.status.is_empty() && s.error.is_empty());
        assert_eq!(s.phase, SavePhase::Requested);
    }

    #[test]
    fn take_request_only_when_requested() {
        let mut s = PhotoSaveState::default();
        assert!(take_request(&s).is_none());
        save(&mut s);
        assert_eq!(take_request(&s).unwrap().generation, s.generation);
        mark_saving(&mut s);
        assert!(take_request(&s).is_none());
    }
}
