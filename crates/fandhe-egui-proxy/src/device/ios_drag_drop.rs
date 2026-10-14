//! iOS 向けドラッグ & ドロップ受け取り実装（Issue #1015、親 #1013）。`device/drag_drop.rs`
//! の `drag_drop_effect` iOS 分岐から呼ばれ、`UIDropInteraction`
//! （`ios/fandhe_ios_drag_drop.m`、`build.rs` が feature `device` 有効時のみ
//! コンパイル・リンクする）を `unsafe extern "C"` 1 関数（[`ensure_installed`] が
//! 呼ぶ [`fandhe_ios_drag_drop_install`]）で呼ぶ。`unsafe` はこの呼び出し 1 箇所に
//! 閉じ `// SAFETY:` で契約を明記する（`ios_notification.rs::present` と同型、
//! coding-rust.md の FFI 例外規定準拠）。
//!
//! `.m` 側は `UIDropInteractionDelegate` のホバー/ドロップ開始/1 件ずつの読み込み
//! 完了を、本モジュールが公開する 3 つの `extern "C"` 関数
//! （[`fandhe_rust_drag_drop_hover`]・[`fandhe_rust_drag_drop_begin`]・
//! [`fandhe_rust_drag_drop_item`]）経由で push する（pull 型ではない。
//! `ios_notification.rs::fandhe_rust_notification_outcome` と同じ橋渡し方式）。
//! 受けた内容は [`super::drop_session`]（ターゲット非依存の集約器）へそのまま渡し、
//! 状態機械への反映は `device/drag_drop.rs::drive_events` が毎フレーム
//! [`super::drop_session::take_events`] を drain して行う（本モジュールでは
//! `fandhe_core` の状態を直接書き換えない）。
//!
//! # 読み取りは completion スレッド上で同期に行う（一時ファイルの寿命）
//!
//! `NSItemProvider` の `loadFileRepresentationForTypeIdentifier:completionHandler:`
//! が渡す URL の一時ファイルは、completion が return した時点で OS が削除する契約
//! （Apple ドキュメント）。そのため [`fandhe_rust_drag_drop_item`] は、completion
//! （GCD の背景スレッド、main ではない）の中で同期に
//! `fandhe_device::drag_drop::read_dropped` を呼んで読み切る。自前の一時ファイル・
//! コピーは作らないため、契約表 9（失敗時のリソース解放）の対象は生じない。
//!
//! # 判定と更新は同じロックの内側（契約表 2）
//!
//! [`fandhe_rust_drag_drop_item`] は読み取り（I/O）をロックの外（本関数の中、
//! `super::drop_session::item` 呼び出し前）で終えてから、集約器へ結果を渡す
//! （`drop_session.rs` モジュール doc 参照。ロック保持中に I/O を行わない）。
//!
//! # 入力は信頼境界（A03、契約表 5「FFI 境界」）
//!
//! `.m` 側から届く `session`/`index`/`code`/`path`/`name` は外部入力として検証する:
//! null・長さ・`std::str::from_utf8` を確認し、未知の `code` は読めない扱いにする。
//!
//! ファイル名・パス・エラー詳細をログへ出力しない（security.md A09）。

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use fandhe_core::screens::drag_drop::{DragDropState, DropItem};
use fandhe_device::drag_drop::{read_dropped, DropError};

use crate::app::CompletionSender;

/// プロセス起動時刻（単調時計の基準点、`Instant` は絶対値を持たないため）。
/// [`now_ms`] からのみ参照する。
static START: OnceLock<Instant> = OnceLock::new();

/// 起動からの経過ミリ秒（単調増加）。[`ensure_installed`] の再試行間隔判定と
/// [`mark_active`]/[`fandhe_rust_drag_drop_is_active`] の画面有効期間判定の
/// 両方が使う共通の時計。
fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

// SAFETY: `ios/fandhe_ios_drag_drop.m` が定義する C ABI（`build.rs` が feature
// `device` 有効時のみコンパイル・リンクする）。契約は `.m` ファイル冒頭コメントおよび
// 下記 `ensure_installed` 実装のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_drag_drop_install() -> i32;
}

