//! アイコンウィジェット `Icon<S>`（TASK-8.3-S37 / REQ-8 / REQ-9 / REQ-13、Issue #132）。
//!
//! `Widget<S>` トレイト（TASK-8.3-S2 / #97）と、表示専用・ソース識別子ベースという点で
//! 最も近い同型の先行ウィジェット [`super::image::Image`]（#129）が確立したパターンを
//! 踏襲し、`Widget<S>` を実装する具体型 [`Icon`] が基本描画（sync_from_state /
//! measure / paint / hit_test / accessibility_role）を実装する。
//!
//! 設計文書 `docs/architecture/widget-architecture.md` §4.1 の方針に従い、
//! `Node<S>` enum は温存し `Node::Widget(Box<dyn Widget<S>>)` 経由でツリーへ組み込む。
//!
//! - アイコン名（識別子）の SSOT はアプリ状態 `S`（[`IconSource::Bound`]）または構築時の
//!   固定値（[`IconSource::Static`]）。ウィジェットは [`crate::state::ReadFn`] 経由の
//!   [`Widget::sync_from_state`] で表示用キャッシュ `cached_name` を更新するのみで、
//!   中間編集状態を持たない（`Image` と同じ構造上の理由）。
//! - アイコン名は閉じた列挙型 [`IconName`]（#1437）。「知らない名前」を型で作れなくし、
//!   描画側に未知名の分岐を持たせない。各名前は Unicode の記号 1 文字（[`IconName::glyph`]）へ
//!   写る。`fandhe-core` は記号の字面を持つだけで、フォントの解決・アセット I/O は一切
//!   行わない（ヘッドレス）。egui は記号専用の family（既定フォントの emoji-icon-font・Noto Emoji）へ、
//!   web-dom は同じ文字をテキストとして出す。
//! - [`super::widget::AccessibilityRole::Image`] を共用する（`widget.rs` は変更しない）。
//!   accesskit にアイコン専用ロールは存在せず `Role::Image` が包含するため、プラット
//!   フォームロールが区別しない場合に専用バリアントを立てない判断
//!   （`ProgressBar` / `Spinner` が `AccessibilityRole::ProgressIndicator` を共用する
//!   前例と同じ判断）。
//! - `PaintCommand` は拡張しない（`Text(String)` / `Rect` の 2 種のまま、3 バックエンドの
//!   プレースホルダ契約に波及するため安全側に倒す — `Image` と同じ判断）。描画列は
//!   `[Rect（アイコンプレースホルダ枠）, Text(label)]` とする。`label` はアクセシビリティ
//!   ラベル（開発者指定）で、`Text` へ出すのはこれだけ。アイコン名は閉じた列挙型で外部の文字列
//!   ではないが、ヘッドレスの `paint` には記号も出さない（`Image` の alt 方式と揃える）。
//! - サイズ（`size`）は正方形の 1 辺長として構築時に数値健全性を担保する: 非有限値
//!   （NaN/inf）または `<= 0.0` の場合は既定寸法（`DEFAULT_ICON_SIZE`。egui・web-dom が描く大きさ）へ
//!   フォールバックする（`Slider::normalize_range` / `Image::normalize_intrinsic_size` と
//!   同型の安全側判断、OWASP A04 相当）。
//!
//! **sync の経路**（[`super::widget`] モジュール doc 参照）: `render_headless`（`fandhe-core`）・
//! `to_snapshot_node`（`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため
//! [`Widget::sync_from_state`] を呼べず、`Node::Widget` を状態非依存のプレースホルダとして扱う。
//! 一方 `fandhe-render-egui` と `fandhe-web-dom`（`convert_icon_widget`）は `clone_box` した複製へ
//! `sync_from_state` をかけて読むので、`icon_bound` は状態に追従して描かれる。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) SVG / アイコンフォントの同梱・tint / テーマ配色（#1437 では egui の既定フォントの記号に
//! 写す。swiftui-bridge は `Node::Widget` を型を問わず共通の変換にしているため未対応）、
//! (b) タップ可能アイコン等の状態・イベント処理（S38 / #133）、
//! (c) `WidgetCatalog` 登録・テスト網羅拡充（S39 / #134）、
//! (d) `render_headless`・swiftui-bridge が `sync_from_state` を呼ばないこと（上記「sync の経路」参照、既存契約を変更しない）。

