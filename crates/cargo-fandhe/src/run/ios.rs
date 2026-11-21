//! `cargo fandhe run|build --os ios` の手順（`run/mod.rs` の [`prepare`](super::prepare) から呼ばれる、Issue #1424）。
//!
//! iOS シミュレータ向けだけを扱う（実機の `--device` は #1425）。Xcode プロジェクトを持たず、xcodebuild にも委譲しない。
//! 流れは gen（iOS の分だけ）→ `cargo rustc --bin <bin> --target <シミュレータの triple>` → `.app` の作り直し →
//! 実行体と Info.plist（gen の出力に `CFBundleSupportedPlatforms` を足したもの）の配置 → `codesign --sign -`（ad-hoc）→
//! （run は）`xcrun simctl install`・`launch`。
//! entitlements は署名に入れず、実行体の `__TEXT,__entitlements` に埋める（Xcode のシミュレータ向けと同じ形）。
//! ad-hoc 署名に HealthKit 等の制限付きの entitlements を入れると、シミュレータが起動を拒否する（SBMainWorkspace に拒否される）。
//! シミュレータ（iOS 26.5 で確認）が読むのは XML の `__entitlements` で、DER の `__ents_der` だけでは HealthKit が entitlement 無しと答える。
//! HealthKit は `application-identifier` も見るため、無ければ bundle id を値として足す（Xcode のシミュレータ向けも足す）。
//! 埋め込みは最終の bin のリンクにだけ要るため、依存を作り直さない `cargo rustc` の追加引数で渡す。
//! cargo はリンク引数が指すファイルの中身を見ないため、置き場の名前に中身のハッシュを入れて、中身が変われば引数ごと変わるようにする。
//! cargo には gen の Info.plist の `MinimumOSVersion` と同じ値を `IPHONEOS_DEPLOYMENT_TARGET` で渡す
//! （cargo の `[env]` は依存先から引き継がれず、無いと rustc の既定の最小 OS でリンクされて Info.plist と食い違う）。
//! `.app` と gen の出力は `<target_dir>/fandhe/<package>/ios/` に置く（アプリの `gen/` には触れない）。
//! 拡張（`ios/extensions/<name>/`）は組み込まない（拡張のビルドは Xcode のターゲットを要する。#1212 系）。
//! シミュレータは自動で起動しない（Android のエミュレータと同じ）。署名は鍵の要らない ad-hoc だけで、秘密値は扱わない。
//! eject した Info.plist が bundle id・実行体名・最小 OS で Cargo.toml と食い違うと install・launch が原因の分からない失敗になるため、
//! 組み立て前に突き合わせて止める。`--device` とホストの検査は `load_app` より前に `run::check_os` が行う。

use std::hash::{Hash, Hasher};
use std::path::Path;

use super::{output_of, Action, App, Env, Op, Options, Step};
use crate::native::{esc_xml, IOS_MIN_VERSION};

/// gen の出力のうち本体の Info.plist と entitlements（`gen/` からの相対）。
const INFO_PLIST: &str = "ios/Info.plist";
const ENTITLEMENTS: &str = "ios/App.entitlements";
const EXTENSIONS: &str = "ios/extensions/";

/// 起動中の iOS シミュレータ 1 台。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Simulator {
    pub udid: String,
    pub name: String,
    /// ランタイムの iOS の版（major, minor）。
    pub version: (u32, u32),
}

/// iOS の環境のスナップショット。
#[derive(Clone, Debug, Default)]
pub struct IosEnv {
    /// cargo-fandhe がビルドされた CPU（`std::env::consts::ARCH`）。シミュレータの triple を決める
    /// （Rosetta 下の x86_64 版なら x86_64 のシミュレータ向けになり、Apple silicon でもそのまま動く）。
    pub arch: String,
    /// `xcrun --sdk iphonesimulator --show-sdk-path` が通る（Xcode が入って選ばれている）。
    pub sdk: bool,
    /// `xcrun --find simctl` が通る（`run` のときだけ調べる）。
    pub simctl: bool,
    /// 起動中の iOS シミュレータ（`run` のときだけ調べる）。
    pub booted: Vec<Simulator>,
}

