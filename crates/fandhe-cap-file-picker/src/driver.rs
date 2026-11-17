//! Phase 11（親 #871）型サンプル「ファイル選択」（Issue #872 / #873 / #874）の effect ドライバ。
//! `fandhe_core::screens::file_pick`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`fandhe_runtime::EffectFn` 参照）。
//!
//! 非 wasm32 でのみ使われる（Web は `fandhe-web-dom::file_picker` が別経路）。入口は [`platform_file_picker`]（デスクトップの同期経路）と
//! `file_pick_effect`（画面レジストリへ配線する effect）。
//!
//! # デスクトップ（macOS/Windows）: `FilePicker` トレイト経由の同期呼び出し
//!
//! `NSOpenPanel`/`IFileOpenDialog` はメインスレッド必須かつ同期 API のため、
//! `clipboard_text` と同型: 呼ばれたスレッド（UI スレッド）でそのまま
//! `FilePicker::pick()` をブロッキング呼び出しし、戻ってきたら同じ呼び出しの中で
//! 完了を送る（[`drive_with`]）。固定タイムアウトは掛けない（[`crate::contract::FilePicker`] の契約）。
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
//! [`bridge`] は `fandhe-runtime/src/request_slot.rs::RequestSlot` の facade で、
//! `CompletionSender` も画面の `generation` も保持しない **pull 型**。
//! OS コールバックは結果を置くだけで、画面への反映は毎フレーム
//! [`drive_mobile_with`] が **その時点の** アプリ（描画層）の `sender`/`state` を使って行う
//! （`fandhe-cap-tilt-motion` と同じ考え方）。Android の Activity 再生成で `ProxyApp`・画面状態・
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
//! `bridge::SLOT` は本モジュールと [`crate::tsv_import`] の両方が使う static。
//! `FANDHE_PROXY_APP` で起動する画面は 1 プロセスに 1 つだけのため、同じプロセスで
//! 両方の effect ドライバが動くことは無く、共有しても要求が混線しない（`tsv_import.rs`
//! モジュール doc も参照）。`Outcome` を `PickedSummary` ではなく生の
//! `Result<Option<Arc<PickedFile>>, FilePickerError>` に留めているのは、`tsv_import` 側は
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

use crate::error::FilePickerError;
use fandhe_core::screens::file_pick::{
    FilePickState, PickPhase, PickedSummary, finish_pick, mark_picking, request_pick,
};
use fandhe_device::file_picker::{MAX_FILE_BYTES, PickedFile};

use fandhe_runtime::CompletionSender;

/// OS のファイル選択ダイアログを使えないときに画面へ出す案内（デスクトップ専用の
/// 経路でのみ使う。モバイルは常に [`bridge`] を通るため到達しない）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub(crate) const UNAVAILABLE: &str = "この環境ではファイル選択ダイアログを使えません";

/// `FilePickerError` を画面へ渡す文言へ変換する。サイズ超過だけは上限値（MiB）を
/// 含む案内に差し替える（上限の SSOT は `fandhe_device::file_picker::MAX_FILE_BYTES`
/// のみとし、`fandhe-core` 側に定数を複製しない）。それ以外は `Display` の文言を
/// そのまま転送する（`FilePickerError` の内部構造は含まないため security.md A09 に抵触しない）。
fn to_message(e: FilePickerError) -> String {
    match e {
        FilePickerError::InvalidInput(_) => {
            format!(
                "{} MiB を超えるファイルは選べません",
                MAX_FILE_BYTES / (1024 * 1024)
            )
        }
        other => other.to_string(),
    }
}

/// `Result<Option<PickedFile>, FilePickerError>` を橋渡し共通の生の結果へ変換する
/// （デスクトップの [`drive_with`] 用。モバイルの [`bridge`] は変換前の生の結果を slot に置く）。画面固有の要約
/// （`PickedSummary`/バイト列）への変換は行わない（各画面の [`PickHooks::finish`] が担う）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn to_outcome(
    result: Result<Option<PickedFile>, FilePickerError>,
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
    picker: &dyn crate::contract::FilePicker,
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
    // （coding-rust.md: panic を API 契約にしない、`fandhe-cap-clipboard` の `driver.rs` と同じ判断）。
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
///
/// 画面 effect と公開入口（[`begin_file_pick`] / [`poll_file_pick`]）が同じ slot を共有する。
/// 持ち主（[`bridge::SCREEN_OWNER`]・[`bridge::PUBLIC_OWNER`]）で自分の分の要求を区別し、片方が進行中・結果未取得のとき
/// 他方は置き換えず busy で断る。slot には変換前の生の結果（`FilePickerError`）を置き、画面へ渡すときに文言へ変換する。
///
/// テストでホストからも検証できるよう `cfg(test)` でもコンパイルする（OS 呼び出しは含まない）。
#[cfg(any(test, target_os = "ios", target_os = "android"))]
#[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
pub(crate) mod bridge {
    use std::sync::{Arc, Mutex};

