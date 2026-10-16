//! iOS（CallKit）向け通話 UI 実装（Issue #277、親 #87 / #276 の b-ios）。
//! `device/call_ui.rs::platform_call_ui` の ios 経路から呼ばれ、`ios/fandhe_ios_call_ui.m`
//! （`CXProvider` / `CXProviderDelegate` / `CXCallController`。`build.rs` が iOS の feature `device`
//! 有効時にコンパイルし CallKit をリンクする）を C ABI で呼ぶ。dispatch・共通コア画面・trait 契約
//! （`fandhe_device::call_ui`）は変えない。call_id の照合・段階遷移・期限はドライバ側が持つ。
//!
//! # ABI
//!
//! - Rust → `.m`: `fandhe_ios_call_ui_report(call_id)` / `fandhe_ios_call_ui_end(call_id)`。どのスレッドから
//!   でも呼べ、`.m` が main queue へ非同期に渡す（同期で通知を返さない。ドライバのロックと再入しない）
//! - `.m` → Rust: `fandhe_rust_call_ui_event(call_id, kind, domain, code)`。main queue から呼ばれる。
//!   整数だけを渡し、kind は [`event_from`]、(domain, code) は [`failure_from`] が契約値へ写す
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 失敗・未知の kind・未知のコードを成功へ倒さない、`end` 後に届いた通知を捨てる、
//!   OS の生コードを 4 種へ全写像する
//! - 守らない: 背景での通話の継続、プロセス死、Bluetooth・車載機器の表示、複数通話
//!
//! # 既知の天井
//!
//! シミュレータでの CallKit の挙動、`unentitled` が実際に出るか、背景での扱いは実機の #275 で確かめる。

use fandhe_device::call_ui::{CallUiEvent, CallUiFailure, CallUiSink, EndReason};
use std::sync::Mutex;

/// `.m` の kind: 受理（`Ringing`）。
const KIND_RINGING: i32 = 1;
/// 応答。
const KIND_ANSWERED: i32 = 2;
/// 着信中の拒否（`Ended(Declined)`）。
const KIND_DECLINED: i32 = 3;
/// 通話中に OS 側で終了（`Ended(RemoteEnded)`）。
const KIND_REMOTE_ENDED: i32 = 4;
/// 報告エラー（domain・code つき）。6（`providerDidReset`）・7（action のタイムアウト）と
/// 未知の kind は `Failed`。
const KIND_REPORT_ERROR: i32 = 5;

/// `CXErrorDomainIncomingCall` を表す domain 値。
const DOMAIN_INCOMING_CALL: i32 = 1;

/// `CXErrorCodeIncomingCallError` の生値（SDK の `CXError.h`）。`.m` は生の NSInteger を渡すだけ。
/// 0 unknown・1 unentitled・2 callUUIDAlreadyExists は `Failed`（下の `_` 腕）。
const CODE_FILTERED_BY_DND: i64 = 3;
const CODE_FILTERED_BY_BLOCK_LIST: i64 = 4;
const CODE_FILTERED_RESTRICTED_SHARING: i64 = 5;
const CODE_CALL_IS_PROTECTED: i64 = 6;
const CODE_FILTERED_BY_SENSITIVE: i64 = 7;

/// 報告エラーを契約の失敗種別へ写す（コード表の SSOT）。OS が着信を拒んだ 5 種は `Disallowed`、
/// unknown・unentitled・callUUIDAlreadyExists・未知のコード・他 domain は `Failed`。
/// `unentitled` が実機で出たら `voip` の追加を #275 で判断する。
pub(crate) fn failure_from(domain: i32, code: i64) -> CallUiFailure {
    if domain != DOMAIN_INCOMING_CALL {
        return CallUiFailure::Failed;
    }
    match code {
        CODE_FILTERED_BY_DND
        | CODE_FILTERED_BY_BLOCK_LIST
        | CODE_FILTERED_RESTRICTED_SHARING
        | CODE_CALL_IS_PROTECTED
        | CODE_FILTERED_BY_SENSITIVE => CallUiFailure::Disallowed,
        _ => CallUiFailure::Failed,
    }
}

