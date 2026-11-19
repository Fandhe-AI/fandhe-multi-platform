//! Phase 11（端末機能の拡充ループ、親 #896）型サンプル「共有」（Issue #897 macOS →
//! #898 Windows → #899 iOS/Android 横展開）の effect ドライバ。
//! `fandhe_core::screens::share_text`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`fandhe_runtime::EffectFn` 参照）。
//!
//! # macOS/iOS/Android/Windows: pull 型 `bridge`（OS の共有 UI はいずれもコールバック型）
//!
//! `NSSharingServicePicker`（macOS）/ `UIActivityViewController`（iOS）/
//! `Intent.createChooser`（Android）/ `DataTransferManager`（Windows）はいずれも
//! delegate・コールバック型（提示 → 結果を非同期通知）のため、`Share` トレイトを
//! 通さずコアの状態機械へ直接書き戻す（`fandhe-cap-file-picker` の `driver.rs` の iOS/Android 経路と
//! 同型）。[`bridge`] は `fandhe-runtime/src/request_slot.rs::RequestSlot` の facade で、
//! `CompletionSender` も画面の `generation` も保持しない pull 型: OS 側コールバックは
//! 結果を置くだけで、画面への反映は毎フレーム [`drive_callback`] が **その時点の**
//! `sender`/`state` を使って行う（`RequestSlot`/`poll_and_finish` のモジュール doc
//! 参照、Issue #943 で 3 機能の共通部品へ統合）。macOS/iOS/Android は [`present`] が
//! OS ごとの提示処理（`apple::present`/`android::request_share`）へ振り分ける。
//!
//! Windows のみ、`DataTransferManager` に「共有 UI を閉じた」イベントが無いため
//! （公知の制約）、`drive_callback` がフォーカス復帰ヒューリスティックでキャンセルを
//! 補う（[`cancel_due`] 参照）。
//!
//! # 上記 4 OS 以外（Linux）: `Share` トレイト経由
//!
//! Linux は [`UnsupportedShare`]（`crate::contract::UnsupportedShare`）を
//! `Share` トレイト経由で呼び、`Unsupported("share")` を画面へ「この環境では共有
//! シートを使えません」として表示する。
//!
//! Web は本 Issue のスコープ外（親 #896 の子 (d)）。
//!
//! # 既知の天井（Issue #899 §2.6）
//!
//! Android/iOS とも、共有 UI 表示中に画面（`ShareTextState`）が破棄・再生成される
//! （Android の Activity 再生成が典型）と、後から届く結果は `phase != Idle` 判定で
//! 反映されず捨てられる（固着はしない）。無検証の再開 API は追加しない
//! （`file_pick` の `RestoreRequested` は本 Issue でも導入しない、実装計画 §2.6 参照）。

use crate::contract::{Share, ShareOutcome as DeviceShareOutcome, ShareRequest};
use crate::error::ShareSheetError;
use fandhe_core::screens::share_text::{
    ShareOutcome as CoreShareOutcome, ShareTextState, finish_share, mark_sharing, take_request,
};

use fandhe_runtime::CompletionSender;

/// `Share`/`bridge::deliver` 未対応環境（Unsupported）で画面へ出す固定案内
/// （`ShareSheetError` の内部構造は含めない、security.md A09）。
const UNAVAILABLE: &str = "この環境では共有シートを使えません";
/// 公開入口の共有が進行中・結果が未取得のときに画面へ出す固定文言。
#[cfg_attr(
    not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )),
    allow(dead_code)
)]
const BUSY: &str = "前の共有がまだ終わっていません";
/// 画面の要求が失われたときの固定文言。
#[cfg_attr(
    not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )),
    allow(dead_code)
)]
const LOST: &str = "共有の要求が失われました。もう一度お試しください";

/// `ShareSheetError` を画面へ渡す文言へ変換する。`Unsupported` は固定文言へ畳み、
/// それ以外は `Display` の文言をそのまま転送する
/// （`fandhe-cap-file-picker` の `driver.rs::to_message` と同じ方針）。
fn to_message(e: ShareSheetError) -> String {
    match e {
        ShareSheetError::Unsupported => UNAVAILABLE.to_string(),
        other => other.to_string(),
    }
}

/// `Result<DeviceShareOutcome, ShareSheetError>` を画面表示用の結果へ変換する
/// （macOS/iOS/Android/Windows 経路・`Share` トレイト経路の共通部分）。
fn to_outcome(
    result: Result<DeviceShareOutcome, ShareSheetError>,
) -> Result<CoreShareOutcome, String> {
    result
        .map(|outcome| match outcome {
            DeviceShareOutcome::Shared => CoreShareOutcome::Shared,
            DeviceShareOutcome::Cancelled => CoreShareOutcome::Cancelled,
            DeviceShareOutcome::NoTarget => CoreShareOutcome::NoTarget,
            DeviceShareOutcome::Unknown => CoreShareOutcome::Unknown,
        })
        .map_err(to_message)
}