    use crate::error::FilePickerError;
    use fandhe_device::file_picker::PickedFile;

    use fandhe_runtime::request_slot::{RequestSlot, SlotOwner};

    /// 公開入口が追う自分の分の要求（画面側が置き換え・横取りしない）。
    pub(crate) static PUBLIC_OWNER: SlotOwner = SlotOwner::new();
    /// 画面 effect が追う自分の分の要求（公開入口と slot を共有するため区別する）。
    pub(crate) static SCREEN_OWNER: SlotOwner = SlotOwner::new();
    /// 公開側の poll 本体（持ち主の照合・取得・喪失判定・解放）を直列にする。並行 poll で成功と喪失が二重に返らないため。
    static PUBLIC_POLL_LOCK: Mutex<()> = Mutex::new(());

    /// 画面へ反映する完了結果（OS からの生の結果を `Arc` で包んだもの。
    /// 要約への変換は各画面固有の `PickHooks::finish` が担う、モジュール doc 参照）。
    /// `PickedFile`（公開 API、`bytes: Vec<u8>`）自体は変更せず、ここでのみ `Arc` で
    /// 包んで毎フレームの `snapshot` クローンを参照カウント共有に留める
    /// （Issue #980 codex-review 指摘: 公開 API 破壊の回避）。
    pub(crate) type Outcome = Result<Option<Arc<PickedFile>>, FilePickerError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 画面の要求を登録する。公開入口の進行中・未取得の要求があれば置き換えず `None`（busy）。
    pub(crate) fn begin_screen() -> Option<u64> {
        SCREEN_OWNER.begin_keeping_foreign(&SLOT)
    }

    /// 公開入口の要求を登録する。進行中の要求と自分の未取得の結果があれば `None`（busy）。画面側の届いた未取得の結果は置き換える
    /// （画面の effect が回らず取り出されないままでも公開入口が詰まらないため。置き換えられた画面の poll は lost の失敗になる。結果の到着から poll までの間に呼ばれても置き換え、画面の本物の成功結果が lost の失敗として報告される。成功には化けない）。
    pub(crate) fn begin_public() -> Option<u64> {
        PUBLIC_OWNER.begin_excluding(&SLOT, &[&PUBLIC_OWNER])
    }

    /// 公開入口の提示依頼が同期的に失敗したときに、登録を取り消す。
    pub(crate) fn abort_public(request_id: u64) {
        SLOT.clear(request_id);
        PUBLIC_OWNER.release(request_id);
    }

    /// 公開入口の結果を取り出す（[`super::poll_file_pick`] の本体）。届いていれば 1 度だけ `Some`、未着・旧 id・公開入口でない id は `None`。
    /// slot の要求が失われたとき（reset 等）は、1 度だけ `Err(Internal("file_picker lost"))`、以後は `None`。
    /// 照合から解放までを直列にし、並行 poll でも成功と喪失が二重に返らない。
    pub(crate) fn poll_public(
        request_id: u64,
    ) -> Option<Result<Option<PickedFile>, FilePickerError>> {
        let _guard = PUBLIC_POLL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if request_id == 0 || PUBLIC_OWNER.current() != request_id {
            return None;
        }
        if let Some(result) = SLOT.take_ready(request_id) {
            PUBLIC_OWNER.release(request_id);
            // 取り出した値が唯一の所有者のため、複製せず剥がす（共有が残っていれば複製へ倒す）。
            return Some(
                result.map(|opt| opt.map(|a| Arc::try_unwrap(a).unwrap_or_else(|a| (*a).clone()))),
            );
        }
        if SLOT.snapshot().is_some_and(|(id, _)| id == request_id) {
            return None;
        }
        PUBLIC_OWNER.release(request_id);
        Some(Err(FilePickerError::Internal("file_picker lost")))
    }

