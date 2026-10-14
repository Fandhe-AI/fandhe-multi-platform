//! Phase 11（親 #871）型サンプル「ファイル選択」（Issue #872 / #873 / #874）の effect ドライバ。
//! `fandhe_core::screens::file_pick`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # デスクトップ（macOS/Windows）: `FilePicker` トレイト経由の同期呼び出し
//!
//! `NSOpenPanel`/`IFileOpenDialog` はメインスレッド必須かつ同期 API のため、
//! `clipboard_text` と同型: 呼ばれたスレッド（UI スレッド）でそのまま
//! `FilePicker::pick()` をブロッキング呼び出しし、戻ってきたら同じ呼び出しの中で
//! 完了を送る（[`drive_with`]）。固定タイムアウトは掛けない（[`fandhe_device::file_picker::
//! FilePicker`] の契約）。
//!
//! # iOS / Android（案 B）: トレイトを通さずコア状態機械へ直接書き戻す
//!
//! iOS/Android は `FilePicker` トレイトを **通らない**（`fandhe-web-dom::file_picker`
//! と同型）。OS がコールバック型（ダイアログ提示 → 結果を非同期通知）であり、
//! トレイト実装・ワーカースレッド・mailbox・コア状態機械の 3〜4 層管理を作ると、
//! 再生成・プロセス死・遅延配送で継ぎ目がずれるため。読み取り・8 MiB 判定・EOF・
//! 読み取り失敗の扱いは `fandhe_device::file_picker::read_picked` に一本化する
//! （OS 側で読み取りループを書かない）。
//!
//! # pull 型（改訂 4）: `bridge` は sender も generation も持たない
//!
//! [`bridge`] は `device/request_slot.rs::RequestSlot` の facade で、
//! `CompletionSender` も画面の `generation` も保持しない **pull 型**。
//! OS コールバックは結果を置くだけで、画面への反映は毎フレーム
//! [`drive_mobile_with`] が **その時点の** `ProxyApp` の `sender`/`state` を使って行う
//! （`device/tilt.rs` と同じ考え方）。Android の Activity 再生成で `ProxyApp`・画面状態・
//! チャンネルが作り直されても、`bridge` 側が古い sender を握り続けることが無いため
//! 「送ったが反映されない」という継ぎ目が構造的に起きない。完了クロージャは
//! 自分で `finish_pick`/`finish_import` を反映できたことを確認してから
//! [`bridge::clear`] するため、「キュー投入成功 ≠ 反映」の隙間も無い。
//!
//! 固定タイムアウトを掛けないため、OS がコールバックしないケース（シーン破棄等）は
//! `Picking` のまま残る（既知の天井、out-of-scope-tracking.md 対象外の設計上の割り切り）。
//!
//! # `bridge` は `file_pick`/`tsv_import` の 2 画面で共有する
//!
//! `bridge::SLOT` は本モジュールと [`super::tsv_import`] の両方が使う static。
//! `FANDHE_PROXY_APP` で起動する画面は 1 プロセスに 1 つだけのため、同じプロセスで
//! 両方の effect ドライバが動くことは無く、共有しても要求が混線しない（`device/tsv_import.rs`
//! モジュール doc も参照）。`Outcome` を `PickedSummary` ではなく生の
//! `Result<Option<PickedFile>, String>` に留めているのは、`tsv_import` 側は
//! バイト列を解析に使う（要約に潰さない）ため。要約（`PickedSummary` への変換）は
//! 各画面固有の [`PickHooks::finish`] 側で行う。
//!
//! # `PickHooks<S>`: ライフサイクル処理（Restore 分岐）の 1 か所化
//!
//! [`drive_with`]/[`drive_mobile_with`] は画面状態 `S` へのアクセスを [`PickHooks<S>`]
//! （関数ポインタの束）経由で行うジェネリック関数にしてあり、`file_pick`/`tsv_import`
//! はそれぞれ const の hooks を渡すだけにする。Activity 再生成時の Restore 分岐
//! （[`next_action`] 参照）を画面ごとにコピーすると、契約表 1（ライフサイクル境界）が
//! 再発するため（`.claude/rules/device-bridge-contracts.md`）。trait ではなく
//! 関数ポインタの構造体にしているのは、「新しい trait は作らない」という Issue #980 の
//! 要件と紛らわしくならないようにするため。

