//! 環境音の分類の契約（Issue #725、親 #142、対応表 `sound-classification`）。マイクで拾った
//! まわりの音を OS の内蔵分類器で分類し、上位 3 件のラベルと信頼度を画面に出すための橋渡し。
//! `fandhe-egui-proxy` の `device/sound_classification.rs` が `start` / `stop` と間隔つきの
//! [`SoundClassifier::read`] を行う（pull 型。OS 側が最新の結果を持ち、画面側が読む。
//! `activity_recognition` と同型）。実体は macOS の SoundAnalysis + `AVAudioEngine`（#725）、
//! iOS は後続の b-ios。
//!
//! 分類の結果は表示のためだけの値で、認可・施錠・本人確認などの判断には使わない。
//! 音声バッファは保存も送信もしない。
//!
//! # 契約
//!
//! - 結果は [`SoundPoll`] か [`SoundFailure`]。`DeviceError` へ variant を足すと既存の網羅 match が壊れるため
//!   機能固有の enum にする（`compass::CompassFailure` と同じ判断）
//! - 結果なしを、空の結果や直前の値で代用しない（`device-bridge-contracts.md`「6. 嘘の成功」）。
//!   結果が空（検証で全候補を捨てた場合を含む）のときは [`SoundPoll::Listening`] にする
//! - ラベルは OS の識別子のまま出し、翻訳しない。[`validate_label`] を通ったものだけを表示する
//! - ラベルと信頼度の検証は丸め（[`to_percent`]）の前に行う（契約表 5）
//!
//! # 結果の分類（契約表 10）
//!
//! | 場面 | 分類 |
//! | --- | --- |
//! | 利用者の「停止」 | キャンセルとして待機へ戻す。失敗ではない |
//! | 背景遷移・音声の割り込み・入力機器の変更（[`SoundPoll::Interrupted`]） | 中断。実装が資源を解放済みで、自動では再開しない |
//! | 非対応・許可拒否・マイクなし・期限切れ・その他 | それぞれ別の失敗 |
//!
//! 停止のタイミングごとの扱い:
//!
//! | 停止の時点 | 扱い |
//! | --- | --- |
//! | 許可待ち中 | 実装内の世代を進め、遅れて届く許可の応答を捨てる |
//! | 聞き取り中 | 入力タップ・エンジン・analyzer を解放して待機へ。失敗にしない |
//! | 結果表示中 | 解放して待機へ。最後の結果は残さない |
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内の開始・停止・結果の順序（画面の `session` と実装内の世代で照合）、古い結果を
//! 見せない（[`StaleTracker`]）、権限拒否・マイクなし・中断・期限切れを成功に見せない、停止・失敗・
//! 中断の全経路でマイクと解析の資源を解放する（iOS は音声セッションの復元も）、ラベル・信頼度を
//! ログへ出さない。
//!
//! 守らない: ラベルの翻訳、精度・更新間隔・表示の遅れ（OS の分類器が決める。[`READ_INTERVAL`] を含む）、
//! 背景での分類、割り込み後の自動再開、マイクを使う他の機能との同時使用、複数プロセス、独自カテゴリ、
//! 録音の保存。デスクトップの「画面を離れる」は 1 画面 1 プロセスのためプロセス終了に当たり、
//! マイクと `AVAudioEngine` は OS が解放するため無理に同期させない（契約表 11 の最終項目）。
//!
//! # 並行経路（契約表 11）
//!
//! Rust 側は画面の driver 1 本の `wanted` の立ち上がり・立ち下がりでだけ `start` / `stop` する。
//! 実装（Apple の `.m`）は、開始・停止・許可の応答・通知・結果の格納・`read` を 1 本の制御
//! serial queue に集める。音声の解析は別の serial queue で回す。main へ `dispatch_sync` しない。
//!
//! # 時刻
//!
//! 期限・古さの判定は、呼び出し側が単調時計（`Instant` 由来）から作ったミリ秒の `f64` で行う
//! （壁時計は使わない、契約表 3）。
//!
//! # ログ
//!
//! ラベル・信頼度・`NSError` のコードをログへ出さない（契約表 8、`security.md`）。
//!
//! # 後続 iOS 実装（b-ios）への要点
//!
//! - `AVAudioSession` を Record にし、開始前の category / options を保存して、失敗を含む全終了経路で戻す
//! - 背景遷移（[`InterruptReason::Background`]）と `AVAudioSessionInterruptionNotification`
//!   （[`InterruptReason::AudioInterruption`]）で止め、[`SoundPoll::Interrupted`] を返す
//! - 値の確定直後に `crate::ios::request_repaint` を呼ぶ（契約表 4）
//! - `ios_sound_classification.rs` を `apple_sound_classification::implementation` への委譲に差し替える
//!   （dispatch・共通コア画面・`device.rs`・`build.rs` は変えない）

