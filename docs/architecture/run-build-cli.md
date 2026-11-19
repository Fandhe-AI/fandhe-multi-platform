# `cargo fandhe run` / `build` の設計

利用者のアプリを、OS ごとの手順を知らずに 1 コマンドでビルド・起動するための CLI です（REQ-21 / TASK-21.5、#1201）。
OS ごとに PR を分けて足します（デスクトップ・Web #1422、Android #1423、iOS シミュレータ #1424、iOS 実機 #1425）。

## 形

```sh
cargo fandhe run|build [--os macos|windows|linux|web] [--release] [--manifest-path <アプリの Cargo.toml>]
```

- `--os` を省くとホストのデスクトップ OS です。同じオプションの重複と、`-` で始まる `--manifest-path` の値は引数エラー（2）です
- 終了コードは 0 正常 / 1 前提の不足・入力の不正・子プロセスの失敗 / 2 未知の引数です

## OS ごとの手順

| OS | build | run |
| --- | --- | --- |
| macos・windows・linux | `cargo build`。`--os` がホストと違えば止める | `cargo run` |
| web | `cargo build --lib --target wasm32-unknown-unknown` → 出力先を作り直す → `wasm-bindgen --target web` →（`--release` は `wasm-opt -Os`）→ `web/`・manifest・フォントを足す | build のあと、cargo-fandhe 自身が 127.0.0.1 の空きポートで配り、URL を出す（Ctrl-C で終了） |

- Web の出力先は `<target_dir>/fandhe/<package 名>/web/` です。`cargo build` が成功したあと、`wasm-bindgen` の前にこのディレクトリを消して作り直します（コンパイルに失敗しても前回の成果物は残ります）
  （`target_dir` の配下で、途中に symlink が無いことを確かめてから消します）。`wasm-bindgen` の出力は直下に置きます
- アプリの `web/` ディレクトリがあれば丸ごと写します（symlink は写しません）。`web/index.html` が無ければ（symlink も無いものとして扱います）、全画面の canvas を持つ
  既定の `index.html` を出します。`[package.metadata.fandhe-app.web]` があれば、`web/` の写しのあとに
  `cargo fandhe gen --os web` と同じ manifest を置きます（テーブル以外の書き方は gen と同じエラーです）
- フォント（Noto Sans JP）は、依存に `fandhe-fonts` があれば `fonts/noto-sans-jp-regular.otf` へ写します。無ければ既定フォントで続行します
- 配信は GET だけで、配信ディレクトリ配下の通常ファイルだけを返します（`..`・絶対パス・ドライブ文字・symlink は 400）。
  `Host` が `127.0.0.1:<port>`・`localhost:<port>` 以外のリクエストも 400 にします（DNS rebinding 対策）。
  外部プロセス（python 等）を使わないため、cargo-fandhe が終わればポートも閉じます。127.0.0.1 の外へは開きません
- 手順は cargo-fandhe が持ちます。`scripts/` は本リポジトリの gallery 専用で、利用者には配られないため呼びません

## 前提の検査

- 実行前に不足を全件集め、1 件 1 行で「何が無いか。直し方」を出します。環境を黙って変えません（`rustup target add` などは案内だけ）
- 見るもの: Rust の target（rustup があるとき）、bin（デスクトップ）・cdylib（Web）の有無、`wasm-bindgen` CLI の有無と
  Cargo.lock の `wasm-bindgen` との版の一致、`--release` の `wasm-opt`
- エラーに環境変数の値を出しません（`security.md`）
- 子プロセスの失敗は出力をそのまま流し、最後にどの段（cargo・wasm-bindgen・wasm-opt）で止まったかを 1 行出します

## 作り

`crates/cargo-fandhe/src/run/` は 3 層です。

- `prepare`: アプリ情報・オプション・環境のスナップショットから、不足の一覧か手順（書くファイル・実行するコマンドの列）を返す純粋関数。
  `crates/cargo-fandhe/tests/run.rs` が OS ごとの手順と検査の分岐を固定します
- `load_app`・`detect_env`: `cargo metadata` と実環境から入力を作ります
- `execute`: 手順を順に実行するだけです

## 範囲外

- Windows の signtool による署名とパッケージ ID（#1208）
- 生成物の追記と `--eject`（#1202）
- 既存の `scripts/` を CLI 呼び出しへ寄せること