#[cfg(not(any(target_os = "ios", target_os = "android")))]
use std::sync::OnceLock;

use fandhe_core::screens::file_pick::{
    finish_pick, mark_picking, request_pick, FilePickState, PickPhase, PickedSummary,
};
use fandhe_device::file_picker::{PickedFile, MAX_FILE_BYTES};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// OS のファイル選択ダイアログを使えないときに画面へ出す案内（デスクトップ専用の
/// 経路でのみ使う。モバイルは常に [`bridge`] を通るため到達しない）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub(crate) const UNAVAILABLE: &str = "この環境ではファイル選択ダイアログを使えません";

/// `DeviceError` を画面へ渡す文言へ変換する。サイズ超過だけは上限値（MiB）を
/// 含む案内に差し替える（上限の SSOT は `fandhe_device::file_picker::MAX_FILE_BYTES`
/// のみとし、`fandhe-core` 側に定数を複製しない）。それ以外は `Display` の文言を
/// そのまま転送する（`DeviceError` の内部構造は含まないため security.md A09 に抵触しない）。
fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::InvalidInput(_) => {
            format!(
                "{} MiB を超えるファイルは選べません",
                MAX_FILE_BYTES / (1024 * 1024)
            )
        }
        other => other.to_string(),
    }
}

/// `Result<Option<PickedFile>, DeviceError>` を橋渡し共通の生の結果へ変換する
/// （[`drive_with`]・[`bridge::deliver`] の共通部分）。画面固有の要約
/// （`PickedSummary`/バイト列）への変換は行わない（各画面の [`PickHooks::finish`] が担う）。
fn to_outcome(
    result: Result<Option<PickedFile>, DeviceError>,
) -> Result<Option<PickedFile>, String> {
    result.map_err(to_message)
}

/// `file_pick`/`tsv_import` が画面状態 `S` へのアクセスを渡す関数ポインタの束
/// （モジュール doc「`PickHooks<S>`」節参照）。すべて `fn` ポインタ（クロージャ環境
/// キャプチャ不可）のため `Copy`（`derive` は `S: Copy` を要求してしまうため手実装、
/// モジュール doc 参照）。
pub(crate) struct PickHooks<S> {
    /// 現在の [`PickPhase`]。
    pub(crate) phase: fn(&S) -> PickPhase,
    /// 現在の要求世代番号。
    pub(crate) generation: fn(&S) -> u64,
    /// Restore 分岐（[`Action::RestoreRequested`]）で呼ぶ「要求し直す」ハンドラ
    /// （画面の `request_pick`/`request_import` 相当）。デスクトップ経路
    /// （[`drive_with`]/[`fail_request_with`]）は Restore 分岐を持たないため参照しない
    /// （`drive_mobile_with` 専用、iOS/Android 以外のビルドでは未使用になる）。
    #[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
    pub(crate) request: fn(&mut S),
    /// `Requested` → `Picking` へ進める（画面の `mark_picking` 相当）。
    pub(crate) mark_picking: fn(&mut S),
    /// 完了を反映する（画面の `finish_pick`/`finish_import` 相当）。
    pub(crate) finish: fn(&mut S, u64, Result<Option<PickedFile>, String>),
}

impl<S> Clone for PickHooks<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for PickHooks<S> {}

/// `hooks.phase`/`hooks.generation` から [`take_request`] 相当の判定を行う
/// （デスクトップ経路専用。モバイルは `state.phase` を [`next_action`] へ直接渡す）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn take_request_generic<S>(state: &S, hooks: &PickHooks<S>) -> Option<u64> {
    ((hooks.phase)(state) == PickPhase::Requested).then(|| (hooks.generation)(state))
}

