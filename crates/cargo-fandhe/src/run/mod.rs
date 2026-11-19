//! `cargo fandhe run|build`: アプリを各 OS 向けにビルド・起動する（REQ-21 / TASK-21.5、Issue #1422・親 #1201）。
//!
//! 構成は 3 層に分ける。
//! - 純粋関数 [`prepare`]: アプリ情報（[`App`]）・オプション・環境のスナップショット（[`Env`]）から、
//!   前提の不足を全件集めて返すか、実行する手順（[`Op`] の列）を返す。ファイルにも環境変数にも触れない
//! - 入力の取得 [`load_app`]・[`detect_env`]: `cargo metadata` と実環境（PATH・rustup・wasm-bindgen の版）から入力を作る
//! - 実行器 [`execute`]: 手順を順に実行するだけ。判断は持たない
//!
//! デスクトップ（macOS・Windows・Linux）は cargo への委譲、Web は `web` モジュール。Android・iOS は後続 PR で
//! `Os` と `match os` の腕を足す（漏れはコンパイルで分かる）。`scripts/` は gallery 専用で利用者に配られないため呼ばない。
//! 終了コードは `main.rs` が決める（失敗は 1）。エラー文に環境変数の値は出さない（security.md）。
//! 設計は `docs/architecture/run-build-cli.md`。

mod serve;
mod web;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::native::{app_root, generate_web, AppInfo, WEB_MANIFEST_PATH, WEB_TRIPLE};
use crate::workspace::run_metadata;

/// 対象 OS。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Macos,
    Windows,
    Linux,
    Web,
}

impl Os {
    /// `--os` の値。
    pub fn name(self) -> &'static str {
        match self {
            Os::Macos => "macos",
            Os::Windows => "windows",
            Os::Linux => "linux",
            Os::Web => "web",
        }
    }

    fn parse(s: &str) -> Option<Os> {
        [Os::Macos, Os::Windows, Os::Linux, Os::Web]
            .into_iter()
            .find(|o| o.name() == s)
    }

    /// このプロセスが動いているデスクトップ OS。対応外（BSD 等）は `None`。
    pub fn host() -> Option<Os> {
        match std::env::consts::OS {
            "macos" => Some(Os::Macos),
            "windows" => Some(Os::Windows),
            "linux" => Some(Os::Linux),
            _ => None,
        }
    }

    /// `cargo metadata --filter-platform` に渡す triple。デスクトップはホスト向けなので絞らない。
    fn triple(self) -> Option<&'static str> {
        match self {
            Os::Web => Some(WEB_TRIPLE),
            Os::Macos | Os::Windows | Os::Linux => None,
        }
    }
}

/// サブコマンド。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Run,
    Build,
}

/// `run` / `build` のオプション。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// `run`（ビルドして起動）か `build`。
    pub action: Action,
    /// 省略はホストのデスクトップ OS。
    pub os: Option<Os>,
    /// `--release`。
    pub release: bool,
    /// `--manifest-path`。省略はカレントのパッケージ。
    pub manifest: Option<PathBuf>,
}

/// `args` は `run` / `build` から始まる引数列。解釈できなければ `None`（使い方を出す側が終了コード 2 にする）。
/// 同じオプションの重複、`-` で始まる `--manifest-path` の値、未知のオプションは拒否する。
pub fn parse_args(args: &[&str]) -> Option<Options> {
    let (action, rest) = match args.split_first()? {
        (&"run", r) => (Action::Run, r),
        (&"build", r) => (Action::Build, r),
        _ => return None,
    };
    let mut o = Options {
        action,
        os: None,
        release: false,
        manifest: None,
    };
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--release" if !o.release => o.release = true,
            "--os" if o.os.is_none() => o.os = Some(Os::parse(it.next()?)?),
            "--manifest-path" if o.manifest.is_none() => {
                let v = it.next()?;
                if v.starts_with('-') {
                    return None;
                }
                o.manifest = Some(PathBuf::from(v));
            }
            _ => return None,
        }
    }
    Some(o)
}