/// OS 側の結果コード（iOS/macOS/Android の 3 経路で共通、Issue #899 §2.2 の表参照）を
/// [`crate::contract::ShareOutcome`] へ写す純関数。`cfg` 無しで全ターゲットから
/// テストできる（iOS/Android を同じ表に載せるため #899 で統合）。未知のコードは `Internal("share outcome")` として扱う
/// （偽の成功・偽のキャンセルを作らない）。Windows（#898）は
/// `DataTransferManager`/`windows::present` 側で `Result` を直接返すため、
/// このコード表には乗らない。
///
/// | code | 意味 | iOS | macOS | Android |
/// |---|---|---|---|---|
/// | 0 | `Ok(Shared)` | completed==YES | service 非 nil | 選択 broadcast 受信 |
/// | 1 | `Ok(Cancelled)` | completed==NO&&error==nil | service nil | （使わない） |
/// | 2 | `Ok(NoTarget)` | （使わない） | services 空 | （使わない） |
/// | 3 | `Err(Internal("share present"))` | VC 不在/error!=nil | window 不在等 | launch() 例外 |
/// | 4 | `Ok(Unknown)` | （使わない） | （使わない） | chooser 終了・broadcast 未受信 |
///
/// 呼び出し元（`apple.rs`/`android.rs`）が macOS/iOS/Android の 3 OS にのみ
/// 存在する（`lib.rs` の `mod` の cfg 参照）ため、それ以外のターゲット（例:
/// Linux CI runner 上の clippy）では本関数も未使用になり `dead_code` で
/// `-D warnings` に落ちる。呼び出し元と同じ cfg を付けて対象を揃える。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
pub(crate) fn outcome_from_code(code: i32) -> Result<DeviceShareOutcome, ShareSheetError> {
    match code {
        0 => Ok(DeviceShareOutcome::Shared),
        1 => Ok(DeviceShareOutcome::Cancelled),
        2 => Ok(DeviceShareOutcome::NoTarget),
        3 => Err(ShareSheetError::Internal),
        4 => Ok(DeviceShareOutcome::Unknown),
        _ => Err(ShareSheetError::Internal),
    }
}

/// macOS/iOS/Android/Windows 共通: OS 側コールバックとの橋渡し（pull 型、
/// `fandhe-runtime/src/request_slot.rs::RequestSlot` の facade。モジュール doc 参照）。
/// 画面 effect（`SCREEN_OWNER`）と Windows の公開入口（[`super::begin_share_sheet`]、`PUBLIC_OWNER`）が
/// 持ち主の印で分けて共有する（`consumer-app-structure.md` §8.2）。macOS・iOS・Android の [`super::platform_share_sheet`]
/// は印を付けない（画面と同時に使わない契約）。ホストのテストでも slot の扱いを確かめるため、`test` でもコンパイルする。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows",
    test
))]
pub(crate) mod bridge {
    use crate::contract::ShareOutcome as DeviceShareOutcome;
    use crate::error::ShareSheetError;
    use fandhe_core::screens::share_text::ShareOutcome as CoreShareOutcome;

    use fandhe_runtime::request_slot::{RequestSlot, SlotOwner};