/// `.m` の通知を契約の通知へ写す。未知の kind は `Failed` で、成功へ倒さない。
pub(crate) fn event_from(call_id: u64, kind: i32, domain: i32, code: i64) -> CallUiEvent {
    match kind {
        KIND_RINGING => CallUiEvent::Ringing { call_id },
        KIND_ANSWERED => CallUiEvent::Answered { call_id },
        KIND_DECLINED => CallUiEvent::Ended {
            call_id,
            reason: EndReason::Declined,
        },
        KIND_REMOTE_ENDED => CallUiEvent::Ended {
            call_id,
            reason: EndReason::RemoteEnded,
        },
        KIND_REPORT_ERROR => CallUiEvent::Failed {
            call_id,
            failure: failure_from(domain, code),
        },
        _ => CallUiEvent::Failed {
            call_id,
            failure: CallUiFailure::Failed,
        },
    }
}

/// 現在の通話 1 件の届け先。`.m` からの通知を、現在の call_id と一致するときだけ sink へ流す。
#[derive(Default)]
pub(crate) struct Router {
    cur: Mutex<Option<(u64, CallUiSink)>>,
}

impl Router {
    /// 新しい通話の届け先を登録する（前の登録は置き換わる）。
    pub(crate) fn set(&self, id: u64, sink: CallUiSink) {
        *self.cur.lock().unwrap_or_else(|e| e.into_inner()) = Some((id, sink));
    }

    /// `id` が現在のものなら登録を外す。
    pub(crate) fn clear_if(&self, id: u64) {
        let mut g = self.cur.lock().unwrap_or_else(|e| e.into_inner());
        if g.as_ref().is_some_and(|(cur, _)| *cur == id) {
            *g = None;
        }
    }

    /// `id` が現在のときだけ sink を呼ぶ（ロックの外で）。終端の通知なら登録を外す。
    /// 戻り値は `None` = 現在の通話でないので捨てた、`Some(b)` = sink へ渡し、`b` は sink の戻り値
    /// （`false` は受け手の消失）。
    pub(crate) fn deliver(&self, id: u64, ev: CallUiEvent) -> Option<bool> {
        let sink = {
            let mut g = self.cur.lock().unwrap_or_else(|e| e.into_inner());
            let (cur, sink) = g.as_ref()?;
            if *cur != id {
                return None;
            }
            let sink = sink.clone();
            if matches!(ev, CallUiEvent::Ended { .. } | CallUiEvent::Failed { .. }) {
                *g = None;
            }
            sink
        };
        Some(sink(ev))
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use super::*;
    use fandhe_device::call_ui::CallUi;
    use std::sync::{Arc, OnceLock};

    static ROUTER: OnceLock<Router> = OnceLock::new();

    pub(super) fn router() -> &'static Router {
        ROUTER.get_or_init(Router::default)
    }

    // `ios/fandhe_ios_call_ui.m` が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_call_ui_report(call_id: u64);
        fn fandhe_ios_call_ui_end(call_id: u64);
    }

    struct IosCallUi;

    impl CallUi for IosCallUi {
        fn report_incoming(&self, call_id: u64, sink: CallUiSink) -> Result<(), CallUiFailure> {
            router().set(call_id, sink);
            // SAFETY: 引数は整数だけで、所有権の移転もポインタの受け渡しも無い。`.m` は main queue へ
            // 非同期に渡して即戻る。
            #[allow(unsafe_code)]
            unsafe {
                fandhe_ios_call_ui_report(call_id)
            };
            Ok(())
        }

        fn end(&self, call_id: u64) {
            router().clear_if(call_id);
            // SAFETY: 同上。冪等で、id 不一致なら `.m` 側で何もしない。
            #[allow(unsafe_code)]
            unsafe {
                fandhe_ios_call_ui_end(call_id)
            };
        }
    }

    /// ネイティブの通話を閉じる（冪等。id 不一致なら `.m` 側で何もしない）。
    pub(super) fn end_native(call_id: u64) {
        // SAFETY: 引数は整数だけ。`.m` は main queue へ非同期に渡して即戻る。
        #[allow(unsafe_code)]
        unsafe {
            fandhe_ios_call_ui_end(call_id)
        };
    }

    pub(super) fn implementation() -> Arc<dyn CallUi> {
        Arc::new(IosCallUi)
    }
}