/// [`fandhe_ios_drag_drop_install`] を高々 1 回だけ呼んだか。成功時のみ `true` へ
/// 倒す（失敗時は次のフレームで再試行できるよう `false` のまま残す、契約表 4
/// 「送信を省略する分岐でも次回のポーリングを予約する」）。
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// インストール失敗時に一度だけ出す案内の接頭辞込み文言。同じ文言を毎フレーム
/// 連打しない判定に使う（下記 `ensure_installed` 参照）。
const INSTALL_FAILED_MESSAGE: &str = "失敗しました: ドロップの受け取りを準備できませんでした";

/// インストール失敗時の再試行間隔（ミリ秒）。この間隔より短い間隔で
/// [`ensure_installed`] が呼ばれても [`fandhe_ios_drag_drop_install`] を呼び直さず
/// 早期 return する（codex 指摘 #1019: 失敗が続く間、毎フレーム
/// `sender.send()` → `CompletionSender::send()` の `request_repaint()` が呼ばれ
/// 再描画が止まらない問題への対処、契約表 4）。
const INSTALL_RETRY_INTERVAL_MS: u64 = 1000;

/// 直近に [`fandhe_ios_drag_drop_install`] を試みた時刻（[`now_ms`] 基準）。0 は
/// 「一度も試みていない」を表す番兵（[`LAST_ACTIVE_MS`] と同じ考え方）。この番兵と
/// 実際の時刻 0 が衝突するのはプロセス起動直後の一瞬のみで実害は無い（同 doc 参照）。
static LAST_INSTALL_ATTEMPT_MS: AtomicU64 = AtomicU64::new(0);

/// `device/drag_drop.rs::drag_drop_effect` の iOS 分岐から毎フレーム呼ばれる。
/// [`INSTALLED`] が `false` の間だけ [`fandhe_ios_drag_drop_install`] を呼び、
/// 冪等な `UIDropInteraction` の取り付けを試みる（`.m` 側も
/// `objc_setAssociatedObject` 相当の static で二重付与を防ぐ二重の安全網、
/// `.m` ファイル冒頭コメント参照）。
///
/// 失敗時は画面へ「失敗しました: ...」を出す（契約表 6、黙ってダミー化しない）。
/// `state.error` が既に同じ文言なら送らない（同じ文言を連打しない、契約表 4
/// 「失敗状態のまま再描画し続けていないか」の逆: 再描画自体は続けるが completion
/// 送信は止める）。成功したら、直前まで同じ失敗文言が出ていた場合のみ消す
/// （利用者が既に読んだ他のエラーを上書きしない）。
#[allow(unsafe_code)]
pub(super) fn ensure_installed(sender: &CompletionSender<DragDropState>) {
    if INSTALLED.load(Ordering::SeqCst) {
        return;
    }

    // last == 0 は「一度も試みていない」番兵（`LAST_INSTALL_ATTEMPT_MS` doc 参照）。
    // 初回はここを飛ばして即座に試行する（codex/Bugbot 指摘 #1019: 番兵初期値の
    // せいで起動後 1 秒間、初回インストールがスロットル扱いでスキップされる問題
    // への対処）。
    let now = now_ms();
    let last = LAST_INSTALL_ATTEMPT_MS.load(Ordering::SeqCst);
    if last != 0 {
        let elapsed = now.saturating_sub(last);
        if elapsed < INSTALL_RETRY_INTERVAL_MS {
            // `drag_drop_effect` は egui が repaint した時だけ呼ばれるため、
            // アイドル中（利用者操作なし）は次フレームが保証されない。残りの
            // 再試行間隔ぶん明示的に repaint を予約し、送信を省略する分岐でも
            // 次回のポーリングを保証する（契約表 4、`location.rs::drive` と
            // 同じ考え方）。
            sender.request_repaint_after(std::time::Duration::from_millis(
                INSTALL_RETRY_INTERVAL_MS - elapsed,
            ));
            return;
        }
    }
    LAST_INSTALL_ATTEMPT_MS.store(now, Ordering::SeqCst);

    // SAFETY: 引数を取らない同期呼び出し。`.m` 側は呼び出しスレッドに関わらず
    // 内部で `dispatch_async(main)` へ委譲してから return するため、本呼び出し自体は
    // どのスレッドから行っても安全（`.m` ファイル冒頭コメント参照）。戻り値は
    // 「取り付けを依頼できたか」の同期判定のみで、実際の取り付け完了を待たない。
    let code = unsafe { fandhe_ios_drag_drop_install() };
    if code == 0 {
        INSTALLED.store(true, Ordering::SeqCst);
        let _ = sender.send(Box::new(|s: &mut DragDropState| {
            if s.error == INSTALL_FAILED_MESSAGE {
                s.error.clear();
            }
        }));
        return;
    }

    let _ = sender.send(Box::new(|s: &mut DragDropState| {
        if s.error != INSTALL_FAILED_MESSAGE {
            s.error = INSTALL_FAILED_MESSAGE.to_string();
        }
    }));
}

