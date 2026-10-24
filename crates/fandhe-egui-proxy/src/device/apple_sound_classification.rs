//! macOS・iOS 向け環境音の分類実装（Issue #725、親 #142）。`apple/fandhe_apple_sound_classification.m`
//! （SoundAnalysis の内蔵分類器 `SNClassifySoundRequest` と `AVAudioEngine` の入力タップ。
//! `build.rs` が macOS・iOS 双方の feature `device` 分岐でコンパイルする）を `unsafe extern "C"`
//! 3 関数で呼ぶ。`device/sound_classification.rs` の macOS 経路が [`implementation`] を呼ぶ。
//! iOS（Issue #726）も同じ `.m` を使い、`ios_sound_classification.rs` がこのモジュールへ委譲する
//! （iOS では背景遷移・音声の割り込みを中断にし、`AVAudioSession` を保存して全終了経路で戻す）。dispatch・共通コア画面・ドライバ・trait 契約
//! （`fandhe_device::sound_classification`）は #725 の最終形のまま変えない。
//!
//! `.m` は OS の生値と状態コードを返すだけで、契約値への写像は [`start_result`] / [`read_result`] が
//! 行う（ホストでテストできるよう `cfg(any(ios, macos, test))`）。ラベルと信頼度の検証・上位 3 件の
//! 選択は `fandhe_device::sound_classification::select_top` が SSOT で、丸めの前に検査する。
//! 未知のコード・範囲外の件数や理由は成功へ倒さず `Failed` / 結果なしに写す。
//!
//! # 流れ
//!
//! - `start`: macOS 12 未満は `Unsupported`、許可拒否は `PermissionDenied`、マイクなしは `NoMicrophone`。
//!   許可が未決定のときは受理し、`read` が `AwaitingPermission`（期限に数えない）を返す
//! - `read`: 入力機器の変更は `Interrupted(InputChanged)` で、`stop` まで同じ値を返す
//! - `stop`: 制御キューへ積んで戻る。世代を進め、遅れて届く許可の応答と解析結果を `.m` が捨てる
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 非対応・拒否・マイクなし・中断・未知のコードを成功値へ畳まない、旧世代の通知を返さない、
//!   `seq` を巻き戻さない、全終了経路でマイクと解析の資源を解放する、ラベルと信頼度をログへ出さない
//! - 守らない: ラベルの翻訳、精度と更新間隔、背景での分類、中断後の自動再開、複数プロセス
//!
//! # 既知の天井
//!
//! TCC の許可を伴う実際の分類と入力変更時の中断は Mac 目視（#716）で確かめる。

use fandhe_device::sound_classification::{
    select_top, ClassificationResult, InterruptReason, SoundFailure, SoundPoll, MAX_LABEL_BYTES,
    TOP_K,
};

/// `.m` が返す候補の最大件数（`fandhe_device::sound_classification::TOP_K` と一致させる）。
pub(crate) const MAX_CANDIDATES: usize = TOP_K;

/// `.m` が候補 1 件ごとに確保するラベルのバッファ長（バイト）。
pub(crate) const LABEL_SLOT_BYTES: usize = MAX_LABEL_BYTES;

/// `.m` の start の戻り値を契約へ写す（0 受理 / 1 非対応 / 2 権限拒否 / 3 マイクなし / 他 失敗）。
pub(crate) fn start_result(code: i32) -> Result<(), SoundFailure> {
    match code {
        0 => Ok(()),
        1 => Err(SoundFailure::Unsupported),
        2 => Err(SoundFailure::PermissionDenied),
        3 => Err(SoundFailure::NoMicrophone),
        _ => Err(SoundFailure::Failed),
    }
}

/// `.m` が `read` で埋める出力領域の写し。
pub(crate) struct RawRead {
    /// 結果の番号。
    pub seq: u64,
    /// 候補ごとのラベル（`LABEL_SLOT_BYTES` ずつ詰める）。
    pub labels: [u8; MAX_CANDIDATES * LABEL_SLOT_BYTES],
    /// 候補ごとの元の UTF-8 長（スロット長を超えたものは捨てる）。
    pub label_lens: [u32; MAX_CANDIDATES],
    /// 候補ごとの信頼度。
    pub confidences: [f64; MAX_CANDIDATES],
    /// 有効な候補の件数。
    pub count: u32,
    /// 中断の理由（コード 4 のとき。1 背景 / 2 割り込み / 3 入力変更）。
    pub reason: i32,
}

impl Default for RawRead {
    fn default() -> Self {
        Self {
            seq: 0,
            labels: [0; MAX_CANDIDATES * LABEL_SLOT_BYTES],
            label_lens: [0; MAX_CANDIDATES],
            confidences: [0.0; MAX_CANDIDATES],
            count: 0,
            reason: 0,
        }
    }
}