use crate::state::ReadFn;

use super::widget::{
    AccessibilityRole, InteractionState, Measurement, PaintCommand, VisualState, Widget, WidgetId,
};

/// `with_size` が非有限値・非正値を無害化する際にフォールバックする既定のアイコン
/// サイズ（1 辺長、論理ピクセル相当のプレースホルダ定数）。
///
/// `with_size` を呼ばないときに egui・web-dom が描く一辺の長さ（論理ピクセル）でもある。
const DEFAULT_ICON_SIZE: f32 = 24.0;

/// 名前を `IconName` の変種・記号の対応表として 1 か所に定義するマクロ（変種の追加・記号の変更が
/// 1 行で済み、`ALL` と `glyph` の食い違いを作らない）。
macro_rules! icon_names {
    ($($(#[$doc:meta])* $variant:ident => $glyph:literal,)+) => {
        /// アイコンの名前（閉じた語彙、#1437）。`Screen::icon` / `Screen::icon_bound` へ渡す。
        ///
        /// 文字列ではなく列挙型にして、綴りの誤りや未対応の名前をコンパイル時に弾く
        /// （AI 型安全 DSL、REQ-13）。各名前は [`Self::glyph`] の Unicode 記号 1 文字へ写り、
        /// egui（既定フォント）・web-dom（テキスト）が同じ文字を使う。語彙の追加は後方互換
        /// （`#[non_exhaustive]`）。
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum IconName {
            $($(#[$doc])* $variant,)+
        }

        impl IconName {
            /// 全名前（宣言順）。
            pub const ALL: &'static [IconName] = &[$(IconName::$variant,)+];

            /// 名前に対応する Unicode 記号 1 文字。フォント解決は描画側の責務（`fandhe-core` は
            /// 字面だけを持つ）。egui の既定フォントに字形があることは `fandhe-render-egui` の
            /// テストが全名前について確かめている。
            pub const fn glyph(self) -> char {
                match self {
                    $(IconName::$variant => $glyph,)+
                }
            }
        }
    };
}

icon_names! {
    /// 設定（歯車）。
    Settings => '\u{2699}',
    /// 検索（虫眼鏡）。
    Search => '\u{1F50D}',
    /// ホーム。
    Home => '\u{1F3E0}',
    /// メニュー（三本線）。
    Menu => '\u{2630}',
    /// 戻る（左向き）。
    Back => '\u{23F4}',
    /// 進む（右向き）。
    Forward => '\u{23F5}',
    /// 上向き。
    Up => '\u{23F6}',
    /// 下向き。
    Down => '\u{23F7}',
    /// 閉じる（×）。
    Close => '\u{2716}',
    /// 確認（チェック）。
    Check => '\u{2714}',
    /// 追加（＋）。
    Add => '\u{2795}',
    /// 取り除く（－）。
    Remove => '\u{2796}',
    /// 編集（鉛筆）。
    Edit => '\u{270F}',
    /// 削除（ごみ箱）。
    Delete => '\u{1F5D1}',
    /// コピー。
    Copy => '\u{1F4CB}',
    /// 保存。
    Save => '\u{1F4BE}',
    /// 共有。
    Share => '\u{1F4E4}',
    /// 更新（回転矢印）。
    Refresh => '\u{1F504}',
    /// ダウンロード。
    Download => '\u{2B07}',
    /// アップロード。
    Upload => '\u{2B06}',
    /// リンク。
    Link => '\u{1F517}',
    /// 情報。
    Info => '\u{2139}',
    /// 警告。
    Warning => '\u{26A0}',
    /// 星。
    Star => '\u{2605}',
    /// ハート。
    Heart => '\u{2665}',
    /// 通知（ベル）。
    Bell => '\u{1F514}',
    /// メール。
    Mail => '\u{2709}',
    /// 電話。
    Phone => '\u{1F4DE}',
    /// カメラ。
    Camera => '\u{1F4F7}',
    /// 画像。
    Image => '\u{1F5BC}',
    /// ファイル。
    File => '\u{1F4C4}',
    /// フォルダ。
    Folder => '\u{1F4C1}',
    /// カレンダー。
    Calendar => '\u{1F4C5}',
    /// 時計。
    Clock => '\u{1F553}',
    /// 位置情報。
    Location => '\u{1F4CD}',
    /// ユーザー。
    User => '\u{1F464}',
    /// 施錠。
    Lock => '\u{1F512}',
    /// 解錠。
    Unlock => '\u{1F513}',
    /// 表示（目）。
    Visibility => '\u{1F441}',
    /// 再生。
    Play => '\u{25B6}',
    /// 一時停止。
    Pause => '\u{23F8}',
    /// 停止。
    Stop => '\u{23F9}',
    /// マイク。
    Microphone => '\u{1F3A4}',
    /// 音量あり。
    Volume => '\u{1F50A}',
    /// ミュート。
    Mute => '\u{1F507}',
}

/// [`Icon`] のアイコンソース指定方法（[`super::image::ImageSource`] の前例を踏襲）。
///
/// REQ-13（NFR-9）の方針に従い、状態束縛はクロージャ（環境キャプチャ）を許さず
/// 関数ポインタ（[`ReadFn<S, IconName>`]、実体は `fn(&S) -> IconName`）に限定する。
pub enum IconSource<S> {
    /// 固定のアイコン名。
    Static(IconName),
    /// アプリ状態 `S` からアイコン名を読み取る関数ポインタ（REQ-13、環境キャプチャ不可）。
    /// [`Widget::sync_from_state`] で `cached_name` を更新する。
    Bound(ReadFn<S, IconName>),
}

// `IconSource<S>` は `IconName`（`Copy`）と関数ポインタ（`Copy`）のみで構成されるため、
// 状態 `S` を所有しない手動 `Debug` / `Clone` 実装で `derive` の
// `S: Debug` / `S: Clone` 境界を回避する（`ImageSource` と同じ設計判断）。
impl<S> std::fmt::Debug for IconSource<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IconSource::Static(name) => f.debug_tuple("Static").field(name).finish(),
            IconSource::Bound(_) => f.debug_tuple("Bound").field(&"<fn ptr>").finish(),
        }
    }
}

impl<S> Clone for IconSource<S> {
    fn clone(&self) -> Self {
        match self {
            IconSource::Static(name) => IconSource::Static(*name),
            IconSource::Bound(get) => IconSource::Bound(*get),
        }
    }
}

/// アイコンサイズ（1 辺長）を正規化する（非有限値・非正値は既定プレースホルダ寸法へ
/// フォールバック）。
///
/// 数値健全性の確保（OWASP A04 相当）のため、後続の `measure` がゼロ除算・NaN 伝播を
/// 起こさないよう構築時点で無害化する（`image::normalize_intrinsic_size` と同じ判断）。
fn normalize_icon_size(size: f32) -> f32 {
    if size.is_finite() && size > 0.0 {
        size
    } else {
        DEFAULT_ICON_SIZE
    }
}

/// アイコンウィジェット（TASK-8.3-S37、`Widget<S>` 実装型）。
///
/// `Debug` / `Clone` は手動実装する（`Image` / `Slider` と同じく、状態型 `S` を
/// 所有していないため `derive` マクロが自動付与する `S: Debug` / `S: Clone` という
/// 不要な境界を避ける）。アイコン名・ラベル・寸法は秘密情報に該当しないため、
/// `Debug` にそのまま含める。
pub struct Icon<S> {
    id: WidgetId,
    source: IconSource<S>,
    /// アクセシビリティラベル（REQ-9。`paint` の `Text` と AX の名前に使う）。アイコン名とは独立した、
    /// 開発者指定の文字列。装飾用アイコンは空文字列を許容する（AX・aria に出さない）。
    label: String,
    /// 正方形前提の 1 辺長。構築時に [`normalize_icon_size`] で正規化済み。
    size: f32,
    /// [`Widget::sync_from_state`] が更新する表示用キャッシュ（現在のアイコン名）。
    /// `bound` は初回の sync まで `None`（何も描かない）。
    cached_name: Option<IconName>,
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for Icon<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Icon")
            .field("id", &self.id)
            .field("source", &self.source)
            .field("label", &self.label)
            .field("size", &self.size)
            .field("cached_name", &self.cached_name)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for Icon<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            source: self.source.clone(),
            label: self.label.clone(),
            size: self.size,
            cached_name: self.cached_name,
            interaction: self.interaction,
        }
    }
}