use std::time::Duration;

/// driver が OS を読み直す間隔。
pub const READ_INTERVAL: Duration = Duration::from_millis(250);

/// 聞き取りの開始から最初の結果が届くまでの期限（ミリ秒）。許可待ちは数えない。
pub const FIRST_RESULT_TIMEOUT_MS: f64 = 5000.0;

/// 結果の番号が進まない時間がこれ（ミリ秒）を超えたら、古い結果を出さない。
pub const STALE_AFTER_MS: f64 = 3000.0;

/// 画面に出す候補の上限件数。
pub const TOP_K: usize = 3;

/// ラベルの上限バイト数。
pub const MAX_LABEL_BYTES: usize = 64;

/// 分類の 1 候補。
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    /// OS の識別子のままのラベル（英語。[`validate_label`] 通過済み）。
    pub label: String,
    /// 信頼度（`0.0..=1.0`、[`validate_confidence`] 通過済み）。
    pub confidence: f64,
}

/// 分類の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct ClassificationResult {
    /// 上位の候補（最大 [`TOP_K`] 件、信頼度の降順。空にはならない）。
    pub top: Vec<Classification>,
    /// 実装がプロセス内で単調増加させる番号。開始し直しても巻き戻さない。
    pub seq: u64,
}

/// 中断の理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptReason {
    /// 背景遷移（iOS）。
    Background,
    /// 音声の割り込み（iOS の `AVAudioSessionInterruptionNotification`）。
    AudioInterruption,
    /// 入力機器・入力形式の変更（macOS・iOS）。
    InputChanged,
}

/// [`SoundClassifier::read`] の成功結果。
#[derive(Debug, Clone, PartialEq)]
pub enum SoundPoll {
    /// 未開始・停止後。
    Idle,
    /// 許可ダイアログの応答待ち。期限に数えない。
    AwaitingPermission,
    /// 聞き取り中で結果が無い。
    Listening,
    /// 最新の分類結果。
    Result(ClassificationResult),
    /// 中断。実装が資源を解放済みで、`stop` まで同じ値を返す。自動では再開しない。
    Interrupted(InterruptReason),
}

/// 環境音の分類の失敗種別。OS 固有のコードは含めない（契約表 8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundFailure {
    /// API が無い・OS 版が足りない（macOS 11 以下を含む）。
    Unsupported,
    /// 利用者または OS 設定がマイクの利用を拒否した。
    PermissionDenied,
    /// 入力機器が無い。
    NoMicrophone,
    /// 上記以外の OS 呼び出し失敗。
    Failed,
}

/// 環境音の分類の契約（モジュール doc の「契約」節）。UI スレッドから同期で呼ばれるため、
/// どのメソッドも短時間で戻ること。
pub trait SoundClassifier: Send + Sync {
    /// 聞き取りを始める。冪等。同期で分かる失敗（非対応・許可拒否・マイクなし）はここで返す。
    fn start(&self) -> Result<(), SoundFailure>;
    /// 現在の状態。直前の値を返し続けない。中断は `stop` まで同じ値を返す。
    fn read(&self) -> Result<SoundPoll, SoundFailure>;
    /// 止めて資源を解放し、保持している結果を捨てる。冪等で失敗しない。実装内の世代を進め、
    /// 以降に処理される OS の応答（許可・解析結果・通知）を捨てる。
    fn stop(&self);
}

/// 対象外 OS・未実装 OS の dispatch 先。ダミーの成功実装へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedSoundClassifier;

impl SoundClassifier for UnsupportedSoundClassifier {
    fn start(&self) -> Result<(), SoundFailure> {
        Err(SoundFailure::Unsupported)
    }
    fn read(&self) -> Result<SoundPoll, SoundFailure> {
        Err(SoundFailure::Unsupported)
    }
    fn stop(&self) {}
}

