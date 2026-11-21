# fandhe アプリの AI 向けメタ情報

- 対応: #1205、REQ-13 / REQ-21

`fandhe` を依存に持つアプリ（利用者のリポジトリ）で、AI が画面を作り端末機能を足すときに読む資料です。
fandhe 本体のリポジトリの開発（カタログの画面・機能 crate の実装）には使いません。

本書に書くのは、アプリの形・`fandhe::` から届くもの・effect の書き方・端末機能の足し方・利用者のアプリで誤りやすい点だけです。
次は fandhe のソース（1 節）の中にあり、そちらが正です。本書には写しません。

| 知りたいこと | 正 |
| --- | --- |
| 画面の部品・型制約・使用パターン・誤りやすい点 | `docs/ai/SCHEMA.md`（冒頭の「利用者のリポジトリでの読み替え」節に従って読む） |
| 各端末機能の入口・契約・OS ごとの対応 | `crates/fandhe-cap-<id>/src/lib.rs` の crate doc と契約の trait doc |
| 機能が要求する権限・説明文のキー | `crates/fandhe-cap-<id>/Cargo.toml` の `[package.metadata.fandhe.*]` |
| どの OS で動くか | `docs/device/capabilities/<id>.json` の `platforms.<os>.status`・`implStatus`・`note` |
| 人向けの導入手順（環境・実行・配布） | `docs/guide/getting-started.md` |

本書の Rust のコード例は、fandhe の `cargo test` が doctest としてコンパイルします。

## 1. fandhe のソースの場所

cargo が取得した fandhe のソースを読みます。依存の版と同じ版の資料が読めます。アプリの直下で次を実行します（POSIX シェル）。

```sh
FANDHE_SRC=$(cargo metadata --format-version 1 \
  | grep -o '"manifest_path":"[^"]*/crates/fandhe/Cargo.toml"' | head -n 1 \
  | sed -e 's/^"manifest_path":"//' -e 's#/crates/fandhe/Cargo.toml"$##')
if [ -d "$FANDHE_SRC/skills/fandhe-app" ]; then
  echo "$FANDHE_SRC"
else
  echo "fandhe のソースが見つかりません" >&2
fi
```

- git 依存なら `~/.cargo/git/checkouts/` の下、path 依存ならその clone を指します
- 見つからないときは、`cargo metadata` が失敗した（エラーを読む）、`Cargo.toml` に `fandhe` の依存が無い、
  パスの区切りが `\`（Windows）のどれかです。`cargo metadata` の出力から
  `crates/fandhe/Cargo.toml` で終わる `manifest_path` を探します

## 2. `fandhe::` から届くもの

| パス | 中身 |
| --- | --- |
| `fandhe::{Screen, Node, WidgetId, render_headless, click_button, …}` | 画面の宣言（`fandhe-core` を丸ごと再公開。SCHEMA.md の `fandhe_core::X` は `fandhe::X`） |
| `fandhe::app::{ProxyApp, ProxyAppDyn, Menu, CompletionSender, Completion, EffectFn, LaunchError}` | 画面と effect を束ねて入口へ渡す部品 |
| `fandhe::runtime::{timer, image_store, …}` | 時間の予約の型（`Armed`・`TimerRefused`）、画素の保管所 |
| `fandhe::egui` | 入口の関数が受け取る `egui::Context` |
| `#[fandhe::main]` | 各 OS の入口（デスクトップ・iOS の `main`、Android の `android_main`、Web の `start`）を生成する属性 |
| `fandhe::<stem>` | feature で選んだ端末機能（`<stem>` は id の `-` を `_` にした名前） |

使わないもの:

- `fandhe::screens::*`: fandhe のカタログの画面。自分の状態型と画面は自分の crate に書く
- `fandhe::__private`: `#[fandhe::main]` の生成コード用
- 機能 crate の `driver` モジュール: `fandhe::<stem>::driver::…` のパスでは届かない（カタログの画面専用）。
  ただし `driver` で定義され crate 直下に再公開された入口（`platform_<stem>`・`begin_*` など）は、`fandhe::<stem>::…` から使う