/// 画面が現在有効（表示中）とみなす期間（ミリ秒）。[`mark_active`] の呼び出し間隔
/// （`drag_drop_effect` からの毎フレーム呼び出し、契約表 4）より十分長く取り、
/// フレーム落ちを誤検知しない一方、画面遷移で呼び出しが止まってからの検知遅延は
/// 許容範囲に収める。
const SCREEN_ACTIVE_WINDOW_MS: u64 = 500;

/// [`mark_active`] が repaint を予約する間隔（ミリ秒）。[`SCREEN_ACTIVE_WINDOW_MS`]
/// より十分短く取り、利用者操作が無いアイドル中でも有効期間が切れる前に必ず
/// 次の [`mark_active`] 呼び出し（= 次フレーム）が来るようにする（codex/Bugbot
/// 指摘 #1019: egui は repaint 要求が無いとアイドル中 `drag_drop_effect` 自体が
/// 呼ばれず、`mark_active` が更新されないまま有効期間が切れて最初のドロップを
/// 拒否する問題への対処、契約表 1・4）。
const SCREEN_ACTIVE_POLL_INTERVAL_MS: u64 = SCREEN_ACTIVE_WINDOW_MS / 2;

/// 直近に [`mark_active`] が呼ばれた時刻（[`now_ms`] 基準）。0 は「一度も
/// 呼ばれていない」を表す番兵（`now_ms() == 0` はプロセス起動直後の一瞬のみで、
/// その間はまだ画面表示前のため実害は無い）。
static LAST_ACTIVE_MS: AtomicU64 = AtomicU64::new(0);

/// `device/drag_drop.rs::drag_drop_effect` の iOS 分岐から毎フレーム呼ばれる。
/// `UIDropInteraction` はウィンドウ全体へ一度取り付けたら本モジュールの
/// `INSTALLED` が `true` の間ずっと delegate が動き続けるため（本ファイル冒頭
/// コメント「ライフサイクル・登録トークン」参照）、この画面の effect が実際に
/// 駆動されている（= 画面が表示中の）期間を別途記録し、
/// [`fandhe_rust_drag_drop_is_active`] 経由で `.m` 側へ伝える（codex 指摘 #1019:
/// 別画面表示中に受けたドロップを本画面の結果として誤表示する問題への対処、
/// 契約表 1）。現状の `screens/registry.rs` は 1 プロセス 1 画面構成のため
/// 到達しないが、将来 `ScreenStack` 等で複数画面が同一プロセス内に共存する
/// 構成になっても壊れないよう、画面側の生存期間を境界として持たせる。
///
/// [`SCREEN_ACTIVE_POLL_INTERVAL_MS`] 後の repaint も併せて予約し、利用者操作の
/// 有無に関わらずこの画面が表示され続ける限り有効期間を切らさない（doc 上記参照）。
pub(super) fn mark_active(sender: &CompletionSender<DragDropState>) {
    LAST_ACTIVE_MS.store(now_ms(), Ordering::SeqCst);
    sender.request_repaint_after(std::time::Duration::from_millis(
        SCREEN_ACTIVE_POLL_INTERVAL_MS,
    ));
}