    /// 公開入口の自分の分の要求を捨てる（[`super::discard_file_pick`] の本体）。持ち主が `request_id` と一致するときだけ
    /// slot を空にして持ち主を外し `true`。他人の id・取り出し済み・0 は `false`。poll と同じロックで直列にする。
    pub(crate) fn discard_public(request_id: u64) -> bool {
        let _guard = PUBLIC_POLL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if request_id == 0 || PUBLIC_OWNER.current() != request_id {
            return false;
        }
        SLOT.clear(request_id);
        PUBLIC_OWNER.release(request_id);
        true
    }

    /// 画面が観測する「自分の分の」要求と結果。持ち主が画面でない要求（公開入口の分）は見せず `None` にする
    /// （画面が公開入口の要求を再生成後の復元と取り違えない）。失敗は画面へ渡す文言へ変換する。
    pub(crate) fn screen_snapshot() -> Option<(u64, Option<super::MobileOutcome>)> {
        let (request_id, ready) = SLOT.snapshot()?;
        if request_id != SCREEN_OWNER.current() {
            return None;
        }
        Some((request_id, ready.map(|r| r.map_err(super::to_message))))
    }

    /// 画面の要求が公開入口に置き換えられ、結果を取り出せなくなったとき、その要求 id を返す（それ以外は `None`）。
    /// 画面の持ち主の印が残っているのに、slot の現在の要求が別の id（空を含む）になっている状態で、
    /// 画面が `Picking` のまま止まらないよう [`super::next_action`] が lost の失敗へ倒す根拠にする。
    pub(crate) fn screen_lost() -> Option<u64> {
        let mine = SCREEN_OWNER.current();
        if mine == 0 || SLOT.snapshot().is_some_and(|(id, _)| id == mine) {
            return None;
        }
        Some(mine)
    }

    /// OS からの結果を `Arc` で包んでから置く（任意スレッド）。`request_id` 不一致・未登録（プロセス死後の再配送等）・
    /// 既に結果あり（重複通知）は捨てて `false`。再描画の要求は呼び出し元（FFI / JNI
    /// 受け口）がプラットフォーム固有の経路で行う。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<Option<PickedFile>, FilePickerError>,
    ) -> bool {
        SLOT.deliver(request_id, result.map(|opt| opt.map(Arc::new)))
    }

    /// 現在の要求と、届いていれば結果（クローン）。画面の effect ドライバは [`screen_snapshot`] 経由で毎フレーム観測する。
    /// 結果が届いてから `Idle` へ反映されるまでの数フレーム、この `clone` が毎フレーム走るが、`Outcome` が `PickedFile` を
    /// `Arc` で包んでいるため参照カウント共有のみで、最大 `MAX_FILE_BYTES`（8 MiB）のバイト列自体は複製されない
    /// （Issue #980 codex-review 指摘の是正。`PickedFile.bytes` は `Vec<u8>` のまま、公開 API は変更しない）。
    #[cfg(test)]
    pub(crate) fn snapshot() -> Option<(u64, Option<Outcome>)> {
        SLOT.snapshot()
    }

    /// reset 相当（テスト用）。slot を空にして持ち主の印を外す。
    #[cfg(test)]
    pub(crate) fn reset() {
        SLOT.reset();
    }

    /// 完了クロージャが画面へ反映できたときだけ呼ぶ。`request_id` が現在の要求と一致する
    /// ときのみ消す（反映確認前に新しい要求が始まることは無いが、契約として照合する）。
    pub(crate) fn clear(request_id: u64) {
        SLOT.clear(request_id);
        SCREEN_OWNER.release(request_id);
    }
}

/// 画面へ渡す結果の形。[`bridge::Outcome`] の失敗（`FilePickerError`）を画面向けの文言（`String`）へ変換した型
/// （[`bridge::screen_snapshot`] が変換する。成功側の `Option<Arc<PickedFile>>` は同一）。
#[cfg(any(test, target_os = "ios", target_os = "android"))]
type MobileOutcome = Result<Option<std::sync::Arc<PickedFile>>, String>;

/// [`next_action`] が受け取る「画面自身の要求と、届いていれば結果」（[`bridge::screen_snapshot`] の戻り値と同じ形）。
#[cfg(any(test, target_os = "ios", target_os = "android"))]
type MobileSnapshot = Option<(u64, Option<MobileOutcome>)>;

/// [`drive_mobile_with`] の分岐（`state.phase` × [`bridge::screen_snapshot`]）をホスト側でも
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
    /// 画面の要求が公開入口に置き換えられた（[`bridge::screen_lost`]）。成功にせず lost の失敗で終え、画面の持ち主の印を外す。
    Lost { request_id: u64 },
}