impl IosEnv {
    /// 使うシミュレータ。起動中で、iOS が最小 OS 以上のものがちょうど 1 台のとき。
    fn chosen(&self) -> Result<&Simulator, String> {
        let min = parse_version(IOS_MIN_VERSION).unwrap_or_default();
        let names = |s: &[&Simulator]| {
            s.iter()
                .map(|s| format!("{}（iOS {}.{}）", s.name, s.version.0, s.version.1))
                .collect::<Vec<_>>()
                .join("・")
        };
        let all: Vec<&Simulator> = self.booted.iter().collect();
        let usable: Vec<&Simulator> = all.iter().copied().filter(|s| s.version >= min).collect();
        match (usable.as_slice(), all.as_slice()) {
            ([one], _) => Ok(one),
            ([], []) => Err("起動中の iOS シミュレータがありません。Simulator.app か `xcrun simctl boot \"<端末名>\"` で起動してください（端末名は `xcrun simctl list devices available`。iOS のランタイムが無ければ `xcodebuild -downloadPlatform iOS` で入れます。自動では起動しません）".into()),
            ([], old) => Err(format!(
                "起動中の iOS シミュレータ {} は最小 OS（iOS {IOS_MIN_VERSION}）より古いため入れられません。iOS {IOS_MIN_VERSION} 以上のシミュレータを起動してください",
                names(old)
            )),
            (many, _) => Err(format!(
                "起動中の iOS シミュレータが複数あります（{}）。使う 1 台を残し、ほかを `xcrun simctl shutdown \"<端末名>\"` で止めてください",
                names(many)
            )),
        }
    }
}

/// `17.0`・`26.5` を (major, minor) にする。
fn parse_version(s: &str) -> Option<(u32, u32)> {
    let mut it = s.split(['.', '-']);
    let major = it.next()?.parse().ok()?;
    let minor = it.next().map_or(Some(0), |m| m.parse().ok())?;
    Some((major, minor))
}

/// `xcrun simctl list devices booted -j` の出力から、起動中の iOS シミュレータを取り出す
/// （watchOS・tvOS・visionOS のランタイム、`Booted` でないもの、`isAvailable` が false のものは除く）。
pub fn parse_booted_simulators(json: &str) -> Vec<Simulator> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(runtimes) = v["devices"].as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (runtime, devices) in runtimes {
        let Some(version) = runtime
            .split_once("SimRuntime.iOS-")
            .and_then(|(_, v)| parse_version(v))
        else {
            continue;
        };
        for d in devices.as_array().into_iter().flatten() {
            let (Some(udid), Some(name)) = (d["udid"].as_str(), d["name"].as_str()) else {
                continue;
            };
            if d["state"].as_str() == Some("Booted") && d["isAvailable"].as_bool() != Some(false) {
                out.push(Simulator {
                    udid: udid.into(),
                    name: name.into(),
                    version,
                });
            }
        }
    }
    out
}

/// cargo-fandhe がビルドされた CPU から、シミュレータ向けの Rust の triple を決める（aarch64 は arm64 のシミュレータ、x86_64 は x86_64）。
pub(crate) fn simulator_triple(arch: &str) -> Option<&'static str> {
    match arch {
        "aarch64" => Some("aarch64-apple-ios-sim"),
        "x86_64" => Some("x86_64-apple-ios"),
        _ => None,
    }
}

/// 実環境を観測する。`commands` は PATH 上にあったコマンド名。
pub(super) fn detect(commands: &std::collections::BTreeSet<String>, action: Action) -> IosEnv {
    let mut i = IosEnv {
        arch: std::env::consts::ARCH.to_string(),
        ..IosEnv::default()
    };
    if !commands.contains("xcrun") {
        return i;
    }
    i.sdk = output_of("xcrun", &["--sdk", "iphonesimulator", "--show-sdk-path"]).is_some();
    if action == Action::Run {
        i.simctl = output_of("xcrun", &["--find", "simctl"]).is_some();
        if i.simctl {
            i.booted = output_of("xcrun", &["simctl", "list", "devices", "booted", "-j"])
                .map(|o| parse_booted_simulators(&o))
                .unwrap_or_default();
        }
    }
    i
}