    /// 画面へ反映する完了結果（`super::to_outcome` 後の形）。
    pub(crate) type Outcome = Result<CoreShareOutcome, String>;

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();
    /// 画面 effect が追う自分の分の要求。
    pub(crate) static SCREEN_OWNER: SlotOwner = SlotOwner::new();
    /// Windows の公開入口が追う自分の分の要求。
    pub(crate) static PUBLIC_OWNER: SlotOwner = SlotOwner::new();
    /// 公開側の poll・discard 本体を直列にする。並行 poll で結果が二重に返らないため。
    static PUBLIC_POLL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 画面側の開始。自分の前の要求は置き換え（従来どおり）、公開入口の進行中の要求・未取得の結果は置き換えず `None`。
    pub(crate) fn begin_screen() -> Option<u64> {
        SCREEN_OWNER.begin_replacing(&SLOT, &[&PUBLIC_OWNER])
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    /// 公開入口の開始。進行中の要求と、自分の未取得の結果があれば `None`（busy）。画面側の届いた未取得の結果は
    /// 捨てずに画面の受け取り待ちへ移す（`SlotOwner::begin_parking`。受け取られない分は画面の次の開始で解放、残るのは最大 1 件）。
    pub(crate) fn begin_public() -> Option<u64> {
        PUBLIC_OWNER.begin_parking(&SLOT, &SCREEN_OWNER)
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    /// 公開入口の依頼自体が失敗したときの後片付け（結果は届かないので slot と印を空ける）。
    pub(crate) fn abort_public(request_id: u64) {
        SLOT.clear(request_id);
        PUBLIC_OWNER.release(request_id);
    }

    /// 画面用の結果を公開入口の結果へ戻す。失敗の種類は `Internal` へ畳む（`CallbackShare` と同じ）。
    pub(crate) fn to_device(outcome: Outcome) -> Result<DeviceShareOutcome, ShareSheetError> {
        outcome
            .map_err(|_| ShareSheetError::Internal)
            .map(|o| match o {
                CoreShareOutcome::Shared => DeviceShareOutcome::Shared,
                CoreShareOutcome::Cancelled => DeviceShareOutcome::Cancelled,
                CoreShareOutcome::NoTarget => DeviceShareOutcome::NoTarget,
                CoreShareOutcome::Unknown => DeviceShareOutcome::Unknown,
            })
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    /// [`super::poll_share_sheet`] の本体。持ち主が `request_id` のときだけ見る。結果待ちの間は `on_waiting` を
    /// 呼ぶ（Windows のキャンセル検知）。届いていれば 1 度だけ `Some` で持ち主を外す。slot の要求が失われた
    /// （別の要求に替わった・消えた）ときは `Err(Internal)` を 1 度だけ返す。
    pub(crate) fn poll_public(
        request_id: u64,
        on_waiting: impl FnOnce(u64),
    ) -> Option<Result<DeviceShareOutcome, ShareSheetError>> {
        let _guard = PUBLIC_POLL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if request_id == 0 || PUBLIC_OWNER.current() != request_id {
            return None;
        }
        if SLOT
            .snapshot()
            .is_some_and(|(id, ready)| id == request_id && ready.is_none())
        {
            on_waiting(request_id);
        }
        if let Some(result) = SLOT.take_ready(request_id) {
            PUBLIC_OWNER.release(request_id);
            return Some(to_device(result));
        }
        if SLOT.snapshot().is_some_and(|(id, _)| id == request_id) {
            return None;
        }
        PUBLIC_OWNER.release(request_id);
        Some(Err(ShareSheetError::Internal))
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    /// [`super::discard_share_sheet`] の本体。持ち主が `request_id` のときだけ、slot の記録と持ち主を外して `true`。
    /// 共有 UI は閉じない。Windows の共有 UI は閉じた通知が無く、捨てた要求を観測し続ける口も無いため、進行中でも
    /// 記録を消す（後から届く `TargetApplicationChosen` は id の照合で捨てる）。
    pub(crate) fn discard_public(request_id: u64) -> bool {
        let _guard = PUBLIC_POLL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if request_id == 0 || PUBLIC_OWNER.current() != request_id {
            return false;
        }
        SLOT.clear(request_id);
        PUBLIC_OWNER.release(request_id);
        true
    }

    /// OS からの結果（`crate::contract::ShareOutcome` を経由した生の
    /// `Result`）を `super::to_outcome` で画面用へ変換してから置く（任意スレッド。
    /// 実際にはメインスレッドから届く契約、`apple.rs`/`android.rs` の doc
    /// 参照）。`request_id` 不一致・未登録・既に結果ありは捨てて `false`。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<crate::contract::ShareOutcome, ShareSheetError>,
    ) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }
}

/// OS へ提示を依頼する（Apple 系は `apple_share::present`、Android は
/// `android::request_share`）。戻り値は「提示依頼を受理できたか」（`false` は
/// 呼び出し元が `Err(Internal("share present"))` として畳む）。Windows は戻り値の形が
/// 異なる（`Result` で失敗理由まで返す）ため、この `present` には含めず
/// [`drive_callback`] から `windows::present` を直接呼ぶ。
#[cfg(target_os = "android")]
fn present(request_id: u64, request: &ShareRequest) -> bool {
    crate::android::request_share(request_id, request)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn present(request_id: u64, request: &ShareRequest) -> bool {
    crate::apple::present(request_id, request)
}

/// Windows 限定: フォーカス喪失 → 復帰後にこの猶予を過ぎても結果未着ならキャンセル扱い
/// にする（`cancel_due` 参照）。
#[cfg(target_os = "windows")]
const CANCEL_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// `TargetApplicationChosen` の到着を待つ間、直前に共有 UI へフォーカスを奪われた
/// （`lost_focus`）状態から復帰してもなお `grace` 経過して未着なら `true`（キャンセル
/// 扱い）を返す純関数（OS 非依存、cfg 無しでテストできる。ponytail: 実イベント未提供の
/// 代替ヒューリスティックで、上流が「共有 UI を閉じた」イベントを提供するようになれば
/// 撤去できる）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn cancel_due(
    lost_focus: bool,
    regained_at: Option<std::time::Instant>,
    now: std::time::Instant,
    grace: std::time::Duration,
) -> bool {
    lost_focus && regained_at.is_some_and(|t| now.duration_since(t) >= grace)
}

/// Windows 限定: 直近の要求に対するフォーカス復帰キャンセル検知の状態
/// （`cancel_due` へ渡す入力を貯める）。`request_id` が不一致になったら新しい要求の
/// ものとして状態をリセットする（前回要求の残骸を持ち越さない）。
#[cfg(target_os = "windows")]
struct CancelTracking {
    request_id: u64,
    lost_focus: bool,
    regained_at: Option<std::time::Instant>,
}

#[cfg(target_os = "windows")]
static CANCEL_TRACKING: std::sync::Mutex<Option<CancelTracking>> = std::sync::Mutex::new(None);

/// Windows 限定: 今フレームのフォーカス状態を観測してキャンセル判定を更新する。
/// キャンセルと判定した場合のみ `true` を返す（呼び出し元が `bridge::deliver` する）。
#[cfg(target_os = "windows")]
fn windows_cancel_check(request_id: u64, focused: bool) -> bool {
    let now = std::time::Instant::now();
    let mut guard = CANCEL_TRACKING.lock().unwrap_or_else(|e| e.into_inner());
    let tracking = guard.get_or_insert_with(|| CancelTracking {
        request_id,
        lost_focus: false,
        regained_at: None,
    });
    if tracking.request_id != request_id {
        *tracking = CancelTracking {
            request_id,
            lost_focus: false,
            regained_at: None,
        };
    }
    if !focused {
        tracking.lost_focus = true;
        tracking.regained_at = None;
    } else if tracking.lost_focus && tracking.regained_at.is_none() {
        tracking.regained_at = Some(now);
    }
    cancel_due(tracking.lost_focus, tracking.regained_at, now, CANCEL_GRACE)
}

/// macOS/iOS/Android/Windows 経路の共有 effect ドライバ本体（旧 `drive_mac`。Windows
/// 追加により OS 中立の名前へ改名、モジュール doc「pull 型」参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
))]
fn drive_callback(state: &ShareTextState, sender: &CompletionSender<ShareTextState>) {
    use fandhe_core::screens::share_text::SharePhase;

    if let Some(request) = take_request(state) {
        let _ = sender.send(Box::new(mark_sharing));

        // 公開入口の進行中の要求・未取得の結果は置き換えず、画面には処理中の失敗を返す。
        let Some(request_id) = bridge::begin_screen() else {
            let generation = request.generation;
            let _ = sender.send(Box::new(move |s: &mut ShareTextState| {
                finish_share(s, generation, Err(BUSY.to_string()));
            }));
            return;
        };
        match ShareRequest::new(&request.text, request.url.as_deref()) {
            Ok(share_request) => {
                #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
                if !present(request_id, &share_request) {
                    // 提示依頼自体が同期的に失敗した（コールバックは二度と来ない）。
                    bridge::deliver(request_id, Err(ShareSheetError::Internal));
                }
                #[cfg(target_os = "windows")]
                if let Err(e) = crate::windows::present(request_id, &share_request) {
                    // 提示依頼自体が同期的に失敗した（`TargetApplicationChosen` は
                    // 二度と来ない）。
                    bridge::deliver(request_id, Err(e));
                }
            }
            Err(e) => {
                // 入力検証で弾かれた（共有 UI は開かない）。
                bridge::deliver(request_id, Err(e));
            }
        }
        return;
    }

    if state.phase != SharePhase::Sharing {
        return;
    }
    let generation = state.generation;
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &bridge::SCREEN_OWNER,
        sender,
        |_request_id| {
            // Windows 限定: `DataTransferManager` に「共有 UI を閉じた」イベントが
            // 無いため、フォーカス復帰ヒューリスティックでキャンセルを補う
            // （モジュール doc・`cancel_due` 参照）。
            #[cfg(target_os = "windows")]
            if windows_cancel_check(_request_id, sender.host_view().focused()) {
                bridge::deliver(_request_id, Ok(crate::contract::ShareOutcome::Cancelled));
            }
        },
        move |s: &mut ShareTextState, outcome| {
            // 反映できる状態か（`Idle` へ既に戻っていないか）を確かめてから
            // `finish_share` し、反映できたときだけ `true` を返す（`RequestSlot` 側が
            // それを見て `clear` する）（`fandhe-cap-file-picker` の `driver.rs` の「キュー投入成功 ≠
            // 反映」対策と同じ判断）。
            if s.phase != SharePhase::Idle {
                finish_share(s, generation, outcome);
                true
            } else {
                false
            }
        },
        // 自分の要求が失われた。待ち続けず失敗で終える。
        move |s: &mut ShareTextState| {
            finish_share(s, generation, Err(LOST.to_string()));
        },
    );
}