/// ラベルの検証。空でなく [`MAX_LABEL_BYTES`] 以内で、ASCII の英数字・`_`・`-` だけを通す。
/// OS から来る文字列は信頼しない（OWASP A03）。
pub fn validate_label(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || bytes.len() > MAX_LABEL_BYTES {
        return None;
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
    {
        return None;
    }
    // ASCII だけを通したので UTF-8 として常に妥当。
    String::from_utf8(bytes.to_vec()).ok()
}

/// 信頼度の検証。有限で `0.0..=1.0` のときだけ通す。丸めの前に行う（契約表 5）。
pub fn validate_confidence(raw: f64) -> Option<f64> {
    if raw.is_finite() && (0.0..=1.0).contains(&raw) {
        Some(raw)
    } else {
        None
    }
}

/// 検証済みの信頼度を百分率（0〜100）へ丸める。
pub fn to_percent(c: f64) -> u8 {
    (c * 100.0).round().clamp(0.0, 100.0) as u8
}

/// 候補を検証して上位 [`TOP_K`] 件にまとめる。不正な候補（ラベル・信頼度）は捨て、信頼度の降順に並べる。
/// 全部不正なら空（呼び出し側は [`SoundPoll::Listening`] にする）。
pub fn select_top(candidates: &[(&[u8], f64)]) -> Vec<Classification> {
    let mut valid: Vec<Classification> = candidates
        .iter()
        .filter_map(|(label, conf)| {
            Some(Classification {
                label: validate_label(label)?,
                confidence: validate_confidence(*conf)?,
            })
        })
        .collect();
    valid.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    valid.truncate(TOP_K);
    valid
}

/// [`SoundPoll`] の種別（期限判定に渡す、値を持たない写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollKind {
    /// [`SoundPoll::Idle`]。
    Idle,
    /// [`SoundPoll::AwaitingPermission`]。
    AwaitingPermission,
    /// [`SoundPoll::Listening`]。
    Listening,
    /// [`SoundPoll::Result`]。
    Result,
    /// [`SoundPoll::Interrupted`]。
    Interrupted,
}

impl From<&SoundPoll> for PollKind {
    fn from(p: &SoundPoll) -> Self {
        match p {
            SoundPoll::Idle => PollKind::Idle,
            SoundPoll::AwaitingPermission => PollKind::AwaitingPermission,
            SoundPoll::Listening => PollKind::Listening,
            SoundPoll::Result(_) => PollKind::Result,
            SoundPoll::Interrupted(_) => PollKind::Interrupted,
        }
    }
}

/// 聞き取りの開始から最初の結果までの期限の追跡（時刻は単調時計由来のミリ秒の `f64`）。
/// 1 件届いた後は期限を設けない（以降の欠測は [`StaleTracker`] が扱う）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FirstResultDeadline {
    since: Option<f64>,
    received: bool,
}

impl FirstResultDeadline {
    /// 起点を置く（開始時）。受信済みの記録は消す。
    pub fn begin(&mut self, now_ms: f64) {
        self.since = Some(now_ms);
        self.received = false;
    }

    /// 1 回の読み取り結果を記録し、期限切れなら `true`。
    /// - `Result`: 受信済みにして期限を外す
    /// - `AwaitingPermission`: 起点を消す（許可待ちは数えない。許可後の最初の観測が新しい起点）
    /// - `Interrupted`: 終端なので期限は見ない
    /// - `Idle` / `Listening`: 受信済みなら `false`。未受信なら起点からの経過が
    ///   [`FIRST_RESULT_TIMEOUT_MS`] を超えたときだけ `true`（`Idle` は開始後には出ないはずで、
    ///   出ても結果なしとして数える）
    pub fn observe(&mut self, poll: PollKind, now_ms: f64) -> bool {
        match poll {
            PollKind::Result => {
                self.received = true;
                false
            }
            PollKind::AwaitingPermission => {
                self.since = None;
                false
            }
            PollKind::Interrupted => false,
            PollKind::Idle | PollKind::Listening => {
                if self.received {
                    return false;
                }
                let since = *self.since.get_or_insert(now_ms);
                now_ms - since > FIRST_RESULT_TIMEOUT_MS
            }
        }
    }

    /// 起点と受信記録を捨てる（停止時）。
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// 結果の番号が進まない時間を測り、古い結果を出さないための記憶。
#[derive(Debug, Clone, Copy, Default)]
pub struct StaleTracker {
    last: Option<(u64, f64)>,
}

impl StaleTracker {
    /// 記憶を捨てる。
    pub fn reset(&mut self) {
        self.last = None;
    }