/// アプリ（`cargo metadata` の root package）から、手順の組み立てに要る値だけを抜いたもの。
#[derive(Clone, Debug)]
pub struct App {
    /// package 名。成果物の置き場 `<target_dir>/fandhe/<name>/<os>/` に使う。
    pub name: String,
    /// アプリの `Cargo.toml`（絶対パス）。cargo へ `--manifest-path` で渡す。
    pub manifest: PathBuf,
    /// アプリの crate のディレクトリ。
    pub dir: PathBuf,
    /// `cargo metadata` の `target_directory`。成果物の置き場の根。
    pub target_dir: PathBuf,
    /// 実行体（bin target）の名前。
    pub bins: Vec<String>,
    /// `cdylib` を含む lib の名前（`_` 区切り）。Web の wasm の元。
    pub cdylib: Option<String>,
    /// アプリの依存解決（Cargo.lock）にある `wasm-bindgen` の版（Web のときだけ調べる）。
    pub wasm_bindgen: Option<String>,
    /// アプリの `web/` ディレクトリ（あれば。Web のときだけ調べる）。成果物へ丸ごと写す。
    pub web_dir: Option<PathBuf>,
    /// `web_dir` に `index.html` があるか。無ければ既定の `index.html` を出す。
    pub web_has_index: bool,
    /// 配信する CJK フォント（`fandhe-fonts` の資産）。解決できなければ `None`（既定フォントで続行）。
    pub font: Option<PathBuf>,
    /// `cargo fandhe gen --os web` 相当の manifest の中身。`fandhe-app.web` があるアプリだけ `Some`。
    pub web_manifest: Option<String>,
}

/// 実行環境のスナップショット。判定を純粋にするため、実環境の観測は [`detect_env`] だけが行う。
#[derive(Clone, Debug)]
pub struct Env {
    /// ホストのデスクトップ OS。対応外は `None`。
    pub host: Option<Os>,
    /// PATH 上にあるコマンド名（`rustup`・`wasm-bindgen`・`wasm-opt`）。
    pub commands: BTreeSet<String>,
    /// `rustup target list --installed`。rustup が無い・取得できないときは `None`（検査を飛ばす）。
    pub installed_targets: Option<BTreeSet<String>>,
    /// `wasm-bindgen --version` の版。取得できなければ `None`。
    pub wasm_bindgen_version: Option<String>,
    /// 子プロセスとして起動する cargo（環境変数 `CARGO` か `cargo`）。値はエラー文に出さない。
    pub cargo: OsString,
}

/// 実行する 1 コマンド。`stage` は失敗時に利用者へ出す段の名前。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub stage: &'static str,
    pub program: OsString,
    pub args: Vec<String>,
}

/// 手順の 1 要素。実行器は上から順に処理する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Spawn(Step),
    /// `dir` を消して作り直す。`dir` が `within` の配下で、`within` から `dir` までに symlink が無いことを確かめてから消す。
    Reset {
        dir: PathBuf,
        within: PathBuf,
    },
    /// ファイルを書く（親ディレクトリは作る）。
    Write {
        path: PathBuf,
        body: String,
    },
    /// ファイルを写す（親ディレクトリは作る）。
    Copy {
        from: PathBuf,
        to: PathBuf,
    },
    /// ディレクトリを丸ごと写す。symlink は写さない。
    CopyDir {
        from: PathBuf,
        to: PathBuf,
    },
    /// 利用者への案内（標準出力）。
    Say(String),
    /// `dir` を 127.0.0.1 の空きポートで配る。URL を出し、プロセスが終わるまで戻らない。
    Serve {
        dir: PathBuf,
    },
}

