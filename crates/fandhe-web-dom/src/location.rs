//! 型サンプル「位置情報」の Web 経路（Issue #891）。
//!
//! `fandhe_core::screens::location_show`（端末依存ゼロ）の状態を
//! [`super::mount::mount_with_effect`] の effect として観測し、
//! `navigator.geolocation.watchPosition`（継続 push 型、`Motion` と同じ束）で
//! 測位して結果を書き戻す。`fandhe-device` の `Location` トレイトは Windows 実装
//! （Issue #888）のみが使う経路で、Web は `clipboard.rs`/`tts.rs`/`file_picker.rs` と
//! 同じ理由（`fandhe-egui-proxy` の feature `device` が wasm32 に依存を持ち込まない設計）で
//! これを通らず、本モジュールが DOM レンダラ側の対応実装にあたる。
//! ただし座標の妥当性検証だけは [`fandhe_device::location::is_valid_fix`] を共有する
//! （検証ロジックの SSOT を 1 箇所に保つ）。
//!
//! # ブラウザ側の失敗種別の対応（`fandhe_device::location` モジュール doc の対応表と対）
//!
//! | `PositionError.code` | 画面の [`FailureKind`] |
//! |---|---|
//! | `PERMISSION_DENIED`（1） | `PermissionDenied` |
//! | `POSITION_UNAVAILABLE`（2） / `TIMEOUT`（3） / その他 | `Retryable` |
//!
//! `navigator.geolocation` 自体が無い・secure context 外・`watchPosition` の起動失敗は
//! `Unsupported` へ倒す。Web には「OS の位置情報サービスが無効」を区別する API が無いため
//! `ServiceDisabled`（対応表の `Disabled` 表示）へは写さない。
//!
//! 座標・`PositionError.message()` は `console` 出力・保存・送信を一切行わない
//! （拒否理由の文言にブラウザ・環境情報が乗り得るため、security.md A09。
//! `clipboard.rs::to_message` と同じ方針）。
//!
//! # 純関数を `wasm32` gate の外に置く理由
//!
//! [`failure_kind_from_code`] と [`is_retry_request`] はコード → 種別の対応・
//! 再試行検知という、ブラウザ API を呼ばない判定ロジックのみを持つ。
//! `fandhe-egui-proxy` 側の `device/location.rs::is_retry_request` と同じ観点で
//! native の `cargo test` から固定できるようにするため、他モジュールと異なり
//! ブラウザ API を直接使う部分（[`mod@web`]）だけを `wasm32` へ gate する。

use fandhe_core::screens::location_show::{FailureKind, LocationView};

/// ブラウザの `PositionError.code`（1 = `PERMISSION_DENIED` / 2 = `POSITION_UNAVAILABLE` /
/// 3 = `TIMEOUT`）を画面の [`FailureKind`] へ写す純関数（モジュール doc の対応表参照）。
// native ビルドでは `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る
// （`fandhe-egui-proxy::device::location::is_retry_request` と同じ理由）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_kind_from_code(code: u16) -> FailureKind {
    match code {
        1 => FailureKind::PermissionDenied,
        _ => FailureKind::Retryable,
    }
}

/// 直前・今回の `state.view` から「再試行ボタン押下」を判定する純関数。
/// 表示済みの失敗（`Waiting` 以外）から今回 `Waiting` へ戻った場合のみ真とし、
/// 起動直後（前回も今回も `Waiting`）を誤検知しない
/// （`fandhe-egui-proxy` の `device/location.rs::is_retry_request` と同じ判定基準）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn is_retry_request(current_view: LocationView, previous_view: LocationView) -> bool {
    current_view == LocationView::Waiting && previous_view != LocationView::Waiting
}