/// `(state.phase, bridge::screen_snapshot())` から次に行う操作を決める純関数
/// （[`Action`] doc 参照）。OS 呼び出し・`CompletionSender` へのアクセスを一切含まないため
/// `cfg` 無しでホスト上の `cargo test` から検証できる（4 腕 + 該当なしの `Nothing`）。
#[cfg(any(test, target_os = "ios", target_os = "android"))]
pub(crate) fn next_action(phase: PickPhase, snapshot: MobileSnapshot, lost: Option<u64>) -> Action {
    match (phase, snapshot) {
        (PickPhase::Requested, None) => Action::StartPick,
        (PickPhase::Idle, Some(_)) => Action::RestoreRequested,
        (PickPhase::Requested, Some(_)) => Action::RestorePicking,
        (PickPhase::Picking, Some((request_id, Some(outcome)))) => Action::Finish {
            request_id,
            outcome,
        },
        (PickPhase::Picking, None) => {
            lost.map_or(Action::Nothing, |request_id| Action::Lost { request_id })
        }
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
    match next_action(
        (hooks.phase)(state),
        bridge::screen_snapshot(),
        bridge::screen_lost(),
    ) {
        Action::Nothing => {}
        Action::StartPick => {
            let _ = sender.send(Box::new(move |s: &mut S| (hooks.mark_picking)(s)));
            // 公開入口の進行中・未取得の要求は置き換えず、画面側を処理中の失敗で終える。
            let Some(request_id) = bridge::begin_screen() else {
                let generation = (hooks.generation)(state);
                let message = to_message(FilePickerError::Internal("file_picker busy"));
                let _ = sender.send(Box::new(move |s: &mut S| {
                    (hooks.finish)(s, generation, Err(message));
                }));
                return;
            };
            if !present(request_id) {
                // 提示依頼自体が同期的に失敗した（コールバックは二度と来ない）。
                bridge::deliver(
                    request_id,
                    Err(FilePickerError::Internal("file_picker present")),
                );
            }
        }
        Action::RestoreRequested => {
            let _ = sender.send(Box::new(move |s: &mut S| (hooks.request)(s)));
        }
        Action::RestorePicking => {
            let _ = sender.send(Box::new(move |s: &mut S| (hooks.mark_picking)(s)));
        }
        Action::Lost { request_id } => {
            bridge::SCREEN_OWNER.release(request_id);
            let message = to_message(FilePickerError::Internal("file_picker lost"));
            let _ = sender.send(Box::new(move |s: &mut S| {
                if (hooks.phase)(s) != PickPhase::Idle {
                    let generation = (hooks.generation)(s);
                    (hooks.finish)(s, generation, Err(message));
                }
            }));
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
                    // 毎フレームの `bridge::screen_snapshot` クローンとは別。`bridge::Outcome`
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

/// iOS 提示関数への委譲（`ios.rs::present_file_picker`、Issue #873）。
/// `request_id` は [`bridge`] が払い出したプロセス内一意な ID
/// （画面の `generation` ではない、[`bridge`] のモジュール doc 参照）。
#[cfg(target_os = "ios")]
fn present(request_id: u64) -> bool {
    crate::ios::present_file_picker(request_id)
}

/// Android 提示関数への委譲（`android.rs::request_file_pick`、Issue #874）。
/// `request_id` は [`bridge`] が払い出したプロセス内一意な ID
/// （画面の `generation` ではない、[`bridge`] のモジュール doc 参照）。
#[cfg(target_os = "android")]
fn present(request_id: u64) -> bool {
    crate::android::request_file_pick(request_id)
}

/// デスクトップ（macOS/Windows）向け実装を保持する `OnceLock`。`RfdFilePicker` は
/// 内部に `busy` ガードを持つため、呼び出しのたびに作り直さず 1 個だけ使い回す
/// （`clipboard::CLIPBOARD`/`tts::TTS` と同型）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
static PICKER: OnceLock<Result<Box<dyn crate::contract::FilePicker>, String>> = OnceLock::new();

/// 画面の状態型に依存しない同期の入口（全ターゲットで公開し、戻り値の型は同じ）。
///
/// macOS・Windows は `RfdFilePicker`（同期でダイアログを開いて結果を返す。呼び出しは UI スレッドから行う）。Linux 等は `Err`（固定の案内）。
/// iOS・Android は OS がコールバック型で結果が非同期に届くため同期実装を持たず、`pick()` が `Unsupported` を返す実装を返す
/// （成功を偽装しない）。モバイルでは [`begin_file_pick`] / [`poll_file_pick`] を使う。
pub fn platform_file_picker() -> Result<&'static dyn crate::contract::FilePicker, &'static str> {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        Ok(&crate::contract::UnsupportedFilePicker)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let cached = PICKER.get_or_init(|| {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                use crate::contract::FilePicker;
                use crate::desktop::RfdFilePicker;
                Ok(Box::new(RfdFilePicker::new()) as Box<dyn FilePicker>)
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                Err(FilePickerError::Unsupported.to_string())
            }
        });
        match cached {
            Ok(picker) => Ok(picker.as_ref()),
            // 接続時のエラー文言は内部要因を含み得ないが、他ドライバ（clipboard/tts）と
            // 同じ方針で固定の案内へ畳む（security.md A09）。
            Err(_) => Err(UNAVAILABLE),
        }
    }
}

/// ファイル選択を OS へ依頼し、結果を [`poll_file_pick`] で受け取るための要求 id を返す（iOS・Android）。
///
/// 画面の状態型に依存しない非同期の入口で、フレームワークの外のアプリも使える。選択条件の引数は無い（画面経路と同じく 1 ファイル・
/// 種類の絞り込みなし）。1 プロセスに 1 要求で、画面 effect と slot を共有する。
///
/// - 進行中の要求は busy で断る。
/// - 画面側の届いた未取得の結果は公開入口が置き換える（画面が描かれず取り出されないままでも詰まらない）。置き換えられた画面が後で poll すると lost の失敗になり、成功にはならない。
///   結果が届いてから次フレームの poll までの間に公開入口を始めた場合も、画面の本物の成功結果が置き換えで lost の失敗として報告される（失敗側に倒れ、成功には化けない）。
/// - 自分の未取得の結果があれば `Internal`（busy）で断る。
/// - 画面 effect 側も、この入口の進行中・未取得の要求があれば
///   置き換えず処理中の失敗で終える。依頼自体の失敗は `Err`（結果は届かない）。
///
/// デスクトップ（macOS・Windows）は同期で使える [`platform_file_picker`] の `pick()` を使うため、この入口は持たず
/// `Unsupported` を返す。Linux 等も `Unsupported`。
///
/// 結果の解放: 選んだファイルは OS 側で読み切ってから届くため、結果が握るのはメモリ上のバイト列（上限 `MAX_FILE_BYTES`）だけで、
/// fd・一時パスは届く前に解放済み。結果は [`poll_file_pick`] で取り出すと同時に解放される。取り出さない要求（id を捨てた、
/// OS がコールバックしない等）が slot に残ると、次の要求も（画面 effect の開始も）busy で断られるため、不要になった要求は
/// [`discard_file_pick`] で捨てる。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn begin_file_pick() -> Result<u64, FilePickerError> {
    let request_id = bridge::begin_public().ok_or(FilePickerError::Internal("file_picker busy"))?;
    if !present(request_id) {
        bridge::abort_public(request_id);
        return Err(FilePickerError::Internal("file_picker present"));
    }
    Ok(request_id)
}

/// iOS・Android 以外では `Unsupported`（モジュール doc・[`platform_file_picker`] 参照）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn begin_file_pick() -> Result<u64, FilePickerError> {
    Err(FilePickerError::Unsupported)
}

