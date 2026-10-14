//! 任意テキストの読み上げ（TTS）の抽象（Phase 11、Issue #865）。
//!
//! `Clipboard` と同じ request/response の一発呼び出し型（`one-shot` 束）で、
//! `Motion` のような継続 push の共有状態は持たない。
//!
//! # なぜ `tts` クレート（crates.io）を使わないか
//!
//! 実査の結果、不採用とした（PR 本文に詳細）。要点:
//! - `tts::Tts` は `Send`/`Sync` を実装しない（`OnceLock<Box<dyn Tts>>` 方式が成立しない）
//! - macOS 経路が保守終了気味の `objc 0.2` + `cocoa-foundation 0.1` に依存する
//! - Linux 経路は `libspeechd` という C ライブラリ必須（CI runner の事故要因）
//! - iOS/Android/Web は本クレートが担当しない（#866/#867 が FFI・ブラウザ API で配線）ため
//!   共通化の利得が小さい
//!
//! 代替として **OS 同梱の音声合成コマンドを `std::process::Command` で起動する**方式を採る
//! （新規依存ゼロ、`#![forbid(unsafe_code)]` 維持、`Send + Sync` が自明）。
//! - macOS: `/usr/bin/say`（テキストは **stdin** で渡す。`say` は引数もファイルも
//!   無ければ標準入力を読む）
//! - Windows: PowerShell + `System.Speech.Synthesis.SpeechSynthesizer`
//!   （テキストは **環境変数 `FANDHE_TTS_TEXT`** で渡す。スクリプト文字列は固定で
//!   テキストを連結しない = コード実行経路を作らない。security.md A03）
//! - それ以外（Linux・想定外ターゲット）は `DeviceError::Unsupported("tts")`
//!
//! iOS/Android は `fandhe-egui-proxy` 側（FFI 境界を持てるレイヤ）で実装する
//! （#866）。Web は `fandhe-web-dom` 側でブラウザの `SpeechSynthesis` を使う（#867）。
//!
//! ponytail: サンドボックス配布（Mac App Store・MSIX）は子プロセス起動を拒否する
//! 天井がある。上げ先は #866 で追加される ObjC 側 `AVSpeechSynthesizer` 橋渡しを
//! macOS へ流用すること（`AVSpeechSynthesizer` は iOS/macOS 共通 API のため、
//! その時点で本モジュールの subprocess 方式を置き換えられる）。

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::DeviceError;

/// 起動直後の即時失敗（Windows で `System.Speech` のロード・`Speak` 呼び出しが
/// 失敗する等）を検知するための猶予時間。通常の発話はこれよりずっと長く続くため
/// 正常系の体感レイテンシへの影響は小さい（P1 指摘: spawn 成功と発話成功の混同）。
/// この猶予を過ぎてから失敗するケース（例: 猶予後に音声デバイス初期化が破綻する）は
/// [`Tts::wait_outcome`] が別スレッドから非同期にポーリングして拾う
/// （codex-review P1 指摘の是正、`wait_outcome` のトレイト doc 参照）。
const STARTUP_FAILURE_GRACE: Duration = Duration::from_millis(200);

