# Changelog — fandhe-egui-proxy

書式は [Keep a Changelog](https://keepachangelog.com/ja/1.1.0/) に、バージョニングは
[Semantic Versioning](https://semver.org/lang/ja/)（0.x 系: minor で破壊的変更を許容）に従う。

## [Unreleased]

画面遷移スタック（`fandhe_core::ScreenStack`）の戻る操作を Esc/Android バックキーへ配線（#726）。

### 追加

- `dispatch_back_if_requested`（`lib.rs`）: `egui::Key::Escape` / `egui::Key::BrowserBack`
  を検知して `fandhe_core::Event::BackRequested` を dispatch する窓口
- `ProxyApp::with_screen_fn`（`app.rs`）: `ScreenStack<S>` のような状態駆動の画面遷移向けに、
  毎フレーム `fn(&S) -> Screen<S>` を呼び直してアプリを構築するエントリ
- `generated_screens!` マクロ（`screens.rs`）に `nav "<name>" => 関数,` 区画を追加。
  `crates/fandhe-egui-proxy/src/screens/registry.rs` へ `nav "list_detail"` を登録した

### 変更

- `ProxyApp<S>` の内部表現を `screen: Screen<S>` 直接保持から、`Fixed`（固定 1 画面）/
  `FromState`（状態駆動）を区別する非公開の `ScreenSource<S>` 所有へ変更（公開 API である
  `ProxyApp::with_screen` のシグネチャ自体は非破壊）。`update` は毎フレーム
  `source.build(store.state())` で描画対象を得たうえで `dispatch_back_if_requested` を呼ぶ

TV フォーカスの決定操作を `StateStore` 経由へ統一（#536）。`fandhe-core` に追加された
`Event::ButtonActivatedAt`（id ベースの決定操作、`ButtonClickedAt` と対称）を通す設計へ揃え、
`TvFocusController` が `StateStore` を迂回して状態を直接所有していた意図的逸脱（#280 で検出）を解消した。

### 破壊的変更（BREAKING CHANGE）

- `TvFocusController::update` のシグネチャを `state: &mut S` から
  `store: &mut fandhe_core::StateStore<S>` へ変更。読み取りは `store.state()`、
  決定操作・クリックは内部で `store.dispatch(screen, &Event::ButtonActivatedAt { .. })`
  を呼ぶ形へ統一した
- `TvFocusApp`（`app.rs`）の内部状態を `state: TvState` 直接所有から
  `store: StateStore<TvState>` 所有へ変更（公開 API である `TvFocusApp::new` の
  シグネチャ自体は非破壊）

### 変更

- `TvFocusController` の独自 `version` カウンタを廃止し、`StateStore::version()` を
  そのまま `ListRecycler::acquire` のキャッシュ無効化キーに使う設計へ統一
- マウス/タッチクリックの発火経路を、走査中の即時発火（`&mut S` 直接操作）から
  「走査完了後にまとめて `dispatch`」する `pending_events` 方式（`lib.rs::RenderContext`
  と同じ契約）へ切り替え。キーボード決定操作・クリックの両方が同一の
  `Event::ButtonActivatedAt` 経路を通るようになった

## [0.1.0]

初期実装（TASK-1.2 以降の `eframe::App` 実装・DSL → egui マッピング）。
