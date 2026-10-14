//! ウィジェットテストハーネス（TASK-8.3-S5 / REQ-8、Issue #100）。
//!
//! 設計文書 `docs/architecture/widget-architecture.md`（Issue #96）§4.4 が定義する
//! 7 状態ライフサイクル（宣言 → mount → update → layout → paint → event → unmount）と、
//! `ui::ListRecycler` と同じ再利用条件（状態バージョンが不変ならスナップショットを
//! 再構築しない）を、各ウィジェット実装が個別に組み立て直さず共通利用できる形で
//! テストケース化するための駆動・観測 API を提供する。
//!
//! `ui::hit_test` / `ui::pointer`（TASK-8.3-S3、Issue #98）と同じ判断で、既存 main の
//! 公開 API（[`super::screen::Screen`] / [`crate::state::StateStore`] /
//! [`super::render::render_headless`] / [`super::hit_test::HitRegion`] /
//! [`super::pointer::PointerDispatcher`]）のみに依存する自己完結の実装とする。
//! S2 マージ後は `InteractionState` の観測 API を、S4 マージ後は宣言から矩形を
//! 自動算出する経路をそれぞれ追加する拡張点として、[`WidgetHarness::set_regions`] の
//! ドキュメンテーションコメントに残す。

use super::hit_test::HitRegion;
use super::pointer::{PointerDispatcher, PointerEvent, PointerEventKind};
use super::render::render_headless;
use super::screen::Screen;
use crate::state::{Event, StateStore};

/// 設計文書 §4.4 の 7 状態ライフサイクルをテスト観測するためのフェーズ列挙。
///
/// [`WidgetHarness`] の各メソッドがどのフェーズを記録するかは、対応するメソッドの
/// ドキュメンテーションコメントを参照。テスト側は [`WidgetHarness::phases`] で
/// 記録済みの列を取得し、期待する遷移順序（例: `Event` の後に `Updated` へ戻る）を
/// 自前の `assert!` で検証する（アサーション自体をハーネス API に持たせず、
/// ハーネスは観測データを返すことに徹する設計判断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecyclePhase {
    /// [`WidgetHarness::new`] 呼び出し時点（画面が宣言されたのみで、まだ描画していない）。
    Declared,
    /// 初回の [`WidgetHarness::run_frame`] 呼び出し（初回マウント・初回描画に相当）。
    Mounted,
    /// 2 回目以降の [`WidgetHarness::run_frame`] 呼び出しで、状態バージョンが変化していた場合。
    /// 設計文書 §4.4 の `Event --> Updated`（イベント発火後、次フレームで再び
    /// `sync_from_state` される）遷移に対応する。
    Updated,
    /// `run_frame` が実際に描画したフレームで、`Mounted` / `Updated` の直後・
    /// `Paint` の直前に記録される（設計文書 §4.4 の `Updated --> Layout --> Paint`
    /// 遷移順。mount/update（reconcile）の**後**に layout（measure）が来る点に注意）。
    Layout,
    /// `run_frame` が実際に `render_headless` を実行してスナップショットを更新した回。
    /// `Layout` の直後に併記される（描画結果の確定を表す）。
    Paint,
    /// [`WidgetHarness::pointer`] / [`WidgetHarness::tap`] によるポインタイベント処理。
    Event,
    /// [`WidgetHarness::unmount`] 呼び出し時点。以降ハーネスは駆動を受け付けない。
    Unmounted,
}

