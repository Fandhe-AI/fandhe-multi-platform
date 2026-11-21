//! 終了処理・画面の破棄・Android の Activity 再生成時の破棄の登録表（Issue #1453）。
//!
//! 機能 crate は、初めて使われたとき（公開の入口・effect・公開型のメソッドの先頭）に [`register`] で自分の片付けを登録する。
//! 使っていない機能は OS 資源を持たないので、片付けも要らない。`fandhe` の feature で選んだ標準機能も、
//! 直接依存で選んだ標準機能も、第三者の機能も同じ口を通る。
//!
//! 表を回すのは描画層と入口で、機能 crate は回さない。
//!
//! - アプリの終了（ウィンドウを閉じる経路。`fandhe-render-egui` の `EguiApp::on_exit`）で [`run_exit`]
//! - 画面（`ProxyApp`）の破棄（同じプロセスで別の画面へ切り替えたとき）で [`run_screen_drop`]
//! - Android の Activity 再生成の前（`fandhe` の `android_main`。新しい画面状態を作る前）で [`run_android_recreate`]
//!
//! 起動前の登録（URL scheme の受信など、イベントループの開始前に済ませるもの）はこの表に載せない。
//! 初めて使われるより前に要るため遅らせられず、`fandhe` の feature で選ぶか利用者が `main` で呼ぶ
//! （`docs/architecture/capability-crate-contract.md` 5.3 節）。
//!
//! # 守備範囲
//!
//! 守る:
//!
//! - 同じ id の登録は 1 件にまとめる（先に登録したものを使う）。入口・effect が毎回呼んでよい
//! - 段階（[`Stage`]）の順に呼ぶ。[`Stage::Feature`] をすべて呼んでから [`Stage::Shared`] を呼ぶ（登録順に依らない）
//! - 1 回の実行の途中で登録された機能も、同じ回で呼ぶ（終了処理の途中で初めて使われた機能も止める）
//! - 表のロックを持ったまま登録済みの処理を呼ばない。処理の中から [`register`] を呼んでもデッドロックしない
//! - 実行の後も登録は残る。再生成の後に次の再生成・終了が来れば、また呼ぶ
//!
//! 守らない:
//!
//! - 実行が終わった後に初めて使われた機能は、その回には呼ばない（終了処理なら、プロセスは直後に終わる）
//! - 実行の途中で遅れて登録された [`Stage::Feature`] の処理は、その回で既に呼んだ [`Stage::Shared`] より後になる
//! - 同じ段階の中の順。順に依存がある機能は段階で表す
//! - 処理が panic したときの残りの処理
//! - 複数のスレッドから同時に実行したときの重複。登録する処理は 2 回呼ばれても安全にする（停止済みなら何もしない）
//! - 処理の中から `run_*` を呼ぶこと（入れ子の実行は同じ処理をもう一度呼ぶ）

use std::sync::{Mutex, MutexGuard};

/// 止める段階。小さい段階から先に呼ぶ。段階は後から足しうるため `non_exhaustive`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Stage {
    /// 既定。機能ごとの資源。
    #[default]
    Feature,
    /// 他の機能が上に載る共有の OS 資源（audio-session の音声セッションなど）。[`Stage::Feature`] の後に止める。
    Shared,
}

/// 1 機能ぶんの片付け。`const` で組み立てられる。
#[derive(Clone, Copy, Debug)]
pub struct Teardown {
    id: &'static str,
    stage: Stage,
    on_exit: Option<fn()>,
    on_screen_drop: Option<fn()>,
    on_android_recreate: Option<fn()>,
}

impl Teardown {
    /// `id` は、登録する crate 自身の宣言の id（`[package.metadata.fandhe]` の `id`）に限る。他の機能の id で登録しない
    /// （同じ id の登録は 1 件にまとめるため、後から来た方が捨てられる）。
    pub const fn new(id: &'static str) -> Self {
        Self {
            id,
            stage: Stage::Feature,
            on_exit: None,
            on_screen_drop: None,
            on_android_recreate: None,
        }
    }