/// 対象 OS を決める。省略はホストのデスクトップ OS。決める場所はここだけ（`run_cli` と `prepare` が使う）。
fn resolve_os(opts: &Options, host: Option<Os>) -> Result<Os, Vec<String>> {
    opts.os.or(host).ok_or_else(|| {
        vec!["ホスト OS がデスクトップ（macos・windows・linux）ではありません。--os を指定してください".to_string()]
    })
}

/// 成果物の置き場 `<target_dir>/fandhe/<package 名>/<os>/`。
fn out_dir(app: &App, os: Os) -> PathBuf {
    app.target_dir
        .join("fandhe")
        .join(&app.name)
        .join(os.name())
}

/// 前提を全件検査し、問題がなければ手順を返す。問題は 1 件 1 行「何が無いか。直し方」。
pub fn prepare(app: &App, opts: &Options, env: &Env) -> Result<Vec<Op>, Vec<String>> {
    let os = resolve_os(opts, env.host)?;
    match os {
        Os::Web => web::prepare(app, opts, env, &out_dir(app, os)),
        Os::Macos | Os::Windows | Os::Linux => desktop_prepare(os, app, opts, env),
    }
}

fn desktop_prepare(os: Os, app: &App, opts: &Options, env: &Env) -> Result<Vec<Op>, Vec<String>> {
    let mut problems = Vec::new();
    if env.host != Some(os) {
        let host = env.host.map_or("対応外", Os::name);
        problems.push(format!(
            "--os {} はホスト（{host}）と違うため、クロスビルドの実行はできません。{} 上で実行してください",
            os.name(),
            os.name()
        ));
    }
    if app.bins.is_empty() {
        problems.push(
            "実行体（bin）がありません。src/main.rs か `[[bin]]` を足してください".to_string(),
        );
    }
    if problems.is_empty() {
        Ok(vec![Op::Spawn(cargo_step(app, opts, env, &[]))])
    } else {
        Err(problems)
    }
}

/// `cargo <build|run> [extra] [--release] --manifest-path <app>`。
fn cargo_step(app: &App, opts: &Options, env: &Env, extra: &[&str]) -> Step {
    let sub = match opts.action {
        Action::Run => "run",
        Action::Build => "build",
    };
    let mut args = vec![sub.to_string()];
    args.extend(extra.iter().map(|s| s.to_string()));
    if opts.release {
        args.push("--release".into());
    }
    args.push("--manifest-path".into());
    args.push(app.manifest.display().to_string());
    Step {
        stage: "cargo",
        program: env.cargo.clone(),
        args,
    }
}