## 3. アプリの形

`Cargo.toml`・`src/main.rs`・`rust-toolchain.toml` は `docs/guide/getting-started.md` 3 節・4 節のとおりです。
`src/lib.rs` には、状態の型・状態を変える関数・状態から画面を作る関数・`#[fandhe::main]` を付けた入口を書きます。

- 1 画面のアプリは `ProxyApp::with_screen_fn(ctx, 画面を作る関数, 初期状態)` を返します（getting-started.md 4 節）
- 複数画面は `Menu` で組みます。画面ごとに状態型を分け、`ProxyApp` を作る閉包を `.screen(表示名, 閉包)` で足します。
  画面はメニューで選ぶたびに作り直し、戻ると捨てます（状態を持ち越しません）
- 画面の中で画面を積む・戻る（一覧 → 詳細）は SCHEMA.md パターン 8（`ScreenStack`）です

```rust
// Android は生成される `android_main` が `no_mangle` を伴うため deny へ緩める。
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

use fandhe::app::{Menu, ProxyApp, ProxyAppDyn};
use fandhe::{Screen, WidgetId, egui};

/// カウンター画面の状態。
#[derive(Default)]
pub struct Counter {
    count: u32,
}

fn increment(s: &mut Counter) {
    s.count += 1;
}

fn counter_screen(_: &Counter) -> Screen<Counter> {
    Screen::new("カウンター")
        .dynamic_text(|s: &Counter| format!("押した回数: {}", s.count))
        .button(WidgetId::from_raw(1), "押す", increment)
}

/// メモ画面の状態。画面ごとに別の型にする。
#[derive(Default)]
pub struct Memo {
    text: String,
}

fn memo_screen(_: &Memo) -> Screen<Memo> {
    Screen::new("メモ")
        .text_field("メモ", |s: &Memo| s.text.clone(), |s: &mut Memo, v| s.text = v)
        .dynamic_text(|s: &Memo| format!("{} 文字", s.text.chars().count()))
}

#[fandhe::main]
fn app(ctx: &egui::Context) -> Box<dyn ProxyAppDyn> {
    Box::new(
        Menu::new(ctx, "メニュー")
            .screen("カウンター", |ctx| {
                Box::new(ProxyApp::with_screen_fn(ctx, counter_screen, Counter::default()))
            })
            .screen("メモ", |ctx| {
                Box::new(ProxyApp::with_screen_fn(ctx, memo_screen, Memo::default()))
            }),
    )
}
```

## 4. 状態と effect

- 画面を作る関数（`fn(&S) -> Screen<S>`）は毎フレーム呼ばれます。状態に応じて部品を出し分けてよく、副作用を持たせません
- ボタン・入力欄の処理は状態を変える関数ポインタです。環境をキャプチャする閉包は渡せません（SCHEMA.md「型制約」）
- 端末機能・時間・外部 I/O は effect（`fn(&S, &CompletionSender<S>)`）に書き、`.with_effect(effect)` で渡します。
  effect は毎フレーム、画面の描画の後に呼ばれます
- effect は状態を `&S` でしか受け取りません。状態は `tx.send(Box::new(閉包))` で積んだ完了でだけ変えます。積んだ完了は次のフレームの先頭で適用されます
- 時間をおいて状態を変えるときは `tx.after(キー, 遅れ, 閉包)` です（SCHEMA.md パターン 9-2）。`Err` は予約されていないので捨てません
- 「ボタンで 1 回だけ」の処理は、ボタンが状態に要求の印を立て、effect が印を見て処理し、完了で印を下ろします。
  処理の開始が別スレッドや OS へ渡るときは、開始した印（要求 id など）を `send` で状態へ移し、次のフレームで二重に開始しないようにします（5 節）
