//! Phase 11 の拡充ループ（親 #170、Issue #835）: OS 管理のバックグラウンド送信の契約。
//! `fandhe-egui-proxy` の `device/background_upload.rs`（effect ドライバ）が [`BackgroundUpload`] を呼び、
//! 各 OS 実装（macOS: background `NSURLSession` = #835、iOS = #836、Android = #837）が本トレイトを実装する。
//! アプリが作る固定内容のファイル 1 つ（[`FILE_SIZE`] バイト）の送信を OS に依頼し、アプリが閉じても OS が続ける。
//! `background_download`（受信）の逆向きで、`http-get`（前面の GET 1 回）・`background_task`（短時間の処理）とは別機能。
//!
//! 範囲は「開始」「取り消し」「状態の読み出し」「状態が変わった合図の登録」のみ。再開・複数ファイル・
//! 一時停止・URL やファイルの選択は扱わない。送るのはアプリが作る [`file_contents`] だけで、利用者のデータと
//! サーバー由来のファイル名は使わない（A01）。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 完了: HTTP が 2xx、送信バイト数が [`FILE_SIZE`] と一致、リダイレクトされていない、の全てが揃ったときだけ
//!   [`UploadStatus::Completed`]（判定は [`completion_status`] を全 OS で共有する）
//! - 取り消し: 利用者の操作（Apple の `NSURLErrorCancelled`、Android の `cancelUniqueWork` 後）は
//!   [`UploadStatus::Cancelled`]。失敗にしない
//! - 失敗: 2xx でない・送信バイト数の不一致・接続の失敗・ファイルを用意できない・リダイレクト（OS が必ず追うため阻止できず、完了時に OS の計測（経由回数。最終 URL の一致は証拠にしない）で検出する）は
//!   [`UploadStatus::Failed`]（理由は [`UploadFailure`]。固定の種別で、URL・パス・OS の生の文言を持たない。A09）
//! - 非対応・初期化の失敗: `Err(Unsupported("background upload"))`。ダミーの成功実装へ倒さない
//! - 読み出しの失敗: `status` が `Err`。`Idle` に畳まない
//!
//! # OS を正にする
//!
//! 送信中か・進み具合・結果をアプリ内に二重に持たない。[`BackgroundUpload::status`] は呼ぶたびに OS の一覧
//! （Apple は `getAllTasks`、Android は `getWorkInfosForUniqueWork`）を読む。OS の通知は
//! [`BackgroundUpload::set_signal`] で登録した [`OnSignal`] を呼ぶ「読み直せ」の合図にだけ使い、状態を運ばない。
//! 事前に通信できるかを判定せず、周期のタイマーで状態を推測しない。Apple は OS の一覧から消えた完了の結果だけを
//! 1 枠持ち、OS の `taskIdentifier` で照合する。
//!
//! # 世代（契約表 2）・期限（契約表 3）
//!
//! 置き換え・取り消しの後に届いた古い転送の通知は、OS が採番する識別子で照合して捨てる。転送そのものの期限は
//! 設けず OS の既定に任せる（取り消しで止められる）。経過時間は測らない。OS の列挙を待つ実装は単調時計の
//! 期限で待つ（壁時計を使わない）。
//!
//! # 送信先の制限と残る範囲（A01・A10）
//!
//! background session はリダイレクトを OS が自動で追い、アプリは送信前に止められない。そのため送信先の
//! 制限は「開始時の `check_url`」と「送るのは [`file_contents`] だけ（利用者のデータ・秘密を含まない）」で
//! 担保し、転送先への送信そのものは防げない範囲として残す。経由を検出したら必ず `Failed` にする。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! `start` と `cancel` は呼び出し側が 1 本の直列キューから呼ぶ前提（ドライバが専用スレッドに集約する）。
//!
//! 守る: 1 プロセス内の開始・取り消し・完了の順序、置き換え・取り消しの後に届いた古い転送の通知を捨てること、
//! 嘘の成功の禁止、背景起動の完了通知を 1 回だけ返すこと（iOS）。
//!
//! 守らない範囲: Apple でアプリが終わっている間に完了した転送の結果（OS の一覧から消えるため失われる）、
//! Android で WorkManager が消した古い結果、iOS で利用者がマルチタスク画面から終了したときの取り消し
//! （OS が行う）、接続できない間は OS が待ち続け待機中・送信中のままになりうること、複数プロセス。

use std::sync::Arc;

use crate::DeviceError;

/// 送るファイルの名前（アプリのキャッシュ内の固定名）。
pub const FILE_NAME: &str = "background-upload";

/// 送るファイルの大きさ（1 MiB）。完了の判定（送信バイト数の一致）に使う。
pub const FILE_SIZE: u64 = 1024 * 1024;

/// ファイルを用意できないことを表す `DeviceError::Internal` の識別子。ドライバが `FileUnavailable` へ写す。
pub const FILE_ERROR: &str = "background upload file";

/// 送るファイルの中身。長さ [`FILE_SIZE`]、`i % 251` の繰り返し。利用者のデータを含まない。
/// Android など Rust から渡さない実装も、同じ式で同じ中身を作る。
pub fn file_contents() -> Vec<u8> {
    (0..FILE_SIZE).map(|i| (i % 251) as u8).collect()
}