/// 画面の状態型に依存しない入口。フレームワークの外のアプリも自分の状態型の effect から使える。
///
/// macOS・iOS・Android は OS の共有 UI の結果をコールバックで受け、結果が届くまで呼び出しスレッドを塞ぐ
/// [`Share`] を返す。進行中の要求があるときの 2 件目は提示せず `Internal` で断る。メインスレッドから呼ばず（コールバックがメインで届くためデッドロックする）、
/// 画面ドライバ（[`share_text_effect`]）と同時に使わない（要求の追跡を共有する）。失敗の種類は
/// `Internal` へ畳む。Windows は呼び出しスレッドを塞ぐとキャンセルの観測（フォーカス）が進まないため、
/// Linux と同じく `Unsupported` を返し、非同期の [`begin_share_sheet`] / [`poll_share_sheet`] / [`discard_share_sheet`] を使う。
pub fn platform_share_sheet() -> Box<dyn Share> {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    {
        Box::new(CallbackShare)
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    {
        Box::new(crate::contract::UnsupportedShare)
    }
}

/// コールバック型の OS 実装を [`Share`] に見せるブロッキング版（[`platform_share_sheet`] 参照）。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
struct CallbackShare;

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
impl Share for CallbackShare {
    fn share(&self, request: &ShareRequest) -> Result<DeviceShareOutcome, ShareSheetError> {
        // 進行中・未取得の要求があれば置き換えず断る（他スレッドの待機を落とさない）。
        let Some(id) = bridge::SLOT.try_begin_unless(|_| true) else {
            return Err(ShareSheetError::Internal);
        };
        if !present(id, request) {
            bridge::deliver(id, Err(ShareSheetError::Internal));
        }
        loop {
            match bridge::SLOT.snapshot() {
                Some((rid, Some(outcome))) if rid == id => {
                    bridge::SLOT.clear(id);
                    return bridge::to_device(outcome);
                }
                // 別の要求に置き換えられた・消された場合は結果が来ないため失敗で返す。
                Some((rid, None)) if rid == id => {
                    std::thread::sleep(std::time::Duration::from_millis(50))
                }
                _ => return Err(ShareSheetError::Internal),
            }
        }
    }
}

/// 公開入口（Windows）: 共有 UI を開き、結果を受け取るための要求 id を返す。画面の状態型に依存せず、フレームワークの
/// 外のアプリも使える（`consumer-app-structure.md` §8.2）。結果は [`poll_share_sheet`] で 1 度だけ取り出し、使わない要求は
/// [`discard_share_sheet`] で捨てる。
///
/// - ウィンドウを作った UI スレッドから呼ぶ（`DataTransferManager` の STA 前提）。HWND は
///   `fandhe_platform_windows::remember_window` で控えたものを使う（無ければ `Internal`）
/// - 進行中の要求（画面経由を含む）と、自分の結果が未取得のうちは `Internal` で断る（busy）。画面 effect 側も、
///   この入口の要求が進行中・未取得なら処理中の失敗で終える
/// - メインウィンドウが前面でない（共有 UI が開いている等）ときも `Internal` で断る。discard の直後で、共有 UI が
///   まだ前面を取る前の開始は防げない（天井）
/// - 画面側の届いた未取得の結果は捨てずに画面の受け取り待ちへ移して始める（画面はそのまま受け取る）
/// - 失われた旧 id を poll せずに begin すると、喪失は通知されない
/// - 依頼自体の失敗は `Err`（結果は届かない）。`DataTransferManager` が使えない環境は `Unsupported`
///
/// macOS・iOS・Android は同期の [`platform_share_sheet`] を使い、この入口は Windows 以外では `Unsupported`。
pub fn begin_share_sheet(request: &ShareRequest) -> Result<u64, ShareSheetError> {
    #[cfg(target_os = "windows")]
    {
        // 共有 UI が前面にある（前の要求を discard した後もまだ開いている等）間は始めない。閉じた通知が無いため、
        // 前面の観測で OS 側の進行中を見る（§8.2「OS 側でまだ進行中なら次の begin を busy にする」）。
        if !crate::windows::window_is_foreground() {
            return Err(ShareSheetError::Internal);
        }
        let request_id = bridge::begin_public().ok_or(ShareSheetError::Internal)?;
        if let Err(e) = crate::windows::present(request_id, request) {
            bridge::abort_public(request_id);
            return Err(e);
        }
        Ok(request_id)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = request;
        Err(ShareSheetError::Unsupported)
    }
}

/// [`begin_share_sheet`] の結果を取り出す。届いていれば 1 度だけ `Some`、未着・旧 id・公開入口でない id・取り出し済み・
/// 0・Windows 以外は `None`。要求が失われたときは `Err(Internal)` を 1 度だけ返す。
///
/// `DataTransferManager` には共有 UI を閉じた通知が無いため、結果待ちの間は呼ぶたびにメインウィンドウが前面か
/// （`GetForegroundWindow`）を観測し、前面を失ってから戻り、猶予を過ぎても届かなければ `Ok(Cancelled)` にする
/// （画面のキャンセル検知と同じ判定）。結果が出るまで、前面を失っている間も含めて一定の間隔（100ms 程度）で呼ぶ。
/// 前面を失った状態を一度も観測できなければキャンセルは確定しないので、そのときは [`discard_share_sheet`] で捨てる。
pub fn poll_share_sheet(request_id: u64) -> Option<Result<DeviceShareOutcome, ShareSheetError>> {
    #[cfg(target_os = "windows")]
    {
        bridge::poll_public(request_id, |rid| {
            if windows_cancel_check(rid, crate::windows::window_is_foreground()) {
                bridge::deliver(rid, Ok(DeviceShareOutcome::Cancelled));
            }
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = request_id;
        None
    }
}

/// [`begin_share_sheet`] の自分の要求を捨てる。`request_id` が公開入口の現在の要求のときだけ `true`
/// （他人の id・取り出し済み・0・Windows 以外は `false`）。以後 [`poll_share_sheet`] は `None`。
/// 共有 UI は閉じない。閉じた通知が無いため進行中でも記録を消し、後から届く結果は捨てる。
pub fn discard_share_sheet(request_id: u64) -> bool {
    #[cfg(target_os = "windows")]
    {
        bridge::discard_public(request_id)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = request_id;
        false
    }
}

/// 上記 4 OS 以外（Linux）向け: `Share` トレイト経由の同期呼び出し。
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
fn drive(state: &ShareTextState, sender: &CompletionSender<ShareTextState>, share: &dyn Share) {
    let Some(request) = take_request(state) else {
        return;
    };

    let _ = sender.send(Box::new(mark_sharing));

    let result = ShareRequest::new(&request.text, request.url.as_deref())
        .and_then(|share_request| share.share(&share_request));
    let outcome = to_outcome(result);
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut ShareTextState| {
        finish_share(s, generation, outcome);
    }));
}

/// `registry.rs` の `effect` 区画から `share_text` 画面用に配線される effect ドライバ
/// （macOS/iOS/Android/Windows 版、モジュール doc「pull 型」参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
))]
pub fn share_text_effect(state: &ShareTextState, sender: &CompletionSender<ShareTextState>) {
    drive_callback(state, sender);
}