/// 要求を失敗として畳み、画面のボタンを戻す（要求が残ったままだと操作不能になる）。
/// デスクトップ専用（モバイルは [`bridge::deliver`] が失敗経路も兼ねる）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub(crate) fn fail_request_with<S: 'static>(
    state: &S,
    sender: &CompletionSender<S>,
    message: &'static str,
    hooks: PickHooks<S>,
) {
    let Some(generation) = take_request_generic(state, &hooks) else {
        return;
    };
    let _ = sender.send(Box::new(move |s: &mut S| {
        (hooks.finish)(s, generation, Err(message.to_string()));
    }));
}

/// `picker.pick()` を呼び、要求 1 件を処理して完了を送るジェネリック版
/// （デスクトップ専用の同期経路、モジュール doc「デスクトップ」節参照）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub(crate) fn drive_with<S: 'static>(
    state: &S,
    sender: &CompletionSender<S>,
    picker: &dyn fandhe_device::file_picker::FilePicker,
    hooks: PickHooks<S>,
) {
    let Some(generation) = take_request_generic(state, &hooks) else {
        return;
    };

    // `mark_picking` を先に送ってボタンを外す（ダイアログが開いている間の
    // 二重要求を画面側でも防ぐ）。この後の `pick()` はブロッキングで長時間かかりうる。
    let _ = sender.send(Box::new(move |s: &mut S| (hooks.mark_picking)(s)));

    let result = to_outcome(picker.pick());
    // 受信側（ProxyApp）がドロップ済み（アプリ終了中）の送信失敗は無視してよい
    // （coding-rust.md: panic を API 契約にしない、`device/clipboard.rs` と同じ判断）。
    let _ = sender.send(Box::new(move |s: &mut S| {
        (hooks.finish)(s, generation, result);
    }));
}

/// モバイル（iOS / Android）の橋渡し。OS のコールバック型ダイアログを同期 trait に
/// 変換せず、コアの状態機械へ直接書き戻す（`fandhe-web-dom::file_picker` と同型、
/// `fandhe_device::file_picker` モジュール doc の「モバイルは trait を通らない」参照）。
///
/// **pull 型**: ここは `CompletionSender` も画面の `generation` も持たない。OS コールバックは
/// 結果を `ready` に置くだけで、反映は毎フレームの [`drive_mobile_with`] が **その時の**
/// `ProxyApp` の sender で行う（`tilt` と同じ考え方）。Android の Activity 再生成で
/// `ProxyApp`・画面状態・チャンネルが作り直されても、sender が app の寿命をまたがないので
/// 「送ったが反映されない」が構造的に起きない。完了クロージャは自分で反映できたことを
/// 確認してから [`clear`] するため、「キュー投入成功 ≠ 反映」の隙間も無い。
///
/// 状態は `PENDING` の 1 要求ぶんのみ（プロセス内 static、`file_pick`/`tsv_import` 共有
/// モジュール doc「`bridge` は 2 画面で共有する」節参照）。プロセス死で消えるので、
/// OS が再配送した旧結果は `deliver` が捨てる。OS へ渡す要求 ID はプロセス内で一意な
/// 連番 `request_id`（再生成で画面の `generation` が 1 から再開しても衝突しない）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use std::sync::Arc;

    use fandhe_device::file_picker::PickedFile;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    /// 画面へ反映する完了結果（`super::to_outcome` 後の生の形を `Arc` で包んだもの。
    /// 要約への変換は各画面固有の `PickHooks::finish` が担う、モジュール doc 参照）。
    /// `PickedFile`（公開 API、`bytes: Vec<u8>`）自体は変更せず、ここでのみ `Arc` で
    /// 包んで毎フレームの `snapshot` クローンを参照カウント共有に留める
    /// （Issue #980 codex-review 指摘: 公開 API 破壊の回避）。
    pub(crate) type Outcome = Result<Option<Arc<PickedFile>>, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 提示直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を `super::to_outcome` で橋渡し共通の形へ変換し、`Arc` で包んでから
    /// 置く（任意スレッド）。`request_id` 不一致・未登録（プロセス死後の再配送等）・
    /// 既に結果あり（重複通知）は捨てて `false`。再描画の要求は呼び出し元（FFI / JNI
    /// 受け口）がプラットフォーム固有の経路で行う。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<Option<PickedFile>, DeviceError>,
    ) -> bool {
        let outcome = super::to_outcome(result).map(|opt| opt.map(Arc::new));
        SLOT.deliver(request_id, outcome)
    }

    /// 現在の要求と、届いていれば結果（クローン）。各画面の effect ドライバが
    /// 毎フレーム観測する。結果が届いてから `Idle` へ反映されるまでの数フレーム、
    /// この `clone` が毎フレーム走るが、`Outcome` が `PickedFile` を `Arc` で包んでいる
    /// ため参照カウント共有のみで、最大 `MAX_FILE_BYTES`（8 MiB）のバイト列自体は
    /// 複製されない（Issue #980 codex-review 指摘の是正。`PickedFile.bytes` は
    /// `Vec<u8>` のまま、公開 API は変更しない）。
    pub(crate) fn snapshot() -> Option<(u64, Option<Outcome>)> {
        SLOT.snapshot()
    }

    /// 完了クロージャが画面へ反映できたときだけ呼ぶ。`request_id` が現在の要求と一致する
    /// ときのみ消す（反映確認前に新しい要求が始まることは無いが、契約として照合する）。
    pub(crate) fn clear(request_id: u64) {
        SLOT.clear(request_id)
    }
}