/// ウィジェットテストハーネス（TASK-8.3-S5 / REQ-8、Issue #100）。
///
/// [`Screen`] + [`StateStore`] + [`HitRegion`] + [`PointerDispatcher`] +
/// [`render_headless`] を束ね、S7〜S60 の個別ウィジェットテストが共通で使う
/// 駆動 API（`run_frame` / `pointer` / `tap` / `unmount`）と観測 API
/// （`state` / `version` / `last_snapshot` / `phases` / `frames_rendered` /
/// `frames_reused`）を提供する。
///
/// 呼び出し文脈: S7〜S60 の各ウィジェット結合テスト（`crates/fandhe-core/tests/`）から
/// 直接インスタンス化して使われる想定。本体はテストコードに閉じ、ライブラリ本体の
/// 実行時経路（描画・イベント配線）には組み込まれない（テスト専用ヘルパー）。
///
/// # Examples
///
/// ```
/// use fandhe_core::{HitRegion, Point, Rect, Screen, Size, WidgetHarness, WidgetId};
///
/// #[derive(Default)]
/// struct Counter {
///     count: u32,
/// }
///
/// fn increment(state: &mut Counter) {
///     state.count += 1;
/// }
///
/// let screen: Screen<Counter> =
///     Screen::new("home").button(WidgetId::from_raw(1), "increment", increment);
/// let mut harness = WidgetHarness::new(screen, Counter::default());
///
/// // レイアウトエンジンと未接続のため、矩形はテスト側で明示的に登録する。
/// harness.set_regions(HitRegion::leaf(
///     Rect::new(Point::new(0.0, 0.0), Size::new(20.0, 20.0)),
///     "increment",
/// ));
/// harness.run_frame();
///
/// let clicked = harness.tap(Point::new(10.0, 10.0));
///
/// assert!(clicked);
/// assert_eq!(harness.state().count, 1);
/// assert_eq!(harness.version(), 1);
/// ```
pub struct WidgetHarness<S> {
    screen: Screen<S>,
    store: StateStore<S>,
    dispatcher: PointerDispatcher,
    /// レイアウト確定後の矩形ツリー。レイアウトエンジンと未接続のため、
    /// テスト側が [`Self::set_regions`] で明示的に登録する（`ui::hit_test` と同じ判断）。
    regions: Option<HitRegion>,
    /// 直近の `render_headless` 出力。バージョン不変時はこの値をそのまま再利用する。
    last_snapshot: Vec<String>,
    /// 直近にスナップショットを取得した時点の `StateStore::version()`。
    /// `None` は「まだ 1 度も描画していない」ことを表す。
    last_version: Option<u64>,
    /// 駆動したフェーズの記録（順序検証用）。
    phases: Vec<LifecyclePhase>,
    /// `render_headless` を実際に実行してスナップショットを更新した回数。
    frames_rendered: u64,
    /// バージョン不変によりスナップショットを再利用した回数
    /// （`ui::ListRecycler` と同じ再利用条件の判定に使う）。
    frames_reused: u64,
    /// `unmount` 済みかどうか。真の場合、以降の駆動 API は no-op / no-op 相当を返す。
    unmounted: bool,
}

impl<S> WidgetHarness<S> {
    /// `screen` と初期状態 `initial` からハーネスを作る。[`LifecyclePhase::Declared`] を記録する。
    ///
    /// この時点ではまだ `render_headless` を呼ばない（`Mounted` は最初の
    /// [`Self::run_frame`] 呼び出しまで遅延する。宣言と初回マウントを区別する
    /// §4.4 のライフサイクル契約に合わせた設計）。
    pub fn new(screen: Screen<S>, initial: S) -> Self {
        Self {
            screen,
            store: StateStore::new(initial),
            dispatcher: PointerDispatcher::new(),
            regions: None,
            last_snapshot: Vec::new(),
            last_version: None,
            phases: vec![LifecyclePhase::Declared],
            frames_rendered: 0,
            frames_reused: 0,
            unmounted: false,
        }
    }

    /// レイアウト確定後の矩形ツリーを登録する。
    ///
    /// S4（レイアウトプロトコル、Issue #99）マージ後は、ここが「宣言（`Screen`）から
    /// 矩形を自動算出する」実装に置き換わる拡張点。それまではテスト側が明示的に
    /// 矩形を組み立てて渡す（`ui::hit_test` の `HitRegion` 手動構築と同じ運用）。
    /// 本メソッド自体は [`LifecyclePhase::Layout`] を記録しない
    /// （設計文書 §4.4 の layout は mount/update（reconcile）の**後**・paint の**前**に
    /// 毎フレーム発生する状態機械上の遷移であり、矩形データの登録タイミングとは
    /// 独立している。実際のフェーズ記録は [`Self::run_frame`] が担う）。
    /// `unmount` 済みの場合は no-op（ライフサイクル終了後の駆動を拒否する契約、
    /// [`Self::unmount`] 参照）。
    pub fn set_regions(&mut self, root: HitRegion) {
        if self.unmounted {
            return;
        }
        self.regions = Some(root);
    }

    /// 現在の状態への共有参照を返す（[`StateStore::state`] への委譲）。
    pub fn state(&self) -> &S {
        self.store.state()
    }

    /// 現在の状態バージョンを返す（[`StateStore::version`] への委譲）。
    pub fn version(&self) -> u64 {
        self.store.version()
    }

    /// 直近の `run_frame` で確定したヘッドレス描画スナップショットを返す。
    pub fn last_snapshot(&self) -> &[String] {
        &self.last_snapshot
    }

    /// これまでに記録したライフサイクルフェーズの列を、駆動順に返す。
    pub fn phases(&self) -> &[LifecyclePhase] {
        &self.phases
    }

    /// `render_headless` を実際に実行してスナップショットを更新した回数。
    pub fn frames_rendered(&self) -> u64 {
        self.frames_rendered
    }

    /// バージョン不変によりスナップショットを再利用した回数。
    pub fn frames_reused(&self) -> u64 {
        self.frames_reused
    }

