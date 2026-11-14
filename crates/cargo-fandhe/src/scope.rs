//! 選ばなかった機能の宣言が成果物に混ざっていないことの検査（`cargo fandhe verify-scope`、Issue #1178）。
//!
//! 禁止集合 = カタログ（全機能の宣言）−（アプリが到達した宣言）。成果物から外部ツール
//! （aapt2・codesign・otool・Gradle 等）で取り出したテキストに、禁止集合の値が 1 つでも
//! 現れたら違反とする。ツールの実行は `scripts/audit/verify-capability-scope.sh` の担当で、
//! 本モジュールはテキストの照合だけを行う（ネットワーク・外部コマンドは `cargo metadata` のみ）。
//! 到達した宣言と値が重なるもの（複数機能が同じ権限を宣言する等）は、正当に現れるため禁止にしない。
//! 宣言から禁止集合を作るので、機能が増えても検査側の修正は要らない。
//! 呼び出し元は `main.rs` と統合テスト。範囲外: 権限のフラグ（権限名が対象で、フラグ名は他の権限にも正当に現れる）、main-activity の属性・intent-filter（他 library の
//! manifest にも正当に現れるため。漏れは機能のコンポーネント名で検出できる）、Windows、AAB。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::declaration::{Apple, Declaration};
use crate::native::{ANDROID_TRIPLE, APP_GROUPS_KEY, IOS_TRIPLE, MACOS_TRIPLE};
use crate::reach::reachable;
use crate::workspace::{collect, run_metadata};

/// 検査結果。`unselected` が 0 なら検査対象なし（成功）。
#[derive(Debug, PartialEq)]
pub struct ScopeReport {
    pub unselected: usize,
    pub forbidden: usize,
}

/// 値の持ち主（機能 id）と種別。違反メッセージに使う。
type Owner = (String, &'static str);

/// 機能固有でなく、クリーンな成果物にも OS・署名が付ける entitlement。禁止集合に入れない。
const GENERIC_KEYS: &[&str] = &["com.apple.security.app-sandbox"];

fn apple_tokens(a: &Apple, out: &mut Vec<(&'static str, String)>) {
    let groups = |g: &[String], out: &mut Vec<(&'static str, String)>| {
        if !g.is_empty() {
            out.push(("entitlement", APP_GROUPS_KEY.to_string()));
        }
    };
    out.extend(
        a.usage_descriptions
            .iter()
            .map(|v| ("説明文キー", v.clone())),
    );
    out.extend(a.frameworks.iter().map(|v| ("framework", v.clone())));
    out.extend(a.entitlements.keys().map(|v| ("entitlement", v.clone())));
    out.extend(a.info_plist.keys().map(|v| ("Info.plist キー", v.clone())));
    groups(&a.app_groups, out);
    for e in &a.extensions {
        // 成果物では `PlugIns/<name>.appex` として現れる（`.` 続きは境界外なので拡張子まで含める）。
        out.push(("拡張ターゲット", format!("{}.appex", e.name)));
        out.extend(
            e.usage_descriptions
                .iter()
                .map(|v| ("説明文キー", v.clone())),
        );
        out.extend(e.frameworks.iter().map(|v| ("framework", v.clone())));
        out.extend(e.entitlements.keys().map(|v| ("entitlement", v.clone())));
        out.extend(e.info_plist.keys().map(|v| ("Info.plist キー", v.clone())));
        groups(&e.app_groups, out);
    }
}

/// 1 機能の宣言が成果物に残す値（OS ごと）。
fn tokens(d: &Declaration, os: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    match os {
        "android" => {
            if let Some(a) = &d.android {
                out.extend(a.permissions.iter().map(|p| ("権限", p.name.clone())));
                out.extend(a.features.iter().map(|f| ("uses-feature", f.name.clone())));
                out.extend(
                    a.application_meta_data
                        .iter()
                        .map(|m| ("application の meta-data", m.name.clone())),
                );
                out.extend(
                    a.components
                        .iter()
                        .map(|c| ("コンポーネント", c.name.clone())),
                );
                // `group:artifact:version` のうち版は照合に含めない。
                out.extend(a.gradle.iter().map(|g| {
                    let ga: Vec<&str> = g.splitn(3, ':').take(2).collect();
                    ("Gradle 依存", ga.join(":"))
                }));
            }
        }
        "ios" => d.ios.iter().for_each(|a| apple_tokens(a, &mut out)),
        _ => d.macos.iter().for_each(|a| apple_tokens(a, &mut out)),
    }
    out
}

/// 禁止集合: カタログのうち到達しなかった機能の値から、到達した機能が宣言する値を引く。
pub(crate) fn forbidden_tokens(
    reached: &[&Declaration],
    catalog: &[&Declaration],
    os: &str,
) -> BTreeMap<String, Owner> {
    let allowed: BTreeSet<String> = reached
        .iter()
        .flat_map(|d| tokens(d, os))
        .map(|(_, v)| v)
        .collect();
    let reached_ids: BTreeSet<&str> = reached.iter().map(|d| d.id.as_str()).collect();
    let mut out = BTreeMap::new();
    for d in catalog
        .iter()
        .filter(|d| !reached_ids.contains(d.id.as_str()))
    {
        for (kind, v) in tokens(d, os) {
            if !allowed.contains(&v) && !GENERIC_KEYS.contains(&v.as_str()) {
                out.entry(v).or_insert((d.id.clone(), kind));
            }
        }
    }
    out
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-')
}

/// `text` に `needle` がトークン境界つきで現れるか。前は英数字・`_`・`-`・`.` でなく、
/// 後は英数字・`_`・`-` でなく、かつ `.` + 英数字（`camera` に対する `camera.autofocus`）でもない。
fn contains_token(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let mut after = text[i + needle.len()..].chars();
        let a1 = after.next();
        let a2 = after.next();
        before.is_none_or(|c| !is_word(c) && c != '.')
            && a1.is_none_or(|c| !is_word(c) && !(c == '.' && a2.is_some_and(is_word)))
    })
}