/// XML の plist の最上位の dict の末尾（最後の `</dict>` の前）へ、キーと値（XML の断片）を足す。
fn insert_top(body: &str, what: &str, key: &str, value: &str) -> Result<String, String> {
    let at = body.rfind("</dict>").ok_or_else(|| {
        format!("{what} を読めません（XML の plist の最上位に dict がありません）")
    })?;
    let mut out = body.to_string();
    out.insert_str(at, &format!("\t<key>{key}</key>\n\t{value}\n"));
    Ok(out)
}

fn unescape_xml(v: &str) -> String {
    v.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// XML の plist の最上位の dict にあるキーと、値が `<string>` ならその文字列（それ以外は `None`）。
/// 入れ子の dict・array の中のキーは数えない（`info-plist` の追記は入れ子に同名キーを持ちうる）。
/// ponytail: タグを数えるだけの走査。CDATA は扱わない（gen も Xcode も出さない）。
fn top_entries(body: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut depth = 0usize; // dict・array の深さ。最上位の dict の中が 1
    let mut pending: Option<String> = None; // 値を待つ最上位のキー
    let mut rest = body;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.split_once("-->").map_or("", |(_, r)| r);
            continue;
        }
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        let closing = tag.starts_with('/');
        let empty = tag.ends_with('/');
        let name = tag
            .trim_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("");
        if closing {
            if matches!(name, "dict" | "array") {
                depth = depth.saturating_sub(1);
            }
            continue;
        }
        if depth == 1 && name == "key" && !empty {
            let (text, r) = rest.split_once("</key>").unwrap_or((rest, ""));
            rest = r;
            pending = Some(unescape_xml(text));
            continue;
        }
        if depth == 1 {
            if let Some(k) = pending.take() {
                let v = match (name, empty) {
                    ("string", true) => Some(String::new()),
                    ("string", false) => {
                        let (text, r) = rest.split_once("</string>").unwrap_or((rest, ""));
                        rest = r;
                        Some(unescape_xml(text))
                    }
                    _ => None,
                };
                out.push((k, v));
            }
        }
        if matches!(name, "dict" | "array") && !empty {
            depth += 1;
        }
    }
    out
}

fn has_top_key(body: &str, key: &str) -> bool {
    top_entries(body).iter().any(|(k, _)| k == key)
}

/// XML の plist の最上位の dict で、`key` の値の文字列。無い・文字列でなければ `None`。
fn plist_string(body: &str, key: &str) -> Option<String> {
    top_entries(body)
        .into_iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v)
}

/// gen の Info.plist に、シミュレータ向けの `CFBundleSupportedPlatforms` を足す。
/// 生成物も eject した XML の plist も、最上位の dict の末尾（最後の `</dict>` の前）へ入れる。
/// 既に書かれていれば（eject・`info-plist` の追記）、組み立て時に決まる値と食い違いうるため拒否する。
pub fn simulator_info_plist(body: &str) -> Result<String, String> {
    if has_top_key(body, "CFBundleSupportedPlatforms") {
        return Err("Info.plist に CFBundleSupportedPlatforms があります。シミュレータか実機かで cargo fandhe が決める値のため、eject した Info.plist・`info-plist` の追記から消してください".into());
    }
    insert_top(
        body,
        "Info.plist",
        "CFBundleSupportedPlatforms",
        "<array>\n\t\t<string>iPhoneSimulator</string>\n\t</array>",
    )
}

/// 最終の Info.plist が、Cargo.toml の bundle-id・bin 名・cargo へ渡す最小 OS と一致するかを見る。
/// 食い違いは eject した Info.plist でしか起きない（gen は同じ値から作る）ため、直し方は eject 側を案内する。
fn info_plist_problems(plist: &str, bundle: &str, bin: &str) -> Vec<String> {
    [
        ("CFBundleIdentifier", "Cargo.toml の bundle-id", bundle),
        ("CFBundleExecutable", "実行体（bin）の名前", bin),
        ("MinimumOSVersion", "cargo fandhe が cargo へ渡す最小 OS", IOS_MIN_VERSION),
    ]
    .into_iter()
    .filter_map(|(key, what, want)| {
        let got = plist_string(plist, key);
        (got.as_deref() != Some(want)).then(|| {
            let got = got.map_or("文字列で書かれていません".to_string(), |g| format!("`{g}` です"));
            format!("Info.plist の {key} が {got}。{what}（`{want}`）にそろえてください。eject した `eject/ios/Info.plist` を直すか、消して生成へ戻してください")
        })
    })
    .collect()
}