fn interrupt_reason(raw: i32) -> Option<InterruptReason> {
    match raw {
        1 => Some(InterruptReason::Background),
        2 => Some(InterruptReason::AudioInterruption),
        3 => Some(InterruptReason::InputChanged),
        _ => None,
    }
}

/// 候補を検証して結果にする。件数は [`MAX_CANDIDATES`] に切り、長さがスロットを超える候補は捨てる。
/// 全部不正なら `None`（呼び出し側は聞き取り中にする）。
fn result_from_raw(raw: &RawRead) -> Option<ClassificationResult> {
    let n = (raw.count as usize).min(MAX_CANDIDATES);
    let mut candidates: Vec<(&[u8], f64)> = Vec::with_capacity(n);
    for i in 0..n {
        let len = raw.label_lens[i] as usize;
        if len > LABEL_SLOT_BYTES {
            continue;
        }
        let start = i * LABEL_SLOT_BYTES;
        candidates.push((&raw.labels[start..start + len], raw.confidences[i]));
    }
    let top = select_top(&candidates);
    if top.is_empty() {
        None
    } else {
        Some(ClassificationResult { top, seq: raw.seq })
    }
}

/// `.m` の read の戻り値を契約へ写す（0 停止中 / 1 許可待ち / 2 聞き取り中 / 3 結果あり / 4 中断 /
/// 5 権限拒否 / 6 マイクなし / 9 失敗 / 他 失敗）。未知の理由の中断は失敗にする。
pub(crate) fn read_result(code: i32, raw: &RawRead) -> Result<SoundPoll, SoundFailure> {
    match code {
        0 => Ok(SoundPoll::Idle),
        1 => Ok(SoundPoll::AwaitingPermission),
        2 => Ok(SoundPoll::Listening),
        3 => Ok(match result_from_raw(raw) {
            Some(r) => SoundPoll::Result(r),
            None => SoundPoll::Listening,
        }),
        4 => match interrupt_reason(raw.reason) {
            Some(r) => Ok(SoundPoll::Interrupted(r)),
            None => Err(SoundFailure::Failed),
        },
        5 => Err(SoundFailure::PermissionDenied),
        6 => Err(SoundFailure::NoMicrophone),
        _ => Err(SoundFailure::Failed),
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod imp {
    use std::sync::Arc;

    use fandhe_device::sound_classification::{SoundClassifier, SoundFailure, SoundPoll};

    use super::{read_result, start_result, RawRead};

    // SAFETY: `apple/fandhe_apple_sound_classification.m`（`build.rs` が macOS・iOS の feature `device`
    // 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_apple_sound_classification_start() -> i32;
        fn fandhe_apple_sound_classification_read(
            seq: *mut u64,
            labels: *mut u8,
            label_lens: *mut u32,
            confidences: *mut f64,
            count: *mut u32,
            reason: *mut i32,
        ) -> i32;
        fn fandhe_apple_sound_classification_stop();
    }

    struct AppleSoundClassifier;

    impl SoundClassifier for AppleSoundClassifier {
        #[allow(unsafe_code)]
        fn start(&self) -> Result<(), SoundFailure> {
            // SAFETY: 引数なし。`.m` は制御キューへ同期で状態の判定を依頼して戻る（main へは同期で待たない）。
            start_result(unsafe { fandhe_apple_sound_classification_start() })
        }

        #[allow(unsafe_code)]
        fn read(&self) -> Result<SoundPoll, SoundFailure> {
            let mut raw = RawRead::default();
            // SAFETY: 各ポインタは `raw` の固定長フィールドを指す排他参照で、呼び出しの間有効。
            // `labels` は `MAX_CANDIDATES * LABEL_SLOT_BYTES` バイト、`label_lens` / `confidences` は
            // `MAX_CANDIDATES` 要素で、`.m` はこの範囲を超えて書かず、ポインタを保持しない。
            let code = unsafe {
                fandhe_apple_sound_classification_read(
                    &mut raw.seq,
                    raw.labels.as_mut_ptr(),
                    raw.label_lens.as_mut_ptr(),
                    raw.confidences.as_mut_ptr(),
                    &mut raw.count,
                    &mut raw.reason,
                )
            };
            read_result(code, &raw)
        }

        #[allow(unsafe_code)]
        fn stop(&self) {
            // SAFETY: 引数なし。`.m` は制御キューへ非同期に積んで戻る。
            unsafe { fandhe_apple_sound_classification_stop() }
        }
    }

    /// Apple 系の `SoundClassifier` 実装を返す（`device/sound_classification.rs` の macOS 経路と、
    /// b-ios が委譲する `ios_sound_classification` から呼ばれる）。
    pub(crate) fn implementation() -> Arc<dyn SoundClassifier> {
        Arc::new(AppleSoundClassifier)
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) use imp::implementation;

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_with(items: &[(&str, f64)]) -> RawRead {
        let mut raw = RawRead::default();
        for (i, (l, c)) in items.iter().enumerate().take(MAX_CANDIDATES) {
            let start = i * LABEL_SLOT_BYTES;
            let b = l.as_bytes();
            let n = b.len().min(LABEL_SLOT_BYTES);
            raw.labels[start..start + n].copy_from_slice(&b[..n]);
            raw.label_lens[i] = b.len() as u32;
            raw.confidences[i] = *c;
        }
        raw.count = items.len() as u32;
        raw.seq = 9;
        raw
    }

    #[test]
    fn start_codes_never_fold_to_ok() {
        assert_eq!(start_result(0), Ok(()));
        assert_eq!(start_result(1), Err(SoundFailure::Unsupported));
        assert_eq!(start_result(2), Err(SoundFailure::PermissionDenied));
        assert_eq!(start_result(3), Err(SoundFailure::NoMicrophone));
        assert_eq!(start_result(4), Err(SoundFailure::Failed));
        assert_eq!(start_result(-1), Err(SoundFailure::Failed));
        assert_eq!(start_result(i32::MAX), Err(SoundFailure::Failed));
    }

    #[test]
    fn read_state_codes() {
        let raw = RawRead::default();
        assert_eq!(read_result(0, &raw), Ok(SoundPoll::Idle));
        assert_eq!(read_result(1, &raw), Ok(SoundPoll::AwaitingPermission));
        assert_eq!(read_result(2, &raw), Ok(SoundPoll::Listening));
        assert_eq!(read_result(5, &raw), Err(SoundFailure::PermissionDenied));
        assert_eq!(read_result(6, &raw), Err(SoundFailure::NoMicrophone));
        assert_eq!(read_result(9, &raw), Err(SoundFailure::Failed));
    }

    #[test]
    fn unknown_codes_are_failures() {
        let raw = RawRead::default();
        for code in [7, 8, 10, 99, -1, i32::MIN] {
            assert_eq!(read_result(code, &raw), Err(SoundFailure::Failed));
        }
    }

    #[test]
    fn result_code_carries_validated_top() {
        let raw = raw_with(&[("dog_bark", 0.2), ("speech", 0.9)]);
        match read_result(3, &raw) {
            Ok(SoundPoll::Result(r)) => {
                assert_eq!(r.seq, 9);
                assert_eq!(r.top[0].label, "speech");
                assert_eq!(r.top[1].label, "dog_bark");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn invalid_candidates_never_become_a_result() {
        // 不正なラベル・範囲外の信頼度だけなら結果にせず聞き取り中にする。
        let raw = raw_with(&[("bad label", 0.9), ("ok", 1.5), ("nan", f64::NAN)]);
        assert_eq!(read_result(3, &raw), Ok(SoundPoll::Listening));
    }

    #[test]
    fn out_of_range_count_is_clamped() {
        let mut raw = raw_with(&[("a", 0.5), ("b", 0.4), ("c", 0.3)]);
        raw.count = 1000;
        match read_result(3, &raw) {
            Ok(SoundPoll::Result(r)) => assert_eq!(r.top.len(), 3),
            other => panic!("unexpected: {other:?}"),
        }
        raw.count = 0;
        assert_eq!(read_result(3, &raw), Ok(SoundPoll::Listening));
    }

    #[test]
    fn oversized_label_len_is_dropped() {
        let mut raw = raw_with(&[("a", 0.5), ("b", 0.4)]);
        raw.label_lens[0] = (LABEL_SLOT_BYTES as u32) + 1;
        match read_result(3, &raw) {
            Ok(SoundPoll::Result(r)) => {
                assert_eq!(r.top.len(), 1);
                assert_eq!(r.top[0].label, "b");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn interrupt_reasons() {
        let mut raw = RawRead::default();
        for (reason, want) in [
            (1, InterruptReason::Background),
            (2, InterruptReason::AudioInterruption),
            (3, InterruptReason::InputChanged),
        ] {
            raw.reason = reason;
            assert_eq!(read_result(4, &raw), Ok(SoundPoll::Interrupted(want)));
        }
        for bad in [0, 4, -1, 99] {
            raw.reason = bad;
            assert_eq!(read_result(4, &raw), Err(SoundFailure::Failed));
        }
    }
}
