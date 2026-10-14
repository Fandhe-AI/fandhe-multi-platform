//! Phase 11 の拡充ループ型サンプル「TSV 読み込み」（親 #979、Issue #980）の effect ドライバ。
//! `fandhe_core::screens::tsv_import`（端末依存ゼロ）の要求を観測し、選択された
//! ファイルのバイト列を `CompletionSender` 経由で返す（`crate::screens::registry` の
//! `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約）。
//!
//! ライフサイクル処理（デスクトップの同期呼び出し・モバイルの pull 型 Restore 分岐）は
//! [`super::file_pick`] の [`super::file_pick::PickHooks`]/`drive_with`/`drive_mobile_with`
//! をそのまま再利用する（契約表 1 の再発防止、`file_pick.rs` モジュール doc
//! 「`PickHooks<S>`」節参照）。橋渡しの実体（`bridge`、iOS/Android 提示関数）も
//! `file_pick` と共有する（`FANDHE_PROXY_APP` で起動する画面は 1 プロセスに 1 つだけ
//! のため混線しない、`file_pick.rs` モジュール doc「`bridge` は 2 画面で共有する」節参照）。
//!
//! # サイズ上限は 2 段
//!
//! (1) `FilePicker`/`bridge` 側（[`fandhe_device::file_picker::MAX_FILE_BYTES`]、8 MiB）で
//! 「そもそも選べるファイルサイズ」を絞り、(2) 解析側（[`fandhe_core::tsv::MAX_TSV_BYTES`]、
//! 1 MiB）で「TSV として読み込める行数・列数」を絞る。前者を通過しても後者で
//! `TsvError::TooLarge` になり得る（別々の上限、SSOT を混ぜない）。

use fandhe_core::screens::tsv_import::{
    finish_import, mark_picking, request_import, TsvImportState,
};
use fandhe_device::file_picker::PickedFile;

#[cfg(any(target_os = "ios", target_os = "android"))]
use super::file_pick::drive_mobile_with;
use super::file_pick::PickHooks;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use super::file_pick::{drive_with, fail_request_with, shared_picker};
use crate::app::CompletionSender;

/// `tsv_import` 画面用の [`PickHooks`]（`TsvImportState` 版）。`finish` は `PickedFile` を
/// バイト列（`Vec<u8>`）だけへ潰してから `finish_import` へ渡す（解析は
/// `fandhe_core::screens::tsv_import::finish_import` 内部で行う。要約に潰す
/// `file_pick::FILE_PICK_HOOKS` とはここが異なる）。`PickedFile`（`fandhe_device::
/// file_picker::PickedFile`、`bytes: Vec<u8>`）は共通コアが依存できない型のため、
/// この完了時 1 回だけ `Vec<u8>` を取り出す。毎フレームの `RequestSlot::snapshot`
/// クローンは `bridge::Outcome` が `Arc<PickedFile>` で包んでいるため参照カウント
/// 共有のみで、バイト列自体はここでの 1 回しか複製されない
/// （`file_pick.rs` の `bridge` モジュール doc 参照）。
const TSV_IMPORT_HOOKS: PickHooks<TsvImportState> = PickHooks {
    phase: |s| s.phase,
    generation: |s| s.generation,
    request: request_import,
    mark_picking,
    finish: |s, generation, result| {
        finish_import(
            s,
            generation,
            result.map(|opt| opt.map(|f: PickedFile| f.bytes)),
        );
    },
};

/// `registry.rs` の `effect` 区画から `tsv_import` 画面用に配線される effect ドライバ
/// （iOS/Android 版）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn tsv_import_effect(state: &TsvImportState, sender: &CompletionSender<TsvImportState>) {
    drive_mobile_with(state, sender, TSV_IMPORT_HOOKS);
}