    /// 止める段階を指定する。既定は [`Stage::Feature`]。
    pub const fn stage(mut self, stage: Stage) -> Self {
        self.stage = stage;
        self
    }

    /// アプリの終了で呼ぶ処理。
    pub const fn on_exit(mut self, f: fn()) -> Self {
        self.on_exit = Some(f);
        self
    }

    /// 画面（`ProxyApp`）の破棄で呼ぶ処理。停止操作なしで残りうる購読の解放に使う。
    pub const fn on_screen_drop(mut self, f: fn()) -> Self {
        self.on_screen_drop = Some(f);
        self
    }

    /// Android の Activity 再生成の前に呼ぶ処理。保留中の要求と処理済み世代の破棄に使う。
    pub const fn on_android_recreate(mut self, f: fn()) -> Self {
        self.on_android_recreate = Some(f);
        self
    }
}

/// 登録表の本体。プロセスでは [`GLOBAL`] の 1 つだけを使う（テストは別の表を作る）。
struct Registry(Mutex<Table>);

struct Table {
    entries: Vec<Teardown>,
    /// 食い違う登録を警告済みの id（警告は id ごとに 1 回）。
    warned: Vec<&'static str>,
}

/// [`Registry::register`] の結果。
#[derive(Debug, PartialEq, Eq)]
enum Registered {
    Added,
    /// 同じ id・同じ内容が登録済み（入口・effect が毎回呼ぶ通常の経路）。
    Already,
    /// 同じ id で段階か処理が違う登録が来た。後の登録は捨てる。`warned` は今回はじめて警告したか。
    Conflict {
        warned: bool,
    },
}

static GLOBAL: Registry = Registry::new();

impl Teardown {
    /// 段階と処理（関数ポインタ）が同じか。関数ポインタの一致は Rust が保証しない（同じ関数でもコード生成の単位で
    /// 別の番地になりうる・別の関数が 1 つにまとめられうる）ため、食い違いの検出は警告だけに使い、動作は変えない。
    fn same_hooks(&self, other: &Teardown) -> bool {
        let addr = |f: Option<fn()>| f.map(|f| f as usize);
        self.stage == other.stage
            && addr(self.on_exit) == addr(other.on_exit)
            && addr(self.on_screen_drop) == addr(other.on_screen_drop)
            && addr(self.on_android_recreate) == addr(other.on_android_recreate)
    }
}