/// この OS の `CallUi` 実装。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::call_ui::CallUi> {
    imp::implementation()
}

/// `.m` からの通知の入口（main queue）。登録中の通話の sink へ流した直後に再描画を要求する
/// （入力が無いと egui が描画せず画面が止まるため。契約表 4）。panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_call_ui_event(call_id: u64, kind: i32, domain: i32, code: i64) {
    if imp::router().deliver(call_id, event_from(call_id, kind, domain, code)) == Some(false) {
        // 受け手が消えた。登録を外し、OS の通話を閉じる（Active のまま残さない）。
        imp::router().clear_if(call_id);
        imp::end_native(call_id);
    }
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn failure_table_covers_all_codes() {
        for c in 3..=7 {
            assert_eq!(failure_from(1, c), CallUiFailure::Disallowed, "code {c}");
        }
        for c in [0, 1, 2, 8, 99, -1] {
            assert_eq!(failure_from(1, c), CallUiFailure::Failed, "code {c}");
        }
        assert_eq!(failure_from(0, 3), CallUiFailure::Failed);
    }

    #[test]
    fn unknown_kind_is_never_success() {
        for k in [0, 6, 7, 8, -1, 999] {
            assert_eq!(
                event_from(1, k, 0, 0),
                CallUiEvent::Failed {
                    call_id: 1,
                    failure: CallUiFailure::Failed
                }
            );
        }
        assert_eq!(
            event_from(1, 3, 0, 0),
            CallUiEvent::Ended {
                call_id: 1,
                reason: EndReason::Declined
            }
        );
        assert_eq!(
            event_from(1, 5, 1, 3),
            CallUiEvent::Failed {
                call_id: 1,
                failure: CallUiFailure::Disallowed
            }
        );
    }

    fn rec() -> (CallUiSink, Arc<Mutex<Vec<CallUiEvent>>>) {
        let v = Arc::new(Mutex::new(Vec::new()));
        let v2 = v.clone();
        (
            Arc::new(move |e| {
                v2.lock().unwrap().push(e);
                true
            }),
            v,
        )
    }

    #[test]
    fn router_drops_mismatch_and_clears_on_terminal() {
        let r = Router::default();
        let (sink, got) = rec();
        r.set(1, sink);
        assert_eq!(r.deliver(2, event_from(2, 1, 0, 0)), None);
        assert_eq!(r.deliver(1, event_from(1, 1, 0, 0)), Some(true));
        assert_eq!(r.deliver(1, event_from(1, 3, 0, 0)), Some(true));
        assert_eq!(r.deliver(1, event_from(1, 2, 0, 0)), None);
        assert_eq!(got.lock().unwrap().len(), 2);
    }

    #[test]
    fn router_drops_after_clear_if() {
        let r = Router::default();
        let (sink, got) = rec();
        r.set(1, sink);
        r.clear_if(2);
        r.clear_if(1);
        assert_eq!(r.deliver(1, event_from(1, 1, 0, 0)), None);
        assert!(got.lock().unwrap().is_empty());
    }

    #[test]
    fn router_sink_can_reenter_without_deadlock() {
        let r = Arc::new(Router::default());
        let r2 = r.clone();
        let sink: CallUiSink = Arc::new(move |_| {
            let (s, _) = rec();
            r2.set(9, s);
            true
        });
        r.set(1, sink);
        assert_eq!(r.deliver(1, event_from(1, 1, 0, 0)), Some(true));
    }

    #[test]
    fn router_reports_sink_rejection() {
        let r = Router::default();
        r.set(1, Arc::new(|_| false));
        assert_eq!(r.deliver(1, event_from(1, 1, 0, 0)), Some(false));
    }
}
