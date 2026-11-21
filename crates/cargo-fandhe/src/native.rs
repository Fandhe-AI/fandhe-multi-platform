//! `cargo fandhe gen`: アプリが選んだ機能の宣言だけからネイティブ設定を生成する（REQ-21 / TASK-21.3、Issue #1176）。
//!
//! 呼び出し元は `main.rs` の `gen` サブコマンド（と統合テスト）。入力は `cargo metadata` の解決結果で、
//! どの機能 crate を含めるかは `reach` が決め、宣言の統合は `merge` が行う。ここは統合結果を
//! 各 OS の文法（XML・plist・Groovy）へ直列化する層で、値はすべて出力先の文法でエスケープする
//! （宣言は第三者の crate からも来る。security.md A03）。
//!
//! 殻（Android の Gradle・Manifest、iOS の Info.plist）が要る値のうち、機能の宣言とアプリの crate から
//! 決まるものもここで出す（#1259）。基盤の Gradle 依存は `fandhe-platform-android` の宣言から、
//! MainActivity の宣言（exported・launchMode・configChanges・ライブラリ名・LAUNCHER）と
//! `FandheLibrary.NAME`（機能の宣言からは、MainActivity の許可済み属性と `<application>` 直下の meta-data も出す）、iOS の確定値の Info.plist（実行体名・bundle id・版・`UIDeviceFamily` 等）は
//! アプリの crate（lib/bin target・version）と `fandhe-app.ios.bundle-id` から出す。
//!
//! iOS の `UIApplicationSceneManifest` も殻の確定値として常に出す（アプリ全体の構成で、機能の宣言に
//! 書けない。無いと実機で起動直後に終了する、#1292）。機能の `info-plist` が殻のキーを上書きする宣言はエラーにする。
//!
//! アプリ自身の追記（`fandhe-app.<os>.info-plist`・`entitlements`）は生成値へ同じ統合規則で足し、食い違えばエラーにする（#1202）。
//! Android は `fandhe-app.android` を擬似の宣言者（id `[package.metadata.fandhe-app]`）として機能と同じ `merge` へ入れる（#1202）。
//!
//! 出力先は `<アプリの crate>/gen/` に固定する（consumer-app-structure.md §4、gitignore する）。
//! Info.plist の説明文（`*UsageDescription`）は既定値を持たず、アプリ側の
//! `[package.metadata.fandhe-app]` に無ければエラーにする（既定文言のまま審査に出る事故を防ぐ）。
//! 範囲外: `CFBundleSupportedPlatforms`（シミュレータか実機かは組み立て時に決まる。`run|build --os ios` が足す、#1424）、
//! applicationId・compileSdk・`<application>` の属性（バックアップ規則の 2 属性を除く）、macOS・拡張の Info.plist の確定値化、
//! Xcode のターゲット・framework のリンク（#1212）、Windows の identity（#1208）、
//! 既存設定からの切り替え（#1194）。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::declaration::{
    android_problems, check_paths, entitlements_problems, info_plist_problems,
    main_activity_attribute_allows, no_ctrl, strip_qualifier, Android, BackupDomain, ComponentKind,
    Declaration, MetaData,
};
use crate::merge::{merge, merge_app_map, AndroidMerged, AppleMerged};
use crate::reach::{lib_name, reachable};
use crate::workspace::{collect, run_metadata, Found};

/// 生成物の印。これがある `gen/` だけを次回の生成で置き換える（利用者のディレクトリを消さない）。
const MARKER: &str = ".fandhe-gen";
pub(crate) const MAIN_ACTIVITY: &str = "com.fandhe.platform.MainActivity";
/// Android の MainActivity が受け取る設定変更。受け取らないと Activity が作り直され、winit が古い描画スレッドを
/// 残して黒画面・ANR になる（#73）。Rust の実行層の前提でアプリ固有ではない。古い端末では未知の flag は無視される。
const MAIN_ACTIVITY_CONFIG_CHANGES: &str = "orientation|screenSize|keyboardHidden|uiMode|fontScale|density|locale|layoutDirection|smallestScreenSize|screenLayout|colorMode|fontWeightAdjustment|keyboard|navigation|touchscreen|mcc|mnc|grammaticalGender";
/// iOS の scene delegate クラス名。`fandhe-platform-apple/ios/fandhe_ios_scene.m` の `@implementation` と
/// 一致させる（一致は tests/gen.rs が読み合わせる）。Info.plist が存在しないクラスを名指すと起動しない（#1292）。
const IOS_SCENE_DELEGATE: &str = "FandheSceneDelegate";
/// iOS の最低対応版。`.cargo/config.toml` の `IPHONEOS_DEPLOYMENT_TARGET` と揃える（一致は tests/gen.rs が保証する）。
/// `cargo fandhe run|build --os ios` は cargo へ同じ値を `IPHONEOS_DEPLOYMENT_TARGET` で渡す（#1201 の申し送り）。
pub(crate) const IOS_MIN_VERSION: &str = "17.0";
/// ライブラリ名を Java から参照させるクラス（アプリは `System.loadLibrary(FandheLibrary.NAME)` と書く）。
const LIBRARY_CLASS_PATH: &str = "android/java/com/fandhe/platform/FandheLibrary.java";
/// OS ごとに `cargo metadata --filter-platform` へ渡す triple（対象 OS に限った依存の cfg 評価は cargo に任せる）。
pub(crate) const ANDROID_TRIPLE: &str = "aarch64-linux-android";
pub(crate) const IOS_TRIPLE: &str = "aarch64-apple-ios";
pub(crate) const MACOS_TRIPLE: &str = "aarch64-apple-darwin";
pub(crate) const WEB_TRIPLE: &str = "wasm32-unknown-unknown";
/// Web の出力先（`gen/` からの相対）。配信側（`scripts/web/build-wasm-dom.sh`）がここから dist へ写す。
pub(crate) const WEB_MANIFEST_PATH: &str = "web/manifest.webmanifest";
pub(crate) const APP_GROUPS_KEY: &str = "com.apple.security.application-groups";

/// 生成結果。`files` のキーは `gen_dir` からの相対パス（区切りは `/`）。
#[derive(Debug)]
pub struct Generated {
    pub gen_dir: PathBuf,
    pub files: BTreeMap<String, String>,
    /// 成功時に添える通知（生成を飛ばした OS と理由、`eject/` のファイルで差し替えた分の「eject 済み: <rel>」）。失敗ではないため終了コードは変えない（Issue #1399）。
    pub notes: Vec<String>,
}

/// cdylib の無いアプリで Android の生成を飛ばしたときの通知。iOS・デスクトップだけのアプリでは正しい状態なので
/// 警告にはせず、Android を対象にするつもりで書き忘れた利用者が気付けるよう直し方を添える（Issue #1399）。
pub const ANDROID_SKIPPED_NOTE: &str =
    "Android: 生成しません（lib の crate-type に cdylib がありません。\
Android を対象にするなら `[lib]` の `crate-type` に \"cdylib\" を足してください）";

/// アプリ側の入力 `[package.metadata.fandhe-app]`。`metadata.fandhe` と別キーなのは、
/// 機能宣言として解釈されて `id` 必須で失敗するのを避けるため。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct AppConfig {
    /// App Group を使う機能があるときだけ必須。`com.example` のようなドット区切り。
    app_group_prefix: Option<String>,
    #[serde(default)]
    ios: OsConfig,
    #[serde(default)]
    macos: OsConfig,
    /// Web を出すアプリだけが書く。無いアプリには `web/` を出さない。
    web: Option<WebConfig>,
    #[serde(default)]
    android: AndroidConfig,
}

/// `[package.metadata.fandhe-app.android]`。`application-id`・`application-attributes`（殻への入力）を除いた残りは、
/// 機能の宣言と同じ [`Android`] として読み、アプリを「もう 1 つの宣言者」として機能の分と統合する
/// （`capability-declaration.md` §4 の規則のまま。食い違いはエラー）。
/// アプリの Manifest に `dataExtractionRules`／`fullBackupContent` を手書きすると、overlay の属性と
/// manifest merger で競合してビルドが失敗する（`tools:replace` は出さない。黙って片方が消えるより失敗させる）。
/// serde の `flatten` は `deny_unknown_fields` と併用できないので、キーを手で分けてから読む。
#[derive(Debug, Default, Deserialize)]
#[serde(try_from = "Map<String, Value>")]
struct AndroidConfig {
    decl: Android,
    /// `applicationId`。`cargo fandhe run|build --os android` で必須（殻の Gradle へ渡す。gen の出力には入らない）。
    application_id: Option<String>,
    /// 殻の `<application>` へ足す属性（`android:` を除いた名前 → 値）。殻の既定はキー単位で置き換える。
    /// 殻への入力で、gen の overlay には出さない。
    application_attributes: BTreeMap<String, String>,
}

impl TryFrom<Map<String, Value>> for AndroidConfig {
    type Error = String;

    fn try_from(mut m: Map<String, Value>) -> Result<Self, String> {
        let mut take = |k: &str| m.remove(k);
        let application_id = take("application-id");
        let attrs = take("application-attributes");
        Ok(AndroidConfig {
            application_id: application_id
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| format!("application-id: {e}"))?,
            application_attributes: attrs
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| format!("application-attributes: {e}"))?
                .unwrap_or_default(),
            decl: serde_json::from_value(Value::Object(m)).map_err(|e| {
                format!(
                    "android: {e}（`android` 欄は機能の宣言と同じ欄に加えて、`application-id`・`application-attributes` を書けます）"
                )
            })?,
        })
    }
}

/// アプリを宣言者として統合へ入れるときの id。kebab-case でないので機能の id と衝突しない。
pub(crate) const APP_DECL_ID: &str = "[package.metadata.fandhe-app]";

/// `application-attributes` に書けないキーと誘導先。overlay が持つ・ビルド種別が決める値は、殻から足せない。
const RESERVED_APP_ATTRIBUTES: &[(&str, &str)] = &[
    (
        "dataExtractionRules",
        "`backup-exclusions` に書いてください",
    ),
    ("fullBackupContent", "`backup-exclusions` に書いてください"),
    ("debuggable", "ビルド種別（debug・release）が決めます"),
];

/// Android の `<application>` の既定の属性（殻の Manifest）。`application-attributes` がキー単位で置き換える。
/// 値は仮の既定値で、アプリが `application-attributes` で確定する。
pub(crate) const SHELL_APPLICATION_ATTRIBUTES: &[(&str, &str)] = &[
    ("allowBackup", "false"),
    ("icon", "@android:drawable/sym_def_app_icon"),
    ("label", "@string/fandhe_app_name"),
    ("theme", "@style/FandheAppTheme"),
];

/// アプリの基礎 manifest。名前・アイコン・`start_url` などアプリが持つ値だけを書く。
/// 機能が持つメンバー（`share_target` 等）は書けず、`gen` が到達した機能の宣言から足す。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct WebConfig {
    /// アプリの crate からの相対パス。
    manifest: String,
}

/// 本体と拡張の Info.plist が共有する説明文（キー → 文言）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct OsConfig {
    #[serde(default)]
    usage_descriptions: BTreeMap<String, String>,
    /// バンドル ID（`CFBundleIdentifier`）。iOS の Info.plist を出すとき必須。macOS は書いたときだけ
    /// 本体の Info.plist を Xcode の変数ではなく確定値で出す（Xcode を通さずに実行体へ埋め込む用、#1194）。
    bundle_id: Option<String>,
    /// 表示名（`CFBundleDisplayName`）。アプリ全体の値で、機能の宣言には書かない。
    display_name: Option<String>,
    /// アプリが対応する言語（`CFBundleLocalizations`）。per-app-language の選択肢にもなる。
    #[serde(default)]
    localizations: Vec<String>,
    /// LAN 内の相手への平文通信を許す（ATS の `NSAllowsLocalNetworking`）。通信先の制限はアプリが決める。
    #[serde(default)]
    allows_local_networking: bool,
    /// 本体の Info.plist へ足す任意のキー（機能の宣言の `info-plist` と同じ形）。生成値と食い違えばエラー。
    #[serde(default)]
    info_plist: Map<String, Value>,
    /// 本体の `App.entitlements` へ足す任意のキー（機能の宣言の `entitlements` と同じ形）。
    #[serde(default)]
    entitlements: Map<String, Value>,
}