impl Registry {
    const fn new() -> Self {
        Self(Mutex::new(Table {
            entries: Vec::new(),
            warned: Vec::new(),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, Table> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 同じ id が無ければ足す。同じ id で内容が違えば先の登録を残し、id ごとに 1 回だけ標準エラーへ警告する。
    ///
    /// `debug_assert` にしない理由: 依存の中に同じ機能 crate の互換しない 2 版が並ぶと、正しい使い方でも同じ id で
    /// 別の処理が届く（`capability-crate-contract.md` 7 節）。関数ポインタの比較も確実ではない（[`Teardown::same_hooks`]）。
    /// どちらも利用者のアプリを debug ビルドで止める理由にならないため、気づける形（警告）に留める。
    fn register(&self, t: Teardown) -> Registered {
        let mut table = self.lock();
        let Some(existing) = table.entries.iter().find(|e| e.id == t.id) else {
            table.entries.push(t);
            return Registered::Added;
        };
        if existing.same_hooks(&t) {
            return Registered::Already;
        }
        let warned = !table.warned.contains(&t.id);
        if warned {
            table.warned.push(t.id);
            // id は宣言の公開値で、秘密を含まない。
            eprintln!(
                "[fandhe-runtime] teardown: id `{}` に別の片付けが登録されたため、後の登録を捨てた（id は自 crate の宣言の id に限る）",
                t.id
            );
        }
        Registered::Conflict { warned }
    }

    fn registered(&self, id: &str) -> bool {
        self.lock().entries.iter().any(|e| e.id == id)
    }

    /// 段階・登録順で次の未実行の 1 件を取り、ロックを放してから呼ぶ。途中で登録された分も拾う。
    fn run(&self, pick: fn(&Teardown) -> Option<fn()>) {
        let mut done: Vec<&'static str> = Vec::new();
        loop {
            let next = {
                let table = self.lock();
                table
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| !done.contains(&t.id))
                    .min_by_key(|(i, t)| (t.stage, *i))
                    .map(|(_, t)| (t.id, pick(t)))
            };
            let Some((id, f)) = next else { break };
            done.push(id);
            if let Some(f) = f {
                f();
            }
        }
    }
}

/// 機能の片付けを登録する。同じ id が登録済みなら何もしない。公開の入口・effect の先頭で毎回呼んでよい。
///
/// 同じ id で段階か処理が違う登録は捨て、id ごとに 1 回だけ標準エラーへ警告する（id は自 crate の宣言の id に限る）。
pub fn register(t: Teardown) {
    let _ = GLOBAL.register(t);
}

/// テスト用。`id` の機能が登録済みか。機能 crate が「入口を呼んだら登録される」ことをテストで確かめるために使い、
/// 本番のコードの分岐には使わない。
#[doc(hidden)]
pub fn registered(id: &str) -> bool {
    GLOBAL.registered(id)
}

/// 描画層・入口専用。登録済みの終了処理（[`Teardown::on_exit`]）を呼ぶ。描画層がウィンドウを閉じる経路から呼ぶ。
/// 機能 crate は呼ばない。
#[doc(hidden)]
pub fn run_exit() {
    GLOBAL.run(|t| t.on_exit);
}

/// 描画層・入口専用。登録済みの画面の破棄の処理（[`Teardown::on_screen_drop`]）を呼ぶ。描画層が `ProxyApp` の
/// `Drop` から呼ぶ（Web を含む全 OS）。機能 crate は呼ばない。
#[doc(hidden)]
pub fn run_screen_drop() {
    GLOBAL.run(|t| t.on_screen_drop);
}

/// 描画層・入口専用。登録済みの再生成時の破棄（[`Teardown::on_android_recreate`]）を呼ぶ。`fandhe` の Android の入口が、
/// 新しい画面状態を作る前に呼ぶ。機能 crate は呼ばない。
#[doc(hidden)]
pub fn run_android_recreate() {
    GLOBAL.run(|t| t.on_android_recreate);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 呼ばれた順の記録。テストごとに別の static を使う（テストは並行に走る）。
    type Log = Mutex<Vec<&'static str>>;

    fn push(log: &Log, name: &'static str) {
        log.lock().unwrap().push(name);
    }

    #[test]
    fn duplicate_registration_runs_once() {
        static REG: Registry = Registry::new();
        static LOG: Log = Mutex::new(Vec::new());
        fn a() {
            push(&LOG, "a");
        }
        fn other() {
            push(&LOG, "other");
        }
        REG.register(Teardown::new("a").on_exit(a));
        REG.register(Teardown::new("a").on_exit(a));
        // 同じ id の別の登録は、先の登録を残して捨てる。
        REG.register(Teardown::new("a").on_exit(other));
        // 入口・effect は毎フレーム登録するため、表は伸びない。
        assert_eq!(REG.lock().entries.len(), 1);
        REG.run(|t| t.on_exit);
        assert_eq!(*LOG.lock().unwrap(), ["a"]);
    }

    /// 同じ id で処理が違う登録は検出して捨て、警告は id ごとに 1 回だけ。同じ内容の再登録は食い違いにしない。
    #[test]
    fn conflicting_registration_is_detected_once() {
        static REG: Registry = Registry::new();
        // 本体を変えておく（同じ本体の関数は 1 つにまとめられ、番地が一致しうる）。
        static LOG: Log = Mutex::new(Vec::new());
        fn a() {
            push(&LOG, "a");
        }
        fn b() {
            push(&LOG, "b");
        }
        assert_eq!(
            REG.register(Teardown::new("x").on_exit(a)),
            Registered::Added
        );
        assert_eq!(
            REG.register(Teardown::new("x").on_exit(a)),
            Registered::Already
        );
        assert_eq!(
            REG.register(Teardown::new("x").on_exit(b)),
            Registered::Conflict { warned: true }
        );
        assert_eq!(
            REG.register(Teardown::new("x").on_exit(a).stage(Stage::Shared)),
            Registered::Conflict { warned: false }
        );
        assert_eq!(REG.lock().warned, ["x"]);
    }

    /// 共有資源（audio-session）を先に登録しても、機能（media-playback）の後に止める。
    #[test]
    fn shared_stage_runs_after_feature_stage_regardless_of_order() {
        static REG: Registry = Registry::new();
        static LOG: Log = Mutex::new(Vec::new());
        fn audio_session() {
            push(&LOG, "audio-session");
        }
        fn media_playback() {
            push(&LOG, "media-playback");
        }
        fn now_playing() {
            push(&LOG, "now-playing-controls");
        }
        REG.register(
            Teardown::new("audio-session")
                .stage(Stage::Shared)
                .on_exit(audio_session)
                .on_android_recreate(audio_session),
        );
        REG.register(
            Teardown::new("media-playback")
                .on_exit(media_playback)
                .on_android_recreate(media_playback),
        );
        REG.register(Teardown::new("now-playing-controls").on_exit(now_playing));
        REG.run(|t| t.on_exit);
        assert_eq!(
            *LOG.lock().unwrap(),
            ["media-playback", "now-playing-controls", "audio-session"]
        );
        LOG.lock().unwrap().clear();
        REG.run(|t| t.on_android_recreate);
        assert_eq!(*LOG.lock().unwrap(), ["media-playback", "audio-session"]);
    }

    /// 処理の中から登録してもデッドロックせず、途中で登録された機能も同じ回で呼ぶ。
    #[test]
    fn registration_during_run_is_called_in_the_same_run() {
        static REG: Registry = Registry::new();
        static LOG: Log = Mutex::new(Vec::new());
        fn late() {
            push(&LOG, "late");
        }
        fn first() {
            push(&LOG, "first");
            REG.register(Teardown::new("late").on_exit(late));
        }
        REG.register(Teardown::new("first").on_exit(first));
        REG.run(|t| t.on_exit);
        assert_eq!(*LOG.lock().unwrap(), ["first", "late"]);
    }

    /// 実行の後も登録は残り、次の実行（再生成の 2 回目・終了）でも呼ぶ。再登録は増やさない。
    #[test]
    fn registrations_survive_a_run() {
        static REG: Registry = Registry::new();
        static LOG: Log = Mutex::new(Vec::new());
        fn reset() {
            push(&LOG, "reset");
        }
        fn exit() {
            push(&LOG, "exit");
        }
        let t = Teardown::new("x").on_exit(exit).on_android_recreate(reset);
        REG.register(t);
        REG.run(|t| t.on_android_recreate);
        REG.register(t);
        REG.run(|t| t.on_android_recreate);
        REG.run(|t| t.on_exit);
        assert_eq!(*LOG.lock().unwrap(), ["reset", "reset", "exit"]);
    }

    /// 別のスレッドの初回登録と実行が重なっても、どちらも止まらずに終わり、登録は失われない。
    #[test]
    fn concurrent_registration_and_run_do_not_block() {
        static REG: Registry = Registry::new();
        fn noop() {}
        REG.register(Teardown::new("seed").on_exit(noop));
        let workers: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    const IDS: [&str; 8] = ["w0", "w1", "w2", "w3", "w4", "w5", "w6", "w7"];
                    REG.register(Teardown::new(IDS[i]).on_exit(noop));
                    REG.run(|t| t.on_exit);
                })
            })
            .collect();
        for w in workers {
            w.join().unwrap();
        }
        for id in ["seed", "w0", "w7"] {
            assert!(REG.registered(id));
        }
    }

    #[test]
    fn hooks_absent_for_the_event_are_skipped() {
        static REG: Registry = Registry::new();
        static LOG: Log = Mutex::new(Vec::new());
        fn drop_only() {
            push(&LOG, "drop");
        }
        REG.register(Teardown::new("d").on_screen_drop(drop_only));
        REG.run(|t| t.on_exit);
        assert!(LOG.lock().unwrap().is_empty());
        REG.run(|t| t.on_screen_drop);
        assert_eq!(*LOG.lock().unwrap(), ["drop"]);
    }
}