- effect の中で長く待つ呼び出し（crate doc に「OS の完了まで待つ」とある同期の入口・ネットワーク）をすると、待つ間画面が止まります。
  begin / poll がある機能はそちらを使い、無ければ別スレッドで呼んで結果を `send` します

クリップボードへのコピーと、2 秒後に表示を戻す例です（feature `clipboard`）。

```rust
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

use std::time::Duration;

use fandhe::app::{CompletionSender, ProxyApp, ProxyAppDyn};
use fandhe::clipboard::platform_clipboard;
use fandhe::{Screen, WidgetId, egui};

#[derive(Default)]
pub struct Share {
    text: String,
    /// ボタンが立て、effect が処理して下ろす要求の印。
    copy_requested: bool,
    status: String,
}

fn request_copy(s: &mut Share) {
    s.copy_requested = true;
}

fn screen(_: &Share) -> Screen<Share> {
    Screen::new("コピー")
        .text_field("本文", |s: &Share| s.text.clone(), |s: &mut Share, v| s.text = v)
        .button(WidgetId::from_raw(1), "コピー", request_copy)
        .dynamic_text(|s: &Share| s.status.clone())
}

fn effect(s: &Share, tx: &CompletionSender<Share>) {
    if s.copy_requested {
        let status = match platform_clipboard() {
            Ok(clipboard) => match clipboard.set_text(&s.text) {
                Ok(()) => "コピーしました".to_string(),
                Err(e) => format!("コピーできません: {e}"),
            },
            Err(message) => message.to_string(),
        };
        // コピーし直したら、前の結果の予約を取り下げる（下の after が新しい結果から 2 秒で予約し直す）。
        tx.cancel_timer("status-reset");
        let _ = tx.send(Box::new(move |s: &mut Share| {
            s.copy_requested = false;
            s.status = status;
        }));
    }
    // 表示が出ている間は毎フレーム呼んでよい（同じキーの予約が残っていれば何もしない。期限は延びない）。
    if !s.status.is_empty() {
        if let Err(e) = tx.after("status-reset", Duration::from_secs(2), |s: &mut Share| {
            s.status.clear()
        }) {
            eprintln!("表示を戻す予約ができません: {e}");
        }
    }
}

#[fandhe::main]
fn app(ctx: &egui::Context) -> Box<dyn ProxyAppDyn> {
    Box::new(ProxyApp::with_screen_fn(ctx, screen, Share::default()).with_effect(effect))
}
```

## 5. 端末機能の入口の形

入口の名前・引数・戻り値は機能ごとに違います。推測で書かず、`$FANDHE_SRC/crates/fandhe-cap-<id>/src/lib.rs` の `pub use` と
crate doc、契約の trait の doc を読んでから書きます。形は次の 3 つです。

| 形 | 呼び方 | 例 |
| --- | --- | --- |
| 同期 | `platform_<stem>()` が trait の実装（または `Result`）を返し、そのメソッドを呼ぶ | `platform_clipboard()?.set_text(..)`、`platform_network_status().read()` |
| begin / poll / discard | `begin_*` が要求 id を返し、`poll_*(id)` で結果を 1 度だけ受け取る。要らなくなった要求は `discard_*(id)` | `begin_file_pick()`・`poll_file_pick(id)`・`discard_file_pick(id)` |
| 共有状態を読み続ける | `platform_<stem>()` が OS から値が届き続ける共有状態を返し、effect から定期的に読む | `platform_location().last_fix()` |

- 共有状態を読み続ける形は、OS ごとに測定・受信を始める条件が違います（`platform_<stem>()` を呼ぶだけで始まる OS と、
  別の関数を main スレッドで呼ぶ OS がある）。機能 crate の `platform_<stem>` の doc を読み、OS ごとの始め方をそのとおりに書きます。
  location の入口は #1458 で整える予定です
- 同じ機能でも OS で入口が分かれることがあります（file-picker は macOS・Windows が同期、iOS・Android が begin / poll。
  合わない OS では `Unsupported` を返します）。crate doc の OS ごとの記述を読みます