/// `registry.rs` の `effect` 区画から `share_text` 画面用に配線される effect ドライバ
/// （上記 4 OS 以外版。Linux は常に `Unsupported`）。
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
pub fn share_text_effect(state: &ShareTextState, sender: &CompletionSender<ShareTextState>) {
    drive(state, sender, &crate::contract::UnsupportedShare);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    use fandhe_core::screens::share_text::{set_input, set_url, share};

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    fn sender_with_channel() -> (
        CompletionSender<ShareTextState>,
        std::sync::mpsc::Receiver<fandhe_runtime::Completion<ShareTextState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            CompletionSender::new(tx, std::sync::Arc::new(NoopWaker)),
            rx,
        )
    }

    /// 起床を行わない起床口（テストは受信側で完了を直接読む）。
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    struct NoopWaker;

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _: std::time::Duration) {}
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    fn apply_all(
        state: &mut ShareTextState,
        rx: &std::sync::mpsc::Receiver<fandhe_runtime::Completion<ShareTextState>>,
    ) {
        while let Ok(apply) = rx.try_recv() {
            apply(state);
        }
    }

    #[test]
    fn to_outcome_maps_all_variants() {
        assert_eq!(
            to_outcome(Ok(DeviceShareOutcome::Shared)),
            Ok(CoreShareOutcome::Shared)
        );
        assert_eq!(
            to_outcome(Ok(DeviceShareOutcome::Cancelled)),
            Ok(CoreShareOutcome::Cancelled)
        );
        assert_eq!(
            to_outcome(Ok(DeviceShareOutcome::NoTarget)),
            Ok(CoreShareOutcome::NoTarget)
        );
        assert_eq!(
            to_outcome(Ok(DeviceShareOutcome::Unknown)),
            Ok(CoreShareOutcome::Unknown)
        );
    }

    #[test]
    fn to_outcome_maps_unsupported_to_fixed_message() {
        let result = to_outcome(Err(ShareSheetError::Unsupported));
        assert_eq!(result, Err(UNAVAILABLE.to_string()));
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    #[test]
    fn outcome_from_code_maps_known_codes() {
        assert!(matches!(
            outcome_from_code(0),
            Ok(DeviceShareOutcome::Shared)
        ));
        assert!(matches!(
            outcome_from_code(1),
            Ok(DeviceShareOutcome::Cancelled)
        ));
        assert!(matches!(
            outcome_from_code(2),
            Ok(DeviceShareOutcome::NoTarget)
        ));
        assert!(matches!(
            outcome_from_code(3),
            Err(ShareSheetError::Internal)
        ));
        assert!(matches!(
            outcome_from_code(4),
            Ok(DeviceShareOutcome::Unknown)
        ));
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    #[test]
    fn outcome_from_code_treats_unknown_code_as_internal_error() {
        assert!(matches!(
            outcome_from_code(99),
            Err(ShareSheetError::Internal)
        ));
    }

    #[test]
    fn cancel_due_requires_lost_focus_and_grace_elapsed() {
        let now = std::time::Instant::now();
        let grace = std::time::Duration::from_millis(500);
        assert!(!cancel_due(false, Some(now), now, grace), "未喪失は対象外");
        assert!(!cancel_due(true, None, now, grace), "未復帰は対象外");
        assert!(
            !cancel_due(true, Some(now), now, grace),
            "復帰直後（猶予未経過）は対象外"
        );
        let past = now.checked_sub(grace).expect("減算できる");
        assert!(
            cancel_due(true, Some(past), now, grace),
            "喪失 → 復帰 → 猶予経過でキャンセル扱い"
        );
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let state = ShareTextState::default();
        drive(&state, &sender, &crate::contract::UnsupportedShare);
        assert!(rx.try_recv().is_err());
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    #[test]
    fn drive_reports_unavailable_without_success_wording() {
        let (sender, rx) = sender_with_channel();
        let mut state = ShareTextState::default();
        set_input(&mut state, "hello".to_string());
        share(&mut state);

        drive(&state, &sender, &crate::contract::UnsupportedShare);
        apply_all(&mut state, &rx);

        assert!(state.error.contains(UNAVAILABLE), "{}", state.error);
        assert_eq!(state.status, "");
        assert_ne!(
            state.status,
            fandhe_core::screens::share_text::SHARED_STATUS
        );
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    #[test]
    fn drive_reports_invalid_url_as_error() {
        struct AlwaysShared;
        impl Share for AlwaysShared {
            fn share(
                &self,
                _request: &ShareRequest,
            ) -> Result<DeviceShareOutcome, ShareSheetError> {
                Ok(DeviceShareOutcome::Shared)
            }
        }

        let (sender, rx) = sender_with_channel();
        let mut state = ShareTextState::default();
        set_input(&mut state, "hello".to_string());
        set_url(&mut state, "file:///etc/passwd".to_string());
        share(&mut state);

        drive(&state, &sender, &AlwaysShared);
        apply_all(&mut state, &rx);

        assert!(state.error.contains("入力が不正です"), "{}", state.error);
        assert_ne!(
            state.status,
            fandhe_core::screens::share_text::SHARED_STATUS
        );
    }
}

/// [`bridge`] の単体テスト。要求の追跡そのもの（採番・first-wins・照合）は
/// `fandhe-runtime/src/request_slot.rs::RequestSlot` 側で検証済みのため、ここでは facade が担う
/// 「`deliver` 境界での `to_outcome` 変換」と、画面と公開入口の slot の共有を確かめる。OS 側実装を要さず、
/// `bridge` を `test` でもコンパイルするためホストの `cargo test` で回せる。slot は static なので直列に走らせる。
#[cfg(test)]
mod bridge_tests {
    use super::bridge;
    use crate::contract::ShareOutcome as DeviceShareOutcome;
    use crate::error::ShareSheetError;
    use fandhe_core::screens::share_text::ShareOutcome as CoreShareOutcome;
    use std::sync::Mutex;

    static SERIAL: Mutex<()> = Mutex::new(());

    fn clear() {
        bridge::SLOT.reset();
        bridge::SCREEN_OWNER.release(bridge::SCREEN_OWNER.current());
        bridge::PUBLIC_OWNER.release(bridge::PUBLIC_OWNER.current());
    }

    #[test]
    fn deliver_converts_raw_result_via_to_outcome() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let request_id = bridge::SLOT.begin();
        assert!(bridge::deliver(request_id, Ok(DeviceShareOutcome::Shared)));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済みなので Some");
        assert_eq!(ready, Some(Ok(CoreShareOutcome::Shared)));
        clear();
    }

    #[test]
    fn public_result_is_returned_once_and_kept_from_screen() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let public = bridge::begin_public().unwrap();
        let mut waited = 0;
        assert_eq!(bridge::poll_public(public, |_| waited += 1), None);
        assert_eq!(waited, 1, "結果待ちの間だけキャンセル検知を呼ぶ");
        assert!(
            bridge::begin_screen().is_none(),
            "公開側の進行中は画面が置き換えない"
        );
        assert!(bridge::begin_public().is_none());
        assert!(bridge::deliver(public, Ok(DeviceShareOutcome::Cancelled)));
        assert!(
            bridge::begin_screen().is_none(),
            "公開側の未取得の結果も守る"
        );
        assert_eq!(
            bridge::poll_public(public, |_| panic!("届いた後は観測しない")),
            Some(Ok(DeviceShareOutcome::Cancelled))
        );
        assert_eq!(bridge::poll_public(public, |_| {}), None, "結果は 1 度だけ");
        assert!(bridge::begin_screen().is_some());
        clear();
    }

    #[test]
    fn public_begin_parks_screen_result_and_screen_keeps_replacing_its_own() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let first = bridge::begin_screen().unwrap();
        let second = bridge::begin_screen().expect("画面は自分の前の要求を置き換える");
        assert!(bridge::begin_public().is_none(), "画面の進行中は busy");
        assert!(!bridge::deliver(first, Ok(DeviceShareOutcome::Shared)));
        assert!(bridge::deliver(second, Ok(DeviceShareOutcome::Shared)));
        let public = bridge::begin_public().expect("画面の未取得の結果で busy にならない");
        assert_eq!(
            bridge::SLOT.snapshot_of(second),
            Some(Some(Ok(CoreShareOutcome::Shared))),
            "画面の結果は受け取り待ちへ"
        );
        assert!(bridge::discard_public(public));
        assert!(!bridge::discard_public(public), "2 回目は何もしない");
        assert_eq!(bridge::poll_public(public, |_| {}), None);
        assert!(
            !bridge::deliver(public, Ok(DeviceShareOutcome::Shared)),
            "捨てた後の結果は捨てる"
        );
        clear();
    }

    #[test]
    fn abort_and_lost_public_request() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let public = bridge::begin_public().unwrap();
        bridge::abort_public(public);
        assert!(bridge::SLOT.snapshot().is_none());
        assert_eq!(bridge::poll_public(public, |_| {}), None);
        let public = bridge::begin_public().unwrap();
        bridge::SLOT.reset();
        assert_eq!(
            bridge::poll_public(public, |_| {}),
            Some(Err(ShareSheetError::Internal)),
            "失われた要求は 1 度だけ失敗"
        );
        assert_eq!(bridge::poll_public(public, |_| {}), None);
        assert_eq!(
            bridge::to_device(Err("x".to_string())),
            Err(ShareSheetError::Internal)
        );
        clear();
    }

    /// 公開側のキャンセル検知（`on_waiting`）が届けた取り消しは、同じ poll で 1 度だけ返り、画面が始められる。
    #[test]
    fn public_cancel_from_waiting_is_returned_by_same_poll() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let public = bridge::begin_public().unwrap();
        assert_eq!(
            bridge::poll_public(public, |rid| {
                assert!(bridge::deliver(rid, Ok(DeviceShareOutcome::Cancelled)));
            }),
            Some(Ok(DeviceShareOutcome::Cancelled))
        );
        assert_eq!(bridge::poll_public(public, |_| {}), None);
        assert!(bridge::begin_screen().is_some());
        clear();
    }

    /// Windows 以外の公開入口は成功へ倒さない。
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn public_entry_is_unsupported_off_windows() {
        let req = crate::contract::ShareRequest::new("x", None).unwrap();
        assert_eq!(
            super::begin_share_sheet(&req),
            Err(ShareSheetError::Unsupported)
        );
        assert_eq!(super::poll_share_sheet(1), None);
        assert!(!super::discard_share_sheet(1));
    }
}