/// [`CommandTts::wait_outcome`] が子プロセスの終了を待つ間隔。`stop()` が
/// 同じ `Mutex<Option<Child>>` を取り合えるよう、待機中は毎回ロックを手放す
/// （ロックを持ったまま `Child::wait` すると `stop()` が発話終了までブロックされ、
/// 中断できなくなる）。短すぎると busy loop で無駄に CPU を消費し、長すぎると
/// 失敗検知・停止の反映が遅れるため、体感で問題にならない範囲の値にする。
const WAIT_OUTCOME_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// テキストを子プロセスへ渡す経路。シェル連結を避けるための選択肢
/// （security.md A03: インジェクション対策）。
enum TextChannel {
    /// 標準入力へ書き込む（macOS の `say`）。
    Stdin,
    /// 指定した名前の環境変数へ設定する（Windows の PowerShell スクリプト）。
    Env(&'static str),
}

/// 発話コマンドの起動情報。`program` は必ず絶対パス
/// （PATH 探索による差し替え = PATH 乗っ取りを避ける、security.md A03/A05）。
struct SpeechCommand {
    program: PathBuf,
    args: Vec<String>,
    text_via: TextChannel,
}

/// macOS 向け発話コマンドを組み立てる（`say` は絶対パスで起動し、テキストは stdin）。
///
/// どのホストでもコンパイル・テストできるよう `#[cfg(target_os = "macos")]` を
/// 付けない（2.4 参照）。macOS 以外のホストでは呼ばれないため `allow(dead_code)`。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_speak_command(_text: &str) -> SpeechCommand {
    SpeechCommand {
        program: PathBuf::from("/usr/bin/say"),
        args: Vec::new(),
        text_via: TextChannel::Stdin,
    }
}

/// 発話に使う環境変数名。Windows のスクリプトはこの名前を固定で参照する。
const WINDOWS_TTS_TEXT_ENV: &str = "FANDHE_TTS_TEXT";

/// Windows 向け発話コマンドを組み立てる。テキストはスクリプト文字列へ連結せず
/// 環境変数 [`WINDOWS_TTS_TEXT_ENV`] 経由で渡す（A03 対策）。
/// `-NoProfile -NonInteractive` でユーザープロファイルのスクリプト読み込みを避ける。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_speak_command(_text: &str) -> SpeechCommand {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let program = PathBuf::from(system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    SpeechCommand {
        program,
        args: vec![
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            format!(
                "Add-Type -AssemblyName System.Speech; \
                 (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak($env:{WINDOWS_TTS_TEXT_ENV})"
            ),
        ],
        text_via: TextChannel::Env(WINDOWS_TTS_TEXT_ENV),
    }
}

/// 現在のホスト OS 向けの発話コマンドを選ぶ。macOS/Windows 以外は `None`
/// （呼び出し元は `Unsupported` を返す）。
fn platform_speak_command(text: &str) -> Option<SpeechCommand> {
    #[cfg(target_os = "macos")]
    {
        Some(macos_speak_command(text))
    }
    #[cfg(target_os = "windows")]
    {
        Some(windows_speak_command(text))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = text;
        None
    }
}

/// 読み上げの契約（`Clipboard` と同じくスレッド間で共有されうるため `Send + Sync`）。
pub trait Tts: Send + Sync {
    /// テキストを発話する。呼び出しは発話開始で戻り、読み終わりまでは待たない。
    /// 直前の発話が続いていれば中断してから始める。
    /// 空文字列・空白のみは発話するものが無いため OS を呼ばずに戻る。
    /// NUL（`\0`）を含むテキストは `Err(DeviceError::InvalidInput)`。
    ///
    /// 戻り値はこの呼び出しを識別する世代 id（単調増加。空発話・猶予内に正常終了
    /// した場合も払い出してよい）。[`Self::wait_outcome`] へそのまま渡す契約
    /// （codex-review P1 指摘の是正: 発話 A の待機スレッドが、後発の発話 B の
    /// 子プロセスを誤って監視し、B の失敗を A の結果として偽装して返す競合が
    /// あった。世代 id で「どの発話を待つか」を明示することで解消する。
    /// `CommandTts::wait_outcome` doc 参照）。
    fn speak(&self, text: &str) -> Result<u64, DeviceError>;

    /// 発話中なら止める。発話していなければ何もせず `Ok(())`。
    fn stop(&self) -> Result<(), DeviceError>;