/// [`FailureKind`] を対応する [`LocationView`] へ写す（`location_show::apply_failure` と
/// 同じ対応表）。effect 側で「前回書き戻した表示」を覚えて重複 push を抑止するための
/// 比較にのみ使う（画面状態そのものの更新は必ず `apply_failure` を経由する）。
#[cfg(target_arch = "wasm32")]
fn failure_view(kind: FailureKind) -> LocationView {
    match kind {
        FailureKind::PermissionDenied => LocationView::Denied,
        FailureKind::ServiceDisabled => LocationView::Disabled,
        FailureKind::Unsupported => LocationView::Unavailable,
        FailureKind::Retryable => LocationView::Retryable,
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::location_show::{
        apply_failure, apply_fix, FailureKind, LocationShowState, LocationView,
    };
    use fandhe_device::location::{is_valid_fix, Fix};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    use super::{failure_kind_from_code, failure_view, is_retry_request};
    use crate::mount::Applier;

    /// 測位中の watch 1 本ぶんの状態。`Closure` はブラウザから呼ばれ続ける間、
    /// 生存させておく必要がある（`tts.rs::SpeakHandlers` と同じ理由）。
    struct Watch {
        geolocation: web_sys::Geolocation,
        watch_id: i32,
        #[allow(dead_code)]
        on_success: Closure<dyn FnMut(JsValue)>,
        #[allow(dead_code)]
        on_error: Closure<dyn FnMut(JsValue)>,
    }

    /// マウント単位のエントリ。`watch: None` は「起動不能で `Unsupported` を
    /// 表示済み」を表し、次回以降の effect 呼び出しを no-op にする
    /// （`location_show_effect` の doc 参照）。
    struct Entry {
        watch: Option<Watch>,
        /// この watch の世代番号（新しい watch を起動するたび `allocate_generation` から
        /// 払い出す。`NEXT_GENERATION` の doc 参照）。
        /// `stop` は `Closure` を drop せず `forget` する（doc 参照）ため、
        /// `clear_watch` 後もブラウザ側に届く遅延コールバックが発火し得る。
        /// 同じ `mount_id` に新しい `Entry` を即挿入すると、
        /// 「`ACTIVE` にエントリが無ければ握り潰す」だけでは新旧の watch を
        /// 区別できず、旧コールバックが新 watch の `last_applied` を上書きし得る
        /// （Bugbot 指摘・PR #895）。`allocate_generation` は `ACTIVE` の状態に関わらず
        /// 単調増加するため、`stop_all_location_watches` を挟んだ再開（`pagehide` →
        /// `pageshow`）でも世代番号が再利用されない（codex-review P1 指摘・PR #895）。
        /// コールバック側は自身が捕捉した世代番号を現在の `Entry.generation` と
        /// 突き合わせ、不一致なら無視する。
        generation: u32,
        /// 本モジュールが最後に画面へ反映した表示（重複 push の抑止・
        /// 再試行検知の両方に使う）。
        last_applied: LocationView,
    }

    thread_local! {
        /// マウント ID ごとの測位状態（`clipboard.rs::IN_FLIGHT` と同じくマウント単位で持つ。
        /// 同一ページへ本画面を 2 つマウントしたときに一方が他方の watch を止めないため）。
        static ACTIVE: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    thread_local! {
        /// マウント ID ごとに払い出し済みの次の世代番号。`ACTIVE` とは独立して残り続ける
        /// （`stop` は `ACTIVE` から `Entry` を削除するが、ここは削除しない）。
        /// `stop_all_location_watches`（`pagehide`）→ `resume_stopped_location_watches`
        /// （`pageshow`）の再開経路では `ACTIVE` にエントリが無くなるため、そこから
        /// 世代番号を導くと初回 watch と同じ 0 に戻ってしまい、`forget` で生存したままの
        /// 旧コールバックが世代チェックを通過し得る（codex-review P1 指摘・PR #895）。
        /// 新しい watch を起動するたび、ここから単調増加する番号を払い出す。
        static NEXT_GENERATION: RefCell<HashMap<u64, u32>> = RefCell::new(HashMap::new());
    }

    /// `mount_id` の watch に割り当てる次の世代番号を払い出す（`NEXT_GENERATION` の doc 参照）。
    /// `ACTIVE` の状態に関わらず単調増加するため、`stop` を挟んだ再開でも世代番号が
    /// 再利用されない。
    fn allocate_generation(mount_id: u64) -> u32 {
        NEXT_GENERATION.with(|next| {
            let mut map = next.borrow_mut();
            let slot = map.entry(mount_id).or_insert(0);
            let generation = *slot;
            *slot = slot.wrapping_add(1);
            generation
        })
    }

    thread_local! {
        /// `location_show_effect` が呼ばれるたびに更新する、マウント ID ごとの
        /// 直近の [`Applier`]。`pageshow`（bfcache 復帰）で watch を再開する際、
        /// effect は再描画からしか呼べない（モジュール doc 冒頭参照）ため、
        /// ここに控えた `applier.apply` で無害な再描画を起こして再入させる。
        /// `Applier` は内部の `Rc` を共有するだけの薄いハンドルで、マウントが
        /// 生存する限り有効（`mount.rs::Applier` の doc 参照）。
        static APPLIERS: RefCell<HashMap<u64, Applier<LocationShowState>>> =
            RefCell::new(HashMap::new());
    }

    /// `pagehide` / `pageshow` イベント購読クロージャの型
    /// （clippy::type_complexity 回避のためのエイリアス）。
    type PageLifecycleClosure = Closure<dyn FnMut(web_sys::Event)>;

    thread_local! {
        /// `pagehide` イベントへの購読解除ハンドル。初回 watch 開始時に一度だけ登録する
        /// （`mount.rs` にアンマウント API が無いため、画面を離れたあとの受け皿として
        /// ページ単位のイベントに頼る。out-of-scope-tracking.md 対象: マウント単位の
        /// 明示アンマウント API は別 issue の検討事項）。
        static PAGEHIDE: RefCell<Option<PageLifecycleClosure>> = const { RefCell::new(None) };

        /// `pageshow` イベントへの購読解除ハンドル（`PAGEHIDE` と対）。bfcache から
        /// 復帰したとき、`pagehide` が止めた watch を再開する
        /// （`register_pageshow_once` の doc 参照）。
        static PAGESHOW: RefCell<Option<PageLifecycleClosure>> = const { RefCell::new(None) };
    }

    /// `mount_id` の watch を止めて `ACTIVE` から除去する。
    ///
    /// `clear_watch` 呼び出し前後にブラウザ側で既にキューイング済みの
    /// `watchPosition` コールバックは、`clear_watch` の呼び出し後も呼ばれ得る
    /// （retry クリック・`pagehide` と競合するタイミング）。その時点で `Closure` を
    /// drop してしまうと、JS 側は dropped wasm-bindgen `Closure` を呼び出して例外を
    /// 投げ、wasm が abort し得る（Bugbot 指摘・PR #895）。`update_last_applied` は
    /// `ACTIVE` にエントリが無ければ遅延コールバックを握り潰す契約だが、それは
    /// Closure 自体が呼び出し可能である前提のため、ここでは drop せず `forget` して
    /// 生存を延長する。
    // ponytail: forget した Closure は再試行のたびにプロセス終了までリークする
    // （ユーザー操作起点の再試行回数が上限のため実害は小さい想定）。
    // upgrade path: 遅延コールバックが来ないと確認できるタイミング（次フレーム後等）
    // まで実際の drop を遅らせる方式に置き換える。
    fn stop(mount_id: u64) {
        ACTIVE.with(|active| {
            if let Some(Entry {
                watch: Some(watch), ..
            }) = active.borrow_mut().remove(&mount_id)
            {
                watch.geolocation.clear_watch(watch.watch_id);
                watch.on_success.forget();
                watch.on_error.forget();
            }
        });
    }

    /// 全マウントの watch を止める。`pagehide`（タブを離れる・bfcache への退避）から
    /// 呼ばれる契約。
    pub fn stop_all_location_watches() {
        let mount_ids: Vec<u64> = ACTIVE.with(|active| active.borrow().keys().copied().collect());
        for mount_id in mount_ids {
            stop(mount_id);
        }
    }

    /// `pagehide` 購読を（未登録なら）1 度だけ登録する。
    fn register_pagehide_once() {
        PAGEHIDE.with(|cell| {
            if cell.borrow().is_some() {
                return;
            }
            let Some(window) = web_sys::window() else {
                return;
            };
            let closure =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| {
                    stop_all_location_watches();
                });
            // 登録に失敗しても watch 自体は動くため、エラーは無視する
            // （panic を API 契約にしない、coding-rust.md 準拠）。
            let _ = window
                .add_event_listener_with_callback("pagehide", closure.as_ref().unchecked_ref());
            *cell.borrow_mut() = Some(closure);
        });
    }

    /// `ACTIVE` に watch が無い（`pagehide` で止められた、または元々起動不能だった）
    /// マウントのうち、[`APPLIERS`] に控えがあるものだけ無害な再描画を起こして
    /// `location_show_effect` を再入させる。effect 側は「`ACTIVE` にエントリが無い」を
    /// 新規マウントと同じ扱いで watch を起動し直すため、これだけで再開になる
    /// （`location_show_effect` の doc 参照）。
    fn resume_stopped_location_watches() {
        let stopped: Vec<Applier<LocationShowState>> = APPLIERS.with(|appliers| {
            let active_ids: std::collections::HashSet<u64> =
                ACTIVE.with(|active| active.borrow().keys().copied().collect());
            appliers
                .borrow()
                .iter()
                .filter(|(mount_id, _)| !active_ids.contains(mount_id))
                .map(|(_, applier)| applier.clone())
                .collect()
        });
        for applier in stopped {
            applier.apply(|_state| {});
        }
    }

    /// `pageshow` 購読を（未登録なら）1 度だけ登録する。`pagehide` で止めた watch を
    /// bfcache 復帰時に再開する（web.dev の bfcache ガイド「observe when a page is
    /// restored from bfcache」に従い `pageshow` を購読する。`event.persisted` の値は
    /// 判定に使わない ―― 通常ナビゲーションで watch が動いたまま `pageshow` を受けても
    /// `resume_stopped_location_watches` は「`ACTIVE` に無いマウント」だけを対象にする
    /// ため無害）。
    fn register_pageshow_once() {
        PAGESHOW.with(|cell| {
            if cell.borrow().is_some() {
                return;
            }
            let Some(window) = web_sys::window() else {
                return;
            };
            let closure =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| {
                    resume_stopped_location_watches();
                });
            // 登録に失敗しても初回起動時の watch 自体は動くため、エラーは無視する
            // （panic を API 契約にしない、coding-rust.md 準拠）。
            let _ = window
                .add_event_listener_with_callback("pageshow", closure.as_ref().unchecked_ref());
            *cell.borrow_mut() = Some(closure);
        });
    }

    /// `mount_id` のエントリを「起動不能」として先に登録してから `Unsupported` を書き戻す。
    /// 先に登録するのは、`apply` が起こす再描画で effect が再入したとき
    /// 「既にエントリがある」判定で no-op になるようにするため（登録しないと
    /// apply → 再描画 → effect → apply の無限ループになる、`location_show_effect` の
    /// doc 参照）。`generation` は呼び出し元（`location_show_effect`）が算出した
    /// 次世代番号をそのまま引き継ぐ（`Entry.generation` の doc 参照。単調増加を
    /// 保ち、以前の watch の遅延コールバックが偶然同じ番号に一致しないようにする）。
    fn mark_unavailable_and_report(
        mount_id: u64,
        generation: u32,
        applier: &Applier<LocationShowState>,
    ) {
        ACTIVE.with(|active| {
            active.borrow_mut().insert(
                mount_id,
                Entry {
                    watch: None,
                    generation,
                    last_applied: LocationView::Unavailable,
                },
            );
        });
        applier.apply(|s| apply_failure(s, FailureKind::Unsupported));
    }

    /// `mount_id` の `last_applied` を更新する。`generation` はコールバックが
    /// 起動時に捕捉した世代番号で、`ACTIVE` の現在のエントリと一致しなければ
    /// 何もせず `false` を返す（`stop` 済みで別 watch に世代が進んでいる、または
    /// エントリ自体が無い＝遅延コールバックを握り潰すケースの両方を包含する。
    /// `Entry.generation` の doc 参照）。
    /// 変化が無ければ `false`（effect 呼び出し側はこの場合 `applier.apply` を
    /// 呼ばず、無駄な再描画を避ける）。
    fn update_last_applied(mount_id: u64, generation: u32, view: LocationView) -> bool {
        ACTIVE.with(|active| match active.borrow_mut().get_mut(&mount_id) {
            Some(entry) if entry.generation == generation && entry.last_applied != view => {
                entry.last_applied = view;
                true
            }
            _ => false,
        })
    }

    /// `watchPosition` の成功コールバック。`value` は実行時には `Position`
    /// （`web-sys` の安定 API では callback の型が消去されるため、境界でキャストする）。
    fn handle_success(
        mount_id: u64,
        generation: u32,
        applier: &Applier<LocationShowState>,
        value: JsValue,
    ) {
        let position: web_sys::Position = value.unchecked_into();
        let coords = position.coords();
        let fix = Fix {
            latitude_deg: coords.latitude(),
            longitude_deg: coords.longitude(),
            accuracy_m: coords.accuracy(),
        };

        if is_valid_fix(fix) {
            let view = LocationView::Fixed {
                lat: fix.latitude_deg,
                lon: fix.longitude_deg,
                accuracy_m: fix.accuracy_m,
            };
            if update_last_applied(mount_id, generation, view) {
                applier.apply(move |s| {
                    apply_fix(s, fix.latitude_deg, fix.longitude_deg, fix.accuracy_m)
                });
            }
        } else if update_last_applied(mount_id, generation, LocationView::Retryable) {
            // 範囲外・非有限座標は `SharedLocation::push_fix` と同じ fail-safe で
            // 「取得できなかった」扱いへ倒す（security.md A08）。
            applier.apply(|s| apply_failure(s, FailureKind::Retryable));
        }
    }

    /// `watchPosition` の失敗コールバック。`value` は実行時には `PositionError`。
    fn handle_error(
        mount_id: u64,
        generation: u32,
        applier: &Applier<LocationShowState>,
        value: JsValue,
    ) {
        let error: web_sys::PositionError = value.unchecked_into();
        let kind = failure_kind_from_code(error.code());
        if update_last_applied(mount_id, generation, failure_view(kind)) {
            applier.apply(move |s| apply_failure(s, kind));
        }
    }

    /// `mount_with_effect` へ渡す effect。`location_show` 画面の測位を継続駆動する。
    ///
    /// 要求が無ければ何もしない契約（`mount.rs` の「effect フック」節）を、
    /// 「このマウントの watch が既にある（`ACTIVE` にエントリがある）なら、
    /// 再試行ボタン押下（[`is_retry_request`]）以外は no-op」という形で満たす。
    pub fn location_show_effect(state: &LocationShowState, applier: &Applier<LocationShowState>) {
        let mount_id = applier.mount_id();

        // `pageshow` から watch を再開できるよう、このマウントの `Applier` を控える
        // （`APPLIERS` の doc 参照。`Applier` は `Rc` の複製のみで軽量）。
        APPLIERS.with(|appliers| {
            appliers.borrow_mut().insert(mount_id, applier.clone());
        });

        let existing = ACTIVE.with(|active| {
            active
                .borrow()
                .get(&mount_id)
                .map(|e| (e.last_applied, e.generation))
        });

        let next_generation = match existing {
            Some((last_applied, _)) if is_retry_request(state.view, last_applied) => {
                stop(mount_id);
                allocate_generation(mount_id)
            }
            Some(_) => return,
            // `ACTIVE` にエントリが無い場合、本当に初回のマウントか、`stop_all_location_watches`
            // で止められた後の再開か（`resume_stopped_location_watches` 経由）は区別できない。
            // どちらでも `allocate_generation` から取るため、後者でも世代番号が 0 に戻らない
            // （`NEXT_GENERATION` の doc 参照）。
            None => allocate_generation(mount_id),
        };

        let secure_geolocation = web_sys::window()
            .filter(|w| w.is_secure_context())
            .and_then(|w| w.navigator().geolocation().ok());

        let Some(geolocation) = secure_geolocation else {
            mark_unavailable_and_report(mount_id, next_generation, applier);
            return;
        };

        let generation = next_generation;

        let options = web_sys::PositionOptions::new();
        // ロックできない環境で「取得中」のまま固まらせず、TIMEOUT → Retryable へ落とす
        // （Windows 実装の 30 秒と揃える）。
        options.set_timeout(30_000);
        // キャッシュされた古い座標を新規取得と偽らない。
        options.set_maximum_age(0);
        // 高精度測位（GPS 常時起動）は要求しない（必要最小限、security.md A05）。
        options.set_enable_high_accuracy(false);

        let applier_ok = applier.clone();
        let on_success = Closure::<dyn FnMut(JsValue)>::new(move |value: JsValue| {
            handle_success(mount_id, generation, &applier_ok, value);
        });
        let applier_err = applier.clone();
        let on_error = Closure::<dyn FnMut(JsValue)>::new(move |value: JsValue| {
            handle_error(mount_id, generation, &applier_err, value);
        });

        let start = geolocation.watch_position_with_error_callback_and_options(
            on_success.as_ref().unchecked_ref(),
            Some(on_error.as_ref().unchecked_ref()),
            &options,
        );

        match start {
            Ok(watch_id) => {
                ACTIVE.with(|active| {
                    active.borrow_mut().insert(
                        mount_id,
                        Entry {
                            watch: Some(Watch {
                                geolocation,
                                watch_id,
                                on_success,
                                on_error,
                            }),
                            generation,
                            last_applied: LocationView::Waiting,
                        },
                    );
                });
                register_pagehide_once();
                register_pageshow_once();
            }
            Err(_) => {
                mark_unavailable_and_report(mount_id, generation, applier);
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::{location_show_effect, stop_all_location_watches};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_kind_from_code_maps_permission_denied() {
        assert_eq!(failure_kind_from_code(1), FailureKind::PermissionDenied);
    }

    #[test]
    fn failure_kind_from_code_maps_position_unavailable_and_timeout_to_retryable() {
        assert_eq!(failure_kind_from_code(2), FailureKind::Retryable);
        assert_eq!(failure_kind_from_code(3), FailureKind::Retryable);
    }

    #[test]
    fn failure_kind_from_code_maps_unknown_code_to_retryable() {
        assert_eq!(failure_kind_from_code(99), FailureKind::Retryable);
    }

    #[test]
    fn is_retry_request_true_when_waiting_follows_a_failure() {
        assert!(is_retry_request(
            LocationView::Waiting,
            LocationView::Denied
        ));
        assert!(is_retry_request(
            LocationView::Waiting,
            LocationView::Retryable
        ));
    }

    #[test]
    fn is_retry_request_false_on_initial_waiting() {
        assert!(!is_retry_request(
            LocationView::Waiting,
            LocationView::Waiting
        ));
    }

    #[test]
    fn is_retry_request_false_when_current_is_not_waiting() {
        assert!(!is_retry_request(
            LocationView::Denied,
            LocationView::Waiting
        ));
    }
}