/// `cargo metadata` からアプリの情報を作る。Web は wasm32 向けに依存を解決し、`web/`・フォント・manifest も調べる。
pub fn load_app(opts: &Options, os: Os) -> Result<App, Vec<String>> {
    let meta = run_metadata(opts.manifest.as_deref(), os.triple())?;
    let (dir, cfg, pkg) = app_root(&meta)?;
    let info = AppInfo::from_package(&pkg, dir.join("gen"));
    let manifest = pkg["manifest_path"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| vec!["manifest_path を読めません".to_string()])?;
    let target_dir = meta["target_directory"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| vec!["target_directory を読めません".to_string()])?;
    let package = |name: &str| {
        meta["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["name"].as_str() == Some(name))
    };
    let mut app = App {
        name: info.name,
        manifest,
        dir,
        target_dir,
        bins: info.bins,
        cdylib: info.cdylib,
        wasm_bindgen: None,
        web_dir: None,
        web_has_index: false,
        font: None,
        web_manifest: None,
    };
    match os {
        Os::Web => {
            app.wasm_bindgen =
                package("wasm-bindgen").and_then(|p| p["version"].as_str().map(String::from));
            // `fandhe-fonts` は描画層の wasm32 以外の依存なので、wasm32 に絞った解決には現れない。
            // 資産の置き場だけを知りたいため、絞らない解決から探す。
            let all = run_metadata(opts.manifest.as_deref(), None)?;
            app.font = all["packages"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["name"].as_str() == Some("fandhe-fonts"))
                .and_then(|p| p["manifest_path"].as_str())
                .and_then(|m| Path::new(m).parent())
                .map(|d| d.join("assets/NotoSansJP-Regular.otf"))
                .filter(|f| f.is_file());
            // symlink の web/ は読み取り元の外を指しうるため、無いものとして扱う（`copy_dir` も入口で拒否する）。
            app.web_dir = Some(app.dir.join("web")).filter(|d| is_real_dir(d));
            // symlink の index.html は `copy_dir` が写さないため、無いものとして既定ページへ倒す。
            // 採らなかった web/（symlink）の中は見ない（親の symlink を辿って true にしない）。
            app.web_has_index = app.web_dir.as_deref().is_some_and(|d| {
                std::fs::symlink_metadata(d.join("index.html")).is_ok_and(|m| m.is_file())
            });
            // `fandhe-app.web` がテーブル以外ならここで gen と同じエラーで止まる（基礎 manifest の読み込みも同じ関数）。
            if !cfg["web"].is_null() {
                app.web_manifest = generate_web(opts.manifest.as_deref())?
                    .files
                    .remove(WEB_MANIFEST_PATH);
            }
        }
        Os::Macos | Os::Windows | Os::Linux => {}
    }
    Ok(app)
}

/// `wasm-bindgen --version` の出力（`wasm-bindgen 0.2.126`）から版を取り出す。
pub fn parse_wasm_bindgen_version(out: &str) -> Option<String> {
    let mut w = out.split_whitespace();
    (w.next()? == "wasm-bindgen").then(|| w.next().map(String::from))?
}

/// 実環境を観測して [`Env`] にする。Web 以外では rustup・wasm-bindgen の中身を調べない。
pub fn detect_env(os: Os) -> Env {
    let commands: BTreeSet<String> = ["rustup", "wasm-bindgen", "wasm-opt"]
        .into_iter()
        .filter(|c| on_path(c))
        .map(String::from)
        .collect();
    let web = match os {
        Os::Web => true,
        Os::Macos | Os::Windows | Os::Linux => false,
    };
    let installed_targets = (web && commands.contains("rustup"))
        .then(|| output_of("rustup", &["target", "list", "--installed"]))
        .flatten()
        .map(|t| t.lines().map(|l| l.trim().to_string()).collect());
    let wasm_bindgen_version = (web && commands.contains("wasm-bindgen"))
        .then(|| output_of("wasm-bindgen", &["--version"]))
        .flatten()
        .and_then(|t| parse_wasm_bindgen_version(&t));
    Env {
        host: Os::host(),
        commands,
        installed_targets,
        wasm_bindgen_version,
        cargo: std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
    }
}

fn on_path(cmd: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths)
        .any(|d| d.join(cmd).is_file() || d.join(format!("{cmd}.exe")).is_file())
}