    /// `speak` が返した世代 `generation` に対応する子プロセスの最終結果を
    /// ブロッキングで待つ。
    ///
    /// `speak` は起動直後の短い猶予（`CommandTts` の `STARTUP_FAILURE_GRACE`）しか
    /// 待たないため、その猶予を過ぎてから失敗する子プロセス（例: 音声デバイス
    /// 初期化が遅延して失敗する等）は `speak` の戻り値だけでは検知できない
    /// （codex-review P1 指摘: 「200ms 経過だけで発話開始を成功扱いする」）。
    /// 呼び出し元（`fandhe-egui-proxy` 側 effect ドライバ）は UI スレッドを
    /// 塞がない別スレッドから `speak` 直後に `speak` が返した世代で本メソッドを呼び、
    /// 真の結果へ差し替える契約（ブロッキングだが `stop()` は別途 `speak`/`stop` 用の
    /// ロックを介して並行に呼べるため、待機中でも中断できる）。
    /// **他の世代の結果を絶対に回収しない**: `generation` に対応する子プロセスが
    /// 既に無い（未起動・`speak` の猶予内に完了済み・`stop` 済み・別の発話へ
    /// 置き換わり済み）場合は、古い待機として即座に `Ok(())` を返す
    /// （デフォルト実装。子プロセスを持たないモック実装は上書き不要）。
    fn wait_outcome(&self, generation: u64) -> Result<(), DeviceError> {
        let _ = generation;
        Ok(())
    }
}

/// macOS/Windows 向け実装。OS 同梱コマンドを子プロセスとして起動する
/// （モジュール doc 参照）。
///
/// `child` は起動中の子プロセスに世代 id（[`Tts::speak`] の戻り値）を紐付けて
/// 保持する。世代 id が無いと、発話 A の待機（[`Tts::wait_outcome`]）が
/// sleep 中に発話 B が始まった場合、A の待機が B の子プロセスを誤って監視し
/// 「B の失敗を A の結果」として返してしまう（codex-review P1 指摘・
/// `wait_outcome` doc 参照）。
pub struct CommandTts {
    program: PathBuf,
    child: Mutex<Option<(u64, Child)>>,
    next_generation: AtomicU64,
}

impl CommandTts {
    /// 既定のプログラム（`say` / PowerShell）で構築する。存在しなければ
    /// `Err(DeviceError::Unsupported("tts"))`（ダミーへは絶対にフォールバックしない）。
    pub fn new() -> Result<Self, DeviceError> {
        let Some(command) = platform_speak_command("") else {
            return Err(DeviceError::Unsupported("tts"));
        };
        Self::with_program(command.program)
    }

    /// 任意のプログラムパスで構築する（テストから存在しないパスを注入し、
    /// 初期化失敗の経路を再現するための入口）。
    fn with_program(program: PathBuf) -> Result<Self, DeviceError> {
        if !program.is_file() {
            return Err(DeviceError::Unsupported("tts"));
        }
        Ok(Self {
            program,
            child: Mutex::new(None),
            next_generation: AtomicU64::new(1),
        })
    }
}