/// `registry.rs` の `effect` 区画から `tsv_import` 画面用に配線される effect ドライバ
/// （デスクトップ版）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn tsv_import_effect(state: &TsvImportState, sender: &CompletionSender<TsvImportState>) {
    match shared_picker() {
        Ok(picker) => drive_with(state, sender, picker, TSV_IMPORT_HOOKS),
        Err(message) => fail_request_with(state, sender, message, TSV_IMPORT_HOOKS),
    }
}

#[cfg(test)]
#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod tests {
    use super::super::file_pick::UNAVAILABLE;
    use super::*;
    use fandhe_core::screens::tsv_import::request_import;
    use fandhe_device::file_picker::FilePicker;
    use fandhe_device::DeviceError;

    fn sender_with_channel() -> (
        CompletionSender<TsvImportState>,
        std::sync::mpsc::Receiver<crate::app::Completion<TsvImportState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    /// 常に指定した結果を返すモック（GUI 無しで `drive_with` を検証する。
    /// `device/file_pick.rs` の `MockPicker` と同型）。
    struct MockPicker(Result<Option<PickedFile>, DeviceError>);

    impl FilePicker for MockPicker {
        fn pick(&self) -> Result<Option<PickedFile>, DeviceError> {
            match &self.0 {
                Ok(v) => Ok(v.clone()),
                Err(DeviceError::Unsupported(s)) => Err(DeviceError::Unsupported(s)),
                Err(DeviceError::InvalidInput(s)) => Err(DeviceError::InvalidInput(s)),
                Err(DeviceError::Internal(s)) => Err(DeviceError::Internal(s)),
                Err(_) => Err(DeviceError::Internal("tsv_import test")),
            }
        }
    }

    fn apply_all(
        state: &mut TsvImportState,
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<TsvImportState>>,
    ) {
        while let Ok(apply) = rx.try_recv() {
            apply(state);
        }
    }

    #[test]
    fn drive_reflects_valid_tsv_as_row_count_status() {
        let (sender, rx) = sender_with_channel();
        let mut state = TsvImportState::default();
        request_import(&mut state);

        drive_with(
            &state,
            &sender,
            &MockPicker(Ok(Some(PickedFile {
                name: "dict.tsv".to_string(),
                bytes: b"a\tb\nc\td\n".to_vec(),
            }))),
            TSV_IMPORT_HOOKS,
        );
        apply_all(&mut state, &rx);

        assert!(state.status.contains("2 行"));
        assert_eq!(state.error, "");
    }

    #[test]
    fn drive_reports_non_utf8_as_error() {
        let (sender, rx) = sender_with_channel();
        let mut state = TsvImportState::default();
        request_import(&mut state);

        drive_with(
            &state,
            &sender,
            &MockPicker(Ok(Some(PickedFile {
                name: "dict.tsv".to_string(),
                bytes: vec![0xFF, 0xFE],
            }))),
            TSV_IMPORT_HOOKS,
        );
        apply_all(&mut state, &rx);

        assert_eq!(state.status, "");
        assert!(state.error.contains("UTF-8"));
    }

    #[test]
    fn drive_reflects_cancel_without_success_wording() {
        let (sender, rx) = sender_with_channel();
        let mut state = TsvImportState::default();
        request_import(&mut state);

        drive_with(&state, &sender, &MockPicker(Ok(None)), TSV_IMPORT_HOOKS);
        apply_all(&mut state, &rx);

        assert!(!state.status.contains("読み込みました"));
        assert_eq!(state.error, "");
    }

    #[test]
    fn fail_request_reports_unavailable_without_success_wording() {
        let (sender, rx) = sender_with_channel();
        let mut state = TsvImportState::default();
        request_import(&mut state);

        fail_request_with(&state, &sender, UNAVAILABLE, TSV_IMPORT_HOOKS);
        apply_all(&mut state, &rx);

        assert_eq!(state.error, format!("失敗しました: {UNAVAILABLE}"));
        assert_eq!(state.status, "");
    }
}