/// [`bridge::Outcome`] と同じ形（`bridge` 自体は `cfg(any(ios, android))` のため、
/// `cfg(test)` のみでホストがコンパイルする [`next_action`]/[`Action`] からは
/// 型エイリアスの名前を共有できず、こちらを使う。両者は構造的に同一の型）。
#[cfg(any(test, target_os = "ios", target_os = "android"))]
type MobileOutcome = Result<Option<std::sync::Arc<PickedFile>>, String>;

/// [`next_action`] が受け取る「現在の要求と、届いていれば結果」（[`bridge::snapshot`]
/// の戻り値と同じ形）。
#[cfg(any(test, target_os = "ios", target_os = "android"))]
type MobileSnapshot = Option<(u64, Option<MobileOutcome>)>;

/// [`drive_mobile_with`] の分岐（`state.phase` × [`bridge::snapshot`]）をホスト側でも
/// 単体テストできるよう切り出した純関数の出力。`drive_mobile_with` はこの結果を
/// `CompletionSender` へ写像するだけに徹する（モジュール doc「pull 型」節参照）。
#[cfg(any(test, target_os = "ios", target_os = "android"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// 何もしない。
    Nothing,
    /// 通常の要求開始: 画面を Picking へ進め、OS へ提示を依頼する。
    StartPick,
    /// 要求が残っているのに画面が Idle = Activity 再生成で画面状態だけが作り直された。
    /// 画面を Requested へ戻す（次フレームで `RestorePicking` へ落ちる。ダイアログは
    /// 開き直さない）。
    RestoreRequested,
    /// 復元途中、または再生成直後の 1 フレームにユーザーがタップした場合。どちらも
    /// Picking へ進めるだけで吸収する（OS 側のダイアログは既に出ている）。
    RestorePicking,
    /// 結果が届いている。反映できる状態かクロージャ内で確かめてから `finish` し、
    /// 反映できたときだけ `bridge` を畳む（`request_id` は [`bridge::clear`] へ渡す）。
    Finish {
        request_id: u64,
        outcome: MobileOutcome,
    },
}