/// 実行体に埋める entitlements。`application-identifier` が無ければ bundle id を値として足す
/// （HealthKit はシミュレータでもこれが無いと `Missing application-identifier entitlement` で失敗する）。
pub fn simulator_entitlements(body: &str, bundle: &str) -> Result<String, String> {
    if has_top_key(body, "application-identifier") {
        return Ok(body.to_string());
    }
    insert_top(
        body,
        "entitlements",
        "application-identifier",
        &format!("<string>{}</string>", esc_xml(bundle)),
    )
}

/// 前提を全件検査し、問題がなければ手順を返す。`dir` は `.app` と gen の出力の置き場。
pub(super) fn prepare(
    app: &App,
    opts: &Options,
    env: &Env,
    dir: &Path,
) -> Result<Vec<Op>, Vec<String>> {
    let i = &env.ios;
    let run = opts.action == Action::Run;
    let mut problems = Vec::new();
    if !env.commands.contains("xcrun") {
        problems.push(
            "xcrun がありません。Xcode を入れてください（App Store か developer.apple.com）"
                .to_string(),
        );
    } else if !i.sdk {
        problems.push("iOS シミュレータの SDK が見つかりません。Xcode を入れ、`sudo xcode-select -s /Applications/Xcode.app/Contents/Developer` で選んでください（コマンドラインツールだけでは足りません）".to_string());
    } else if run && !i.simctl {
        problems.push("xcrun simctl が使えません。Xcode を入れ、`sudo xcode-select -s /Applications/Xcode.app/Contents/Developer` で選んでください".to_string());
    }
    let triple = simulator_triple(&i.arch);
    match (triple, &env.installed_targets) {
        (None, _) => problems.push(format!(
            "この Mac の CPU（{}）向けの iOS シミュレータには対応していません",
            i.arch
        )),
        (Some(t), Some(installed)) if !installed.contains(t) => problems.push(format!(
            "Rust の target {t} がありません。`rustup target add {t}` を実行してください"
        )),
        _ => {}
    }
    let sim = if run && i.simctl {
        i.chosen().map_err(|e| problems.push(e)).ok()
    } else {
        None
    };
    let plist = match app.ios_gen.as_ref().and_then(|g| g.get(INFO_PLIST)) {
        Some(p) => simulator_info_plist(p).map_err(|e| problems.push(e)).ok(),
        None => {
            problems.push(
                "iOS の gen 出力がありません（load_app を --os ios で呼んでください）".to_string(),
            );
            None
        }
    };
    if app.bundle_id.is_none() {
        problems.push("bundle id がありません。`[package.metadata.fandhe-app.ios]` に `bundle-id = \"com.example.app\"` を書いてください".to_string());
    }
    // 実行体はちょうど 1 つ（gen も同じ条件で止まる。ここは gen を通らない入力への備え）。
    let bin = match app.bins.as_slice() {
        [b] => Some(b.as_str()),
        _ => {
            problems.push("実行体（bin）がちょうど 1 つ要ります。src/main.rs か `[[bin]]` を 1 つにしてください".to_string());
            None
        }
    };
    let ents = match (&app.ios_gen, &app.bundle_id) {
        (Some(g), Some(b)) => g.get(ENTITLEMENTS).and_then(|e| {
            simulator_entitlements(e, b)
                .map_err(|e| problems.push(e))
                .ok()
        }),
        _ => None,
    };
    if let (Some(p), Some(b), Some(n)) = (&plist, &app.bundle_id, bin) {
        problems.extend(info_plist_problems(p, b, n));
    }
    let (Some(triple), Some(plist), Some(bundle), Some(bin), Some(files), true) = (
        triple,
        plist,
        &app.bundle_id,
        bin,
        &app.ios_gen,
        problems.is_empty(),
    ) else {
        return Err(problems);
    };

    let gen_dir = dir.join("gen");
    let bundle_dir = dir.join(format!("{}.app", app.name));
    let within = app.target_dir.clone();
    let mut ops = vec![Op::Gen {
        dir: gen_dir.clone(),
        within: within.clone(),
        files: files.clone(),
    }];
    let mut args: Vec<String> = ["rustc", "--bin", bin, "--target", triple]
        .map(String::from)
        .into();
    if opts.release {
        args.push("--release".into());
    }
    args.extend(["--manifest-path".into(), app.manifest.display().to_string()]);
    if let Some(body) = ents {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        body.hash(&mut h);
        let ent_dir = dir.join("entitlements");
        let ent = ent_dir.join(format!("{:016x}.entitlements", h.finish()));
        ops.push(Op::Reset {
            dir: ent_dir,
            within: within.clone(),
        });
        ops.push(Op::Write {
            path: ent.clone(),
            within: within.clone(),
            body,
        });
        args.push("--".into());
        for a in [
            "-sectcreate",
            "__TEXT",
            "__entitlements",
            &ent.display().to_string(),
        ] {
            args.extend(["-C".into(), "link-arg=-Xlinker".into()]);
            args.extend(["-C".into(), format!("link-arg={a}")]);
        }
    }
    ops.push(Op::Spawn(Step {
        stage: "cargo",
        program: env.cargo.clone(),
        args,
        env: vec![("IPHONEOS_DEPLOYMENT_TARGET".into(), IOS_MIN_VERSION.into())],
    }));
    let profile = if opts.release { "release" } else { "debug" };
    // `.app` の作り直しは cargo の後ろ（コンパイルに失敗しても前回の `.app` を消さない）。
    ops.extend([
        Op::Reset {
            dir: bundle_dir.clone(),
            within: within.clone(),
        },
        Op::Copy {
            from: app.target_dir.join(triple).join(profile).join(bin),
            to: bundle_dir.join(bin),
            within: within.clone(),
        },
        Op::Write {
            path: bundle_dir.join("Info.plist"),
            within: within.clone(),
            body: plist,
        },
        Op::Spawn(Step {
            stage: "codesign",
            program: "codesign".into(),
            args: ["--force", "--sign", "-", "--timestamp=none"]
                .map(String::from)
                .into_iter()
                .chain([bundle_dir.display().to_string()])
                .collect(),
            env: Vec::new(),
        }),
    ]);
    let extensions: Vec<&str> = files
        .keys()
        .filter_map(|k| k.strip_prefix(EXTENSIONS)?.split('/').next())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if !extensions.is_empty() {
        ops.push(Op::Say(format!(
            "iOS: 拡張（{}）は .app に入れません（cargo fandhe は拡張のビルドに未対応です）。拡張を要る機能は動きません",
            extensions.join("・")
        )));
    }
    ops.push(Op::Say(format!(
        "iOS シミュレータ向けの成果物: {}",
        bundle_dir.display()
    )));
    if let Some(sim) = sim {
        let simctl = |stage, args: Vec<String>| {
            let mut a = vec!["simctl".to_string()];
            a.extend(args);
            Op::Spawn(Step {
                stage,
                program: "xcrun".into(),
                args: a,
                env: Vec::new(),
            })
        };
        ops.push(Op::Say(format!(
            "シミュレータ {} へ入れて起動します",
            sim.name
        )));
        ops.push(simctl(
            "simctl-install",
            vec![
                "install".into(),
                sim.udid.clone(),
                bundle_dir.display().to_string(),
            ],
        ));
        // 起動に失敗すると simctl は非 0 で終わる。前回の起動が残っていれば止めてから起動し直す。
        ops.push(simctl(
            "simctl-launch",
            vec![
                "launch".into(),
                "--terminate-running-process".into(),
                sim.udid.clone(),
                bundle.clone(),
            ],
        ));
    }
    Ok(ops)
}