- 値が繰り返し変わる機能（接続状態・センサー）は、effect で読むたびに値が変わったときだけ `send` し、
  変わらなくても `tx.request_repaint_after(間隔)` で次の読み直しを予約します（getting-started.md 5 節の例）
- 結果は成功・キャンセル・失敗を分けて画面に出します。キャンセル（例: `Ok(None)`）と `Unsupported` を成功として見せません
- 失敗の値は `Display` で文字にできます。秘密になりうる値（クリップボードの中身・選んだファイルの中身）はログに出しません

begin / poll の例です（feature `file-picker`）。begin が返した要求 id は、捨てられたときに discard する持ち主（`Ticket`）に
すぐ包みます。完了の閉包が適用される前に画面が捨てられても、閉包と一緒に `Ticket` が捨てられて要求が解放されます。

```rust
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

use std::time::Duration;

use fandhe::app::{CompletionSender, ProxyApp, ProxyAppDyn};
use fandhe::file_picker::{begin_file_pick, discard_file_pick, poll_file_pick};
use fandhe::{Screen, WidgetId, egui};

/// begin が返した要求 id の持ち主。捨てられたら discard する。捨てないと slot に残り、次の begin が busy になる。
/// 結果を受け取り終えた id の discard は何もしないので、受け取った後に捨ててもよい。
struct Ticket(u64);

impl Drop for Ticket {
    fn drop(&mut self) {
        discard_file_pick(self.0);
    }
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    /// ボタンが押され、effect の begin を待つ。
    Requested,
    /// begin 済み。poll で結果を待つ。画面（`Menu` で戻る）と一緒に捨てられると discard される。
    Waiting(Ticket),
}

#[derive(Default)]
pub struct Pick {
    phase: Phase,
    message: String,
}

fn request(s: &mut Pick) {
    if matches!(s.phase, Phase::Idle) {
        s.phase = Phase::Requested;
    }
}

fn screen(_: &Pick) -> Screen<Pick> {
    Screen::new("ファイル")
        .button(WidgetId::from_raw(1), "選ぶ", request)
        .dynamic_text(|s: &Pick| s.message.clone())
}

fn effect(s: &Pick, tx: &CompletionSender<Pick>) {
    match &s.phase {
        Phase::Idle => {}
        Phase::Requested => {
            // begin はこのフレームで 1 回だけ。id はすぐ Ticket に包み、send で状態へ移す（次のフレームから Waiting）。
            let (phase, message) = match begin_file_pick() {
                Ok(id) => (Phase::Waiting(Ticket(id)), "選んでいます".to_string()),
                Err(e) => (Phase::Idle, format!("開けません: {e}")),
            };
            let _ = tx.send(Box::new(move |s: &mut Pick| {
                s.phase = phase;
                s.message = message;
            }));
        }
        Phase::Waiting(ticket) => match poll_file_pick(ticket.0) {
            Some(result) => {
                let message = match result {
                    Ok(Some(file)) => format!("{}（{} バイト）", file.name, file.bytes.len()),
                    Ok(None) => "キャンセルしました".to_string(),
                    Err(e) => format!("読めません: {e}"),
                };
                let _ = tx.send(Box::new(move |s: &mut Pick| {
                    s.phase = Phase::Idle;
                    s.message = message;
                }));
            }
            // 未着。次の poll を予約する。
            None => tx.request_repaint_after(Duration::from_millis(100)),
        },
    }
}

#[fandhe::main]
fn app(ctx: &egui::Context) -> Box<dyn ProxyAppDyn> {
    Box::new(ProxyApp::with_screen_fn(ctx, screen, Pick::default()).with_effect(effect))
}
```

## 6. 宣言と説明文

- 機能は `Cargo.toml` の `fandhe = { …, features = ["<id>", …] }` で選びます。機能 crate を直接依存に書くのは、
  Swift 薄層を持つ機能のリンク指定を受け取るとき（下記）だけです。直接依存だけでは起動前の登録が走らない機能があります（終了処理は機能 crate が自分で登録する）