/// `(state.phase, bridge::snapshot())` から次に行う操作を決める純関数
/// （[`Action`] doc 参照）。OS 呼び出し・`CompletionSender` へのアクセスを一切含まないため
/// `cfg` 無しでホスト上の `cargo test` から検証できる（4 腕 + 該当なしの `Nothing`）。
#[cfg(any(test, target_os = "ios", target_os = "android"))]
pub(crate) fn next_action(phase: PickPhase, snapshot: MobileSnapshot) -> Action {
    match (phase, snapshot) {
        (PickPhase::Requested, None) => Action::StartPick,
        (PickPhase::Idle, Some(_)) => Action::RestoreRequested,
        (PickPhase::Requested, Some(_)) => Action::RestorePicking,
        (PickPhase::Picking, Some((request_id, Some(outcome)))) => Action::Finish {
            request_id,
            outcome,
        },
        _ => Action::Nothing,
    }
}

/// モバイル版 `drive` のジェネリック版（iOS/Android 共通、毎フレーム・UI スレッドから
/// 呼ばれる契約。モジュール doc「pull 型」節参照）。[`next_action`] が返す [`Action`] を
/// `CompletionSender`/OS 呼び出しへ写像するだけで、状態判定のロジック自体は持たない。
/// ブロッキング待ちを一切行わないため、呼び出し元（`ProxyApp::update`）の UI スレッドから
/// そのまま呼んでよい（`drive_async`/ワーカースレッドは不要）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) fn drive_mobile_with<S: 'static>(
    state: &S,
    sender: &CompletionSender<S>,
    hooks: PickHooks<S>,
) {
    match next_action((hooks.phase)(state), bridge::snapshot()) {
        Action::Nothing => {}
        Action::StartPick => {
            let _ = sender.send(Box::new(move |s: &mut S| (hooks.mark_picking)(s)));
            let request_id = bridge::begin();
            if !present(request_id) {
                // 提示依頼自体が同期的に失敗した（コールバックは二度と来ない）。
                bridge::deliver(
                    request_id,
                    Err(DeviceError::Internal("file_picker present")),
                );
            }
        }
        Action::RestoreRequested => {
            let _ = sender.send(Box::new(move |s: &mut S| (hooks.request)(s)));
        }
        Action::RestorePicking => {
            let _ = sender.send(Box::new(move |s: &mut S| (hooks.mark_picking)(s)));
        }
        Action::Finish {
            request_id,
            outcome,
        } => {
            let _ = sender.send(Box::new(move |s: &mut S| {
                // 反映できる状態か（`Idle` へ既に戻っていないか）をクロージャ内で
                // 確かめてから `finish` し、反映できたときだけ `bridge` を畳む
                // （モジュール doc「pull 型」節の「キュー投入成功 ≠ 反映」対策）。
                if (hooks.phase)(s) != PickPhase::Idle {
                    let generation = (hooks.generation)(s);
                    // `bridge::clear` を先に呼び、`SLOT` 側が握っている参照を先に
                    // 手放す（Issue #980 codex-review 指摘: `SLOT.pending.ready` に
                    // 残ったままだと、ここで持つ `outcome` と合わせて強参照 2 本に
                    // なり `Arc::try_unwrap` が毎回失敗して最大 `MAX_FILE_BYTES`
                    // （8 MiB）を複製する退行になっていた）。反映確認後・所有権を
                    // 剥がす前に畳めば、この時点で `outcome` が唯一の強参照になり
                    // 複製なしで取り出せる。
                    bridge::clear(request_id);
                    // ここで初めて `Arc<PickedFile>` を剥がす（完了 1 回のみ、
                    // 毎フレームの `bridge::snapshot` クローンとは別。`bridge::Outcome`
                    // 参照）。他に参照が残っていなければ複製なしで取り出せる。
                    let owned = outcome.map(|opt| {
                        opt.map(|arc| {
                            std::sync::Arc::try_unwrap(arc).unwrap_or_else(|arc| (*arc).clone())
                        })
                    });
                    (hooks.finish)(s, generation, owned);
                }
            }));
        }
    }
}