impl<S> Icon<S> {
    /// 状態 `S` に依存しない固定のアイコン名を持つアイコンを構築する。
    ///
    /// `cached_name` は構築直後から `source` で初期化されるため、
    /// `sync_from_state` 呼び出し前でも `name` が `Some(name)` を返す
    /// （`Image::fixed` と同じ意味論）。サイズは既定寸法
    /// （`DEFAULT_ICON_SIZE`）で初期化し、変更する場合は [`Self::with_size`] を使う。
    pub fn fixed(id: WidgetId, name: IconName, label: impl Into<String>) -> Self {
        Self {
            id,
            cached_name: Some(name),
            source: IconSource::Static(name),
            label: label.into(),
            size: DEFAULT_ICON_SIZE,
            interaction: InteractionState::default(),
        }
    }

    /// 状態 `S` に束縛されたアイコン名を持つアイコンを構築する。
    ///
    /// `cached_name` は構築直後は `None` であり、初回 `sync_from_state` 呼び出しまで
    /// 表示に反映されない（reconcile フェーズの意味論、`Image::bound` と同じ判断）。
    pub fn bound(id: WidgetId, get: ReadFn<S, IconName>, label: impl Into<String>) -> Self {
        Self {
            id,
            source: IconSource::Bound(get),
            label: label.into(),
            size: DEFAULT_ICON_SIZE,
            cached_name: None,
            interaction: InteractionState::default(),
        }
    }