impl OsConfig {
    /// 殻の確定値に足すアプリ全体の値。機能の宣言はこれらのキーを上書きできない（`AppleCtx::plist`）。
    fn app_keys(&self) -> Map<String, Value> {
        let mut m = Map::new();
        if let Some(n) = &self.display_name {
            m.insert("CFBundleDisplayName".into(), Value::String(n.clone()));
        }
        if !self.localizations.is_empty() {
            m.insert(
                "CFBundleLocalizations".into(),
                Value::Array(
                    self.localizations
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }
        if self.allows_local_networking {
            m.insert(
                "NSAppTransportSecurity".into(),
                serde_json::json!({ "NSAllowsLocalNetworking": true }),
            );
        }
        m
    }
}

/// アプリの crate の情報（`cargo metadata` の root package）。殻に要る値の出どころ。
#[derive(Debug, Clone)]
pub(crate) struct AppInfo {
    pub name: String,
    pub version: String,
    /// `cdylib` を含む lib target の名前（Android の `.so` の元）。無ければ `None`。
    pub cdylib: Option<String>,
    pub bins: Vec<String>,
    /// 生成先（`<アプリの crate>/gen`）の絶対パス。Gradle から参照する。
    pub gen_dir: PathBuf,
    /// アプリの crate のディレクトリ。アプリ自身の `res`・`java` の基準になる。
    pub dir: PathBuf,
}

impl AppInfo {
    pub(crate) fn from_package(pkg: &Value, dir: PathBuf, gen_dir: PathBuf) -> Self {
        let targets: Vec<&Value> = pkg["targets"].as_array().into_iter().flatten().collect();
        let has = |t: &Value, key: &str, want: &str| {
            t[key]
                .as_array()
                .into_iter()
                .flatten()
                .any(|k| k.as_str() == Some(want))
        };
        let cdylib = targets
            .iter()
            .any(|t| has(t, "crate_types", "cdylib"))
            .then(|| lib_name(pkg));
        AppInfo {
            name: pkg["name"].as_str().unwrap_or_default().to_string(),
            version: pkg["version"].as_str().unwrap_or_default().to_string(),
            cdylib,
            bins: targets
                .iter()
                .filter(|t| has(t, "kind", "bin"))
                .filter_map(|t| t["name"].as_str().map(String::from))
                .collect(),
            gen_dir,
            dir,
        }
    }
}

/// ドット区切り（2 セグメント以上）の英数字とハイフンで、各セグメントの先頭は英数字。App Group の接頭辞と
/// iOS・macOS の bundle id が共用する（Android の application-id は `is_application_id`）。先頭の `-` を拒否するのは、
/// bundle id が `simctl launch` 等の引数になり、オプションと読まれうるため。
fn is_reverse_dns(p: &str) -> bool {
    p.split('.').count() >= 2
        && p.split('.').all(|s| {
            s.starts_with(|c: char| c.is_ascii_alphanumeric())
                && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// `ja`・`en`・`zh-Hans`・`pt-BR` の形（2〜3 文字の英字 + `-` 区切りの英数字）。
fn is_lang_tag(s: &str) -> bool {
    let mut parts = s.split('-');
    let lang = parts.next().unwrap_or_default();
    (2..=3).contains(&lang.len())
        && lang.chars().all(|c| c.is_ascii_alphabetic())
        && parts.all(|p| (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

fn is_ident(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Android の `applicationId` の形（Gradle へ文字列として埋めるため、文字種はここで絞る）。
fn is_application_id(s: &str) -> bool {
    let mut n = 0;
    for seg in s.split('.') {
        n += 1;
        if !seg.starts_with(|c: char| c.is_ascii_alphabetic())
            || !seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return false;
        }
    }
    n >= 2
}

/// plist に写せない入力の検査。TOML の日時は `cargo metadata` で `$__toml_private_datetime` の
/// 1 キーのテーブルになる（`<date>` は対応しない）ので、その形を日時として拒否する。
fn plist_input_problems(field: &str, m: &Map<String, Value>) -> Vec<String> {
    fn walk(v: &Value, at: &str, out: &mut Vec<String>) {
        match v {
            Value::Object(m) if m.keys().any(|k| k.starts_with("$__toml_private")) => out.push(
                format!("{at}: 日時は plist の値にできません（文字列で書いてください）"),
            ),
            Value::Object(m) => m
                .iter()
                .for_each(|(k, x)| walk(x, &format!("{at}.{k}"), out)),
            Value::Array(a) => a
                .iter()
                .enumerate()
                .for_each(|(i, x)| walk(x, &format!("{at}[{i}]"), out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    m.iter()
        .for_each(|(k, v)| walk(v, &format!("{field}.{k}"), &mut out));
    out
}

impl AppConfig {
    /// アプリの `android` 欄を宣言者として統合へ入れるための擬似宣言（`render_for`・`verify-scope` が使う）。
    pub(crate) fn android_decl(&self) -> Declaration {
        Declaration {
            id: APP_DECL_ID.to_string(),
            ios: None,
            macos: None,
            android: Some(self.android.decl.clone()),
            windows: None,
            web: None,
        }
    }

    /// 追記した `info-plist`・`entitlements` のキー（`verify-scope` が選んだ機能の外でも許す値）。
    /// アプリが追記した `info-plist` のキー（修飾子を落とした素のキーを含む）と `entitlements` のキー。
    /// verify-scope が種別つきで禁止から除く（`scope.rs`）。
    pub(crate) fn appended_keys(&self, os: &str) -> (Vec<String>, Vec<String>) {
        let c = if os == "ios" { &self.ios } else { &self.macos };
        // 修飾子つき（`UIBackgroundModes~ipad`）は、機能が宣言する素のキーとしても照合に現れるので素のキーも許す。
        let plist = c
            .info_plist
            .keys()
            .flat_map(|k| [k.clone(), strip_qualifier(k).to_string()])
            .collect();
        (plist, c.entitlements.keys().cloned().collect())
    }

    pub(crate) fn parse(v: &Value) -> Result<Self, Vec<String>> {
        let cfg: AppConfig = if v.is_null() {
            AppConfig::default()
        } else {
            serde_json::from_value(v.clone())
                .map_err(|e| vec![format!("[package.metadata.fandhe-app]: {e}")])?
        };
        let mut errs = Vec::new();
        if let Some(p) = &cfg.app_group_prefix {
            if !is_reverse_dns(p) {
                errs.push(format!("fandhe-app.app-group-prefix: `{p}` はドット区切り（2 セグメント以上）の英数字とハイフンにし、各セグメントは英数字で始めてください"));
            }
        }
        if let Some(b) = &cfg.ios.bundle_id {
            if !is_reverse_dns(b) {
                errs.push(format!("fandhe-app.ios.bundle-id: `{b}` はドット区切り（2 セグメント以上）の英数字とハイフンにし、各セグメントは英数字で始めてください"));
            }
        }
        if let Some(w) = &cfg.web {
            let m = &w.manifest;
            if m.is_empty()
                || m.starts_with('/')
                || m.contains(['\\', ':'])
                || m.chars().any(char::is_control)
                || m.split('/').any(|s| s == "..")
            {
                errs.push(format!("fandhe-app.web.manifest: `{m}` はアプリの crate からの相対パスにしてください（空・絶対・`..`・`\\`・`:` は不可）"));
            }
        }
        if let Some(id) = &cfg.android.application_id {
            if !is_application_id(id) {
                errs.push(format!("fandhe-app.android.application-id: `{id}` は 2 セグメント以上のドット区切りで、各セグメントは英字で始まる英数字と `_` にしてください"));
            }
        }
        errs.extend(
            android_problems(&cfg.android.decl)
                .iter()
                .map(|p| format!("fandhe-app.{p}")),
        );
        for (k, v) in &cfg.android.application_attributes {
            let at = format!("fandhe-app.android.application-attributes.{k}");
            if let Some((_, hint)) = RESERVED_APP_ATTRIBUTES
                .iter()
                .find(|(r, _)| r.eq_ignore_ascii_case(k))
            {
                errs.push(format!("{at}: `{k}` は書けません（{hint}）"));
            } else if !k.starts_with(|c: char| c.is_ascii_alphabetic())
                || !k.chars().all(|c| c.is_ascii_alphanumeric())
            {
                errs.push(format!(
                    "{at}: 属性名は先頭が英字で、残りは英数字にしてください（`android:` は付けません）"
                ));
            }
            if !no_ctrl(v) {
                errs.push(format!("{at}: 制御文字は使えません"));
            }
        }
        if let Some(b) = &cfg.macos.bundle_id {
            if !is_reverse_dns(b) {
                errs.push(format!("fandhe-app.macos.bundle-id: `{b}` はドット区切り（2 セグメント以上）の英数字とハイフンにし、各セグメントは英数字で始めてください"));
            }
        }
        for (os, c) in [("ios", &cfg.ios), ("macos", &cfg.macos)] {
            if let Some(n) = &c.display_name {
                if n.trim().is_empty() || !no_ctrl(n) {
                    errs.push(format!(
                        "fandhe-app.{os}.display-name: 空白だけ・制御文字を含む値にできません"
                    ));
                }
            }
            let mut seen = BTreeSet::new();
            for l in &c.localizations {
                if !is_lang_tag(l) {
                    errs.push(format!("fandhe-app.{os}.localizations: `{l}` は言語タグ（`ja`・`en`・`zh-Hans` など）にしてください"));
                }
                if !seen.insert(l) {
                    errs.push(format!(
                        "fandhe-app.{os}.localizations: `{l}` が重複しています"
                    ));
                }
            }
            for (f, m) in [
                ("info-plist", &c.info_plist),
                ("entitlements", &c.entitlements),
            ] {
                let field = format!("fandhe-app.{os}.{f}");
                errs.extend(plist_input_problems(&field, m));
                let probs = if f == "info-plist" {
                    info_plist_problems(&field, m)
                } else {
                    entitlements_problems(&field, m)
                };
                errs.extend(probs.iter().map(|p| p.to_string()));
            }
            for (k, t) in &c.usage_descriptions {
                if !(is_ident(k) && k.ends_with("UsageDescription")) {
                    errs.push(format!("fandhe-app.{os}.usage-descriptions: `{k}` は `*UsageDescription` のキーにしてください"));
                }
                if t.trim().is_empty() || !no_ctrl(t) {
                    errs.push(format!("fandhe-app.{os}.usage-descriptions.{k}: 文言は空白だけ・制御文字を含む値にできません"));
                }
            }
        }
        if errs.is_empty() {
            Ok(cfg)
        } else {
            Err(errs)
        }
    }
}

// ---- エスケープ・直列化 ----

pub(crate) fn esc_xml(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c => o.push(c),
        }
    }
    o
}

/// Groovy の単一引用符の文字列（補間なし）にする。
fn groovy_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// plist の値を直列化する。キーの順序は決定的にするためソートする。null は表現できないので拒否する。
fn plist_value(v: &Value, depth: usize, at: &str, out: &mut String) -> Result<(), String> {
    let pad = "\t".repeat(depth);
    match v {
        Value::String(s) => out.push_str(&format!("{pad}<string>{}</string>\n", esc_xml(s))),
        Value::Bool(b) => out.push_str(&format!("{pad}<{b}/>\n")),
        Value::Number(n) if n.is_f64() => out.push_str(&format!("{pad}<real>{n}</real>\n")),
        Value::Number(n) => out.push_str(&format!("{pad}<integer>{n}</integer>\n")),
        Value::Array(a) => {
            out.push_str(&format!("{pad}<array>\n"));
            for (i, x) in a.iter().enumerate() {
                plist_value(x, depth + 1, &format!("{at}[{i}]"), out)?;
            }
            out.push_str(&format!("{pad}</array>\n"));
        }
        Value::Object(m) => plist_dict(m, depth, at, out)?,
        Value::Null => return Err(format!("{at}: plist は null を表せません")),
    }
    Ok(())
}

fn plist_dict(
    m: &Map<String, Value>,
    depth: usize,
    at: &str,
    out: &mut String,
) -> Result<(), String> {
    let pad = "\t".repeat(depth);
    out.push_str(&format!("{pad}<dict>\n"));
    let sorted: BTreeMap<_, _> = m.iter().collect();
    for (k, v) in sorted {
        out.push_str(&format!("{pad}\t<key>{}</key>\n", esc_xml(k)));
        plist_value(v, depth + 1, &format!("{at}.{k}"), out)?;
    }
    out.push_str(&format!("{pad}</dict>\n"));
    Ok(())
}

fn plist_doc(m: &Map<String, Value>, at: &str) -> Result<String, String> {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!-- cargo fandhe gen が生成。編集しない -->\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n",
    );
    plist_dict(m, 0, at, &mut out)?;
    out.push_str("</plist>\n");
    Ok(out)
}

// ---- Android ----

fn xml_attr(k: &str, v: &str) -> String {
    format!(" android:{k}=\"{}\"", esc_xml(v))
}

fn filter_xml(f: &crate::declaration::IntentFilter, pad: &str, out: &mut String) {
    out.push_str(&format!("{pad}<intent-filter>\n"));
    for a in &f.actions {
        out.push_str(&format!("{pad}    <action{} />\n", xml_attr("name", a)));
    }
    for c in &f.categories {
        out.push_str(&format!("{pad}    <category{} />\n", xml_attr("name", c)));
    }
    for d in &f.data {
        let mut attrs = String::new();
        for (k, v) in [
            ("scheme", &d.scheme),
            ("host", &d.host),
            ("mimeType", &d.mime_type),
            ("pathPrefix", &d.path_prefix),
        ] {
            if let Some(v) = v {
                attrs.push_str(&xml_attr(k, v));
            }
        }
        out.push_str(&format!("{pad}    <data{attrs} />\n"));
    }
    out.push_str(&format!("{pad}</intent-filter>\n"));
}

/// `<meta-data>` 1 件。コンポーネント内と `<application>` 直下で共用する。
fn meta_data_xml(md: &MetaData, pad: &str, out: &mut String) {
    let val = match (&md.resource, &md.value) {
        (Some(r), _) => xml_attr("resource", r),
        (None, Some(v)) => xml_attr("value", v),
        (None, None) => String::new(),
    };
    out.push_str(&format!(
        "{pad}<meta-data{}{val} />\n",
        xml_attr("name", &md.name)
    ));
}

/// manifest merger 用の overlay。`package` 属性は持たず、アプリ本体の Manifest へ足される。
/// MainActivity の宣言は常に出す（`MAIN_ACTIVITY` は cargo-fandhe の契約）。機能の intent-filter はその後に続ける。
fn android_manifest(m: &AndroidMerged, lib: &str) -> Result<String, String> {
    let mut o = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!-- cargo fandhe gen が生成。編集しない -->\n<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n",
    );
    for (name, p) in &m.permissions {
        let max = p
            .max_sdk
            .map(|n| xml_attr("maxSdkVersion", &n.to_string()))
            .unwrap_or_default();
        let flags = if p.flags.is_empty() {
            String::new()
        } else {
            let v: Vec<_> = p.flags.iter().map(|f| f.as_str()).collect();
            xml_attr("usesPermissionFlags", &v.join("|"))
        };
        o.push_str(&format!(
            "    <uses-permission{}{max}{flags} />\n",
            xml_attr("name", name)
        ));
    }
    for (name, req) in &m.features {
        o.push_str(&format!(
            "    <uses-feature{}{} />\n",
            xml_attr("name", name),
            xml_attr("required", &req.to_string())
        ));
    }
    if !m.queries.is_empty() {
        o.push_str("    <queries>\n");
        for a in &m.queries {
            o.push_str(&format!(
                "        <intent>\n            <action{} />\n        </intent>\n",
                xml_attr("name", a)
            ));
        }
        o.push_str("    </queries>\n");
    }
    {
        let backup = if m.backup_exclusions.is_empty() {
            String::new()
        } else {
            xml_attr(
                "dataExtractionRules",
                &format!("@xml/{DATA_EXTRACTION_RULES}"),
            ) + &xml_attr("fullBackupContent", &format!("@xml/{BACKUP_RULES}"))
        };
        o.push_str(&format!("    <application{backup}>\n"));
        // 許可リスト外のキーは殻の属性との二重出力や公開範囲の変更になるので、`merge` を直接呼ぶ経路でも拒否する。
        let mut extra = String::new();
        for (k, v) in &m.main_activity_attributes {
            match main_activity_attribute_allows(k, v) {
                None => return Err(format!("MainActivity の属性 `{k}` は書けません")),
                Some(false) => {
                    return Err(format!("MainActivity の属性 `{k}` の値 {v:?} は使えません"))
                }
                Some(true) => {}
            }
            extra.push_str(&xml_attr(k, v));
        }
        o.push_str(&format!(
            "        <activity{}{}{}{}{extra}>\n",
            xml_attr("name", MAIN_ACTIVITY),
            xml_attr("exported", "true"),
            xml_attr("launchMode", "singleTask"),
            xml_attr("configChanges", MAIN_ACTIVITY_CONFIG_CHANGES)
        ));
        o.push_str(&format!(
            "            <meta-data{}{} />\n",
            xml_attr("name", "android.app.lib_name"),
            xml_attr("value", lib)
        ));
        o.push_str("            <intent-filter>\n                <action android:name=\"android.intent.action.MAIN\" />\n                <category android:name=\"android.intent.category.LAUNCHER\" />\n            </intent-filter>\n");
        for f in &m.main_activity_intent_filters {
            filter_xml(f, "            ", &mut o);
        }
        o.push_str("        </activity>\n");
        // alias は対象の <activity> が先に宣言されている必要があるので、非 alias → alias の順に出す。
        let (aliases, others): (Vec<_>, Vec<_>) = m
            .components
            .iter()
            .partition(|(_, c)| c.kind == ComponentKind::ActivityAlias);
        for (name, c) in others.into_iter().chain(aliases) {
            let kind = c.kind.tag();
            let mut attrs = xml_attr("name", name) + &xml_attr("exported", &c.exported.to_string());
            if let Some(t) = &c.target_activity {
                attrs.push_str(&xml_attr("targetActivity", t));
            }
            for (k, v) in &c.attributes {
                if !is_ident(k) {
                    return Err(format!("{name}: 属性名 `{k}` が不正です"));
                }
                attrs.push_str(&xml_attr(k, v));
            }
            o.push_str(&format!("        <{kind}{attrs}>\n"));
            for f in &c.intent_filters {
                filter_xml(f, "            ", &mut o);
            }
            for md in &c.meta_data {
                meta_data_xml(md, "            ", &mut o);
            }
            o.push_str(&format!("        </{kind}>\n"));
        }
        // 機能の宣言から来る application 直下の meta-data（コンポーネントの後、name 順）。
        for md in m.application_meta_data.values() {
            meta_data_xml(md, "        ", &mut o);
        }
        o.push_str("    </application>\n");
    }
    o.push_str("</manifest>\n");
    Ok(o)
}

/// 生成するバックアップ規則の XML のリソース名（`res/xml/<名前>.xml`）。`fandhe_` 接頭辞でアプリのリソースと衝突させない。
const DATA_EXTRACTION_RULES: &str = "fandhe_data_extraction_rules";
const BACKUP_RULES: &str = "fandhe_backup_rules";

/// バックアップ規則の XML 2 本（API 31 以降の `data-extraction-rules`、API 30 以前の `full-backup-content`）。
/// 除外は両方の節と旧形式の 3 か所へ同じ内容を出す。(domain, path) の昇順で決定的。
fn backup_rules_xml(set: &BTreeSet<(BackupDomain, String)>) -> (String, String) {
    const HEAD: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!-- cargo fandhe gen が生成。編集しない -->\n";
    let lines = |pad: &str| -> String {
        set.iter()
            .map(|(d, p)| {
                format!(
                    "{pad}<exclude domain=\"{}\" path=\"{}\" />\n",
                    d.as_str(),
                    esc_xml(p)
                )
            })
            .collect()
    };
    let extraction = format!(
        "{HEAD}<data-extraction-rules>\n    <cloud-backup>\n{}    </cloud-backup>\n    <device-transfer>\n{}    </device-transfer>\n</data-extraction-rules>\n",
        lines("        "),
        lines("        ")
    );
    let legacy = format!(
        "{HEAD}<full-backup-content>\n{}</full-backup-content>\n",
        lines("    ")
    );
    (extraction, legacy)
}

/// Gradle の apply 用スクリプト。srcDirs は crate の絶対パス（区切りは `/` に正規化）。
/// 常に出す: gen の Java（`FandheLibrary`）と Manifest を取り込む配線が要るため。
/// Manifest はアプリ本体の Manifest を残したまま、全 variant の overlay として manifest merger に載せる。
fn android_gradle(m: &AndroidMerged, dirs: &BTreeMap<&str, &Path>, gen_dir: &Path) -> String {
    let norm = |p: &Path| groovy_str(&p.to_string_lossy().replace('\\', "/"));
    let src = |items: &[(String, String)]| -> Vec<String> {
        items
            .iter()
            .filter_map(|(id, rel)| Some(norm(&dirs.get(id.as_str())?.join(rel))))
            .collect()
    };
    let (java, mut res) = (src(&m.java), src(&m.res));
    if !m.backup_exclusions.is_empty() {
        res.push(norm(&gen_dir.join("android/res")));
    }
    let mut o = String::from("// cargo fandhe gen が生成。編集しない\n");
    o.push_str("android {\n    sourceSets {\n        main {\n");
    o.push_str(&format!(
        "            java.srcDir {}\n",
        norm(&gen_dir.join("android/java"))
    ));
    if !java.is_empty() {
        o.push_str(&format!(
            "            java.srcDirs += [{}]\n",
            java.join(", ")
        ));
    }
    if !res.is_empty() {
        o.push_str(&format!(
            "            res.srcDirs += [{}]\n",
            res.join(", ")
        ));
    }
    o.push_str("        }\n    }\n}\n");
    o.push_str(&format!(
        "androidComponents {{\n    onVariants(selector().all()) {{ variant ->\n        variant.sources.manifests.addStaticManifestFile({})\n    }}\n}}\n",
        norm(&gen_dir.join("android/AndroidManifest.xml"))
    ));
    if !m.gradle.is_empty() {
        o.push_str("dependencies {\n");
        for g in &m.gradle {
            o.push_str(&format!("    implementation {}\n", groovy_str(g)));
        }
        o.push_str("}\n");
    }
    o
}

/// ライブラリ名の定数クラス。manifest の `android.app.lib_name` と `[lib] name` の 3 か所一致を 1 か所の正本にする。
fn library_class(lib: &str) -> String {
    format!(
        "// cargo fandhe gen が生成。編集しない\npackage com.fandhe.platform;\n\n/** アプリの cdylib 名（`[lib] name`）。`System.loadLibrary(FandheLibrary.NAME)` に使う。 */\npublic final class FandheLibrary {{\n    private FandheLibrary() {{}}\n\n    public static final String NAME = \"{lib}\";\n}}\n"
    )
}

// ---- Apple ----

/// 最小の Info.plist。値は Xcode のビルド設定変数で、アプリ固有の値は書かない。
fn plist_template() -> Map<String, Value> {
    [
        ("CFBundleExecutable", "$(EXECUTABLE_NAME)"),
        ("CFBundleIdentifier", "$(PRODUCT_BUNDLE_IDENTIFIER)"),
        ("CFBundleInfoDictionaryVersion", "6.0"),
        ("CFBundleName", "$(PRODUCT_NAME)"),
        ("CFBundlePackageType", "$(PRODUCT_BUNDLE_PACKAGE_TYPE)"),
        ("CFBundleShortVersionString", "$(MARKETING_VERSION)"),
        ("CFBundleVersion", "$(CURRENT_PROJECT_VERSION)"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Value::String(v.into())))
    .collect()
}

struct AppleCtx<'a> {
    os: &'a str,
    /// 本体の Info.plist を確定値で出すときの実行体名（iOS は常に、macOS は bundle-id があるとき）。
    concrete_exe: Option<&'a str>,
    cfg: &'a OsConfig,
    prefix: Option<&'a str>,
    /// この OS に到達した (id, 宣言)。説明文が無いときに要求元の機能を示す。
    found: &'a [&'a Found],
}

impl AppleCtx<'_> {
    fn requesters(&self, key: &str) -> String {
        let ids: Vec<&str> = self
            .found
            .iter()
            .filter(|f| {
                let a = if self.os == "ios" {
                    &f.decl.ios
                } else {
                    &f.decl.macos
                };
                a.as_ref().is_some_and(|a| {
                    a.usage_descriptions.iter().any(|k| k == key)
                        || a.extensions
                            .iter()
                            .any(|e| e.usage_descriptions.iter().any(|k| k == key))
                })
            })
            .map(|f| f.decl.id.as_str())
            .collect();
        ids.join(", ")
    }

    fn plist(
        &self,
        base: Map<String, Value>,
        keys: &BTreeSet<String>,
        info: &Map<String, Value>,
        errs: &mut Vec<String>,
    ) -> Map<String, Value> {
        let mut m = base;
        // 殻の確定値（scene 構成・bundle id・実行体名等）を機能の宣言が黙って上書きしないようにする。
        for k in info.keys().filter(|k| m.contains_key(*k)) {
            errs.push(format!(
                "{}: info-plist のキー {k} は殻が確定値を出すため、機能の宣言では上書きできません",
                self.os
            ));
        }
        m.extend(info.clone());
        for k in keys {
            match self.cfg.usage_descriptions.get(k) {
                Some(t) => {
                    m.insert(k.clone(), Value::String(t.clone()));
                }
                None => errs.push(format!(
                    "{}: 機能 [{}] が要求する {k} の文言がありません。[package.metadata.fandhe-app.{}.usage-descriptions] に書いてください",
                    self.os,
                    self.requesters(k),
                    self.os
                )),
            }
        }
        m
    }

    fn entitlements(
        &self,
        base: &Map<String, Value>,
        groups: &BTreeSet<String>,
        at: &str,
        errs: &mut Vec<String>,
    ) -> Map<String, Value> {
        let mut m = base.clone();
        if groups.is_empty() {
            return m;
        }
        let Some(prefix) = self.prefix else {
            errs.push(format!(
                "{at}: app-groups を使う機能があります。[package.metadata.fandhe-app] に app-group-prefix を書いてください"
            ));
            return m;
        };
        let ids = groups.iter().map(|g| {
            Value::String(if self.os == "ios" {
                format!("group.{prefix}.{g}")
            } else {
                format!("$(TeamIdentifierPrefix){prefix}.{g}")
            })
        });
        m.insert(APP_GROUPS_KEY.into(), Value::Array(ids.collect()));
        m
    }
}

/// `UIApplicationSceneManifest`。機能の宣言ではなくアプリ全体の構成で、選んだ機能に関係なく常に出す。
/// 無いと iOS 実機で起動直後に終了する（#1292、TN3187）。application ロールは winit の window を付け、
/// 外部ディスプレイ ロールは window を付けない（`fandhe_ios_scene.m`）。複数 window scene は許可しない。
fn ios_scene_manifest() -> Value {
    let role = |name: &str| {
        serde_json::json!([{
            "UISceneConfigurationName": name,
            "UISceneDelegateClassName": IOS_SCENE_DELEGATE,
        }])
    };
    serde_json::json!({
        "UIApplicationSupportsMultipleScenes": false,
        "UISceneConfigurations": {
            "UIWindowSceneSessionRoleApplication": role("Default"),
            "UIWindowSceneSessionRoleExternalDisplayNonInteractive": role("External"),
        },
    })
}

/// iOS 本体の土台。Xcode の変数ではなく確定値を書く（Xcode を通さない組み立てでもそのまま使える）。
/// `CFBundleSupportedPlatforms` は組み立て時の値（シミュレータか実機か）のため出さない（`run/ios.rs` が足す、#1424）。
fn ios_base(app: &AppInfo, cfg: &OsConfig) -> Result<Map<String, Value>, Vec<String>> {
    let mut errs = Vec::new();
    let bundle = cfg.bundle_id.clone().unwrap_or_else(|| {
        errs.push("ios: Info.plist に bundle id が要ります。[package.metadata.fandhe-app.ios] に bundle-id を書いてください".into());
        String::new()
    });
    if app.bins.len() != 1 {
        errs.push(format!(
            "ios: 実行体（bin target）がちょうど 1 つ要りますが {} 個あります: [{}]",
            app.bins.len(),
            app.bins.join(", ")
        ));
    }
    // CFBundleShortVersionString は数字とドットだけ。pre-release・build metadata は載せられない。
    let v = &app.version;
    if v.is_empty()
        || !v
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        errs.push(format!("ios: version `{v}` は数字とドットだけの版にしてください（pre-release・build metadata は使えません）"));
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    let s = |x: &str| Value::String(x.to_string());
    Ok([
        ("UIApplicationSceneManifest", ios_scene_manifest()),
        ("CFBundleExecutable", s(&app.bins[0])),
        ("CFBundleIdentifier", s(&bundle)),
        ("CFBundleInfoDictionaryVersion", s("6.0")),
        ("CFBundleName", s(&app.name)),
        ("CFBundlePackageType", s("APPL")),
        ("CFBundleShortVersionString", s(v)),
        ("CFBundleVersion", s(v)),
        ("MinimumOSVersion", s(IOS_MIN_VERSION)),
        ("UIDeviceFamily", Value::Array(vec![1.into(), 2.into()])),
        ("UILaunchScreen", Value::Object(Map::new())),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect())
}

/// macOS 本体の土台（`fandhe-app.macos.bundle-id` を書いたときだけ）。Xcode を通さずに実行体へ
/// 埋め込むため確定値を書く。`CFBundleName` は実行体名にそろえる（サービスメニューの `NSPortName` と
/// servicesProvider のポート名を一致させるため。share-receive）。
fn macos_base(app: &AppInfo, bundle: &str) -> Result<Map<String, Value>, Vec<String>> {
    let mut errs = Vec::new();
    if app.bins.len() != 1 {
        errs.push(format!(
            "macos: 実行体（bin target）がちょうど 1 つ要りますが {} 個あります: [{}]",
            app.bins.len(),
            app.bins.join(", ")
        ));
    }
    let v = &app.version;
    if v.is_empty()
        || !v
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        errs.push(format!("macos: version `{v}` は数字とドットだけの版にしてください（pre-release・build metadata は使えません）"));
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    let s = |x: &str| Value::String(x.to_string());
    Ok([
        ("CFBundleExecutable", s(&app.bins[0])),
        ("CFBundleIdentifier", s(bundle)),
        ("CFBundleInfoDictionaryVersion", s("6.0")),
        ("CFBundleName", s(&app.bins[0])),
        ("CFBundlePackageType", s("APPL")),
        ("CFBundleShortVersionString", s(v)),
        ("CFBundleVersion", s(v)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect())
}

/// 確定値の Info.plist で、機能の宣言に残る Xcode の変数を実値へ置き換える。置き換えられない変数が
/// 残ったら返す（Xcode を通さないと解決されないため、黙って出すと実行時に壊れる）。
fn resolve_build_vars(v: &mut Value, exe: &str, left: &mut BTreeSet<String>) {
    match v {
        Value::String(s) => {
            *s = s.replace("$(EXECUTABLE_NAME)", exe);
            if let Some(i) = s.find("$(") {
                left.insert(s[i..].split(')').next().unwrap_or_default().to_string() + ")");
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| resolve_build_vars(x, exe, left)),
        Value::Object(m) => m
            .values_mut()
            .for_each(|x| resolve_build_vars(x, exe, left)),
        _ => {}
    }
}

/// 追記と生成値が食い違ったときの案内。生成値そのものを変えたいなら eject で書き出して直接編集する。
fn eject_hint(rel: &str) -> String {
    format!("生成値を変えたいときは、この追記を外して `cargo fandhe gen --eject {rel}` で書き出し、直接編集してください")
}

/// アプリの追記（`fandhe-app.<os>.<field>`）を生成値へ統合する。食い違った葉は両側の値つきでエラーにし、
/// 生成値を残す。アプリの追記だけは殻のテーブル（`UILaunchScreen` 等）へも再帰できる。
/// `info-plist` の修飾子つきキー（`CFBundleName~ipad`）は、素のキーが生成値にあると OS が修飾子つきを優先して
/// 生成値を黙って上書きするため、素のキーが生成値にあればエラーにする（素のキーが無ければ通す）。
fn append_app(
    generated: Map<String, Value>,
    app: &Map<String, Value>,
    os: &str,
    field: &str,
    errs: &mut Vec<String>,
) -> Map<String, Value> {
    if app.is_empty() {
        return generated;
    }
    let at = format!("fandhe-app.{os}.{field}");
    let before = errs.len();
    if field == "info-plist" {
        for k in app.keys() {
            let base = strip_qualifier(k);
            if base != k && generated.contains_key(base) {
                errs.push(format!(
                    "{os}: {at}.{k}: 素のキー {base} が生成値にあるため書けません（`~ipad` 等の修飾子で生成値を上書きできません）"
                ));
            }
        }
    }
    match merge_app_map(&generated, app) {
        Ok(m) if errs.len() == before => m,
        Ok(_) => generated,
        Err(leaves) => {
            errs.extend(leaves.into_iter().map(|(leaf, d)| {
                format!(
                    "{os}: {at}.{leaf} が生成値と食い違います（生成値と追記の値: {d}）。{}",
                    eject_hint(&format!(
                        "{os}/{}",
                        if field == "info-plist" {
                            "Info.plist"
                        } else {
                            "App.entitlements"
                        }
                    ))
                )
            }));
            generated
        }
    }
}

fn apple_files(
    ctx: &AppleCtx,
    base: Map<String, Value>,
    m: &AppleMerged,
    files: &mut BTreeMap<String, String>,
    errs: &mut Vec<String>,
) {
    let os = ctx.os;
    let mut local = Vec::new();
    let mut base = base;
    base.extend(ctx.cfg.app_keys());
    let mut plist = ctx.plist(base, &m.usage_descriptions, &m.info_plist, &mut local);
    plist = append_app(plist, &ctx.cfg.info_plist, os, "info-plist", &mut local);
    if let Some(exe) = ctx.concrete_exe {
        let mut left = BTreeSet::new();
        for v in plist.values_mut() {
            resolve_build_vars(v, exe, &mut left);
        }
        for x in left {
            local.push(format!("{os}: 確定値の Info.plist に Xcode の変数 {x} が残ります（機能の宣言・アプリの追記の info-plist を確定値にしてください）"));
        }
    }
    let ent = ctx.entitlements(&m.entitlements, &m.app_groups, os, &mut local);
    let ent = append_app(ent, &ctx.cfg.entitlements, os, "entitlements", &mut local);
    let mut outs = vec![(format!("{os}/Info.plist"), plist_doc(&plist, "Info.plist"))];
    if !ent.is_empty() {
        outs.push((
            format!("{os}/App.entitlements"),
            plist_doc(&ent, "entitlements"),
        ));
    }
    for (name, e) in &m.extensions {
        let plist = ctx.plist(
            plist_template(),
            &e.usage_descriptions.iter().cloned().collect(),
            &e.info_plist,
            &mut local,
        );
        let ent = ctx.entitlements(
            &e.entitlements,
            &e.app_groups.iter().cloned().collect(),
            name,
            &mut local,
        );
        outs.push((
            format!("{os}/extensions/{name}/Info.plist"),
            plist_doc(&plist, name),
        ));
        if !ent.is_empty() {
            outs.push((
                format!("{os}/extensions/{name}/{name}.entitlements"),
                plist_doc(&ent, name),
            ));
        }
    }
    local.sort();
    local.dedup();
    errs.extend(local);
    for (p, r) in outs {
        match r {
            Ok(s) => {
                files.insert(p, s);
            }
            Err(e) => errs.push(format!("{os}: {e}")),
        }
    }
}

// ---- 生成本体 ----

/// 到達した機能 (OS ごと) とアプリ設定から、`gen/` 配下の相対パス → 内容を作る。純粋関数。
#[cfg(test)]
pub(crate) fn render(
    android: &[&Found],
    ios: &[&Found],
    macos: &[&Found],
    cfg: &AppConfig,
    app: &AppInfo,
) -> Result<BTreeMap<String, String>, Vec<String>> {
    render_for(None, android, ios, macos, cfg, app)
}

/// [`render`] を対象 OS（`"android"` など）に絞れる形。絞ると他 OS の殻の必須条件（iOS の bundle-id 等）を検査しない。
pub(crate) fn render_for(
    only: Option<&str>,
    android: &[&Found],
    ios: &[&Found],
    macos: &[&Found],
    cfg: &AppConfig,
    app: &AppInfo,
) -> Result<BTreeMap<String, String>, Vec<String>> {
    let want = |os: &str| only.is_none_or(|o| o == os);
    // アプリの `android` 欄は、擬似の宣言者（id `[package.metadata.fandhe-app]`、dir はアプリの crate）として機能の分と統合する。
    let app_found = Found {
        pkg_id: String::new(),
        dir: app.dir.clone(),
        decl: cfg.android_decl(),
    };
    let android: Vec<&Found> = android.iter().copied().chain([&app_found]).collect();
    let mut errs: Vec<String> = Vec::new();
    let mut files = BTreeMap::new();
    // 対象 OS 以外の欄を落としてから統合する（未到達 OS の欄の競合で全体を失敗させない）。
    let merged = |os: &str, set: &[&Found], errs: &mut Vec<String>| {
        let decls: Vec<_> = set
            .iter()
            .map(|f| {
                let mut d = f.decl.clone();
                if os != "ios" {
                    d.ios = None;
                }
                if os != "macos" {
                    d.macos = None;
                }
                if os != "android" {
                    d.android = None;
                }
                d.web = None;
                d
            })
            .collect();
        let refs: Vec<_> = decls.iter().map(|d| (d.id.as_str(), d)).collect();
        merge(&refs)
            .map_err(|e| {
                errs.extend(e.into_iter().map(|e| {
                    let m = e.to_string();
                    // アプリが当事者で Manifest に出る欄の食い違いだけ eject の案内を添える（gradle・java・res は eject できない）。
                    if os == "android"
                        && e.field.starts_with("android.")
                        && (e.a == APP_DECL_ID || e.b == APP_DECL_ID)
                    {
                        format!("{m}。{}", eject_hint("android/AndroidManifest.xml"))
                    } else {
                        m
                    }
                }))
            })
            .ok()
    };

    if let Some(m) = want("android")
        .then(|| merged("android", &android, &mut errs))
        .flatten()
        .map(|m| m.android)
    {
        match &app.cdylib {
            // Android の入口（`android_main`）は cdylib にしか置けない。cdylib の無いアプリは Android を対象にしない
            // （iOS・デスクトップだけのアプリ。Issue #1200）ので、Android の生成物を出さない。
            None => {}
            Some(lib) if !is_ident(lib) => errs.push(format!(
                "android: lib 名 `{lib}` は英数字と `_` の識別子にしてください"
            )),
            Some(lib) => {
                let dirs: BTreeMap<&str, &Path> = android
                    .iter()
                    .map(|f| (f.decl.id.as_str(), f.dir.as_path()))
                    .collect();
                match android_manifest(&m, lib) {
                    Ok(s) => {
                        files.insert("android/AndroidManifest.xml".to_string(), s);
                    }
                    Err(e) => errs.push(format!("android: {e}")),
                }
                if !m.backup_exclusions.is_empty() {
                    let (extraction, legacy) = backup_rules_xml(&m.backup_exclusions);
                    files.insert(
                        format!("android/res/xml/{DATA_EXTRACTION_RULES}.xml"),
                        extraction,
                    );
                    files.insert(format!("android/res/xml/{BACKUP_RULES}.xml"), legacy);
                }
                files.insert(
                    "android/fandhe.gradle".to_string(),
                    android_gradle(&m, &dirs, &app.gen_dir),
                );
                files.insert(LIBRARY_CLASS_PATH.to_string(), library_class(lib));
            }
        }
    }
    for (os, set) in [("ios", ios), ("macos", macos)] {
        if !want(os) {
            continue;
        }
        let Some(m) = merged(os, set, &mut errs).map(|m| if os == "ios" { m.ios } else { m.macos })
        else {
            continue;
        };
        let base = if os == "ios" {
            ios_base(app, &cfg.ios)
        } else if let Some(b) = &cfg.macos.bundle_id {
            macos_base(app, b)
        } else {
            // 機能も bundle-id も無くても、アプリの追記があれば出す。
            if m == AppleMerged::default()
                && cfg.macos.info_plist.is_empty()
                && cfg.macos.entitlements.is_empty()
            {
                continue;
            }
            Ok(plist_template())
        };
        let base = match base {
            Ok(b) => b,
            Err(e) => {
                errs.extend(e);
                continue;
            }
        };
        let concrete = os == "ios" || cfg.macos.bundle_id.is_some();
        let ctx = AppleCtx {
            os,
            concrete_exe: concrete.then(|| app.bins[0].as_str()),
            cfg: if os == "ios" { &cfg.ios } else { &cfg.macos },
            prefix: cfg.app_group_prefix.as_deref(),
            found: set,
        };
        apple_files(&ctx, base, &m, &mut files, &mut errs);
    }
    errs.sort();
    errs.dedup();
    if !errs.is_empty() {
        return Err(errs);
    }
    files.insert(
        MARKER.into(),
        "cargo fandhe gen が生成したディレクトリ。次回の生成で置き換わる\n".into(),
    );
    Ok(files)
}

/// metadata の root パッケージ（アプリ）の (crate のディレクトリ, `metadata.fandhe-app`, package)。
pub(crate) fn app_root(meta: &Value) -> Result<(PathBuf, Value, Value), Vec<String>> {
    let root = meta["resolve"]["root"].as_str().ok_or_else(|| {
        vec!["アプリのパッケージを特定できません。--manifest-path にパッケージの Cargo.toml を指定してください".to_string()]
    })?;
    let pkg = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["id"].as_str() == Some(root))
        .ok_or_else(|| vec![format!("root パッケージ {root} が packages にありません")])?;
    let dir = pkg["manifest_path"]
        .as_str()
        .and_then(|p| Path::new(p).parent())
        .ok_or_else(|| vec!["manifest_path を読めません".to_string()])?;
    Ok((
        dir.to_path_buf(),
        pkg["metadata"]["fandhe-app"].clone(),
        pkg.clone(),
    ))
}

/// `manifest`（省略時はカレントのパッケージ）のアプリが選んだ機能から、ネイティブ設定を生成する。
/// 宣言・入力に問題が 1 件でもあれば何も返さない（check と同じ厳しさ）。書き出しは [`write`]。
pub fn generate(manifest: Option<&Path>) -> Result<Generated, Vec<String>> {
    generate_for(manifest, None, None, IOS_TRIPLE).map(|(g, _)| g)
}

/// Android の分だけ生成する（`cargo fandhe run|build --os android` 用の内部入口）。iOS・macOS の殻の必須条件を検査しない。
/// 出力先は `gen_dir`（アプリの `gen/` には触れない）。戻りは生成結果と、殻への入力
/// （`fandhe-app.android` の `application-id`・`application-attributes`）。
pub(crate) fn generate_android(
    manifest: Option<&Path>,
    gen_dir: PathBuf,
) -> Result<(Generated, ShellInput), Vec<String>> {
    generate_for(manifest, Some("android"), Some(gen_dir), IOS_TRIPLE)
}

/// iOS の分だけ生成する（`cargo fandhe run|build --os ios` 用の内部入口、#1424）。Android・macOS の必須条件を検査しない。
/// 依存は `triple`（ビルドと同じシミュレータの triple）で解く。`target_abi = "sim"` などで分けた依存も、ビルドと同じ集合で設定を作る。
/// 出力先は `gen_dir`（アプリの `gen/` には触れない）。bundle id は戻りの [`ShellInput`] に入る。
pub(crate) fn generate_ios(
    manifest: Option<&Path>,
    gen_dir: PathBuf,
    triple: &str,
) -> Result<(Generated, ShellInput), Vec<String>> {
    generate_for(manifest, Some("ios"), Some(gen_dir), triple)
}

/// 殻（run・build が target 配下に作る Gradle プロジェクト・`.app`）へ渡すアプリの入力。gen の出力には入らない。
#[derive(Debug, Default, Clone)]
pub(crate) struct ShellInput {
    pub application_id: Option<String>,
    pub application_attributes: BTreeMap<String, String>,
    /// `fandhe-app.ios.bundle-id`。`run --os ios` が `simctl launch` に渡す。
    pub ios_bundle_id: Option<String>,
}

fn generate_for(
    manifest: Option<&Path>,
    only: Option<&str>,
    gen_dir: Option<PathBuf>,
    ios_triple: &str,
) -> Result<(Generated, ShellInput), Vec<String>> {
    let (mut g, shell, dir) = generate_parts(manifest, only, gen_dir, ios_triple)?;
    apply_eject(&dir, only, &mut g.files, &mut g.notes)?;
    Ok((g, shell))
}

/// eject の差し替え前の生成結果と、アプリの crate のディレクトリ。`--eject` の書き出し元（生成値そのもの）でもある。
/// `ios_triple` は iOS の依存を解く triple（gen は実機向け、`run|build --os ios` はシミュレータ向け）。
fn generate_parts(
    manifest: Option<&Path>,
    only: Option<&str>,
    gen_dir: Option<PathBuf>,
    ios_triple: &str,
) -> Result<(Generated, ShellInput, PathBuf), Vec<String>> {
    let mut sets: Vec<Vec<Found>> = Vec::new();
    let mut app: Option<(PathBuf, Value, Value)> = None;
    for (os, triple) in [
        ("android", ANDROID_TRIPLE),
        ("ios", ios_triple),
        ("macos", MACOS_TRIPLE),
    ] {
        if only.is_some_and(|o| o != os) {
            sets.push(Vec::new());
            continue;
        }
        let meta = run_metadata(manifest, Some(triple))?;
        if app.is_none() {
            app = Some(app_root(&meta)?);
        }
        let reach = reachable(&meta);
        sets.push(
            collect(&meta)?
                .into_iter()
                .filter(|f| reach.contains(&f.pkg_id))
                .collect(),
        );
    }
    let (dir, cfg, pkg) = app.expect("ループで必ず設定される");
    let cfg = AppConfig::parse(&cfg)?;
    // アプリ自身の `res`・`java` の実在は、機能の宣言と同じ規則（crate の外への脱出も拒否）で確かめる。
    let path_errs: Vec<String> = check_paths(&cfg.android_decl(), &dir)
        .iter()
        .map(|p| format!("fandhe-app.{p}"))
        .collect();
    if !path_errs.is_empty() {
        return Err(path_errs);
    }
    let gen_dir = gen_dir.unwrap_or_else(|| dir.join("gen"));
    let info = AppInfo::from_package(&pkg, dir.clone(), gen_dir.clone());
    let r = |i: usize| sets[i].iter().collect::<Vec<_>>();
    let files = render_for(only, &r(0), &r(1), &r(2), &cfg, &info)?;
    let notes = if info.cdylib.is_none() {
        vec![ANDROID_SKIPPED_NOTE.to_string()]
    } else {
        Vec::new()
    };
    Ok((
        Generated {
            gen_dir,
            files,
            notes,
        },
        ShellInput {
            application_id: cfg.android.application_id,
            application_attributes: cfg.android.application_attributes,
            ios_bundle_id: cfg.ios.bundle_id,
        },
        dir,
    ))
}

/// eject したファイルを置く場所（アプリの crate 直下。コミットする。`gen/` は無視する側）。
const EJECT_DIR: &str = "eject";
/// `eject/` を辿る深さの上限（設定ファイルの置き場は最大 `<os>/extensions/<name>/<file>` の 4 段）。
const EJECT_MAX_DEPTH: usize = 8;
/// 差し替えたときの通知の接頭辞。run・build の `load_app` も同じ文言で出す。
pub const EJECT_NOTE: &str = "eject 済み: ";
/// 生成した先頭の「編集しない」コメントと、eject で差し替える文言。XML のコメントは `--` を含められない。
const GENERATED_COMMENT: &str = "cargo fandhe gen が生成。編集しない";
const EJECTED_COMMENT: &str =
    "cargo fandhe gen の eject で書き出した。`eject/` のこのファイルを消せば生成へ戻る";

/// 殻の重ね置きの置き場（`eject/` からの相対。末尾の `/` つき）。`cargo fandhe run|build --os android` が殻へ重ねる。
const SHELL_OVERLAY_REL: &str = "android/shell/";
/// 殻の重ね置きを辿る深さの上限。Java のパッケージ（`app/src/main/java/com/example/…`）が入るため `EJECT_MAX_DEPTH` より深い。
const SHELL_OVERLAY_MAX_DEPTH: usize = 16;

/// 殻の重ね置きの置き場 `<アプリの crate>/eject/android/shell`。
pub(crate) fn shell_overlay_dir(app_dir: &Path) -> PathBuf {
    SHELL_OVERLAY_REL
        .trim_end_matches('/')
        .split('/')
        .fold(app_dir.join(EJECT_DIR), |p, s| p.join(s))
}

/// 殻の重ね置きのファイル（置き場からの相対、区切りは `/`、昇順）を集める。置き場が無ければ空。
/// symlink（置き場の途中を含む）・特殊ファイル・UTF-8 でない名前・`gen/`（gen の出力先と衝突する）・深すぎる階層はエラー。
/// `.` で始まる名前は飛ばす。中身は検査しない（png などのバイナリも置ける）。
pub(crate) fn shell_overlay(app_dir: &Path) -> Result<Vec<String>, Vec<String>> {
    let root = shell_overlay_dir(app_dir);
    crate::run::check_within(&root, app_dir).map_err(|e| vec![e])?;
    match fs::symlink_metadata(&root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(vec![format!("{} を調べられません: {e}", root.display())]),
        Ok(md) if !md.is_dir() => {
            return Err(vec![format!(
                "{} はディレクトリではありません",
                root.display()
            )])
        }
        Ok(_) => {}
    }
    let mut found = Vec::new();
    walk_eject(&root, "", 0, SHELL_OVERLAY_MAX_DEPTH, None, &mut found).map_err(|e| vec![e])?;
    let at = |rel: &str| format!("{EJECT_DIR}/{SHELL_OVERLAY_REL}{rel}");
    let mut errs = Vec::new();
    let mut rels = Vec::new();
    for (rel, ft) in found {
        if !ft.is_file() {
            errs.push(format!(
                "{}: 通常ファイルではありません（symlink は使えません）",
                at(&rel)
            ));
        } else if rel
            .split('/')
            .next()
            .is_some_and(|c| c.eq_ignore_ascii_case("gen"))
        {
            errs.push(format!(
                "{}: gen/ は置けません（gen の出力先と衝突します）。生成した設定ファイルを直したいときは `cargo fandhe gen --eject android/...` で書き出してください",
                at(&rel)
            ));
        } else {
            rels.push(rel);
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    rels.sort();
    Ok(rels)
}

/// eject できる生成物（設定ファイルだけ）。gradle・Java・バックアップ規則は TOML で足せる／絶対パスを含むため対象外。
fn ejectable(rel: &str) -> bool {
    rel == "android/AndroidManifest.xml"
        || (rel.starts_with("ios/") || rel.starts_with("macos/"))
            && (rel.ends_with(".plist") || rel.ends_with(".entitlements"))
}

/// `eject/` の通常ファイルを集める（`rel` は `eject/` からの相対、区切りは `/`）。symlink・特殊ファイルも種別つきで返す。
/// `.` で始まる名前（`.DS_Store`・エディタの退避・ロックファイル）は、種別を見る前に飛ばす。UTF-8 でない名前は拒否する。
/// `skip`（`rel` と一致するディレクトリ）の中は辿らない（殻の重ね置きは別の上限で `shell_overlay` が辿る）。
fn walk_eject(
    dir: &Path,
    rel: &str,
    depth: usize,
    max_depth: usize,
    skip: Option<&str>,
    out: &mut Vec<(String, fs::FileType)>,
) -> Result<(), String> {
    if depth > max_depth {
        return Err(format!(
            "{} は深すぎます（{max_depth} 段まで）",
            dir.display()
        ));
    }
    let rd = fs::read_dir(dir).map_err(|e| format!("{} を読めません: {e}", dir.display()))?;
    for ent in rd {
        let ent = ent.map_err(|e| format!("{} を列挙できません: {e}", dir.display()))?;
        if ent.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let name = ent
            .file_name()
            .into_string()
            .map_err(|n| format!("{} の名前 {n:?} が UTF-8 ではありません", dir.display()))?;
        let rel = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        let ft = ent
            .file_type()
            .map_err(|e| format!("{} を調べられません: {e}", ent.path().display()))?;
        if ft.is_dir() {
            if skip != Some(rel.as_str()) {
                walk_eject(&ent.path(), &rel, depth + 1, max_depth, skip, out)?;
            }
        } else {
            out.push((rel, ft));
        }
    }
    Ok(())
}

/// `eject/<rel>` が通常ファイルなら、生成した内容の代わりにその内容を採る（gen・run・build・build.rs の全経路が通る）。
/// 今回の対象に無いファイル（綴り違い・対象外のパス）・symlink・非 UTF-8 はエラー。`only` 以外の OS の分は見ない。
fn apply_eject(
    dir: &Path,
    only: Option<&str>,
    files: &mut BTreeMap<String, String>,
    notes: &mut Vec<String>,
) -> Result<(), Vec<String>> {
    let root = dir.join(EJECT_DIR);
    match fs::symlink_metadata(&root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(vec![format!("{} を調べられません: {e}", root.display())]),
        Ok(md) if !md.is_dir() => {
            return Err(vec![format!(
                "{} はディレクトリではありません（symlink は使えません）",
                root.display()
            )])
        }
        Ok(_) => {}
    }
    let mut found = Vec::new();
    walk_eject(
        &root,
        "",
        0,
        EJECT_MAX_DEPTH,
        Some(SHELL_OVERLAY_REL.trim_end_matches('/')),
        &mut found,
    )
    .map_err(|e| vec![e])?;
    found.sort_by(|a, b| a.0.cmp(&b.0));
    let mut errs = Vec::new();
    let mut picked = Vec::new();
    for (rel, ft) in found {
        let os = rel.split('/').next().unwrap_or_default();
        if matches!(os, "android" | "ios" | "macos") && only.is_some_and(|o| o != os) {
            continue;
        }
        let at = format!("{EJECT_DIR}/{rel}");
        if !ft.is_file() {
            errs.push(format!(
                "{at}: 通常ファイルではありません（symlink は使えません）"
            ));
        } else if !(ejectable(&rel) && files.contains_key(&rel)) {
            let skipped =
                rel.starts_with("android/") && !files.keys().any(|k| k.starts_with("android/"));
            errs.push(format!(
                "{at}: 今回の生成対象にありません（綴りを確かめるか、消してください）。書き出せるパス: {}{}",
                ejectable_list(files),
                if skipped { format!("。{ANDROID_SKIPPED_NOTE}") } else { String::new() }
            ));
        } else {
            match fs::read(root.join(&rel)).map(String::from_utf8) {
                Ok(Ok(body)) => picked.push((rel, body)),
                Ok(Err(_)) => errs.push(format!("{at}: UTF-8 のテキストではありません")),
                Err(e) => errs.push(format!("{at} を読めません: {e}")),
            }
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    for (rel, body) in picked {
        notes.push(format!("{EJECT_NOTE}{rel}"));
        files.insert(rel, body);
    }
    Ok(())
}

fn ejectable_list(files: &BTreeMap<String, String>) -> String {
    let v: Vec<&str> = files
        .keys()
        .map(String::as_str)
        .filter(|r| ejectable(r))
        .collect();
    if v.is_empty() {
        "なし".into()
    } else {
        v.join("、")
    }
}

/// `cargo fandhe gen --eject <rel>`: 生成した設定ファイルを `<アプリの crate>/eject/<rel>` へ書き出す。
/// 以後は gen・run・build がそのファイルを生成の代わりに使う（消せば生成へ戻る）。既にあれば上書きしない。
/// 対象外のパスは書き出せるパスの一覧つきでエラー。1 件でも不正なら何も書かない。戻りは書き出したパス。
pub fn eject(manifest: Option<&Path>, rels: &[String]) -> Result<Vec<PathBuf>, Vec<String>> {
    // android/ だけを書き出すなら Android の分だけ生成する（iOS の bundle-id が無いアプリでも使える）。
    let only =
        (!rels.is_empty() && rels.iter().all(|r| r.starts_with("android/"))).then_some("android");
    let (g, _, dir) = generate_parts(manifest, only, None, IOS_TRIPLE)?;
    let mut errs = Vec::new();
    let mut plan = Vec::new();
    for rel in rels.iter().collect::<BTreeSet<_>>() {
        let dest = rel.split('/').fold(dir.join(EJECT_DIR), |p, s| p.join(s));
        if rel
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
            || !ejectable(rel)
            || !g.files.contains_key(rel.as_str())
        {
            errs.push(format!(
                "--eject {rel}: 書き出せません。書き出せるパス: {}{}{}",
                ejectable_list(&g.files),
                if rel.starts_with(SHELL_OVERLAY_REL) {
                    "。殻のファイルは `cargo fandhe run|build --os android` のあと `<target_dir>/fandhe/<package>/android/` からコピーして `eject/android/shell/` へ置いてください"
                } else {
                    ""
                },
                if rel.starts_with("android/") && g.notes.iter().any(|n| n == ANDROID_SKIPPED_NOTE)
                {
                    format!("。{ANDROID_SKIPPED_NOTE}")
                } else {
                    String::new()
                }
            ));
        } else if let Err(e) = crate::run::check_within(&dest, &dir) {
            errs.push(format!("--eject {rel}: {e}"));
        } else if fs::symlink_metadata(&dest).is_ok() {
            errs.push(format!(
                "--eject {rel}: {} が既にあります。上書きしません",
                dest.display()
            ));
        } else {
            plan.push((
                dest,
                g.files[rel.as_str()].replacen(GENERATED_COMMENT, EJECTED_COMMENT, 1),
            ));
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    let mut written = Vec::new();
    for (dest, body) in plan {
        let parent = dest.parent().unwrap_or(&dir);
        // 検査のあとに現れたファイルも上書きしない（create_new）。
        fs::create_dir_all(parent)
            .and_then(|()| {
                use std::io::Write;
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&dest)?
                    .write_all(body.as_bytes())
            })
            .map_err(|e| vec![format!("{} を書けません: {e}", dest.display())])?;
        written.push(dest);
    }
    Ok(written)
}

/// Web だけを生成する（`cargo fandhe gen --os web`、#1238）。bin や iOS 設定を持たない Web 専用の
/// アプリ（fandhe-web-dom）が、Android・iOS の殻の必須条件に阻まれず manifest を得るための入口。
/// 基礎 manifest（`fandhe-app.web.manifest`）に、wasm32 で到達した機能の `web.manifest` を足して出す。
/// 選ばなかった機能のメンバーは入らない。`fandhe-app.web` が無いアプリはエラー（何も出ないため）。
pub fn generate_web(manifest: Option<&Path>) -> Result<Generated, Vec<String>> {
    let meta = run_metadata(manifest, Some(WEB_TRIPLE))?;
    let (dir, cfg, _) = app_root(&meta)?;
    let cfg = AppConfig::parse(&cfg)?;
    let web = cfg.web.ok_or_else(|| {
        vec!["[package.metadata.fandhe-app.web] manifest がありません。Web の出力には基礎 manifest の指定が要ります".to_string()]
    })?;
    let reach = reachable(&meta);
    let found: Vec<Found> = collect(&meta)?
        .into_iter()
        .filter(|f| reach.contains(&f.pkg_id))
        .collect();
    let body = render_web(
        &found.iter().collect::<Vec<_>>(),
        &read_base_manifest(&dir, &web.manifest)?,
    )?;
    Ok(Generated {
        gen_dir: dir.join("gen"),
        files: BTreeMap::from([
            (WEB_MANIFEST_PATH.to_string(), body),
            (
                MARKER.to_string(),
                "cargo fandhe gen が生成したディレクトリ。次回の生成で置き換わる\n".to_string(),
            ),
        ]),
        notes: Vec::new(),
    })
}

/// 基礎 manifest を読む。crate の外（symlink 経由を含む）は拒否し、読めない・JSON のオブジェクトでないときはエラー
/// （黙って機能のメンバーだけの manifest を出さない）。
fn read_base_manifest(crate_dir: &Path, rel: &str) -> Result<Map<String, Value>, Vec<String>> {
    let err = |m: String| vec![format!("fandhe-app.web.manifest: {m}")];
    let root = crate_dir
        .canonicalize()
        .map_err(|e| err(format!("{} を解決できません: {e}", crate_dir.display())))?;
    let path = root
        .join(rel)
        .canonicalize()
        .map_err(|e| err(format!("`{rel}` を解決できません: {e}")))?;
    if !path.starts_with(&root) {
        return Err(err(format!("`{rel}` が crate の外を指しています")));
    }
    let text = fs::read_to_string(&path).map_err(|e| err(format!("`{rel}` を読めません: {e}")))?;
    match serde_json::from_str(&text) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err(err(format!("`{rel}` は JSON のオブジェクトにしてください"))),
        Err(e) => Err(err(format!("`{rel}` を JSON として読めません: {e}"))),
    }
}

/// 到達した機能の `web.manifest` を統合し、基礎 manifest に足して整形済みの JSON にする。純粋関数。
/// 基礎が機能の持つメンバーを書いていたらエラー（二重管理と、選ばない機能の値の残存を防ぐ）。
pub(crate) fn render_web(web: &[&Found], base: &Map<String, Value>) -> Result<String, Vec<String>> {
    let decls: Vec<_> = web
        .iter()
        .map(|f| {
            let mut d = f.decl.clone();
            d.ios = None;
            d.macos = None;
            d.android = None;
            d
        })
        .collect();
    let refs: Vec<_> = decls.iter().map(|d| (d.id.as_str(), d)).collect();
    let merged =
        merge(&refs).map_err(|e| e.into_iter().map(|e| e.to_string()).collect::<Vec<_>>())?;
    // 許可リストのメンバーは基礎に書けない（選ばなかった機能の値が残る・二重管理になるため）。
    if let Some(k) = crate::declaration::WEB_MANIFEST_MEMBERS
        .iter()
        .find(|k| base.contains_key(**k))
    {
        return Err(vec![format!(
            "fandhe-app.web.manifest: `{k}` は機能の宣言から足すメンバーです。基礎 manifest から削除してください"
        )]);
    }
    let mut out = base.clone();
    out.extend(merged.web_manifest);
    let mut s = serde_json::to_string_pretty(&Value::Object(out))
        .map_err(|e| vec![format!("manifest を直列化できません: {e}")])?;
    s.push('\n');
    Ok(s)
}

/// 印が通常ファイルとして在るか。symlink は印と認めない（書き込みがリンク先を上書きするため）。
fn has_marker(gen_dir: &Path) -> bool {
    fs::symlink_metadata(gen_dir.join(MARKER)).is_ok_and(|m| m.is_file())
}

/// `gen_dir` へ書き出す。印（`.fandhe-gen`）のある既存ディレクトリの、今回出す側（`web/` か、それ以外）だけを消して作り直す
/// （選ばなくなった機能の拡張ディレクトリが残って成果物に混ざるのを防ぐ）。
/// symlink と、印の無い既存ディレクトリ（利用者のもの）は拒否する。
pub fn write(gen_dir: &Path, files: &BTreeMap<String, String>) -> Result<(), String> {
    match fs::symlink_metadata(gen_dir) {
        Ok(md) if md.file_type().is_symlink() => {
            return Err(format!("{} は symlink です。書き出しません", gen_dir.display()))
        }
        Ok(_) if !has_marker(gen_dir) => {
            return Err(format!(
                "{} は既に存在し、`{MARKER}` がありません。cargo fandhe gen が作ったものではないため上書きしません",
                gen_dir.display()
            ))
        }
        Ok(_) => {
            // Web の生成（`web/` 配下を出す）とネイティブの生成は互いの成果物を消さない:
            // 今回出す側（web か、web 以外）の直下だけを消す。
            let web_run = files.keys().any(|k| k.starts_with("web/"));
            let rd = fs::read_dir(gen_dir).map_err(|e| format!("{} を読めません: {e}", gen_dir.display()))?;
            for ent in rd {
                let ent = ent.map_err(|e| format!("{} を列挙できません: {e}", gen_dir.display()))?;
                let name = ent.file_name();
                if name == MARKER || (name == "web") != web_run {
                    continue;
                }
                let p = ent.path();
                let md = fs::symlink_metadata(&p).map_err(|e| format!("{} を調べられません: {e}", p.display()))?;
                let r = if md.is_dir() { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
                r.map_err(|e| format!("{} を消せません: {e}", p.display()))?;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{} を調べられません: {e}", gen_dir.display())),
    }
    // 印を最初に書く。途中で失敗しても印付きで残り、次回の gen が作り直せる。
    let mut entries: Vec<_> = files.iter().collect();
    entries.sort_by_key(|(rel, _)| rel.as_str() != MARKER);
    for (rel, body) in entries {
        if rel.split('/').any(|s| s.is_empty() || s == "..") {
            return Err(format!("不正な出力パス `{rel}`"));
        }
        let path = rel.split('/').fold(gen_dir.to_path_buf(), |p, s| p.join(s));
        let dir = path.parent().unwrap_or(gen_dir);
        let res = fs::create_dir_all(dir)
            .map_err(|e| format!("{} を作れません: {e}", dir.display()))
            .and_then(|()| {
                fs::write(&path, body).map_err(|e| format!("{} を書けません: {e}", path.display()))
            });
        if let Err(e) = res {
            // ここへ来る gen/ は新規か印付きを消した後の自前分。印が無いまま残ると次回が利用者のものとして拒否するため片付ける。
            if !has_marker(gen_dir) {
                let _ = fs::remove_dir_all(gen_dir);
            }
            return Err(e);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declaration::parse;
    use serde_json::json;

    fn found(v: Value) -> Found {
        Found {
            pkg_id: "x".into(),
            dir: PathBuf::from("/c/x"),
            decl: parse(v).unwrap(),
        }
    }
    fn cfg(v: Value) -> AppConfig {
        AppConfig::parse(&v).unwrap()
    }
    /// bundle id だけを持つ最小の設定に、足りない欄を足して使う。
    const BID: &str = "com.example.app";
    fn app() -> AppInfo {
        AppInfo {
            dir: PathBuf::from("/a"),
            name: "my-app".into(),
            version: "1.2.3".into(),
            cdylib: Some("my_app".into()),
            bins: vec!["my-app-bin".into()],
            gen_dir: PathBuf::from("/a/gen"),
        }
    }
    fn err(r: Result<BTreeMap<String, String>, Vec<String>>) -> String {
        r.expect_err("拒否されるはず").join("\n")
    }

    #[test]
    fn missing_usage_description_names_os_feature_and_key() {
        let f = found(
            json!({"id": "cam", "ios": {"usage-descriptions": ["NSCameraUsageDescription"]}}),
        );
        let e = err(render(
            &[],
            &[&f],
            &[],
            &cfg(json!({"ios": {"bundle-id": BID}})),
            &app(),
        ));
        assert!(
            e.contains("ios") && e.contains("cam") && e.contains("NSCameraUsageDescription"),
            "{e}"
        );
    }

    #[test]
    fn missing_description_in_extension_is_an_error() {
        let ext = json!({"name": "W", "bundle-suffix": "w", "deployment-target": "14.0", "sources": ["a"],
            "usage-descriptions": ["NSCameraUsageDescription"], "info-plist": {"NSExtension": {}}});
        let f = found(json!({"id": "w", "macos": {"extensions": [ext]}}));
        let e = err(render(&[], &[], &[&f], &cfg(json!(null)), &app()));
        assert!(
            e.contains("macos") && e.contains("NSCameraUsageDescription"),
            "{e}"
        );
    }

    #[test]
    fn app_config_rejects_bad_input() {
        for v in [
            json!({"ios": {"usage-descriptions": {"NSCameraUsageDescription": "  "}}}),
            json!({"ios": {"usage-descriptions": {"NSCameraUsageDescription": "a\nb"}}}),
            json!({"ios": {"usage-descriptions": {"Camera": "a"}}}),
            json!({"app-group-prefix": "com"}),
            json!({"app-group-prefix": "com.x y"}),
            json!({"ios": {"bundle-id": "com"}}),
            json!({"ios": {"bundle-id": "com.a b"}}),
            json!({"ios": {"bundle-id": "-com.example"}}),
            json!({"ios": {"bundle-id": "com.-example"}}),
            json!({"macos": {"bundle-id": "com"}}),
            json!({"macos": {"bundle-id": "-x.y"}}),
            json!({"app-group-prefix": "com.-x"}),
            json!({"ios": {"display-name": " "}}),
            json!({"macos": {"display-name": "a\tb"}}),
            json!({"ios": {"localizations": ["japanese"]}}),
            json!({"ios": {"localizations": ["ja", "ja"]}}),
            json!({"ios": {"localizations": ["ja_JP"]}}),
            json!({"ios": {"allows-local-networking": "yes"}}),
            json!({"unknown": 1}),
        ] {
            assert!(AppConfig::parse(&v).is_err(), "{v}");
        }
        assert!(
            AppConfig::parse(&json!({"ios": {"localizations": ["zh-Hans", "pt-BR", "en"]}}))
                .is_ok()
        );
        // セグメント中・末尾のハイフンと数字始まりは今までどおり通す。
        assert!(AppConfig::parse(
            &json!({"ios": {"bundle-id": "com.my-app.2nd-"}, "app-group-prefix": "com.a-b"})
        )
        .is_ok());
    }

    #[test]
    fn app_keys_reach_the_plist_and_features_cannot_override_them() {
        let ios = json!({"bundle-id": BID, "display-name": "表示名", "localizations": ["ja", "en"], "allows-local-networking": true});
        let files = render(&[], &[], &[], &cfg(json!({"ios": ios})), &app()).unwrap();
        let p = &files["ios/Info.plist"];
        assert!(
            p.contains("<key>CFBundleDisplayName</key>\n\t<string>表示名</string>"),
            "{p}"
        );
        assert!(
            p.contains("<string>ja</string>") && p.contains("<string>en</string>"),
            "{p}"
        );
        assert!(p.contains("<key>NSAllowsLocalNetworking</key>"), "{p}");
        // 書かなければ出さない。
        let files = render(
            &[],
            &[],
            &[],
            &cfg(json!({"ios": {"bundle-id": BID}})),
            &app(),
        )
        .unwrap();
        let p = &files["ios/Info.plist"];
        assert!(
            !p.contains("CFBundleDisplayName") && !p.contains("NSAppTransportSecurity"),
            "{p}"
        );
        // 機能の宣言はアプリ全体の値を上書きできない。
        let f = found(json!({"id": "l", "ios": {"info-plist": {"CFBundleLocalizations": ["fr"]}}}));
        let ios = json!({"bundle-id": BID, "localizations": ["ja"]});
        let e = err(render(&[], &[&f], &[], &cfg(json!({"ios": ios})), &app()));
        assert!(e.contains("CFBundleLocalizations"), "{e}");
    }

    #[test]
    fn macos_bundle_id_makes_the_main_plist_concrete() {
        let f = found(
            json!({"id": "s", "macos": {"info-plist": {"NSServices": [{"NSPortName": "$(EXECUTABLE_NAME)"}]}}}),
        );
        // bundle-id が無ければ Xcode の変数のまま。
        let files = render(
            &[],
            &[],
            &[&f],
            &cfg(json!({"ios": {"bundle-id": BID}})),
            &app(),
        )
        .unwrap();
        assert!(files["macos/Info.plist"].contains("$(PRODUCT_BUNDLE_IDENTIFIER)"));
        // bundle-id があれば確定値で、機能の宣言の $(EXECUTABLE_NAME) も実行体名になる。
        let c = cfg(json!({"ios": {"bundle-id": BID}, "macos": {"bundle-id": "com.example.mac"}}));
        let files = render(&[], &[], &[&f], &c, &app()).unwrap();
        let p = &files["macos/Info.plist"];
        assert!(!p.contains("$("), "{p}");
        assert!(p.contains("<string>com.example.mac</string>"), "{p}");
        assert!(
            p.contains("<key>NSPortName</key>\n\t\t\t<string>my-app-bin</string>"),
            "{p}"
        );
        // 機能が macOS に何も宣言しなくても、bundle-id があれば本体の Info.plist は出す（実行体へ埋め込むため）。
        assert!(render(&[], &[], &[], &c, &app())
            .unwrap()
            .contains_key("macos/Info.plist"));
        // 置き換えられない変数は拒否する（Xcode を通さないと解決されない）。
        let g =
            found(json!({"id": "t", "macos": {"info-plist": {"X": "$(TeamIdentifierPrefix)a"}}}));
        let e = err(render(&[], &[], &[&g], &c, &app()));
        assert!(e.contains("$(TeamIdentifierPrefix)"), "{e}");
    }

    #[test]
    fn app_groups_need_prefix_and_expand_per_os() {
        let f = found(
            json!({"id": "g", "ios": {"app-groups": ["widget"]}, "macos": {"app-groups": ["widget"]}}),
        );
        let ios = json!({"bundle-id": BID});
        let e = err(render(&[], &[&f], &[], &cfg(json!({"ios": ios})), &app()));
        assert!(e.contains("app-group-prefix"), "{e}");
        let c = cfg(json!({"app-group-prefix": "com.fandhe", "ios": ios}));
        let files = render(&[], &[&f], &[&f], &c, &app()).unwrap();
        assert!(files["ios/App.entitlements"].contains("<string>group.com.fandhe.widget</string>"));
        assert!(files["macos/App.entitlements"]
            .contains("<string>$(TeamIdentifierPrefix)com.fandhe.widget</string>"));
    }

    #[test]
    fn values_are_escaped_for_xml_plist_and_groovy() {
        let f = found(json!({"id": "e",
            "ios": {"usage-descriptions": ["NSCameraUsageDescription"]},
            "android": {"components": [{"kind": "service", "name": "a.B", "exported": false,
                "attributes": {"label": "x\"<&>y"}}], "gradle": ["g:a:1.0"]}}));
        let c = cfg(json!({"ios": {"bundle-id": BID,
            "usage-descriptions": {"NSCameraUsageDescription": "<b>&'\""}}}));
        let files = render(&[&f], &[&f], &[], &c, &app()).unwrap();
        assert!(files["ios/Info.plist"].contains("&lt;b&gt;&amp;&apos;&quot;"));
        assert!(files["android/AndroidManifest.xml"]
            .contains("android:label=\"x&quot;&lt;&amp;&gt;y\""));
        assert_eq!(groovy_str("a'b\\c"), "'a\\'b\\\\c'");
    }

    #[test]
    fn other_os_field_conflict_does_not_fail_target_os() {
        let a = found(
            json!({"id": "a", "android": {"permissions": [{"name": "p"}]}, "ios": {"info-plist": {"K": "1"}}}),
        );
        let b = found(
            json!({"id": "b", "android": {"permissions": [{"name": "q"}]}, "ios": {"info-plist": {"K": "2"}}}),
        );
        let files = render(&[&a, &b], &[], &[], &ios_cfg(), &app()).unwrap();
        assert!(files.contains_key("android/AndroidManifest.xml"));
    }

    #[test]
    fn info_plist_cannot_override_shell_values() {
        let f =
            found(json!({"id": "o", "ios": {"info-plist": {"UIApplicationSceneManifest": {}}}}));
        let e = err(render(&[], &[&f], &[], &ios_cfg(), &app()));
        assert!(e.contains("UIApplicationSceneManifest"), "{e}");
    }

    #[test]
    fn plist_rejects_null() {
        let f = found(json!({"id": "n", "ios": {"info-plist": {"K": null}}}));
        let e = err(render(
            &[],
            &[&f],
            &[],
            &cfg(json!({"ios": {"bundle-id": BID}})),
            &app(),
        ));
        assert!(e.contains("null"), "{e}");
    }

    fn ios_cfg() -> AppConfig {
        cfg(json!({"ios": {"bundle-id": BID}}))
    }

    #[test]
    fn shell_values_are_emitted_even_without_features() {
        let files = render(&[], &[], &[], &ios_cfg(), &app()).unwrap();
        let m = &files["android/AndroidManifest.xml"];
        assert!(m.contains("android:name=\"com.fandhe.platform.MainActivity\""));
        assert!(m.contains("android:exported=\"true\""));
        assert!(m.contains("android:launchMode=\"singleTask\""));
        assert!(m.contains("android:name=\"android.app.lib_name\" android:value=\"my_app\""));
        assert!(m.contains("android.intent.category.LAUNCHER"));
        let g = &files["android/fandhe.gradle"];
        assert!(g.contains("java.srcDir '/a/gen/android/java'"), "{g}");
        assert!(
            g.contains("addStaticManifestFile('/a/gen/android/AndroidManifest.xml')"),
            "{g}"
        );
        assert!(files[LIBRARY_CLASS_PATH].contains("NAME = \"my_app\""));
        let p = &files["ios/Info.plist"];
        assert!(!p.contains("$("), "{p}");
        for k in [
            "<string>my-app-bin</string>",
            "<string>com.example.app</string>",
            "<string>1.2.3</string>",
            "<string>17.0</string>",
            "<key>UILaunchScreen</key>",
            "<integer>2</integer>",
            "<key>UIApplicationSceneManifest</key>",
            "<key>UIWindowSceneSessionRoleApplication</key>",
            "<key>UIWindowSceneSessionRoleExternalDisplayNonInteractive</key>",
            "<string>FandheSceneDelegate</string>",
        ] {
            assert!(p.contains(k), "{k}");
        }
        assert!(!p.contains("CFBundleSupportedPlatforms"));
    }

    #[test]
    fn shell_inputs_are_validated() {
        let ok = ios_cfg();
        let with = |f: &dyn Fn(&mut AppInfo)| {
            let mut a = app();
            f(&mut a);
            err(render(&[], &[], &[], &ok, &a))
        };
        // cdylib が無いアプリは Android を対象にしない（Android の生成物を出さずに通る。Issue #1200）。
        let mut no_lib = app();
        no_lib.cdylib = None;
        let files = render(&[], &[], &[], &ok, &no_lib).expect("cdylib が無くても通る");
        assert!(!files.keys().any(|k| k.starts_with("android/")));
        assert!(with(&|a| a.bins.clear()).contains("bin target"));
        assert!(with(&|a| a.bins.push("b".into())).contains("bin target"));
        assert!(with(&|a| a.version = "1.0.0-rc.1".into()).contains("pre-release"));
        let e = err(render(&[], &[], &[], &cfg(json!(null)), &app()));
        assert!(e.contains("bundle-id"), "{e}");
    }

    #[test]
    fn write_refuses_unmarked_dir_and_replaces_marked_dir() {
        let root = std::env::temp_dir().join(format!("fandhe-native-write-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("gen")).unwrap();
        fs::write(root.join("gen/mine.txt"), "x").unwrap();
        let files = BTreeMap::from([
            (MARKER.to_string(), "m".to_string()),
            ("a/b.txt".into(), "1".into()),
        ]);
        assert!(write(&root.join("gen"), &files).is_err());
        assert!(root.join("gen/mine.txt").exists());
        fs::remove_dir_all(root.join("gen")).unwrap();
        write(&root.join("gen"), &files).unwrap();
        fs::write(root.join("gen/stale.txt"), "x").unwrap();
        write(&root.join("gen"), &files).unwrap();
        assert!(!root.join("gen/stale.txt").exists() && root.join("gen/a/b.txt").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn write_refuses_symlinked_marker() {
        let root =
            std::env::temp_dir().join(format!("fandhe-native-marker-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("gen")).unwrap();
        fs::write(root.join("target.txt"), "keep").unwrap();
        std::os::unix::fs::symlink(root.join("target.txt"), root.join("gen").join(MARKER)).unwrap();
        let files = BTreeMap::from([(MARKER.to_string(), "m".to_string())]);
        assert!(write(&root.join("gen"), &files).is_err());
        assert_eq!(fs::read_to_string(root.join("target.txt")).unwrap(), "keep");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn web_and_native_writes_keep_each_other() {
        let root = std::env::temp_dir().join(format!("fandhe-native-web-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let gen = root.join("gen");
        let m = (MARKER.to_string(), "m".to_string());
        let native = BTreeMap::from([m.clone(), ("android/a.txt".into(), "1".into())]);
        let web = BTreeMap::from([m, ("web/manifest.webmanifest".into(), "{}".into())]);
        // native -> web
        write(&gen, &native).unwrap();
        write(&gen, &web).unwrap();
        assert!(
            gen.join("android/a.txt").exists() && gen.join("web/manifest.webmanifest").exists()
        );
        // web -> native（古いネイティブ成果物は消え、web は残る）
        fs::write(gen.join("stale.txt"), "x").unwrap();
        write(&gen, &native).unwrap();
        assert!(!gen.join("stale.txt").exists() && gen.join("web/manifest.webmanifest").exists());
        // web 単独が先でも後続のネイティブが拒否されない
        fs::remove_dir_all(&gen).unwrap();
        write(&gen, &web).unwrap();
        write(&gen, &native).unwrap();
        assert!(
            gen.join("android/a.txt").exists() && gen.join("web/manifest.webmanifest").exists()
        );
        let _ = fs::remove_dir_all(&root);
    }

    // ---- アプリの追記（info-plist・entitlements、#1202）----

    fn ios_with(extra: Value) -> AppConfig {
        let mut ios = json!({"bundle-id": BID});
        ios.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        cfg(json!({"ios": ios}))
    }

    #[test]
    fn app_info_plist_emits_all_six_types_with_escaping() {
        let c = ios_with(json!({"info-plist": {
            "S": "<&>'\"", "B": false, "I": 3, "R": 1.5,
            "A": ["x", 2], "T": {"in<k>": {"deep": true}},
            "k<&>": "v"
        }}));
        let p = &render(&[], &[], &[], &c, &app()).unwrap()["ios/Info.plist"];
        for k in [
            "<key>S</key>\n\t<string>&lt;&amp;&gt;&apos;&quot;</string>",
            "<key>B</key>\n\t<false/>",
            "<key>I</key>\n\t<integer>3</integer>",
            "<key>R</key>\n\t<real>1.5</real>",
            "<key>A</key>\n\t<array>\n\t\t<string>x</string>\n\t\t<integer>2</integer>\n\t</array>",
            "<key>in&lt;k&gt;</key>",
            "<key>deep</key>\n\t\t\t<true/>",
            "<key>k&lt;&amp;&gt;</key>",
        ] {
            assert!(p.contains(k), "{k}\n{p}");
        }
    }

    #[test]
    fn app_info_plist_unions_arrays_and_recurses_into_shell_tables() {
        let f = found(json!({"id": "bg", "ios": {"info-plist": {"UIBackgroundModes": ["fetch"]}}}));
        let c = ios_with(json!({
        "allows-local-networking": true,
        "info-plist": {
            "UIBackgroundModes": ["audio", "fetch"],
            "UILaunchScreen": {"UIColorName": "Bg"},
            "NSAppTransportSecurity": {"NSAllowsArbitraryLoadsInWebContent": true},
        }}));
        let p = &render(&[], &[&f], &[], &c, &app()).unwrap()["ios/Info.plist"];
        assert_eq!(p.matches("<string>fetch</string>").count(), 1, "{p}");
        assert!(p.contains("<string>audio</string>"), "{p}");
        assert!(p.contains("<key>UIColorName</key>"), "{p}");
        // allows-local-networking（殻の値）と同じテーブルへ足せる。
        assert!(p.contains("<key>NSAllowsLocalNetworking</key>"), "{p}");
        assert!(
            p.contains("<key>NSAllowsArbitraryLoadsInWebContent</key>"),
            "{p}"
        );
        // 同値は 1 つ。
        let c = ios_with(json!({"info-plist": {"CFBundleName": "my-app"}}));
        assert!(render(&[], &[], &[], &c, &app()).is_ok());
    }

    #[test]
    fn app_info_plist_conflicts_are_errors_with_leaf_and_values() {
        // 殻のスカラー。
        let c = ios_with(json!({"info-plist": {"CFBundleName": "other"}}));
        let e = err(render(&[], &[], &[], &c, &app()));
        assert!(
            e.contains("CFBundleName") && e.contains("\"my-app\"") && e.contains("\"other\""),
            "{e}"
        );
        // 殻のテーブルの中の葉。
        let c = ios_with(json!({"info-plist": {"UIApplicationSceneManifest":
            {"UIApplicationSupportsMultipleScenes": true}}}));
        let e = err(render(&[], &[], &[], &c, &app()));
        assert!(
            e.contains("UIApplicationSceneManifest.UIApplicationSupportsMultipleScenes")
                && e.contains("false")
                && e.contains("true"),
            "{e}"
        );
        // 機能のスカラー。
        let f = found(json!({"id": "k", "ios": {"info-plist": {"K": "1"}}}));
        let c = ios_with(json!({"info-plist": {"K": "2"}}));
        let e = err(render(&[], &[&f], &[], &c, &app()));
        assert!(
            e.contains("K") && e.contains("\"1\"") && e.contains("\"2\""),
            "{e}"
        );
        // 型違い（配列とスカラー・テーブルとスカラー）。
        let c = ios_with(json!({"info-plist": {"UIDeviceFamily": "1", "UILaunchScreen": 1}}));
        let e = err(render(&[], &[], &[], &c, &app()));
        assert!(
            e.contains("UIDeviceFamily") && e.contains("UILaunchScreen"),
            "{e}"
        );
    }

    #[test]
    fn app_plist_inputs_are_rejected() {
        let bad = |v: Value| {
            AppConfig::parse(&json!({"ios": v}))
                .expect_err("拒否されるはず")
                .join("\n")
        };
        let e = bad(json!({"info-plist": {"K": "a\nb"}}));
        assert!(e.contains("制御文字"), "{e}");
        let e = bad(json!({"entitlements": {APP_GROUPS_KEY: ["g"]}}));
        assert!(e.contains("から生成されるため"), "{e}");
        // 説明文は文字列で、空白だけでない（`usage-descriptions` と同じ基準。修飾子つきも）。
        for v in [json!("  "), json!(3), json!(["a"])] {
            for k in ["NSFaceIDUsageDescription", "NSFaceIDUsageDescription~ipad"] {
                let e = bad(json!({"info-plist": {k: v}}));
                assert!(e.contains("空白だけでない文字列"), "{k} {v}: {e}");
            }
        }
        // 表示名・説明文の文言にも XML 1.0 で不正な文字の拒否を揃える。
        assert!(AppConfig::parse(&json!({"ios": {"display-name": "a\u{FFFF}"}})).is_err());
        assert!(AppConfig::parse(
            &json!({"ios": {"usage-descriptions": {"NSCameraUsageDescription": "a\u{FFFE}"}}})
        )
        .is_err());
        // XML 1.0 で不正な文字。
        let e = bad(json!({"info-plist": {"K": "a\u{FFFE}"}}));
        assert!(e.contains("制御文字"), "{e}");
        let e = bad(json!({"entitlements": {"a\u{FFFF}": true}}));
        assert!(e.contains("制御文字"), "{e}");
        // cargo metadata の日時（`$__toml_private_datetime`）。入れ子の中でも拒否する。
        let dt = json!({"$__toml_private_datetime": "1979-05-27"});
        let e = bad(json!({"info-plist": {"When": dt, "N": {"X": [dt]}}}));
        assert!(
            e.contains("info-plist.When") && e.contains("info-plist.N.X[0]"),
            "{e}"
        );
        let e = bad(json!({"entitlements": {"When": dt}}));
        assert!(e.contains("entitlements.When") && e.contains("日時"), "{e}");
        // null は直列化で拒否する。
        let c = ios_with(json!({"info-plist": {"K": null}}));
        assert!(err(render(&[], &[], &[], &c, &app())).contains("null"));
        // 確定値の Info.plist に Xcode の変数が残る。
        let c = ios_with(json!({"info-plist": {"K": "$(PRODUCT_NAME)"}}));
        assert!(err(render(&[], &[], &[], &c, &app())).contains("$(PRODUCT_NAME)"));
        // 追記の $(EXECUTABLE_NAME) は実行体名になる。
        let c = ios_with(json!({"info-plist": {"K": "$(EXECUTABLE_NAME)"}}));
        let p = &render(&[], &[], &[], &c, &app()).unwrap()["ios/Info.plist"];
        assert!(p.contains("<string>my-app-bin</string>"), "{p}");
    }

    #[test]
    fn app_entitlements_alone_emit_the_file_and_merge_with_features() {
        let c = ios_with(json!({"entitlements": {"aps-environment": "development"}}));
        let files = render(&[], &[], &[], &c, &app()).unwrap();
        let e = &files["ios/App.entitlements"];
        assert!(e.contains("<key>aps-environment</key>"), "{e}");
        // 追記が無ければ出さない（従来どおり）。
        assert!(!render(&[], &[], &[], &ios_cfg(), &app())
            .unwrap()
            .contains_key("ios/App.entitlements"));
        // 機能の entitlements と App Group は残り、食い違いはエラー。
        let f = found(json!({"id": "g", "ios": {"app-groups": ["w"],
            "entitlements": {"x.shared": ["a"], "x.flag": true}}}));
        let c = cfg(
            json!({"app-group-prefix": "com.fandhe", "ios": {"bundle-id": BID,
            "entitlements": {"x.shared": ["b"], "aps-environment": "production"}}}),
        );
        let e = &render(&[], &[&f], &[], &c, &app()).unwrap()["ios/App.entitlements"];
        for k in [
            "group.com.fandhe.w",
            "<string>a</string>",
            "<string>b</string>",
            "production",
        ] {
            assert!(e.contains(k), "{k}\n{e}");
        }
        let c = cfg(json!({"ios": {"bundle-id": BID, "entitlements": {"x.flag": false}}}));
        let e = err(render(&[], &[&f], &[], &c, &app()));
        assert!(
            e.contains("x.flag") && e.contains("true") && e.contains("false"),
            "{e}"
        );
    }

    #[test]
    fn macos_app_append_emits_without_features_or_bundle_id() {
        let none = cfg(json!({"ios": {"bundle-id": BID}, "macos": {}}));
        assert!(!render(&[], &[], &[], &none, &app())
            .unwrap()
            .keys()
            .any(|k| k.starts_with("macos/")));
        let c =
            cfg(json!({"ios": {"bundle-id": BID}, "macos": {"info-plist": {"LSUIElement": true}}}));
        let files = render(&[], &[], &[], &c, &app()).unwrap();
        assert!(files["macos/Info.plist"].contains("<key>LSUIElement</key>"));
        assert!(files["macos/Info.plist"].contains("$(PRODUCT_BUNDLE_IDENTIFIER)"));
        assert!(!files.contains_key("macos/App.entitlements"));
        let c = cfg(json!({"ios": {"bundle-id": BID},
            "macos": {"entitlements": {"com.apple.security.app-sandbox": true}}}));
        let files = render(&[], &[], &[], &c, &app()).unwrap();
        assert!(files["macos/App.entitlements"].contains("com.apple.security.app-sandbox"));
        // bundle-id があれば殻の確定値へ足す。
        let c = cfg(
            json!({"ios": {"bundle-id": BID}, "macos": {"bundle-id": "com.example.mac",
            "info-plist": {"LSUIElement": true, "CFBundleName": "x"}}}),
        );
        let e = err(render(&[], &[], &[], &c, &app()));
        assert!(e.contains("CFBundleName"), "{e}");
    }

    #[test]
    fn app_append_does_not_touch_extension_plists() {
        let ext = json!({"name": "W", "bundle-suffix": "w", "deployment-target": "14.0", "sources": ["a"],
            "info-plist": {"NSExtension": {}}});
        let f = found(json!({"id": "w", "macos": {"extensions": [ext]}}));
        let plain = cfg(json!({"ios": {"bundle-id": BID}, "macos": {}}));
        let with = cfg(
            json!({"ios": {"bundle-id": BID}, "macos": {"info-plist": {"LSUIElement": true},
            "entitlements": {"e": true}}}),
        );
        let a = render(&[], &[], &[&f], &plain, &app()).unwrap();
        let b = render(&[], &[], &[&f], &with, &app()).unwrap();
        let key = "macos/extensions/W/Info.plist";
        assert_eq!(a[key], b[key]);
        assert!(!b[key].contains("LSUIElement"));
        assert!(!a.contains_key("macos/App.entitlements"));
        assert!(b.contains_key("macos/App.entitlements"));
        assert!(!b.keys().any(|k| k.ends_with("W.entitlements")));
    }

    #[test]
    fn app_info_plist_accepts_usage_descriptions_and_conflicts_with_feature_text() {
        let cam = found(
            json!({"id": "cam", "ios": {"usage-descriptions": ["NSCameraUsageDescription"]}}),
        );
        // 機能が要求しないキーは、そのまま Info.plist に出る。
        let c = ios_with(json!({"info-plist": {"NSFaceIDUsageDescription": "顔で解除します"}}));
        let p = &render(&[], &[], &[], &c, &app()).unwrap()["ios/Info.plist"];
        assert!(
            p.contains("<key>NSFaceIDUsageDescription</key>\n\t<string>顔で解除します</string>"),
            "{p}"
        );
        // 機能が要求したキーの文言と食い違えばエラー（値は黙って消えない）。
        let c = ios_with(json!({
            "usage-descriptions": {"NSCameraUsageDescription": "A"},
            "info-plist": {"NSCameraUsageDescription": "B"}}));
        let e = err(render(&[], &[&cam], &[], &c, &app()));
        assert!(
            e.contains("fandhe-app.ios.info-plist.NSCameraUsageDescription")
                && e.contains("\"A\"")
                && e.contains("\"B\""),
            "{e}"
        );
        // 同じ文言なら 1 つにまとまる。
        let c = ios_with(json!({
            "usage-descriptions": {"NSCameraUsageDescription": "A"},
            "info-plist": {"NSCameraUsageDescription": "A"}}));
        let p = &render(&[], &[&cam], &[], &c, &app()).unwrap()["ios/Info.plist"];
        assert_eq!(p.matches("NSCameraUsageDescription").count(), 1, "{p}");
    }

    #[test]
    fn app_info_plist_qualified_keys_follow_the_base_key() {
        // 素のキーが生成値にあれば、修飾子つきは生成値を黙って上書きするのでエラー。
        for k in [
            "CFBundleName~ipad",
            "UIApplicationSceneManifest~ipad",
            "CFBundleName-iphoneos",
        ] {
            let c = ios_with(json!({"info-plist": {k: "x"}}));
            let e = err(render(&[], &[], &[], &c, &app()));
            assert!(
                e.contains(&format!("fandhe-app.ios.info-plist.{k}")) && e.contains("素のキー"),
                "{k}: {e}"
            );
        }
        // 素のキーが生成値に無ければ通る。
        let c = ios_with(
            json!({"info-plist": {"UISupportedInterfaceOrientations~ipad": ["UIInterfaceOrientationPortrait"]}}),
        );
        let p = &render(&[], &[], &[], &c, &app()).unwrap()["ios/Info.plist"];
        assert!(
            p.contains("<key>UISupportedInterfaceOrientations~ipad</key>"),
            "{p}"
        );
    }
}