/// iOS 提示関数への委譲（`device/ios.rs::present_file_picker`、Issue #873）。
/// `request_id` は [`bridge::begin`] が払い出したプロセス内一意な ID
/// （画面の `generation` ではない、[`bridge`] のモジュール doc 参照）。
#[cfg(target_os = "ios")]
fn present(request_id: u64) -> bool {
    crate::device::ios::present_file_picker(request_id)
}

/// Android 提示関数への委譲（`device/android.rs::request_file_pick`、Issue #874）。
/// `request_id` は [`bridge::begin`] が払い出したプロセス内一意な ID
/// （画面の `generation` ではない、[`bridge`] のモジュール doc 参照）。
#[cfg(target_os = "android")]
fn present(request_id: u64) -> bool {
    crate::device::android::request_file_pick(request_id)
}

/// デスクトップ（macOS/Windows）向け実装を保持する `OnceLock`。`RfdFilePicker` は
/// 内部に `busy` ガードを持つため、呼び出しのたびに作り直さず 1 個だけ使い回す
/// （`clipboard::CLIPBOARD`/`tts::TTS` と同型）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
static PICKER: OnceLock<Result<Box<dyn fandhe_device::file_picker::FilePicker>, String>> =
    OnceLock::new();

/// デスクトップ向けファイル選択実装を返す。macOS/Windows は `RfdFilePicker`、
/// それ以外（Linux 等）は `Unsupported` を返す。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub(crate) fn shared_picker(
) -> Result<&'static dyn fandhe_device::file_picker::FilePicker, &'static str> {
    let cached = PICKER.get_or_init(|| {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            use fandhe_device::file_picker::{FilePicker, RfdFilePicker};
            Ok(Box::new(RfdFilePicker::new()) as Box<dyn FilePicker>)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Err(DeviceError::Unsupported("file_picker").to_string())
        }
    });
    match cached {
        Ok(picker) => Ok(picker.as_ref()),
        // 接続時のエラー文言は内部要因を含み得ないが、他ドライバ（clipboard/tts）と
        // 同じ方針で固定の案内へ畳む（security.md A09）。
        Err(_) => Err(UNAVAILABLE),
    }
}

/// `file_pick` 画面用の [`PickHooks`]（`FilePickState` 版）。`finish` は `PickedFile` を
/// `PickedSummary { name, size }` へ要約してから `finish_pick` へ渡す（本画面の
/// 従来挙動を保つ、[`super::tsv_import::TSV_IMPORT_HOOKS`] と対）。
const FILE_PICK_HOOKS: PickHooks<FilePickState> = PickHooks {
    phase: |s| s.phase,
    generation: |s| s.generation,
    request: request_pick,
    mark_picking,
    finish: |s, generation, result| {
        let summary = result.map(|opt| {
            opt.map(|f| PickedSummary {
                name: f.name,
                size: f.bytes.len(),
            })
        });
        finish_pick(s, generation, summary);
    },
};

/// `registry.rs` の `effect` 区画から `file_pick` 画面用に配線される effect ドライバ
/// （iOS/Android 版、モジュール doc「案 B」参照）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn file_pick_effect(state: &FilePickState, sender: &CompletionSender<FilePickState>) {
    drive_mobile_with(state, sender, FILE_PICK_HOOKS);
}

/// `registry.rs` の `effect` 区画から `file_pick` 画面用に配線される effect ドライバ
/// （デスクトップ版）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn file_pick_effect(state: &FilePickState, sender: &CompletionSender<FilePickState>) {
    match shared_picker() {
        Ok(picker) => drive_with(state, sender, picker, FILE_PICK_HOOKS),
        Err(message) => fail_request_with(state, sender, message, FILE_PICK_HOOKS),
    }
}