/// `.m` 側の `canHandleSession:`/`performDrop:`/`sessionDidEnter:` 等から呼ばれ、
/// この画面が「直近 [`SCREEN_ACTIVE_WINDOW_MS`] 以内に [`mark_active`] された
/// (= 表示中の)」かどうかを返す。非表示中は `.m` 側がセッションを受け付けず
/// （`canHandleSession:` が `NO`）、ホバー/ドロップのどちらも Rust 側（延いては
/// [`super::drop_session`] の集約キュー）へ積まれない（契約表 1「画面外イベントを
/// 破棄する」）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_drag_drop_is_active() -> i32 {
    let last = LAST_ACTIVE_MS.load(Ordering::SeqCst);
    i32::from(now_ms().saturating_sub(last) < SCREEN_ACTIVE_WINDOW_MS)
}

/// `.m` 側 `sessionDidEnter:`/`sessionDidExit:`/`sessionDidEnd:` から呼ばれる
/// `extern "C"` 関数。`entered != 0` を「領域内に入った」として
/// [`super::drop_session::hover`] へ委譲する。
///
/// egui の描画ループとは非同期の経路（UIKit のジェスチャ処理スレッド）のため、
/// [`crate::ios::request_repaint`] で次フレームを明示的に起こす
/// （`ios_notification.rs::fandhe_rust_notification_outcome` と同じ理由）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_drag_drop_hover(entered: i32) {
    super::drop_session::hover(entered != 0);
    crate::ios::request_repaint();
}

/// `.m` 側 `performDrop:` の冒頭から呼ばれる `extern "C"` 関数。`total` 件のうち
/// [`super::drop_session::begin`] が計算した `to_load` 件だけを `.m` 側に読み込ませる
/// （上限は `fandhe_device::drag_drop::MAX_DROP_FILES` が SSOT、`.m` 側に定数を
/// 複製しない）。
///
/// `out_session`/`out_to_load` が null なら何もせず戻る（契約違反を検出した経路。
/// `.m` 側は常に non-null なスタック変数へのポインタを渡す契約）。
///
/// # Safety
///
/// `out_session`/`out_to_load` は呼び出し中だけ有効な書き込み可能なポインタで
/// あること（`.m` 側の呼び出し規約）。本関数は指す先の型（`u64`/`usize`）として
/// 1 回だけ書き込み、ポインタの所有権は受け取らない。
#[allow(unsafe_code)]
#[no_mangle]
unsafe extern "C" fn fandhe_rust_drag_drop_begin(
    total: usize,
    out_session: *mut u64,
    out_to_load: *mut usize,
) {
    if out_session.is_null() || out_to_load.is_null() {
        return;
    }
    let (session, to_load) = super::drop_session::begin(total);
    // SAFETY: 呼び出し元契約（本関数 doc の `# Safety`）により、両ポインタは
    // この呼び出しの間だけ有効な書き込み可能な `u64`/`usize` を指す。上で
    // null チェック済み。
    unsafe {
        *out_session = session;
        *out_to_load = to_load;
    }
    crate::ios::request_repaint();
}

/// `.m` 側の `loadFileRepresentationForTypeIdentifier:completionHandler:` completion
/// （またはフォルダ/読めない判定）から、読み込んだアイテム 1 件ぶんごとに呼ばれる
/// `extern "C"` 関数。
///
/// `code` の意味（`.m` ファイル冒頭コメントと対）:
/// - `0`: 通常ファイル。`path_ptr`/`path_len` が UTF-8 の絶対パス
///   （`suggestedName`/`name_ptr`/`name_len` は表示名のヒント、空でもよい）
/// - `1`: フォルダだった（`UTTypeDirectory` に適合）
/// - `2`: 読めなかった（型が無い・`loadFileRepresentation` がエラー等）
/// - それ以外: 契約違反として `2` と同様に扱う
///
/// 読み取り（`read_dropped`）は本関数の中で同期に行う（モジュール doc「読み取りは
/// completion スレッド上で同期に行う」参照）。
///
/// # Safety
///
/// `path_ptr`/`name_ptr` はそれぞれ `path_len`/`name_len` バイトぶん有効な、
/// この呼び出しの間だけ生存する読み取り専用バッファであること（`.m` 側の
/// 呼び出し規約）。`code != 0` のときはどちらも参照しない契約だが、本関数は
/// 念のため常に null/長さを確認してから読む。
#[allow(unsafe_code)]
#[no_mangle]
unsafe extern "C" fn fandhe_rust_drag_drop_item(
    session: u64,
    index: usize,
    code: i32,
    path_ptr: *const u8,
    path_len: usize,
    name_ptr: *const u8,
    name_len: usize,
) {
    // SAFETY: 呼び出し元契約（本関数 doc の `# Safety`）により、null でなければ
    // それぞれの長さぶん有効な読み取り専用バッファを指す。`from_utf8` が失敗すれば
    // `None`（表示名なし/読めない扱い）として扱う（信頼境界の入力検証、A03）。
    let suggested_name = unsafe { bytes_to_str(name_ptr, name_len) }.unwrap_or("");
    let path = unsafe { bytes_to_str(path_ptr, path_len) };

    let item = build_item(code, path, suggested_name);
    super::drop_session::item(session, index, item);
    crate::ios::request_repaint();
}

