---
name: fandhe-app
description: >-
  fandhe（Rust のマルチプラットフォーム UI フレームワーク）を依存に持つアプリで、画面を作る・端末機能（カメラ・位置情報・
  クリップボード等）を足す・権限の説明文を書く・cargo fandhe check / gen を通すときに使う。
  fandhe 本体のリポジトリの開発には使わない
---

# fandhe-app

`fandhe` を依存に持つアプリのリポジトリで、画面を作り、端末機能を足し、`cargo fandhe check`・`gen` を通す手順です。

## 最初に読むもの

1. 同じディレクトリの `reference.md`（アプリの形・`fandhe::` から届くもの・effect・機能の足し方・誤りやすい点）
2. fandhe のソースの場所を求めます（`reference.md` 1 節）。以下 `$FANDHE_SRC` と書きます。
   シェルの変数は次のコマンドの呼び出しに持ち越されないので、求めた絶対パスに置き換えて使います
3. 画面の部品と書き方は `$FANDHE_SRC/docs/ai/SCHEMA.md` が正です。大きいので見出しを grep し、要る節だけ読みます。
   冒頭の「利用者のリポジトリでの読み替え」節に従います（`fandhe_core::X` は `fandhe::X`、リポジトリ内の開発向けの節は読まない）

API を推測で書きません。読んだ資料とソースにある名前だけを使い、書いたらコンパイルして確かめます。

## 手順 A: 画面を作る

1. 状態の型（`#[derive(Default)]`）・状態を変える関数・状態から画面を作る関数（`fn(&S) -> Screen<S>`）を書きます。
   部品は SCHEMA.md の「egui で描画される部品・されない部品」節にある、描画されるものから選びます
2. 1 画面なら `#[fandhe::main]` の関数で `ProxyApp::with_screen_fn` を返します。複数画面は `Menu` です（`reference.md` 3 節）
3. 画面の表示と操作は `fandhe::render_headless`・`fandhe::click_button` のテストで確かめます（SCHEMA.md パターン 7）。
   テストは画面と同じファイルの `#[cfg(test)] mod tests` に置きます
4. `cargo build` と `cargo test` を通します

## 手順 B: 端末機能を足す

1. 機能を選びます。id の一覧は `$FANDHE_SRC/crates/fandhe/Cargo.toml` の `[features]` です。
   何ができ、どの OS で動くかは `$FANDHE_SRC/docs/device/capabilities/<id>.json`（`name`・`platforms.<os>.status`・`implStatus`・`note`）で確かめます。
   対応表の OS は iOS・Android・macOS・Windows・Web で、Linux は載っていません。
   利用者が求める OS で `done` でない機能（Linux を含む）は、書く前に利用者へ伝えます
2. アプリの `Cargo.toml` の `fandhe` の `features` に id を足します
3. 入口を読みます。`$FANDHE_SRC/crates/fandhe-cap-<id>/src/lib.rs` の crate doc と `pub use`、契約の trait の doc から、
   入口の形（同期 / begin・poll・discard / 共有状態を読み続ける）・OS ごとの入口と始め方の違い・結果の値（成功・キャンセル・失敗・`Unsupported`）を確かめます。
   `fandhe::<stem>::driver::…` のパスでは届きませんが、`driver` で定義され crate 直下に再公開された入口（`platform_<stem>` など）は
   `fandhe::<stem>::…` から使います。trait のメソッドを呼ぶときは、その trait も import します
4. effect を書き、`.with_effect(effect)` で画面に渡します（`reference.md` 4 節・5 節）
5. 宣言を読みます。`$FANDHE_SRC/crates/fandhe-cap-<id>/Cargo.toml` の `[package.metadata.fandhe.*]` に `usage-descriptions` があれば、
   手順 C で説明文を書きます。説明文は、依頼の目的から「何のために・何を使うか」を書きます。
   依頼から読み取った目的が実際の目的と違うおそれがあれば、利用者に確かめます。`links = "…"` があれば `reference.md` 6 節（Swift 薄層）の手当てをします
6. 手順 C を通します

## 手順 C: check・gen・build を通す

アプリの直下で実行します。`cargo fandhe` が無ければ `$FANDHE_SRC/docs/guide/getting-started.md` 2 節で入れます。
成否は終了コードで判断します（`cargo fandhe check` は成功しても何も出しません）。

1. `cargo fandhe check`（依存にある機能の宣言の検証）
2. `cargo fandhe gen`（各 OS の設定を `gen/` に作る）。説明文が足りないと、足りないキーを OS ごとに 1 行ずつ出して止まります。
   エラーが示したテーブル（`[package.metadata.fandhe-app.<os>.usage-descriptions]`）に、エラーのキーをそのまま写して文言を書きます。
   文言は「何のために・何を使うか」を利用者の言葉で 1 文にします。目的が分からなければ利用者に聞き、仮の文言で通しません
3. `cargo build`（ホストのデスクトップ）と `cargo test`
4. 入っている target があれば、`cargo check --target aarch64-apple-ios-sim`・`cargo check --target wasm32-unknown-unknown` でも
   型を確かめます。Android は型の確認だけでも NDK（C のコンパイラとリンカ）が要るため、ここでは確かめません
   （`cargo fandhe build --os android`、getting-started.md 8.3 節）
5. `gen/` は手で直しません

## 終わり方

- `cargo fandhe check`・`gen`・`cargo build`・`cargo test` が通ったことを報告します
- 足した機能について、対応表で `done` でない OS と、確かめていないこと（実機での動作・許可ダイアログ）を報告します。
  実機での確認は利用者に頼みます