/// 観測テキストを禁止集合で照合し、違反を 1 件ずつ返す。
pub(crate) fn scan(forbidden: &BTreeMap<String, Owner>, observed: &str) -> Vec<String> {
    forbidden
        .iter()
        .filter(|(v, _)| contains_token(observed, v))
        .map(|(v, (id, kind))| {
            format!("選ばなかった機能 {id} の{kind} `{v}` が成果物に含まれています")
        })
        .collect()
}

/// `catalog`（全機能を含む workspace の Cargo.toml。省略時はカレント）と `app`（アプリの Cargo.toml。
/// 省略時はカレント）から禁止集合を作り、`observed`（成果物から取り出したテキスト）を照合する。
/// `os` は `android` / `ios` / `macos`。違反・入力不正は全件を文字列で返す。
pub fn verify_scope(
    catalog: Option<&Path>,
    app: Option<&Path>,
    os: &str,
    observed: &str,
) -> Result<ScopeReport, Vec<String>> {
    let triple = match os {
        "android" => ANDROID_TRIPLE,
        "ios" => IOS_TRIPLE,
        "macos" => MACOS_TRIPLE,
        _ => return Err(vec![format!("未知の OS `{os}`（android / ios / macos）")]),
    };
    let all = collect(&run_metadata(catalog, None)?)?;
    let meta = run_metadata(app, Some(triple))?;
    if meta["resolve"]["root"].as_str().is_none() {
        return Err(vec![
            "アプリの Cargo.toml を指定してください（virtual workspace では到達集合を作れません）"
                .into(),
        ]);
    }
    let reach = reachable(&meta);
    let app_found = collect(&meta)?;
    let reached: Vec<&Declaration> = app_found
        .iter()
        .filter(|f| reach.contains(&f.pkg_id))
        .map(|f| &f.decl)
        .collect();
    let catalog_decls: Vec<&Declaration> = all.iter().map(|f| &f.decl).collect();
    let reached_ids: BTreeSet<&str> = reached.iter().map(|d| d.id.as_str()).collect();
    let unselected = catalog_decls
        .iter()
        .filter(|d| !reached_ids.contains(d.id.as_str()))
        .count();
    if unselected == 0 {
        return Ok(ScopeReport {
            unselected: 0,
            forbidden: 0,
        });
    }
    if observed.trim().is_empty() {
        return Err(vec![
            "観測テキストが空です（ツールの失敗を見逃さないため失敗にします）".into(),
        ]);
    }
    let forbidden = forbidden_tokens(&reached, &catalog_decls, os);
    let hits = scan(&forbidden, observed);
    if hits.is_empty() {
        Ok(ScopeReport {
            unselected,
            forbidden: forbidden.len(),
        })
    } else {
        Err(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実 workspace の宣言で、camera-capture だけを選んだ場合の禁止集合。
    #[test]
    fn real_declarations_forbid_only_unselected_values() {
        let m = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
        let decls = crate::check_workspace(Some(&m)).unwrap();
        let cat: Vec<&Declaration> = decls
            .iter()
            .filter(|d| {
                [
                    "camera-capture",
                    "document-scan",
                    "nfc-card-emulation",
                    "share-receive",
                    "home-widget",
                ]
                .contains(&d.id.as_str())
            })
            .collect();
        assert_eq!(cat.len(), 5);
        let reached: Vec<&Declaration> = cat
            .iter()
            .copied()
            .filter(|d| d.id == "camera-capture")
            .collect();
        let android = forbidden_tokens(&reached, &cat, "android");
        assert!(android.contains_key("android.permission.NFC"));
        assert!(!android.contains_key("android.permission.CAMERA"));
        assert!(forbidden_tokens(&reached, &cat, "ios").contains_key("VisionKit"));
    }

    #[test]
    fn token_boundary() {
        assert!(contains_token(
            "name=\"android.hardware.camera\"",
            "android.hardware.camera"
        ));
        assert!(!contains_token(
            "android.hardware.camera.autofocus",
            "android.hardware.camera"
        ));
        assert!(contains_token(
            "android.hardware.camera.",
            "android.hardware.camera"
        ));
        assert!(contains_token("x:a:b:1.0", "a:b"));
        assert!(!contains_token("a:b-ktx:1.0", "a:b"));
        assert!(!contains_token("xa:b", "a:b"));
        assert!(contains_token(
            "/Frameworks/VisionKit.framework/VisionKit",
            "VisionKit"
        ));
        assert!(!contains_token("", "VisionKit"));
    }
}