/// [`begin_file_pick`] の自分の要求を捨てて slot を空ける。`request_id` が公開入口の現在の要求のときだけ `true`
/// （他人の id・画面側の id・取り出し済み・0 は `false` で何もしない）。結果が未取得でも進行中でも捨てる。
///
/// discard は選択画面を閉じない（OS の画面は閉じられない）。画面が閉じるまで次の [`begin_file_pick`]（画面 effect の開始も）は
/// OS 側が busy（`Internal("file_picker busy")`）で断る。後から届いた結果は id 不一致で捨てられる。捨てたあとは
/// [`poll_file_pick`] が `None` になり、画面が閉じれば公開入口も画面 effect も再び開始できる。iOS・Android 以外では常に `false`。
///
/// busy の判定は OS の観測で行う。Android は追跡中の要求（結果の通知・launch 失敗・Activity の終了で外れる）、
/// iOS は提示中の選択画面（提示されていない残りは進行中とみなさない）。
pub fn discard_file_pick(request_id: u64) -> bool {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        bridge::discard_public(request_id)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = request_id;
        false
    }
}

/// [`begin_file_pick`] の結果を取り出す。届いていれば 1 度だけ `Some`、未着・旧 id・公開入口でない id・取り出し済みは `None`。
///
/// 結果は 3 値: 選択 `Ok(Some(PickedFile))`、キャンセル `Ok(None)`（失敗ではない）、失敗 `Err`
/// （サイズ超過は `InvalidInput`、読み取り失敗・提示失敗は `Internal`）。
/// slot の要求が失われたとき（reset 等）は、1 度だけ `Err(Internal("file_picker lost"))` を返し、以後は `None`。
/// 失われた後に poll せず別の [`begin_file_pick`] をすると持ち主が新しい id へ替わり、旧 id の喪失は通知されず `None` のまま。
/// 並行して同じ id を poll しても、結果と喪失が二重に返ることはない。
pub fn poll_file_pick(request_id: u64) -> Option<Result<Option<PickedFile>, FilePickerError>> {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        bridge::poll_public(request_id)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = request_id;
        None
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
    match platform_file_picker() {
        Ok(picker) => drive_with(state, sender, picker, FILE_PICK_HOOKS),
        Err(message) => fail_request_with(state, sender, message, FILE_PICK_HOOKS),
    }
}