    /// サイズ（1 辺長）を設定するビルダーメソッド。
    ///
    /// 非有限値（NaN/inf）または `<= 0.0` は既定プレースホルダ寸法へフォールバックする
    /// （[`normalize_icon_size`] 参照、安全側の判断）。
    pub fn with_size(mut self, size: f32) -> Self {
        self.size = normalize_icon_size(size);
        self
    }

    /// 現在の表示キャッシュ値（アイコン名）を返す。`bound` の初回 sync 前は `None`。
    /// 描画側（egui・web-dom）が [`IconName::glyph`] を引くのに使う。
    pub fn name(&self) -> Option<IconName> {
        self.cached_name
    }

    /// アクセシビリティラベルを返す。
    pub fn label_text(&self) -> &str {
        &self.label
    }

    /// サイズ（構築時・`with_size` 時点で正規化済み）を返す。
    pub fn icon_size(&self) -> f32 {
        self.size
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。
    ///
    /// アイコンには中間編集状態がなく確定すべき編集バッファも存在しないため、
    /// `InteractionState::set_disabled` へそのまま委譲するのみで済む
    /// （`Image::set_disabled` と同じ判断）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }
}

impl<S: 'static> Widget<S> for Icon<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` からアイコン名を読み取り `cached_name` を更新する
    /// （reconcile フェーズ）。sync の経路はモジュール doc を参照。
    fn sync_from_state(&mut self, state: &S) {
        match &self.source {
            IconSource::Static(name) => {
                // 固定識別子は状態に依存しないため不変。呼び出し自体は許容し、
                // reconcile ループの呼び出し規約（全ウィジェット一律呼び出し）を崩さない。
                self.cached_name = Some(*name);
            }
            IconSource::Bound(get) => {
                self.cached_name = Some(get(state));
            }
        }
    }

    /// 正方形を維持したまま `available` に収まるサイズへ縮小する（拡大はしない）。
    ///
    /// `available` の幅・高さのいずれかが `0.0` 以下の場合は `0.0` へ倒す
    /// （安全側の判断、`Image::measure` と同型）。1 辺長は `available` の幅・高さ・
    /// `size` のうち最小値を採用し、正方形を維持する。
    fn measure(&self, available: Measurement) -> Measurement {
        if available.width <= 0.0 || available.height <= 0.0 {
            return Measurement {
                width: 0.0,
                height: 0.0,
            };
        }

        let side = available.width.min(available.height).min(self.size);

        Measurement {
            width: side,
            height: side,
        }
    }