impl CommandTts {
    /// 子プロセスを止めて回収する（呼び出し元が `child` のロックを保持したまま呼ぶ内部処理。
    /// `stop()` と [`run_command`](Self::run_command) の両方から使い、
    /// 停止処理を 1 箇所にまとめる）。
    fn stop_locked(guard: &mut Option<(u64, Child)>) {
        if let Some((_, mut child)) = guard.take() {
            // 既に終了済みなら `kill` は ESRCH 相当で失敗しうるが、目的（発話が
            // 止まっていること）は既に満たされているため無視してよい。
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// 停止 → 起動 → ハンドル登録を実行する（OS 判定を経ない実処理本体）。
    ///
    /// `child` のロックを一連の操作全体で保持し続けることで、2 スレッドが
    /// 同時に `speak` を呼んだときに片方の `Child` が上書き破棄されて発話が
    /// 重なる・`stop` で止められなくなる競合を防ぐ（P1 指摘）。
    /// OS 判定（`platform_speak_command`）を経ないため、テストから実在する
    /// 任意のプログラム（`/bin/sh` 等）を注入して起動直後の失敗検知を検証できる。
    ///
    /// 戻り値はこの呼び出しの世代 id（`next_generation` から単調増加で払い出す）。
    /// 呼び出し元は `wait_outcome` へこの id をそのまま渡す（`Tts::speak` doc 参照）。
    fn run_command(&self, text: &str, command: SpeechCommand) -> Result<u64, DeviceError> {
        let mut guard = self.child.lock().unwrap_or_else(|p| p.into_inner());
        Self::stop_locked(&mut guard);
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);

        let mut cmd = Command::new(&self.program);
        cmd.args(&command.args);
        #[cfg(windows)]
        {
            // GUI ホストで発話のたびにコンソールウィンドウが一瞬表示される
            // 不具合を防ぐ（フォーカス奪取・ウィンドウを閉じると発話が止まる懸念、
            // Bugbot 指摘）。CREATE_NO_WINDOW = 0x08000000。
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        match command.text_via {
            TextChannel::Stdin => {
                cmd.stdin(Stdio::piped());
            }
            TextChannel::Env(name) => {
                cmd.env(name, text);
            }
        }

        let mut spawned = cmd
            .spawn()
            .map_err(|_| DeviceError::Internal("tts speak"))?;

        if matches!(command.text_via, TextChannel::Stdin) {
            use std::io::Write;
            let Some(mut stdin) = spawned.stdin.take() else {
                // stdin 消失時も spawn 済みの子プロセスを追跡不能なまま放置しない
                // （kill+wait で確実に回収する。Child は drop 時に自動 kill しないため）。
                let _ = spawned.kill();
                let _ = spawned.wait();
                return Err(DeviceError::Internal("tts speak"));
            };
            // 書き込み結果はブロックを抜けた後で見る。`stdin` をここで明示 drop
            // すると wasm32 の `ChildStdin`（`Drop` を持たないスタブ型）では
            // clippy::drop_non_drop が発火するため、ブロック終端の暗黙 drop に
            // 任せて EOF を送る（`say` に読み終わらせる。ネイティブでの挙動は不変）。
            let write_result = stdin.write_all(text.as_bytes());
            if write_result.is_err() {
                let _ = spawned.kill();
                let _ = spawned.wait();
                return Err(DeviceError::Internal("tts speak"));
            }
        }

        // 起動直後の即時失敗（アセンブリ未ロード・実行不可等）を短い猶予内で検知する。
        // 読み終わりまでは待たないため、猶予後もまだ動いていれば起動成功として扱い
        // ハンドルを登録する（トレイト契約: 発話開始で戻る）。
        std::thread::sleep(STARTUP_FAILURE_GRACE);
        match spawned.try_wait() {
            Ok(Some(status)) if !status.success() => Err(DeviceError::Internal("tts speak")),
            Ok(Some(_)) => {
                // 猶予内に正常終了済み（ごく短い発話）。登録するハンドルは無いが、
                // 世代 id は払い出す（`wait_outcome` はこの id に対し即 `Ok(())`）。
                Ok(generation)
            }
            Ok(None) | Err(_) => {
                // まだ実行中、または状態不明（安全側でハンドルを登録し、
                // stop() で回収できるようにする）。
                *guard = Some((generation, spawned));
                Ok(generation)
            }
        }
    }
}

impl Tts for CommandTts {
    fn speak(&self, text: &str) -> Result<u64, DeviceError> {
        if text.contains('\0') {
            return Err(DeviceError::InvalidInput("tts text"));
        }
        if text.trim().is_empty() {
            // 空発話でも世代 id を払い出す（トレイト契約: この id に対する
            // `wait_outcome` は即 `Ok(())`）。
            return Ok(self.next_generation.fetch_add(1, Ordering::SeqCst));
        }

        let Some(command) = platform_speak_command(text) else {
            return Err(DeviceError::Unsupported("tts"));
        };

        self.run_command(text, command)
    }

    fn stop(&self) -> Result<(), DeviceError> {
        let mut guard = self.child.lock().unwrap_or_else(|p| p.into_inner());
        Self::stop_locked(&mut guard);
        Ok(())
    }

    /// `self.child` を短時間ずつロックしてポーリングし、`stop()` を待たせない
    /// （トレイト doc「`speak` が返した世代に対応する子プロセスの最終結果を待つ」
    /// 参照）。ループの合間は必ずロックを手放してから眠るため、待機中に
    /// `stop()` が割り込めば次の周回で `guard` が `None` になり `Ok(())` で戻る
    /// （ユーザー操作による停止は失敗として扱わない）。
    ///
    /// 登録されている世代 `generation` と一致する間だけ待つ。登録が無い・
    /// 世代が不一致（既に別の発話に置き換わった／stop 済み）なら古い待機として
    /// 即座に `Ok(())` で抜け、**他の世代の結果を絶対に回収しない**（codex-review
    /// P1 指摘の是正: 発話 A の待機が sleep 中に発話 B の子プロセスへ差し替わると、
    /// 世代を見ない旧実装は B の失敗を A の結果として誤って返していた）。
    fn wait_outcome(&self, generation: u64) -> Result<(), DeviceError> {
        loop {
            {
                let mut guard = self.child.lock().unwrap_or_else(|p| p.into_inner());
                match guard.as_mut() {
                    None => return Ok(()),
                    Some((registered_generation, _)) if *registered_generation != generation => {
                        // 既に別の発話へ置き換わっている＝この世代の子プロセスは
                        // もう存在しない。古い待機として扱う。
                        return Ok(());
                    }
                    Some((_, child)) => match child.try_wait() {
                        Ok(Some(status)) => {
                            *guard = None;
                            return if status.success() {
                                Ok(())
                            } else {
                                Err(DeviceError::Internal("tts speak"))
                            };
                        }
                        Ok(None) => {
                            // まだ実行中。ロックを手放してから待つ（下記）。
                        }
                        Err(_) => {
                            *guard = None;
                            return Err(DeviceError::Internal("tts speak"));
                        }
                    },
                }
            }
            std::thread::sleep(WAIT_OUTCOME_POLL_INTERVAL);
        }
    }
}

impl Drop for CommandTts {
    /// ハンドル破棄時に子プロセスを回収する。`Drop` が無いとアプリ終了・
    /// 差し替え時に `say`/PowerShell の発話が止まらず、終了済み子プロセスも
    /// 回収されない（Bugbot 指摘）。
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child.lock() {
            Self::stop_locked(&mut guard);
        }
        // Mutex が poisoned なら他スレッドが panic 済み（プロセス自体が異常終了に
        // 向かっている）ため、ここでの追加回収は諦めてよい。
    }
}

/// 与えたプログラムパスが実在しなくても構築だけを試す薄いラッパー
/// （テスト専用。`CommandTts::with_program` は非公開のため）。
#[cfg(test)]
fn command_tts_at(program: &std::path::Path) -> Result<CommandTts, DeviceError> {
    CommandTts::with_program(program.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// `Tts` 契約側のエラー経路（`Unsupported`）を確認する最小モック。
    struct UnsupportedTts;

    impl Tts for UnsupportedTts {
        fn speak(&self, _text: &str) -> Result<u64, DeviceError> {
            Err(DeviceError::Unsupported("tts"))
        }
        fn stop(&self) -> Result<(), DeviceError> {
            Err(DeviceError::Unsupported("tts"))
        }
    }

    #[test]
    fn unsupported_platform_returns_err() {
        let tts = UnsupportedTts;
        assert!(matches!(tts.speak("x"), Err(DeviceError::Unsupported(_))));
        assert!(matches!(tts.stop(), Err(DeviceError::Unsupported(_))));
    }

    #[test]
    fn missing_program_fails_at_construction() {
        // ダミーへ黙って落ちないことの機械的な裏付け（#859 の P1 指摘の再発防止）。
        let result = command_tts_at(Path::new("/nonexistent/fandhe-tts"));
        assert!(matches!(result, Err(DeviceError::Unsupported("tts"))));
    }

    #[test]
    fn blank_text_is_noop_without_spawning() {
        // 存在しないプログラムで生成した実体でも speak("   ") は Ok
        // （= OS を呼んでいないことの証拠。呼んでいれば spawn 失敗で Err になるはず）。
        // このテストは構築自体を素通りさせるため、存在するダミーバイナリではなく
        // 構築を直接組み立てる。
        let tts = CommandTts {
            program: PathBuf::from("/nonexistent/fandhe-tts"),
            child: Mutex::new(None),
            next_generation: AtomicU64::new(1),
        };
        assert!(tts.speak("   ").is_ok());
    }

    #[test]
    fn text_with_nul_is_rejected() {
        let tts = CommandTts {
            program: PathBuf::from("/nonexistent/fandhe-tts"),
            child: Mutex::new(None),
            next_generation: AtomicU64::new(1),
        };
        assert!(matches!(
            tts.speak("a\0b"),
            Err(DeviceError::InvalidInput(_))
        ));
    }

    #[test]
    fn stop_without_speaking_is_ok() {
        let tts = CommandTts {
            program: PathBuf::from("/nonexistent/fandhe-tts"),
            child: Mutex::new(None),
            next_generation: AtomicU64::new(1),
        };
        assert!(tts.stop().is_ok());
    }

    #[test]
    fn windows_command_passes_text_via_env_not_script() {
        let command = windows_speak_command("秘密のテキスト");
        assert!(
            command.args.iter().all(|a| !a.contains("秘密のテキスト")),
            "テキストが引数へ連結されている: {:?}",
            command.args
        );
        assert!(
            command
                .args
                .iter()
                .any(|a| a.contains(&format!("$env:{WINDOWS_TTS_TEXT_ENV}"))),
            "スクリプトが環境変数を参照していない: {:?}",
            command.args
        );
        assert!(matches!(command.text_via, TextChannel::Env(name) if name == WINDOWS_TTS_TEXT_ENV));
    }

    #[test]
    fn macos_command_uses_absolute_say_path_and_stdin() {
        let command = macos_speak_command("hello");
        assert_eq!(command.program, PathBuf::from("/usr/bin/say"));
        assert!(command.program.is_absolute());
        assert!(matches!(command.text_via, TextChannel::Stdin));
    }

    /// 実際に `say` を起動して発話できることを確認する手動テスト（macOS 限定）。
    /// `#[ignore]` を付けて既定の `cargo test` からは外す（音が出る副作用があるため）。
    /// 実行は `cargo test -p fandhe-device --lib -- --ignored --nocapture`。
    #[test]
    #[ignore = "実際に音が出るため手動実行（--ignored）"]
    #[cfg(target_os = "macos")]
    fn speaks_through_os_synthesizer() {
        let tts = CommandTts::new().expect("macOS では /usr/bin/say が存在する");
        assert!(tts.speak("Fandhe のテスト発話です").is_ok());
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(tts.stop().is_ok());
    }

    /// `run_command` は OS 判定（`platform_speak_command`）を経ないため、
    /// `/bin/sh` を注入して起動直後の失敗検知・排他制御を実プロセスで検証できる
    /// （CI の Linux runner でも動く。Windows には `/bin/sh` が無いため
    /// `cfg(unix)` に限定する）。
    #[cfg(unix)]
    fn shell_tts() -> CommandTts {
        CommandTts {
            program: PathBuf::from("/bin/sh"),
            child: Mutex::new(None),
            next_generation: AtomicU64::new(1),
        }
    }

    /// stdin 経路で `/bin/sh` にスクリプトを渡す `SpeechCommand`
    /// （`macos_speak_command` と同型: 引数なし・テキストは stdin）。
    #[cfg(unix)]
    fn shell_command() -> SpeechCommand {
        SpeechCommand {
            program: PathBuf::new(),
            args: Vec::new(),
            text_via: TextChannel::Stdin,
        }
    }

    #[test]
    #[cfg(unix)]
    fn run_command_reports_immediate_failure_within_grace_window() {
        // P1 指摘の再発防止: 起動直後に失敗する子プロセスを spawn 成功のまま
        // Ok(()) にしないこと。
        let tts = shell_tts();
        let result = tts.run_command("exit 7", shell_command());
        assert!(
            matches!(result, Err(DeviceError::Internal(_))),
            "{result:?}"
        );
        // 失敗した子はハンドルとして残らない。
        assert!(tts.child.lock().unwrap().is_none());
    }

    #[test]
    #[cfg(unix)]
    fn run_command_registers_still_running_child() {
        let tts = shell_tts();
        // 猶予（200ms）より長く生き続けるので Ok(()) かつハンドル登録される。
        let result = tts.run_command("sleep 2", shell_command());
        assert!(result.is_ok(), "{result:?}");
        assert!(tts.child.lock().unwrap().is_some());
        tts.stop().expect("stop は常に Ok");
        assert!(tts.child.lock().unwrap().is_none());
    }

    #[test]
    #[cfg(unix)]
    fn run_command_serializes_concurrent_speak_calls() {
        // 2 スレッド同時 speak で先勝ちの Child が上書き破棄されない
        // （= 常にどちらか一方の Child だけが生き残り、stop で確実に止められる）
        // ことを確認する（P1 指摘: 排他制御の欠如）。
        use std::sync::Arc;
        let tts = Arc::new(shell_tts());
        let t1 = {
            let tts = Arc::clone(&tts);
            std::thread::spawn(move || tts.run_command("sleep 2", shell_command()))
        };
        let t2 = {
            let tts = Arc::clone(&tts);
            std::thread::spawn(move || tts.run_command("sleep 2", shell_command()))
        };
        assert!(t1.join().unwrap().is_ok());
        assert!(t2.join().unwrap().is_ok());
        // 排他区間により後発の speak が先発を確実に止めてから起動するため、
        // 生き残っているハンドルはちょうど 1 個。
        assert!(tts.child.lock().unwrap().is_some());
        tts.stop().expect("stop は常に Ok");
    }

    #[test]
    #[cfg(unix)]
    fn drop_kills_still_running_child() {
        // Bugbot 指摘の再発防止: Drop 未実装だとハンドル破棄時に子プロセスが
        // 回収されない。生存確認は `Child::id()` を kill 前後で捕まえて
        // シグナル送出できることで代える（ゾンビ化しても id は再利用されない
        // 前提の軽量チェック）。
        use std::process::Command;
        let tts = shell_tts();
        let result = tts.run_command("sleep 5", shell_command());
        assert!(result.is_ok(), "{result:?}");
        let pid = {
            let guard = tts.child.lock().unwrap();
            guard.as_ref().expect("実行中のはず").1.id()
        };
        drop(tts);
        // kill 済みプロセスへ再度 `kill -0` すると失敗する（存在しない/権限なし）。
        let still_alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(!still_alive, "Drop 後もプロセスが生存している: pid={pid}");
    }

    #[test]
    #[cfg(unix)]
    fn wait_outcome_reports_late_failure_after_grace_window() {
        // codex-review P1 指摘の再発防止: 猶予（200ms）内はまだ実行中のため
        // `run_command` は Ok(()) を返すが、その後（猶予を過ぎてから）失敗する
        // 子プロセスを `wait_outcome` が拾うことの固定
        // （「sleep 1; exit 7」のような遅延失敗、codex-review 指摘の repro と同型）。
        let tts = shell_tts();
        let generation = tts
            .run_command("sleep 0.3; exit 7", shell_command())
            .expect("猶予内はまだ実行中のはず");
        assert!(tts.child.lock().unwrap().is_some());

        let outcome = tts.wait_outcome(generation);
        assert!(
            matches!(outcome, Err(DeviceError::Internal(_))),
            "{outcome:?}"
        );
        assert!(tts.child.lock().unwrap().is_none());
    }

    #[test]
    #[cfg(unix)]
    fn wait_outcome_reports_success_after_grace_window() {
        let tts = shell_tts();
        let generation = tts
            .run_command("sleep 0.3", shell_command())
            .expect("起動できる");

        assert!(tts.wait_outcome(generation).is_ok());
        assert!(tts.child.lock().unwrap().is_none());
    }

    #[test]
    #[cfg(unix)]
    fn wait_outcome_is_ok_without_registered_child() {
        // 未起動・`speak` の猶予内に完了済み等でハンドルが無い場合は即座に成功。
        let tts = shell_tts();
        assert!(tts.wait_outcome(1).is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn wait_outcome_ignores_result_of_a_later_speak_with_different_generation() {
        // codex-review P1 指摘の再発防止（本 PR の核心）: 発話 A の待機スレッドが
        // sleep 中に発話 B が始まると、世代を見ない旧実装は A の待機が B の子プロセスを
        // 誤って監視し、B の失敗を A の結果として返してしまう（画面が「読み上げました」
        // と嘘の成功を出す）。世代 id で区別すれば、A 側は古い待機として `Ok(())` で
        // 抜け、B 側だけが本当の失敗を受け取ることを固定する。
        //
        // A のハンドルがまだ登録されている間に A の `wait_outcome` を別スレッドで
        // 実行中の状態を作り、その待機中（ポーリングの合間）に B を起動して
        // 子プロセスを差し替える。これにより「A の待機がまだ生きている最中に
        // 監視対象が B へすり替わる」という報告どおりの競合を再現する。
        use std::sync::Arc;
        let tts = Arc::new(shell_tts());
        let generation_a = tts
            .run_command("sleep 5", shell_command())
            .expect("A は起動できる");

        let waiter_a = {
            let tts = Arc::clone(&tts);
            std::thread::spawn(move || tts.wait_outcome(generation_a))
        };
        // `wait_outcome` のポーリングが 1 巡してロックを手放す猶予を与える
        // （A の待機がまだ sleep 中の状態を作る）。
        std::thread::sleep(WAIT_OUTCOME_POLL_INTERVAL / 2);

        // A の待機が生きている間に B を開始する（内部で A の子プロセスを kill し、
        // ハンドルを B のものへ差し替える）。
        let generation_b = tts
            .run_command("sleep 0.3; exit 7", shell_command())
            .expect("B は起動できる");
        assert_ne!(generation_a, generation_b);

        // A の世代の待機は B の失敗を回収せず、古い待機として Ok(()) で抜ける。
        assert!(
            waiter_a.join().unwrap().is_ok(),
            "A は自分より後発の B の結果を回収してはいけない"
        );

        // B の世代の待機は B 自身の失敗を正しく受け取る。
        assert!(matches!(
            tts.wait_outcome(generation_b),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    #[cfg(unix)]
    fn stop_interrupts_wait_outcome_instead_of_waiting_for_it() {
        // `wait_outcome` がロックを持ったまま待つ実装だと、`stop()` が発話終了まで
        // ブロックされて中断できなくなる（P1 指摘の別の再発パターン）。`stop()` が
        // すぐ返ることを確認する。
        use std::sync::Arc;
        let tts = Arc::new(shell_tts());
        let generation = tts
            .run_command("sleep 5", shell_command())
            .expect("起動できる");

        let waiter = {
            let tts = Arc::clone(&tts);
            std::thread::spawn(move || tts.wait_outcome(generation))
        };
        // `wait_outcome` のポーリングが 1 巡してロックを手放す猶予を与える。
        std::thread::sleep(Duration::from_millis(50));

        let started = std::time::Instant::now();
        tts.stop().expect("stop は常に Ok");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "stop が wait_outcome の完了を待ってしまった: {:?}",
            started.elapsed()
        );

        // ユーザー操作による停止は失敗として扱わない。
        assert!(
            waiter.join().unwrap().is_ok(),
            "stop 済みは wait_outcome 側も成功扱いになる"
        );
    }
}