#[cfg(test)]
#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod tests {
    use super::*;
    use crate::contract::FilePicker;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    fn sender_with_channel() -> (
        CompletionSender<FilePickState>,
        std::sync::mpsc::Receiver<fandhe_runtime::Completion<FilePickState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            CompletionSender::new(tx, std::sync::Arc::new(NoopWaker)),
            rx,
        )
    }

    /// 常に指定した結果を返すモック（GUI 無しで `drive_with` を検証する）。
    struct MockPicker(Result<Option<PickedFile>, FilePickerError>);

    impl FilePicker for MockPicker {
        fn pick(&self) -> Result<Option<PickedFile>, FilePickerError> {
            match &self.0 {
                Ok(v) => Ok(v.clone()),
                Err(e) => Err(*e),
            }
        }
    }

    fn apply_all(
        state: &mut FilePickState,
        rx: &std::sync::mpsc::Receiver<fandhe_runtime::Completion<FilePickState>>,
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
            &MockPicker(Err(FilePickerError::InvalidInput("file_picker size"))),
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
    use super::{Action, next_action};
    use fandhe_core::screens::file_pick::PickPhase;
    use fandhe_device::file_picker::PickedFile;

    #[test]
    fn requested_without_pending_starts_pick() {
        assert_eq!(
            next_action(PickPhase::Requested, None, None),
            Action::StartPick
        );
    }

    #[test]
    fn idle_with_pending_restores_to_requested() {
        // Activity 再生成で画面状態だけが作り直された（要求は `bridge` に残っている）。
        assert_eq!(
            next_action(PickPhase::Idle, Some((1, None)), None),
            Action::RestoreRequested
        );
    }

    #[test]
    fn requested_with_pending_restores_to_picking() {
        // 復元途中、または再生成直後の 1 フレームにユーザーがタップした場合。
        assert_eq!(
            next_action(PickPhase::Requested, Some((1, None)), None),
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
            next_action(PickPhase::Picking, Some((7, Some(outcome.clone()))), None),
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
        assert_eq!(next_action(PickPhase::Idle, None, None), Action::Nothing);
        assert_eq!(
            next_action(PickPhase::Picking, Some((1, None)), None),
            Action::Nothing
        );
        assert_eq!(next_action(PickPhase::Picking, None, None), Action::Nothing);
    }

    #[test]
    fn picking_replaced_by_public_is_lost() {
        // 画面の要求が公開入口に置き換えられた（画面のスナップショットは `None`、持ち主の印だけ残る）。
        assert_eq!(
            next_action(PickPhase::Picking, None, Some(5)),
            Action::Lost { request_id: 5 }
        );
        // `Picking` 以外では lost を見ない。
        assert_eq!(next_action(PickPhase::Idle, None, Some(5)), Action::Nothing);
        assert_eq!(
            next_action(PickPhase::Requested, None, Some(5)),
            Action::StartPick
        );
    }
}

/// [`bridge`] と公開入口の単体テスト。OS 呼び出しを含まないためホスト上の `cargo test` で回せる。
/// 同じ static slot を触るため、テストどうしを `TEST_LOCK` で直列にする。
#[cfg(test)]
mod bridge_tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn picked(name: &str) -> PickedFile {
        PickedFile {
            name: name.to_string(),
            bytes: vec![1, 2, 3],
        }
    }

    #[test]
    fn deliver_keeps_raw_result_and_screen_view_converts_it() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let request_id = bridge::begin_screen().unwrap();
        assert!(bridge::deliver(request_id, Ok(Some(picked("a.txt")))));
        let (_, ready) = bridge::screen_snapshot().expect("画面の要求なので見える");
        let got = ready.unwrap().unwrap().unwrap();
        assert_eq!(got.name, "a.txt");
        assert_eq!(got.bytes.to_vec(), vec![1, 2, 3]);
        bridge::clear(request_id);
        assert!(bridge::screen_snapshot().is_none());
        // 失敗は画面向けの文言へ変換される。
        let id = bridge::begin_screen().unwrap();
        bridge::deliver(id, Err(FilePickerError::InvalidInput("file_picker size")));
        let (_, ready) = bridge::screen_snapshot().unwrap();
        assert!(ready.unwrap().unwrap_err().contains("MiB"));
        bridge::clear(id);
        bridge::reset();
    }

    #[test]
    fn public_request_is_exclusive_and_polled_once() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let public = bridge::begin_public().unwrap();
        // 進行中は公開入口の 2 件目も画面側も断る（busy）。画面は公開入口の要求を自分のものとして見ない。
        assert!(bridge::begin_public().is_none());
        assert!(bridge::begin_screen().is_none());
        assert!(bridge::screen_snapshot().is_none());
        assert!(bridge::poll_public(public).is_none());
        assert!(bridge::deliver(public, Ok(None)));
        // 未取得の公開入口の結果を画面側が落とさない。
        assert!(bridge::begin_screen().is_none());
        // キャンセルは Ok(None)（失敗ではない）で、1 度だけ取れる。
        assert!(matches!(bridge::poll_public(public), Some(Ok(None))));
        assert!(bridge::poll_public(public).is_none());
        // 取り出したあとは画面側が使える。
        assert!(bridge::begin_screen().is_some());
        bridge::reset();
    }

    /// 画面の届いた未取得の結果は公開入口が置き換え、置き換えられた画面は lost の失敗へ倒れる（`Picking` のまま止まらない・成功にならない）。
    #[test]
    fn public_begin_replaces_unpolled_screen_result_and_screen_is_lost() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        bridge::SCREEN_OWNER.release(bridge::SCREEN_OWNER.current());
        let screen = bridge::begin_screen().unwrap();
        assert!(bridge::screen_lost().is_none());
        assert!(bridge::deliver(screen, Ok(Some(picked("a.txt")))));
        // 画面が取り出さない結果で公開入口が詰まらない。
        let public = bridge::begin_public().expect("置き換えられる");
        assert_ne!(public, screen);
        // 古い画面の id への配送は捨てられる。
        assert!(!bridge::deliver(screen, Ok(Some(picked("b.txt")))));
        // 画面からは自分の要求が見えず、lost として観測される。
        assert!(bridge::screen_snapshot().is_none());
        assert_eq!(bridge::screen_lost(), Some(screen));
        assert_eq!(
            next_action(
                PickPhase::Picking,
                bridge::screen_snapshot(),
                bridge::screen_lost()
            ),
            Action::Lost { request_id: screen }
        );
        // 印を外したあとは lost を繰り返さない。公開入口の結果は画面へ渡らない。
        bridge::SCREEN_OWNER.release(screen);
        assert!(bridge::screen_lost().is_none());
        assert!(bridge::deliver(public, Ok(None)));
        assert!(matches!(bridge::poll_public(public), Some(Ok(None))));
        bridge::reset();
    }

    /// 画面の要求が進行中なら公開入口は busy で、画面の持ち主は変わらない。
    #[test]
    fn public_begin_is_busy_while_screen_request_is_in_flight() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        bridge::SCREEN_OWNER.release(bridge::SCREEN_OWNER.current());
        let screen = bridge::begin_screen().unwrap();
        assert!(bridge::begin_public().is_none());
        assert_eq!(bridge::SCREEN_OWNER.current(), screen);
        assert!(bridge::screen_lost().is_none());
        bridge::SCREEN_OWNER.release(screen);
        bridge::reset();
    }

    /// 公開入口の届いた未取得の結果は、公開入口も画面も置き換えない。
    #[test]
    fn public_begin_keeps_unpolled_public_result() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let public = bridge::begin_public().unwrap();
        assert!(bridge::deliver(public, Ok(None)));
        assert!(bridge::begin_public().is_none());
        assert!(bridge::begin_screen().is_none());
        assert!(matches!(bridge::poll_public(public), Some(Ok(None))));
        assert!(bridge::poll_public(public).is_none());
        bridge::reset();
    }

    #[test]
    fn selected_file_is_returned_with_bytes() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let id = bridge::begin_public().unwrap();
        bridge::deliver(id, Ok(Some(picked("b.txt"))));
        let got = bridge::poll_public(id).unwrap().unwrap().unwrap();
        assert_eq!((got.name.as_str(), got.bytes.len()), ("b.txt", 3));
        bridge::reset();
    }

    #[test]
    fn public_request_lost_by_reset_fails_once() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let id = bridge::begin_public().unwrap();
        assert!(bridge::poll_public(id).is_none());
        bridge::reset();
        assert!(matches!(
            bridge::poll_public(id),
            Some(Err(FilePickerError::Internal(_)))
        ));
        assert!(bridge::poll_public(id).is_none());
        assert!(bridge::begin_public().is_some());
        bridge::reset();
    }

    #[test]
    fn concurrent_poll_delivers_once() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..500 {
            bridge::reset();
            let id = bridge::begin_public().unwrap();
            assert!(bridge::deliver(id, Ok(Some(picked("c.txt")))));
            let barrier = std::sync::Barrier::new(2);
            let results: Vec<_> = std::thread::scope(|s| {
                let hs: Vec<_> = (0..2)
                    .map(|_| {
                        s.spawn(|| {
                            barrier.wait();
                            bridge::poll_public(id)
                        })
                    })
                    .collect();
                hs.into_iter().map(|h| h.join().unwrap()).collect()
            });
            assert_eq!(results.iter().filter(|r| r.is_some()).count(), 1);
            assert!(results.iter().flatten().all(|r| r.is_ok()));
        }
        bridge::reset();
    }

    #[test]
    fn poll_of_unknown_id_is_none() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let old = bridge::begin_public().unwrap();
        bridge::reset();
        let new = bridge::begin_public().unwrap();
        assert!(bridge::poll_public(0).is_none());
        assert!(bridge::poll_public(old).is_none());
        assert!(bridge::poll_public(new + 1).is_none());
        bridge::reset();
    }

    /// 公開入口の要求を捨てると、公開入口も画面も再び開始できる。他人の id では何もしない。
    #[test]
    fn discard_frees_slot_for_public_and_screen() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let id = bridge::begin_public().unwrap();
        // 進行中の要求を、他人の id・0 では捨てられない。
        assert!(!bridge::discard_public(id + 1));
        assert!(!bridge::discard_public(0));
        assert!(bridge::begin_screen().is_none());
        assert!(bridge::discard_public(id));
        assert!(bridge::poll_public(id).is_none());
        assert!(!bridge::discard_public(id));
        // 結果が未取得でも捨てられ、遅れて届いた結果は捨てられる。
        let id2 = bridge::begin_public().unwrap();
        assert!(bridge::deliver(id2, Ok(None)));
        assert!(bridge::discard_public(id2));
        assert!(!bridge::deliver(id2, Ok(None)));
        // 画面の id は公開入口から捨てられない。
        let screen = bridge::begin_screen().unwrap();
        assert!(!bridge::discard_public(screen));
        bridge::clear(screen);
        assert!(bridge::begin_public().is_some());
        bridge::reset();
    }

    /// 失敗の値は変換されずにそのまま返る。
    #[test]
    fn failure_values_are_returned_as_is() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        for e in [
            FilePickerError::InvalidInput("file_picker size"),
            FilePickerError::Internal("file_picker read"),
        ] {
            let id = bridge::begin_public().unwrap();
            assert!(bridge::deliver(id, Err(e)));
            assert_eq!(bridge::poll_public(id), Some(Err(e)));
        }
        bridge::reset();
    }

    /// 提示依頼の失敗で登録を取り消すと、slot も持ち主も空になり、再び開始できる。
    #[test]
    fn abort_public_empties_slot_and_owner() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let id = bridge::begin_public().unwrap();
        bridge::abort_public(id);
        assert!(bridge::snapshot().is_none());
        assert_eq!(bridge::PUBLIC_OWNER.current(), 0);
        assert!(bridge::poll_public(id).is_none());
        assert!(bridge::begin_public().is_some());
        bridge::reset();
    }

    /// iOS・Android 以外の公開入口は `Unsupported`（デスクトップは同期の `platform_file_picker` を使う）。
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    #[test]
    fn async_entry_is_unsupported_off_mobile() {
        assert_eq!(begin_file_pick(), Err(FilePickerError::Unsupported));
        assert!(poll_file_pick(1).is_none());
        assert!(!discard_file_pick(1));
    }
}