    /// 番号を観測する。進んでいれば真、[`STALE_AFTER_MS`] を超えて進まなければ偽。
    pub fn observe(&mut self, seq: u64, now_ms: f64) -> bool {
        match self.last {
            Some((s, since)) if s == seq => now_ms - since <= STALE_AFTER_MS,
            _ => {
                self.last = Some((seq, now_ms));
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_validation() {
        assert_eq!(validate_label(b""), None);
        assert_eq!(validate_label(&[b'a'; 65]), None);
        assert_eq!(
            validate_label(&[b'a'; 64]).as_deref().map(str::len),
            Some(64)
        );
        assert_eq!(validate_label("音楽".as_bytes()), None);
        assert_eq!(validate_label(b"dog bark"), None);
        assert_eq!(validate_label(b"a/b"), None);
        assert_eq!(validate_label(b"a\n"), None);
        assert_eq!(validate_label(b"\0"), None);
        assert_eq!(validate_label(b"dog_bark-2").as_deref(), Some("dog_bark-2"));
    }

    #[test]
    fn confidence_validation() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.001, 1.001] {
            assert_eq!(validate_confidence(bad), None);
        }
        assert_eq!(validate_confidence(0.0), Some(0.0));
        assert_eq!(validate_confidence(1.0), Some(1.0));
    }

    #[test]
    fn percent_rounds() {
        assert_eq!(to_percent(0.0), 0);
        assert_eq!(to_percent(0.874), 87);
        assert_eq!(to_percent(0.876), 88);
        assert_eq!(to_percent(1.0), 100);
    }

    #[test]
    fn select_top_cuts_to_three_in_descending_order() {
        let c: Vec<(&[u8], f64)> = vec![
            (b"a", 0.1),
            (b"b", 0.9),
            (b"c", 0.5),
            (b"d", 0.7),
            (b"e", 0.3),
        ];
        let top = select_top(&c);
        assert_eq!(
            top.iter().map(|x| x.label.as_str()).collect::<Vec<_>>(),
            ["b", "d", "c"]
        );
    }

    #[test]
    fn select_top_drops_invalid_candidates() {
        let c: Vec<(&[u8], f64)> = vec![
            (b"bad label", 0.99),
            (b"nan", f64::NAN),
            (b"over", 1.5),
            (b"ok", 0.2),
        ];
        let top = select_top(&c);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].label, "ok");
    }

    #[test]
    fn select_top_all_invalid_is_empty() {
        let c: Vec<(&[u8], f64)> = vec![(b"", 0.5), (b"x", -1.0)];
        assert!(select_top(&c).is_empty());
        assert!(select_top(&[]).is_empty());
    }

    #[test]
    fn deadline_expires_only_after_timeout() {
        let mut d = FirstResultDeadline::default();
        d.begin(0.0);
        assert!(!d.observe(PollKind::Listening, FIRST_RESULT_TIMEOUT_MS));
        assert!(d.observe(PollKind::Listening, FIRST_RESULT_TIMEOUT_MS + 1.0));
    }

    #[test]
    fn permission_wait_is_not_counted() {
        let mut d = FirstResultDeadline::default();
        d.begin(0.0);
        assert!(!d.observe(PollKind::AwaitingPermission, 60_000.0));
        assert!(!d.observe(PollKind::Listening, 61_000.0));
        assert!(!d.observe(PollKind::Listening, 61_000.0 + FIRST_RESULT_TIMEOUT_MS));
        assert!(d.observe(PollKind::Listening, 61_001.0 + FIRST_RESULT_TIMEOUT_MS));
    }

    #[test]
    fn no_deadline_after_first_result() {
        let mut d = FirstResultDeadline::default();
        d.begin(0.0);
        assert!(!d.observe(PollKind::Result, 1.0));
        assert!(!d.observe(PollKind::Listening, 999_999.0));
        d.reset();
        assert!(!d.observe(PollKind::Listening, 0.0));
    }

    #[test]
    fn interrupted_never_expires() {
        let mut d = FirstResultDeadline::default();
        d.begin(0.0);
        assert!(!d.observe(PollKind::Interrupted, 999_999.0));
    }

    #[test]
    fn stale_tracker() {
        let mut t = StaleTracker::default();
        assert!(t.observe(1, 0.0));
        assert!(t.observe(1, STALE_AFTER_MS));
        assert!(!t.observe(1, STALE_AFTER_MS + 1.0));
        assert!(t.observe(2, STALE_AFTER_MS + 2.0));
        t.reset();
        assert!(t.observe(2, 9999.0));
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedSoundClassifier;
        assert_eq!(u.start(), Err(SoundFailure::Unsupported));
        assert_eq!(u.read(), Err(SoundFailure::Unsupported));
        u.stop();
    }
}