/// 失敗の種別。表示用の固定文は画面（`fandhe-core`）側が持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadFailure {
    /// HTTP の状態が 2xx でない。
    Http(u16),
    /// 送信バイト数がファイルの大きさと違う。
    SizeMismatch {
        /// 送信できたバイト数。
        sent: u64,
        /// 期待したバイト数（[`FILE_SIZE`]）。
        expected: u64,
    },
    /// 接続の失敗。
    Network,
    /// ファイルを用意できない。
    FileUnavailable,
    /// リダイレクトされた（background session は OS が必ず追う。完了時に検出して失敗にする）。
    RedirectNotAllowed,
    /// その他（HTTP の状態が無いのに成功扱いになった等）。
    Other,
}

/// 送信の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadStatus {
    /// 何も無い（未開始、または OS の一覧に転送が無い）。
    Idle,
    /// 待機中（OS が接続や実行を待っている）。
    Waiting,
    /// 送信中。
    Sending {
        /// 送信済みバイト数。
        sent: u64,
        /// 総バイト数（不明なら `None`）。
        total: Option<u64>,
    },
    /// 完了。
    Completed {
        /// 送信したバイト数。
        bytes: u64,
        /// 受け口が返した HTTP の状態。
        http_status: u16,
    },
    /// 利用者の取り消し。失敗ではない。
    Cancelled,
    /// 失敗。
    Failed(UploadFailure),
}

/// 状態が変わった合図。呼ばれたら呼び出し側は [`BackgroundUpload::status`] を読み直す。
/// OS のコールバックスレッドから呼ばれうる。
pub type OnSignal = Arc<dyn Fn() + Send + Sync>;

/// OS 管理のバックグラウンド送信。
pub trait BackgroundUpload: Send + Sync {
    /// 送信を開始する（置き換え）。最初に [`crate::http::check_url`] を通す。前の転送を取り消してから
    /// ファイルを書き、新しい転送を OS に依頼する。
    fn start(&self, url: &str) -> Result<(), DeviceError>;
    /// 送信を取り消す。冪等（何もなくても `Ok`）。
    fn cancel(&self) -> Result<(), DeviceError>;
    /// 最新状態。呼ぶたびに OS の一覧を読む。`Err` は読み出しの失敗で、`Idle` と混同しない。
    fn status(&self) -> Result<UploadStatus, DeviceError>;
    /// 状態が変わったときに呼ぶ合図を登録する。後から登録したものが勝ち、プロセスの寿命の間保持する。
    fn set_signal(&self, on_signal: OnSignal) -> Result<(), DeviceError>;
}

/// 非対応 OS 向け。4 メソッドとも `Unsupported` を返し、成功を装わない。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedBackgroundUpload;

impl BackgroundUpload for UnsupportedBackgroundUpload {
    fn start(&self, _url: &str) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("background upload"))
    }
    fn cancel(&self) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("background upload"))
    }
    fn status(&self) -> Result<UploadStatus, DeviceError> {
        Err(DeviceError::Unsupported("background upload"))
    }
    fn set_signal(&self, _on_signal: OnSignal) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("background upload"))
    }
}

/// 完了した転送を判定する純関数（全 OS で共有。契約表 6「嘘の成功」）。
/// 順に、リダイレクトされていれば `RedirectNotAllowed`、HTTP の状態が無ければ `Other`
/// （受信と違い、状態の無い成功は認めない）、2xx でなければ `Http`、送信バイト数が [`FILE_SIZE`] と
/// 違えば `SizeMismatch`、全て通れば `Completed`。
pub fn completion_status(http_status: Option<u16>, sent: u64, redirected: bool) -> UploadStatus {
    if redirected {
        return UploadStatus::Failed(UploadFailure::RedirectNotAllowed);
    }
    let Some(code) = http_status else {
        return UploadStatus::Failed(UploadFailure::Other);
    };
    if !(200..300).contains(&code) {
        return UploadStatus::Failed(UploadFailure::Http(code));
    }
    if sent != FILE_SIZE {
        return UploadStatus::Failed(UploadFailure::SizeMismatch {
            sent,
            expected: FILE_SIZE,
        });
    }
    UploadStatus::Completed {
        bytes: sent,
        http_status: code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        let d = UnsupportedBackgroundUpload;
        assert!(matches!(
            d.start("https://x/"),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(d.cancel(), Err(DeviceError::Unsupported(_))));
        assert!(matches!(d.status(), Err(DeviceError::Unsupported(_))));
        assert!(matches!(
            d.set_signal(Arc::new(|| {})),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn completion_requires_every_condition() {
        assert_eq!(
            completion_status(Some(200), FILE_SIZE, false),
            UploadStatus::Completed {
                bytes: FILE_SIZE,
                http_status: 200
            }
        );
        assert_eq!(
            completion_status(Some(404), FILE_SIZE, false),
            UploadStatus::Failed(UploadFailure::Http(404))
        );
        assert_eq!(
            completion_status(Some(200), 10, false),
            UploadStatus::Failed(UploadFailure::SizeMismatch {
                sent: 10,
                expected: FILE_SIZE
            })
        );
        assert_eq!(
            completion_status(None, FILE_SIZE, false),
            UploadStatus::Failed(UploadFailure::Other)
        );
        // リダイレクトされた転送は、応答が 2xx でも成功にしない。
        assert_eq!(
            completion_status(Some(200), FILE_SIZE, true),
            UploadStatus::Failed(UploadFailure::RedirectNotAllowed)
        );
    }

    #[test]
    fn file_contents_is_fixed_and_deterministic() {
        let a = file_contents();
        assert_eq!(a.len() as u64, FILE_SIZE);
        assert_eq!(a, file_contents());
        assert_eq!(a[0], 0);
        assert_eq!(a[251], 0);
        assert_eq!(a[250], 250);
    }
}