/// [`fandhe_rust_drag_drop_item`] 本体の、FFI・`unsafe` を含まない純粋な分岐部分
/// （ホストの `cargo test` で検証できるようにするための切り出し、`drive`/
/// `drive_events` と同じ方針）。`code`/`path`/`suggested_name` は既に検証済みの
/// 安全な値として受け取る（信頼境界の入力検証は呼び出し元で完了済み、A03）。
///
/// `code == 0`（通常ファイル）は [`read_dropped`] で読み切る（ここが唯一の実 I/O。
/// `path` が UTF-8 変換に失敗していた（`None`）場合は `Unreadable` として扱う）。
/// `code == 1` は [`DropError::Folder`]、それ以外（未知の `code` を含む）は
/// [`DropError::Unreadable`] にする（契約表 6「嘘の成功」: 未知の値を成功へ丸めない）。
fn build_item(code: i32, path: Option<&str>, suggested_name: &str) -> DropItem {
    match (code, path) {
        (0, Some(path_str)) => {
            let path = Path::new(path_str);
            match read_dropped(path) {
                Ok(picked) => DropItem {
                    name: display_name(suggested_name, Some(picked.name.as_str())),
                    result: Ok(picked.bytes.len()),
                },
                Err(e) => {
                    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                    DropItem {
                        name: display_name(suggested_name, file_name.as_deref()),
                        result: Err(e.to_string()),
                    }
                }
            }
        }
        (0, None) => DropItem {
            name: display_name(suggested_name, None),
            result: Err(DropError::Unreadable.to_string()),
        },
        (1, _) => DropItem {
            name: display_name(suggested_name, None),
            result: Err(DropError::Folder.to_string()),
        },
        _ => DropItem {
            name: display_name(suggested_name, None),
            result: Err(DropError::Unreadable.to_string()),
        },
    }
}

/// `ptr`/`len` から `&str` を組み立てる。null または UTF-8 変換失敗なら `None`
/// （信頼境界の入力検証、A03）。
///
/// # Safety
///
/// 呼び出し元が `ptr` を `len` バイトぶん有効な読み取り専用バッファとして
/// 渡すこと。`ptr` が null の場合は `len` を無視して `None` を返す。
#[allow(unsafe_code)]
unsafe fn bytes_to_str<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: 呼び出し元契約（本関数 doc の `# Safety`）により、`ptr` は `len`
    // バイトぶん有効な読み取り専用バッファ。
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    std::str::from_utf8(bytes).ok()
}

/// 表示名の決定。`suggested_name`（`.m` 側の `NSItemProvider.suggestedName`）を
/// [`Path::file_name`] で最終要素に絞って使い（ディレクトリ区切りを含む値が
/// 渡されてもフルパス相当の情報を表示に漏らさない）、空ならフォールバック
/// （`read_dropped` が返す実ファイル名、無ければ「（名前なし）」）を使う。
/// フルパス自体は表示・ログに出さない（security.md A09）。
fn display_name(suggested_name: &str, fallback: Option<&str>) -> String {
    let from_suggested = (!suggested_name.is_empty())
        .then(|| {
            Path::new(suggested_name)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .flatten();
    from_suggested
        .or_else(|| fallback.map(str::to_string))
        .unwrap_or_else(|| "（名前なし）".to_string())
}