    /// 1 フレーム分の描画を駆動する。
    ///
    /// `StateStore::version()` を直近値と比較し、**不変ならスナップショットを再利用**
    /// する（`frames_reused` を加算するのみで `render_headless` を呼び直さない、
    /// `ui::ListRecycler` の再利用条件と同じ判断軸。この場合フェーズは何も記録しない
    /// ── 再利用は「フレームが実際には走らなかった」ことを表すため）。
    ///
    /// 変化していれば実際にフレームを駆動する: 初回は [`LifecyclePhase::Mounted`]、
    /// 2 回目以降は [`LifecyclePhase::Updated`] を記録し、続けて
    /// [`LifecyclePhase::Layout`] → [`LifecyclePhase::Paint`] の順に記録したうえで
    /// `render_headless` を実行する（設計文書 §4.4 の状態機械が定める
    /// `Mounted/Updated → Layout → Paint` の遷移順。layout は mount/update
    /// （reconcile）の**後**に来る点に注意。レイアウト確定後の矩形ツリー自体は
    /// [`Self::set_regions`] で別途登録する）。
    ///
    /// `unmount` 済みの場合は駆動を拒否し、直近のスナップショットをそのまま返す
    /// （§4.4 のライフサイクル契約: unmount 後は mount/update/layout/paint のいずれも
    /// 発生しない）。
    pub fn run_frame(&mut self) -> &[String] {
        if self.unmounted {
            return &self.last_snapshot;
        }

        let current_version = self.store.version();
        if self.last_version == Some(current_version) {
            self.frames_reused += 1;
            return &self.last_snapshot;
        }

        let is_first_frame = self.last_version.is_none();
        self.phases.push(if is_first_frame {
            LifecyclePhase::Mounted
        } else {
            LifecyclePhase::Updated
        });
        self.phases.push(LifecyclePhase::Layout);
        self.last_snapshot = render_headless(&self.screen, self.store.state());
        self.last_version = Some(current_version);
        self.frames_rendered += 1;
        self.phases.push(LifecyclePhase::Paint);
        &self.last_snapshot
    }

    /// 登録済み [`HitRegion`] に対して 1 件のポインタイベントを処理する。
    ///
    /// [`PointerDispatcher::dispatch`] でヒットテスト・クリック確定判定を行い、
    /// 返った [`Event`] を [`StateStore::dispatch`] へブリッジする（状態 `S` の
    /// 実書き換えは既存の `StateStore::dispatch` に委譲し、走査ロジックを二重管理
    /// しない設計。`ui::pointer` モジュール冒頭のコメントと同じ方針）。
    /// クリックが確定して `StateStore::dispatch` が状態を更新した場合のみ
    /// [`LifecyclePhase::Event`] を記録し `true` を返す。
    ///
    /// 矩形が未登録（[`Self::set_regions`] 未呼び出し）、または `unmount` 済みの場合は
    /// no-op として `false` を返す（panic を API 契約にしない、coding-rust.md 準拠）。
    pub fn pointer(&mut self, event: PointerEvent) -> bool {
        if self.unmounted {
            return false;
        }
        let Some(root) = self.regions.as_ref() else {
            return false;
        };
        let Some(bridged) = self.dispatcher.dispatch(root, event) else {
            return false;
        };
        let handled = self.dispatch_bridged_event(&bridged);
        if handled {
            self.phases.push(LifecyclePhase::Event);
        }
        handled
    }

    /// ブリッジされた `state::Event` を `StateStore::dispatch` へ渡す内部ヘルパー。
    /// `pointer` からのみ呼ばれる（外部へは公開しない実装詳細）。
    fn dispatch_bridged_event(&mut self, event: &Event) -> bool {
        self.store.dispatch(&self.screen, event)
    }

    /// `point` における Down → Up の合成ヘルパー（クリック確定の典型経路）。
    ///
    /// [`Self::pointer`] を Down・Up の順に 2 回呼び出す糖衣構文。戻り値は Up 側の
    /// 結果（クリックが確定して状態が変化したか）を返す（Down 単体では状態変化が
    /// 起きない設計、`ui::pointer::PointerDispatcher` の契約を参照）。
    pub fn tap(&mut self, point: super::geometry::Point) -> bool {
        self.pointer(PointerEvent::new(PointerEventKind::Down, point));
        self.pointer(PointerEvent::new(PointerEventKind::Up, point))
    }

    /// ハーネスを終了状態にする。[`LifecyclePhase::Unmounted`] を記録し、以降の
    /// [`Self::run_frame`] / [`Self::pointer`] / [`Self::set_regions`] 呼び出しを
    /// no-op にする（§4.4 のライフサイクル契約: unmount 後は駆動を受け付けない）。
    /// 既に unmount 済みの場合は再度記録しない（二重 unmount を許容しつつ
    /// フェーズ列を汚さない）。
    pub fn unmount(&mut self) {
        if self.unmounted {
            return;
        }
        self.unmounted = true;
        self.phases.push(LifecyclePhase::Unmounted);
    }
}