fn output_of(cmd: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(cmd).args(args).output().ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// 手順を順に実行する。子プロセスの出力はそのまま流し、失敗した段を 1 行のエラーで返す。
/// 段の名前しか出さない（コマンド行・環境変数の値は出さない）。
pub fn execute(ops: &[Op]) -> Result<(), String> {
    for op in ops {
        match op {
            Op::Say(m) => println!("{m}"),
            Op::Reset { dir, within } => reset_dir(dir, within)?,
            Op::Write { path, body } => {
                make_parent(path)?;
                std::fs::write(path, body)
                    .map_err(|e| format!("{} を書けません: {e}", path.display()))?;
            }
            Op::Copy { from, to } => {
                make_parent(to)?;
                std::fs::copy(from, to).map_err(|e| {
                    format!("{} を {} へ写せません: {e}", from.display(), to.display())
                })?;
            }
            Op::CopyDir { from, to } => copy_dir(from, to)?,
            Op::Serve { dir } => serve::serve(dir)?,
            Op::Spawn(s) => {
                let status = Command::new(&s.program)
                    .args(&s.args)
                    .status()
                    .map_err(|e| format!("{} の段を起動できません: {e}", s.stage))?;
                if !status.success() {
                    let code = status.code().map_or("シグナルで終了".to_string(), |c| {
                        format!("終了コード {c}")
                    });
                    return Err(format!("{} の段が失敗しました（{code}）", s.stage));
                }
            }
        }
    }
    Ok(())
}

/// `dir` を消して作り直す。消す前に、`within` の配下であること・`within` から `dir` までに symlink が無いことを確かめる。
fn reset_dir(dir: &Path, within: &Path) -> Result<(), String> {
    let rel = dir
        .strip_prefix(within)
        .ok()
        // 空（`within` 自身）と、`..` などの字句上だけ配下に見える形（`within/../x`）は拒否する。
        .filter(|r| {
            r.components().next().is_some()
                && r.components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)))
        })
        .ok_or_else(|| {
            format!(
                "{} は {} の配下ではないため消しません",
                dir.display(),
                within.display()
            )
        })?;
    let mut cur = within.to_path_buf();
    for c in rel.components() {
        cur.push(c);
        match std::fs::symlink_metadata(&cur) {
            Ok(md) if md.file_type().is_symlink() => {
                return Err(format!("{} は symlink のため消しません", cur.display()))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(format!("{} を調べられません: {e}", cur.display())),
        }
    }
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{} を消せません: {e}", dir.display())),
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))
}

/// symlink でない実体のディレクトリか（`is_dir` はリンク先を辿るため使わない）。
fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// `from` 配下を `to` へ写す。`from` 自身と配下の symlink は辿らない（読み取り元の外を成果物に取り込まない）。
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    if !is_real_dir(from) {
        return Err(format!(
            "{} は symlink でないディレクトリではないため写しません",
            from.display()
        ));
    }
    std::fs::create_dir_all(to).map_err(|e| format!("{} を作れません: {e}", to.display()))?;
    let rd =
        std::fs::read_dir(from).map_err(|e| format!("{} を読めません: {e}", from.display()))?;
    for ent in rd {
        let ent = ent.map_err(|e| format!("{} を列挙できません: {e}", from.display()))?;
        let ty = ent
            .file_type()
            .map_err(|e| format!("{} を調べられません: {e}", ent.path().display()))?;
        let dest = to.join(ent.file_name());
        if ty.is_symlink() {
            continue;
        } else if ty.is_dir() {
            copy_dir(&ent.path(), &dest)?;
        } else if ty.is_file() {
            std::fs::copy(ent.path(), &dest).map_err(|e| {
                format!(
                    "{} を {} へ写せません: {e}",
                    ent.path().display(),
                    dest.display()
                )
            })?;
        }
    }
    Ok(())
}

fn make_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(d) => {
            std::fs::create_dir_all(d).map_err(|e| format!("{} を作れません: {e}", d.display()))
        }
        None => Ok(()),
    }
}

/// `cargo fandhe run|build` の本体。問題は 1 件ずつの文字列で返す。
pub fn run_cli(opts: &Options) -> Result<(), Vec<String>> {
    let os = resolve_os(opts, Os::host())?;
    let env = detect_env(os);
    let app = load_app(opts, os)?;
    let ops = prepare(&app, opts, &env)?;
    execute(&ops).map_err(|e| vec![e])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wasm_bindgen_version_output() {
        assert_eq!(
            parse_wasm_bindgen_version("wasm-bindgen 0.2.126\n").as_deref(),
            Some("0.2.126")
        );
        assert_eq!(
            parse_wasm_bindgen_version("wasm-bindgen 0.2.128 (abc123)").as_deref(),
            Some("0.2.128")
        );
        for bad in ["", "wasm-bindgen", "wasm-opt 1.2.3", "0.2.126"] {
            assert_eq!(parse_wasm_bindgen_version(bad), None, "{bad:?}");
        }
    }
}
