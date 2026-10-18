//! Phase 11 の拡充ループ（親 #100、Issue #390）: OS 管理のバックグラウンドダウンロードの契約。
//! `fandhe-egui-proxy` の `device/background_download.rs`（effect ドライバ）が [`BackgroundDownload`] を
//! 呼び、各 OS 実装（macOS: background `NSURLSession` = #390、iOS・Android・Windows・Web は後続の子）が
//! 本トレイトを実装する。ファイル 1 つの転送を OS に依頼し、アプリが閉じても OS が続ける。
//! `http-get`（前面の GET 1 回）や `background_task`（短時間の処理）とは別機能。
//!
//! 範囲は「開始」「取り消し」「状態の読み出し」のみ。再開・複数ファイル・一時停止・URL 入力・
//! 完了通知・背景で失敗したときの理由の保存は扱わない。保存先はアプリのキャッシュ内の固定名
//! 1 ファイル（[`FILE_NAME`]）で、サーバー由来のファイル名は使わない（A01）。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 利用者の取り消し（Apple の `NSURLErrorCancelled`、Android の `remove` 後、BITS の `Cancel`、
//!   Web の abort）は [`DownloadStatus::Cancelled`]。失敗にしない
//! - HTTP エラー・0 バイト・接続失敗・リダイレクト先の不許可は [`DownloadStatus::Failed`]
//!   （判定は [`completion_status`] を全 OS で共有する）
//! - 非対応・初期化の失敗は `Err(Unsupported("background download"))`。ダミーの成功実装へ倒さない
//! - `Failed` の文言は短い固定文にし、URL・パス・OS の生のエラー文を入れない（A09）
//!
//! # 世代（契約表 2）・期限（契約表 3）
//!
//! `start` ごとに実装が転送の世代を採番し、置き換え後や取り消し後に届いた古い転送の通知は
//! 世代で照合して捨てる（判定と更新は同じロックの内側）。期限は設けず OS の既定に任せる
//! （取り消しで止められる）。経過時間は測らない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の開始・取り消し・完了の順序、失敗を成功値へ畳まないこと、一時ファイルと
//! ジョブの解放。二重開始は「置き換え」（前の転送を取り消し、前回のファイルを消してから開始）。
//!
//! 守らない範囲: リダイレクトは OS が自動で追う（Apple は完了時に最終 URL を [`crate::http::check_url`]
//! で検証し直し、外れていればファイルを捨てて `Failed`）。背景で失敗した理由と前回の完了表示は
//! 再起動で失われる（`status` は参考値。再起動後は再接続できた転送を `Running`、それ以外は `Idle`）。
//! 利用者の強制終了時の挙動は OS に従う（iOS のスワイプ終了は取り消し）。複数プロセス・複数タブ。
//! ポーリング間隔ぶんの表示の遅れ。接続できない間に失敗を確定するかは OS に従う（Apple の background session は
//! 接続を待ち続けるため `Running` のままになりうる。利用者は取り消しで止める）。

use crate::DeviceError;

/// 保存先のファイル名（アプリのキャッシュ内の固定名）。
pub const FILE_NAME: &str = "background-download";

/// 転送の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadStatus {
    /// 未開始（再起動後で再接続できなかった場合を含む）。
    Idle,
    /// 転送中。
    Running {
        /// 受信済みバイト数。
        received: u64,
        /// 総バイト数（不明なら `None`）。
        total: Option<u64>,
    },
    /// 完了。
    Completed {
        /// 保存したファイルのバイト数。
        bytes: u64,
    },
    /// 利用者の取り消し。失敗ではない。
    Cancelled,
    /// 失敗。固定の短い文言（URL・パス・OS の生の文言を含めない）。
    Failed(String),
}

/// OS 管理のバックグラウンドダウンロード。
pub trait BackgroundDownload: Send + Sync {
    /// 転送を開始する（置き換え）。最初に [`crate::http::check_url`] を通し、前回のファイルを消す。
    fn start(&self, url: &str) -> Result<(), DeviceError>;
    /// 転送を取り消し、途中まで落としたファイルと完了済みファイルを消す。冪等（何もなくても `Ok`）。
    fn cancel(&self) -> Result<(), DeviceError>;
    /// 最新状態。`Err` は読み出しの失敗で、`Idle` と混同しない。
    fn status(&self) -> Result<DownloadStatus, DeviceError>;
}

/// 非対応 OS 向け。3 メソッドとも `Unsupported` を返し、成功を装わない。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedBackgroundDownload;

impl BackgroundDownload for UnsupportedBackgroundDownload {
    fn start(&self, _url: &str) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("background download"))
    }
    fn cancel(&self) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("background download"))
    }
    fn status(&self) -> Result<DownloadStatus, DeviceError> {
        Err(DeviceError::Unsupported("background download"))
    }
}

/// 完了した転送を判定する純関数（全 OS で共有。契約表 6「嘘の成功」）。HTTP の状態が 2xx でない、
/// または 0 バイトなら `Failed`。HTTP の状態を持たない OS は `None` を渡しバイト数だけで判定する。
pub fn completion_status(http_status: Option<u16>, bytes: u64) -> DownloadStatus {
    match http_status {
        Some(code) if !(200..300).contains(&code) => DownloadStatus::Failed(format!("HTTP {code}")),
        _ if bytes == 0 => DownloadStatus::Failed("空のファイルです".to_string()),
        _ => DownloadStatus::Completed { bytes },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        let d = UnsupportedBackgroundDownload;
        assert!(matches!(
            d.start("https://x/"),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(d.cancel(), Err(DeviceError::Unsupported(_))));
        assert!(matches!(d.status(), Err(DeviceError::Unsupported(_))));
    }

    #[test]
    fn completion_status_checks_http_and_size() {
        assert_eq!(
            completion_status(Some(404), 40),
            DownloadStatus::Failed("HTTP 404".into())
        );
        assert!(matches!(
            completion_status(Some(500), 40),
            DownloadStatus::Failed(_)
        ));
        assert!(matches!(
            completion_status(Some(200), 0),
            DownloadStatus::Failed(_)
        ));
        assert_eq!(
            completion_status(Some(200), 40),
            DownloadStatus::Completed { bytes: 40 }
        );
        assert_eq!(
            completion_status(None, 40),
            DownloadStatus::Completed { bytes: 40 }
        );
        assert!(matches!(
            completion_status(None, 0),
            DownloadStatus::Failed(_)
        ));
    }
}