#[cfg(test)]
#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod tests {
    use super::*;
    use fandhe_device::file_picker::FilePicker;

    fn sender_with_channel() -> (
        CompletionSender<FilePickState>,
        std::sync::mpsc::Receiver<crate::app::Completion<FilePickState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    /// 常に指定した結果を返すモック（GUI 無しで `drive_with` を検証する）。
    struct MockPicker(Result<Option<PickedFile>, DeviceError>);

    impl FilePicker for MockPicker {
        fn pick(&self) -> Result<Option<PickedFile>, DeviceError> {
            match &self.0 {
                Ok(v) => Ok(v.clone()),
                Err(DeviceError::Unsupported(s)) => Err(DeviceError::Unsupported(s)),
                Err(DeviceError::InvalidInput(s)) => Err(DeviceError::InvalidInput(s)),
                Err(DeviceError::Internal(s)) => Err(DeviceError::Internal(s)),
                Err(_) => Err(DeviceError::Internal("file_picker test")),
            }
        }
    }

    fn apply_all(
        state: &mut FilePickState,
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<FilePickState>>,
    ) {
        while let Ok(apply) = rx.try_recv() {
            apply(state);
        }
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let state = FilePickState::default();
        drive_with(&state, &sender, &MockPicker(Ok(None)), FILE_PICK_HOOKS);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drive_reflects_success_as_status_with_name_and_size() {
        let (sender, rx) = sender_with_channel();
        let mut state = FilePickState::default();
        request_pick(&mut state);

        drive_with(
            &state,
            &sender,
            &MockPicker(Ok(Some(PickedFile {
                name: "a.txt".to_string(),
                bytes: vec![0u8; 3],
            }))),
            FILE_PICK_HOOKS,
        );
        apply_all(&mut state, &rx);

        assert!(state.status.contains("a.txt"));
        assert!(state.status.contains('3'));
        assert_eq!(state.error, "");
    }

    #[test]
    fn drive_reflects_cancel_without_success_wording() {
        let (sender, rx) = sender_with_channel();
        let mut state = FilePickState::default();
        request_pick(&mut state);

        drive_with(&state, &sender, &MockPicker(Ok(None)), FILE_PICK_HOOKS);
        apply_all(&mut state, &rx);

        assert!(!state.status.contains("選びました"));
        assert_eq!(state.error, "");
    }

    #[test]
    fn drive_translates_invalid_input_to_size_message() {
        let (sender, rx) = sender_with_channel();
        let mut state = FilePickState::default();
        request_pick(&mut state);

        drive_with(
            &state,
            &sender,
            &MockPicker(Err(DeviceError::InvalidInput("file_picker size"))),
            FILE_PICK_HOOKS,
        );
        apply_all(&mut state, &rx);

        assert!(state.error.contains("MiB"));
        assert_eq!(state.status, "");
    }

    #[test]
    fn fail_request_reports_unavailable_without_success_wording() {
        let (sender, rx) = sender_with_channel();
        let mut state = FilePickState::default();
        request_pick(&mut state);

        fail_request_with(&state, &sender, UNAVAILABLE, FILE_PICK_HOOKS);
        apply_all(&mut state, &rx);

        assert_eq!(state.error, format!("失敗しました: {UNAVAILABLE}"));
        assert_eq!(state.status, "");
    }

    #[test]
    fn fail_request_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let state = FilePickState::default();
        fail_request_with(&state, &sender, UNAVAILABLE, FILE_PICK_HOOKS);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drive_is_noop_while_already_picking() {
        // `take_request_generic` は `Picking` の間 `None` を返す（画面側の二重要求防止）。
        let (sender, rx) = sender_with_channel();
        let state = FilePickState {
            phase: fandhe_core::screens::file_pick::PickPhase::Picking,
            generation: 1,
            ..Default::default()
        };
        drive_with(&state, &sender, &MockPicker(Ok(None)), FILE_PICK_HOOKS);
        assert!(rx.try_recv().is_err());
    }
}

/// [`next_action`]（改訂 4、pull 型）の単体テスト。OS 側実装や `CompletionSender` を
/// 一切使わない純関数のため `cfg` 無しでホスト上の `cargo test` から検証できる
/// （モジュール doc「pull 型」節参照）。4 腕（`StartPick`/`RestoreRequested`/
/// `RestorePicking`/`Finish`）+ 該当なし（`Nothing`）を固定する。
#[cfg(test)]
mod next_action_tests {
    use super::{next_action, Action};
    use fandhe_core::screens::file_pick::PickPhase;
    use fandhe_device::file_picker::PickedFile;

    #[test]
    fn requested_without_pending_starts_pick() {
        assert_eq!(next_action(PickPhase::Requested, None), Action::StartPick);
    }

    #[test]
    fn idle_with_pending_restores_to_requested() {
        // Activity 再生成で画面状態だけが作り直された（要求は `bridge` に残っている）。
        assert_eq!(
            next_action(PickPhase::Idle, Some((1, None))),
            Action::RestoreRequested
        );
    }

    #[test]
    fn requested_with_pending_restores_to_picking() {
        // 復元途中、または再生成直後の 1 フレームにユーザーがタップした場合。
        assert_eq!(
            next_action(PickPhase::Requested, Some((1, None))),
            Action::RestorePicking
        );
    }

    #[test]
    fn picking_with_ready_result_finishes() {
        let outcome = Ok(Some(std::sync::Arc::new(PickedFile {
            name: "a.txt".to_string(),
            bytes: vec![1, 2, 3],
        })));
        assert_eq!(
            next_action(PickPhase::Picking, Some((7, Some(outcome.clone())))),
            Action::Finish {
                request_id: 7,
                outcome,
            }
        );
    }

    #[test]
    fn other_combinations_do_nothing() {
        // 該当なし: Idle かつ要求なし、Picking かつ未着（＝ダイアログ表示中）、
        // Requested かつ結果あり（あり得ないが安全側で `Nothing`）は該当しない。
        assert_eq!(next_action(PickPhase::Idle, None), Action::Nothing);
        assert_eq!(
            next_action(PickPhase::Picking, Some((1, None))),
            Action::Nothing
        );
        assert_eq!(next_action(PickPhase::Picking, None), Action::Nothing);
    }
}

/// [`bridge`] の単体テスト。要求の追跡そのもの（採番・first-wins・照合）は
/// `device/request_slot.rs::RequestSlot` 側で検証済みのため、ここでは facade が
/// 担う「`deliver` 境界での `to_outcome` 変換」だけを固定する（Issue #943 で
/// 3 機能の `bridge_tests` を統合、`share.rs`/`biometric_auth.rs` も同型）。
/// OS 側実装（`.m`/JNI）のビルドを要さず host 上の `cargo test` で回せる。
/// iOS/Android ターゲット向けにコンパイルされるときのみ有効（host 上の
/// `cargo test` ではコンパイル対象外になる既知の制約。実 OS コールバック経路の検証は
/// ターゲットビルド確認で担保する）。
#[cfg(test)]
#[cfg(any(target_os = "ios", target_os = "android"))]
mod bridge_tests {
    use super::bridge;
    use fandhe_device::file_picker::PickedFile;

    #[test]
    fn deliver_converts_raw_result_via_to_outcome() {
        let request_id = bridge::begin();
        assert!(bridge::deliver(
            request_id,
            Ok(Some(PickedFile {
                name: "a.txt".to_string(),
                bytes: vec![1, 2, 3]
            }))
        ));
        let (_, ready) = bridge::snapshot().expect("begin 済みなので Some");
        let picked = ready
            .expect("deliver 済みなので Some")
            .expect("成功のはず")
            .expect("選択されたはず");
        assert_eq!(picked.name, "a.txt");
        assert_eq!(picked.bytes.to_vec(), vec![1, 2, 3]);
        bridge::clear(request_id);
    }
}
