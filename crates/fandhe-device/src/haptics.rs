//! 触覚フィードバック（軽い振動 1 回）の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/haptics.json`）。
//!
//! 扱う範囲は「ボタン押下で軽い振動を 1 回鳴らす」ことだけ。パターン・強さ・長さの指定、
//! 振動の停止、細かい波形（CoreHaptics のパターン再生等）は扱わない。対象は
//! iOS / Android / macOS / Windows / Web。一発呼び出し型（結果を待つ要求を持たない）で、
//! `Clipboard` / `Tts` と同じく 1 回の [`Haptics::tap`] = 1 回の要求。
//!
//! # 契約
//!
//! - **非搭載を判定できる OS は非搭載で `Err(Unsupported)`**。呼んだだけで成功にしない
//!   （`device-bridge-contracts.md`「6. 嘘の成功」）。判定の根拠は iOS が
//!   `CHHapticEngine` の hardware capabilities（`supportsHaptics`）、Android が
//!   `Vibrator.hasVibrator()`、Windows が振動デバイス（`VibrationDevice`）の有無
//! - **`Ok` の意味は OS により異なる**。macOS（`NSHapticFeedbackManager` の `perform` は
//!   戻り値が無い）と Web（`navigator.vibrate` は振動子の有無を返さず、サイレント /
//!   おやすみモードでも何も返さない）は実際に振動したかを検知できないため、`Ok` は
//!   「OS / ブラウザへ要求を渡した」までを意味する。iOS / Android / Windows の `Ok` は
//!   「非搭載ではなく、要求を渡した」まで。いずれも「振動した」ことの保証ではない
//! - **Web は `navigator.vibrate` が無い環境だけ `Err(Unsupported)`**（Safari 等）。
//!   ユーザー操作（ボタン押下）直後にのみ呼ぶ
//! - **連打は待たない**: 前の振動の完了を待たず、次の要求をそのまま OS へ渡す
//!   （キューイング・間引きをしない。重なった場合の挙動は OS 任せ）
//! - **UI スレッド要件**: iOS は main queue で呼び `@autoreleasepool` で囲む
//!   （`device-bridge-contracts.md`「5. FFI 境界」）。Android は JNI 受け口 1 本で、
//!   スレッド制約なし。呼び出し元（橋渡し層）が UI スレッドから呼ぶ前提とする
//! - **失敗の区別**: 非搭載 → `Unsupported`、それ以外の OS 呼び出し失敗 →
//!   `Err(Internal)`。権限は不要（Android の `VIBRATE` は通常権限）なので
//!   `PermissionDenied` は返さない
//!
//! # 守る範囲と既知の制約
//!
//! 守るのは 1 プロセス内で要求を渡す順序と、非搭載・API 無しを成功と見せないこと。
//! 既知の制約（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）として、実際に振動
//! したか・振動が重なったときの体感は OS / 端末 / ユーザー設定に依存し、検知も保証も
//! しない。確保する資源は無い（要求を渡したら終わり）ため、画面離脱・終了時の後片付けも無い。

use crate::DeviceError;

/// 触覚フィードバックの契約（モジュール doc の「契約」節）。
pub trait Haptics: Send + Sync {
    /// 軽い振動を 1 回、OS へ要求する。
    ///
    /// 非搭載を判定できる OS では非搭載のとき `Err(Unsupported)`。判定できない
    /// macOS / Web の `Ok` は「要求を渡した」まで。
    fn tap(&self) -> Result<(), DeviceError>;
}

/// 対象外 OS（Linux 等）・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedHaptics;

impl Haptics for UnsupportedHaptics {
    fn tap(&self) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("haptics"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_haptics_is_not_a_fake_success() {
        let err = UnsupportedHaptics.tap().unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported(_)));
    }
}