- 権限（Android の `uses-permission`）・framework・entitlement は機能の宣言から `cargo fandhe gen` が入れます。アプリは書きません
- iOS・macOS の許可ダイアログの説明文（`*UsageDescription`）だけはアプリが書きます。キーは機能 crate の
  `[package.metadata.fandhe.ios]`・`[package.metadata.fandhe.macos]` の `usage-descriptions` にあり、無ければ `gen` が 1 行ずつ出して止まります。
  書く場所と文言の良し悪しは getting-started.md 6 節です。文言は「何のために・何を使うか」を利用者の言葉で 1 文にし、
  目的が分からなければ利用者に聞きます。仮の文言で `gen` を通しません
- 機能 crate の `Cargo.toml` に `links = "…"` がある機能（Swift 薄層・弱リンク）は、feature だけでは iOS・macOS のリンクが失敗します。
  直接依存・`build.rs`・`.cargo/config.toml` を getting-started.md 10.1 節のとおりに足します
- `gen/` は毎回作り直されます。手で直さず、`[package.metadata.fandhe-app.*]` への追記で変えます（getting-started.md 7 節）

## 7. 利用者のアプリで誤りやすい点

画面の DSL の誤り（キャプチャする閉包・`WidgetId::from_raw`・self 消費のビルダー等）は SCHEMA.md「既知の誤りやすい点」です。
ここでは利用者のアプリに固有のものだけを挙げます。

| 誤り | 症状 | 正しい書き方 |
| --- | --- | --- |
| `#[fandhe::main]` を `fn main` に付ける | `画面を作る関数は main 以外の名前に` のコンパイルエラー | `fn app` などにし、`src/main.rs` から `<lib 名>::main()` を呼ぶ |
| crate に `#![forbid(unsafe_code)]` をそのまま付ける | Android のビルドだけ失敗する | 3 節の `cfg_attr` の 2 行 |
| 画面の先頭の `dynamic_text` の閉包に型を書かない | `type annotations needed`（E0282） | `\|s: &State\| …` と引数の型を書く |
| effect で状態を直接変えようとする | `&S` なので変えられない | `tx.send` で完了を積む |
| 要求の印を見て毎フレーム begin する | 2 回目以降が busy で失敗する | 4 節・5 節のとおり、開始した印を `send` で状態へ移す |
| 値の変わらないフレームで再描画を予約しない | 入力が無いと読み直しが止まり、古い値のまま | `tx.request_repaint_after(間隔)` |
| feature を足さずに `fandhe::<stem>` を使う | `use` なら ``unresolved import `fandhe::<stem>` ``（E0432）、パスで呼ぶなら ``cannot find `<stem>` in `fandhe` ``（E0433） | `Cargo.toml` の `features` に id を足す |
| trait のメソッドを、trait を import せずに呼ぶ | ``no method named `last_fix` found``（E0599） | 契約の trait も import する（例: `use fandhe::location::{Location, platform_location};`） |
| 画面を捨てるときに begin / poll の要求を discard しない | `Menu` で戻ってから同じ機能を使うと busy で失敗する | begin が返した id をすぐ、捨てられたら `discard_*(id)` する持ち主で包む。完了の閉包が適用される前に捨てられても解放される（5 節の例の `Ticket`） |
| 説明文に仮の文言を書いて `gen` を通す | 審査で差し戻される | 目的を利用者に確かめて書く |
| `fandhe::screens::*` の状態型を使う | カタログの画面に縛られ、自分の画面と合わない | 状態型と画面は自分の crate に書く |

OS ごとの既知の制約（macOS の実行体に Info.plist が入らない #1454、Web で `Unsupported` になる機能 #1456 など）は
getting-started.md 10 節です。足した機能が対象 OS で動くかは対応表（`docs/device/capabilities/<id>.json`）で確かめ、
`done` でない OS は利用者に伝えます。