    /// `[Rect（アイコンプレースホルダ枠）, Text(label)]` の描画プリミティブ列を返す。
    ///
    /// `Text` へ出すのは開発者指定のアクセシビリティラベルだけで、アイコン名・記号は出さない
    /// （モジュール doc 参照）。
    /// `PaintCommand` 自体は拡張しない。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        vec![PaintCommand::Rect, PaintCommand::Text(self.label.clone())]
    }

    /// アイコンは表示専用要素であり、常に `false` を返す（`Image::hit_test` と同じ
    /// 安全側の判断）。タップ可能アイコン等の対話は後続工程（S38）で再検討する。
    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        false
    }

    /// `AccessibilityRole::Image` を共用する（モジュール doc 参照。accesskit に
    /// アイコン専用ロールは存在せず `Role::Image` が包含するため）。
    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Image
    }

    /// ラベル（開発者指定）をアクセシブルネームにする。空なら装飾用として `None`
    /// （描画側は読み上げノードを作らない）。アイコン名は出さない（`paint` と同じ方針）。
    fn accessibility_label(&self) -> Option<String> {
        if self.label.is_empty() {
            None
        } else {
            Some(self.label.clone())
        }
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    /// 描画バックエンド（`fandhe-render-egui`・`fandhe-web-dom`）が具象型へ downcast して
    /// [`Icon::name`] を読むための口（#1437）。
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    /// トークン解決込みの描画列（TASK-8.4-S14）。Display カテゴリの既定マッピング
    /// （`style_tokens::display_style_refs`）を用い、`paint()` と同じ構造（装飾的
    /// アイコンはアクセシビリティラベルの `Text` を持たない、モジュール doc 参照）を
    /// 保ったまま色・タイポグラフィのみを解決済み具体値へ差し替える。解決失敗時は
    /// `paint()` の無スタイル列へフォールバックする。
    fn paint_styled(
        &self,
        origin: (f32, f32),
        size: Measurement,
        resolver: &crate::token::TokenResolver,
    ) -> Vec<PaintCommand> {
        let Ok(refs) = super::style_tokens::display_style_refs() else {
            return self.paint(origin, size);
        };
        let Ok(resolved) = super::style_tokens::resolve_widget_style(
            resolver,
            &refs,
            self.interaction_state().visual_state(),
        ) else {
            return self.paint(origin, size);
        };
        match super::style_tokens::style_paint_commands(self.paint(origin, size), &resolved) {
            Some(styled) => styled,
            None => self.paint(origin, size),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SettingsState {
        active_icon: IconName,
    }

    fn get_active_icon(state: &SettingsState) -> IconName {
        state.active_icon
    }

    // 語彙（#1437）

    #[test]
    fn glyphs_are_distinct_single_chars() {
        let mut seen = std::collections::HashSet::new();
        for name in IconName::ALL {
            assert!(seen.insert(name.glyph()), "{name:?} の記号が重複");
        }
        assert_eq!(seen.len(), IconName::ALL.len());
    }

    #[test]
    fn accessibility_label_is_label_or_none_when_decorative() {
        let named: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(1), IconName::Home, "ホーム");
        assert_eq!(named.accessibility_label().as_deref(), Some("ホーム"));
        let decorative: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(2), IconName::Home, "");
        assert_eq!(decorative.accessibility_label(), None);
    }

    // (a) 構築と各アクセサ

    #[test]
    fn fixed_returns_name_immediately_without_sync() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(1), IconName::Settings, "設定");

        assert_eq!(icon.name(), Some(IconName::Settings));
        assert_eq!(icon.label_text(), "設定");
        assert_eq!(icon.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn bound_is_empty_before_first_sync() {
        let icon: Icon<SettingsState> =
            Icon::bound(WidgetId::from_raw(2), get_active_icon, "現在のアイコン");

        assert_eq!(icon.name(), None);
        assert_eq!(icon.label_text(), "現在のアイコン");
    }

    #[test]
    fn default_size_is_placeholder_constant() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(3), IconName::Settings, "設定");

        assert_eq!(icon.icon_size(), DEFAULT_ICON_SIZE);
    }

    // (b) with_size の正規化（NaN / inf / 0 / 負値 → 既定寸法フォールバック）

    #[test]
    fn with_size_sets_positive_finite_size() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(4), IconName::Settings, "設定").with_size(32.0);

        assert_eq!(icon.icon_size(), 32.0);
    }

    #[test]
    fn with_size_falls_back_when_nan() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(5), IconName::Settings, "設定").with_size(f32::NAN);

        assert_eq!(icon.icon_size(), DEFAULT_ICON_SIZE);
    }

    #[test]
    fn with_size_falls_back_when_infinite() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(6), IconName::Settings, "設定").with_size(f32::INFINITY);

        assert_eq!(icon.icon_size(), DEFAULT_ICON_SIZE);
    }

    #[test]
    fn with_size_falls_back_when_zero() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(7), IconName::Settings, "設定").with_size(0.0);

        assert_eq!(icon.icon_size(), DEFAULT_ICON_SIZE);
    }

    #[test]
    fn with_size_falls_back_when_negative() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(8), IconName::Settings, "設定").with_size(-10.0);

        assert_eq!(icon.icon_size(), DEFAULT_ICON_SIZE);
    }

    // (c) sync_from_state の Static 不変・Bound の状態追従

    #[test]
    fn sync_from_state_keeps_static_name_unchanged() {
        let mut icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(9), IconName::Settings, "設定");
        let state = SettingsState {
            active_icon: IconName::Search,
        };

        icon.sync_from_state(&state);

        assert_eq!(icon.name(), Some(IconName::Settings));
    }

    #[test]
    fn sync_from_state_reflects_bound_state() {
        let mut icon: Icon<SettingsState> =
            Icon::bound(WidgetId::from_raw(10), get_active_icon, "現在のアイコン");
        let state = SettingsState {
            active_icon: IconName::Search,
        };

        icon.sync_from_state(&state);

        assert_eq!(icon.name(), Some(IconName::Search));
    }

    #[test]
    fn sync_from_state_tracks_state_changes_across_calls() {
        let mut icon: Icon<SettingsState> =
            Icon::bound(WidgetId::from_raw(11), get_active_icon, "現在のアイコン");
        let mut state = SettingsState {
            active_icon: IconName::Search,
        };

        icon.sync_from_state(&state);
        assert_eq!(icon.name(), Some(IconName::Search));

        state.active_icon = IconName::Home;
        icon.sync_from_state(&state);
        assert_eq!(icon.name(), Some(IconName::Home));
    }

    // (d) measure（正方形維持・十分な余白ではそのまま・縮退時 0.0）

    #[test]
    fn measure_returns_fixed_size_when_available_is_large_enough() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(12), IconName::Settings, "設定").with_size(24.0);

        let measurement = icon.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        assert_eq!(measurement.width, 24.0);
        assert_eq!(measurement.height, 24.0);
    }

    #[test]
    fn measure_shrinks_to_fit_narrower_width_constraint() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(13), IconName::Settings, "設定").with_size(24.0);

        let measurement = icon.measure(Measurement {
            width: 10.0,
            height: 1000.0,
        });

        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 10.0);
    }

    #[test]
    fn measure_shrinks_to_fit_narrower_height_constraint() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(14), IconName::Settings, "設定").with_size(24.0);

        let measurement = icon.measure(Measurement {
            width: 1000.0,
            height: 10.0,
        });

        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 10.0);
    }

    #[test]
    fn measure_returns_zero_when_available_width_is_zero() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(15), IconName::Settings, "設定");

        let measurement = icon.measure(Measurement {
            width: 0.0,
            height: 100.0,
        });

        assert_eq!(measurement.width, 0.0);
        assert_eq!(measurement.height, 0.0);
    }

    #[test]
    fn measure_returns_zero_when_available_height_is_negative() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(16), IconName::Settings, "設定");

        let measurement = icon.measure(Measurement {
            width: 100.0,
            height: -1.0,
        });

        assert_eq!(measurement.width, 0.0);
        assert_eq!(measurement.height, 0.0);
    }

    // (e) paint（列・label 内容検証）

    #[test]
    fn paint_returns_rect_and_label_text() {
        let mut icon: Icon<SettingsState> =
            Icon::bound(WidgetId::from_raw(17), get_active_icon, "現在のアイコン");
        let state = SettingsState {
            active_icon: IconName::Lock,
        };
        icon.sync_from_state(&state);

        let commands = icon.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("現在のアイコン".to_string()),
            ]
        );
    }

    // (f) hit_test は常に false

    #[test]
    fn hit_test_is_always_false_even_inside_bounds() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(19), IconName::Settings, "設定");
        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };

        assert!(!icon.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    #[test]
    fn hit_test_is_always_false_when_disabled() {
        let mut icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(20), IconName::Settings, "設定");
        icon.set_disabled(true);
        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };

        assert!(!icon.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    // (g) accessibility_role / interaction_state / clone_box

    #[test]
    fn accessibility_role_is_image() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(21), IconName::Settings, "設定");

        assert_eq!(icon.accessibility_role(), AccessibilityRole::Image);
    }

    #[test]
    fn visual_state_priority_is_delegated_to_interaction_state() {
        use super::super::widget::InteractionInput;

        let mut icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(22), IconName::Settings, "設定");

        icon.interaction.apply(InteractionInput::PointerEntered);
        assert_eq!(icon.visual_state(), VisualState::Hovered);

        icon.set_disabled(true);
        assert_eq!(icon.visual_state(), VisualState::Disabled);
    }

    #[test]
    fn clone_box_produces_independent_copy() {
        let mut icon: Icon<SettingsState> =
            Icon::bound(WidgetId::from_raw(23), get_active_icon, "現在のアイコン");
        let state = SettingsState {
            active_icon: IconName::Star,
        };
        icon.sync_from_state(&state);

        let mut cloned = icon.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(23));

        let other_state = SettingsState {
            active_icon: IconName::Heart,
        };
        cloned.sync_from_state(&other_state);

        // clone 元は影響を受けない（独立性の検証）。
        assert_eq!(icon.name(), Some(IconName::Star));
    }

    #[test]
    fn debug_output_does_not_panic() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(24), IconName::Settings, "設定");
        let output = format!("{icon:?}");
        assert!(output.contains("Icon"));
    }

    #[test]
    fn icon_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Icon<SettingsState>>();
    }

    // (h) sync -> measure -> paint のライフサイクル順の直接駆動テスト

    #[test]
    fn lifecycle_sync_then_measure_then_paint_reflects_state() {
        let mut icon: Icon<SettingsState> =
            Icon::bound(WidgetId::from_raw(25), get_active_icon, "現在のアイコン").with_size(32.0);
        let state = SettingsState {
            active_icon: IconName::Search,
        };

        icon.sync_from_state(&state);
        let measurement = icon.measure(Measurement {
            width: 20.0,
            height: 1000.0,
        });
        let commands = icon.paint((0.0, 0.0), measurement);

        assert_eq!(measurement.width, 20.0);
        assert_eq!(measurement.height, 20.0);
        assert_eq!(icon.name(), Some(IconName::Search));
        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("現在のアイコン".to_string()),
            ]
        );
    }

    // TASK-8.4-S14: paint_styled のフォールバック契約。識別子（`name`）が
    // `PaintCommand::Text` へ漏れない既存不変条件は `paint()` を経由するため維持される。
    #[test]
    fn paint_styled_falls_back_to_paint_when_resolver_lacks_tokens() {
        let icon: Icon<SettingsState> =
            Icon::fixed(WidgetId::from_raw(200), IconName::Search, "検索");
        let empty_resolver = crate::token::TokenResolver::new(
            crate::token::TokenSet::new(),
            crate::token::TokenSet::new(),
        );
        let size = Measurement {
            width: 24.0,
            height: 24.0,
        };

        assert_eq!(
            icon.paint_styled((0.0, 0.0), size, &empty_resolver),
            icon.paint((0.0, 0.0), size)
        );
    }
}
