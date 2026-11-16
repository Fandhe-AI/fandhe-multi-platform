//! 選ばなかった機能の宣言が成果物に混ざっていないことの検査（`cargo fandhe verify-scope`、Issue #1178）。
//!
//! 禁止集合 = カタログ（全機能の宣言）−（アプリが到達した宣言）。成果物から外部ツール
//! （aapt2・codesign・otool・Gradle 等）で取り出したテキストに、禁止集合の値が 1 つでも
//! 現れたら違反とする。ツールの実行は `scripts/audit/verify-capability-scope.sh` の担当で、
//! 本モジュールはテキストの照合だけを行う（ネットワーク・外部コマンドは `cargo metadata` のみ）。
//! 到達した宣言と値が重なるもの（複数機能が同じ権限を宣言する等）は、正当に現れるため禁止にしない。
//! 宣言から禁止集合を作るので、機能が増えても検査側の修正は要らない。
//! Android は加えて aapt2 の xmltree を要素単位で読み、カタログの権限の `usesPermissionFlags`（ビット値）と
//! MainActivity の許可リスト属性（`MAIN_ACTIVITY_ATTRIBUTES`）を、到達した宣言の値と照合する（フラグ名・属性名は
//! 他の権限・他 library にも正当に現れるため、トークン照合では扱えない）。
//! 呼び出し元は `main.rs` と統合テスト。範囲外: main-activity の intent-filter・queries（他 library の
//! manifest にも正当に現れるため。漏れは機能のコンポーネント名で検出できる）、カタログ外の権限のフラグ、Windows、AAB。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::declaration::{Apple, Declaration, MAIN_ACTIVITY_ATTRIBUTES};
use crate::native::{ANDROID_TRIPLE, APP_GROUPS_KEY, IOS_TRIPLE, MACOS_TRIPLE, MAIN_ACTIVITY};
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

/// aapt2 xmltree の 1 要素（タグと、直下の属性の名前 → 値）。入れ子は追わない（必要なのは直下の属性だけ）。
struct Element {
    tag: String,
    attrs: BTreeMap<String, String>,
}

/// `aapt2 dump xmltree` の `E: <tag>` / `A: <ns:name(0x…)>=<値>` 行を平坦な要素列にする。他の行は無視する。
fn android_elements(observed: &str) -> Vec<Element> {
    let mut out: Vec<Element> = Vec::new();
    for line in observed.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("E: ") {
            let tag = rest.split_whitespace().next().unwrap_or("").to_string();
            out.push(Element {
                tag,
                attrs: BTreeMap::new(),
            });
        } else if let (Some(rest), Some(el)) = (line.strip_prefix("A: "), out.last_mut()) {
            let Some((lhs, rhs)) = rest.split_once('=') else {
                continue;
            };
            let lhs = lhs.split('(').next().unwrap_or(lhs);
            let name = lhs.rsplit(':').next().unwrap_or(lhs).to_string();
            let val = match rhs.strip_prefix('"') {
                Some(r) => r.split('"').next().unwrap_or(""),
                None => rhs.split_whitespace().next().unwrap_or(""),
            };
            el.attrs.insert(name, val.to_string());
        }
    }
    out
}

/// aapt2 が出す数値（`0x…` 16 進または 10 進）を u32 にする。
fn parse_u32(v: &str) -> Option<u32> {
    match v.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None => v.parse().ok(),
    }
}

/// Android の構造照合: カタログの権限のフラグと MainActivity の許可リスト属性が、到達した宣言の値に収まるか。
/// 到達した宣言の値が成果物に無いこと（gen の正しさ）は検査しない。
/// ponytail: 属性値は文字列のまま比較する（許可リストが boolean だけのため）。boolean 以外が増えたら見直す。
fn android_structure(
    reached: &[&Declaration],
    catalog: &[&Declaration],
    observed: &str,
) -> Vec<String> {
    let reached_ids: BTreeSet<&str> = reached.iter().map(|d| d.id.as_str()).collect();
    let perms = |ds: &[&Declaration]| -> Vec<(String, String, crate::declaration::PermissionFlag)> {
        ds.iter()
            .filter_map(|d| d.android.as_ref().map(|a| (d, a)))
            .flat_map(|(d, a)| {
                a.permissions.iter().flat_map(|p| {
                    p.flags
                        .iter()
                        .map(|f| (d.id.clone(), p.name.clone(), *f))
                        .collect::<Vec<_>>()
                })
            })
            .collect()
    };
    let unreached: Vec<&Declaration> = catalog
        .iter()
        .copied()
        .filter(|d| !reached_ids.contains(d.id.as_str()))
        .collect();
    let catalog_perms: BTreeSet<&str> = catalog
        .iter()
        .filter_map(|d| d.android.as_ref())
        .flat_map(|a| a.permissions.iter().map(|p| p.name.as_str()))
        .collect();
    let reached_flags = perms(reached);
    let unreached_flags = perms(&unreached);
    let mut hits = Vec::new();
    let els = android_elements(observed);
    for el in els.iter().filter(|e| e.tag == "uses-permission") {
        let Some(name) = el
            .attrs
            .get("name")
            .filter(|n| catalog_perms.contains(n.as_str()))
        else {
            continue;
        };
        let Some(raw) = el.attrs.get("usesPermissionFlags") else {
            continue;
        };
        let Some(bits) = parse_u32(raw) else {
            hits.push(format!(
                "権限 {name} のフラグ `{raw}` を数値として読めません"
            ));
            continue;
        };
        let allowed = reached_flags
            .iter()
            .filter(|(_, p, _)| p == name)
            .fold(0, |a, (_, _, f)| a | f.bit());
        let extra = bits & !allowed;
        if extra == 0 {
            continue;
        }
        let owners: Vec<_> = unreached_flags
            .iter()
            .filter(|(_, p, f)| p == name && extra & f.bit() != 0)
            .collect();
        for (id, _, f) in &owners {
            hits.push(format!(
                "選ばなかった機能 {id} の権限フラグ `{}`（{name}）が成果物に含まれています",
                f.as_str()
            ));
        }
        let known = owners.iter().fold(0, |a, (_, _, f)| a | f.bit());
        if extra & !known != 0 {
            hits.push(format!(
                "権限 {name} のフラグ `0x{:08x}` は選んだ機能の宣言にありません",
                extra & !known
            ));
        }
    }
    let mains: Vec<&Element> = els
        .iter()
        .filter(|e| e.tag == "activity" && e.attrs.get("name").is_some_and(|n| n == MAIN_ACTIVITY))
        .collect();
    if mains.is_empty() {
        hits.push(format!(
            "MainActivity（{MAIN_ACTIVITY}）が観測テキストにありません（aapt2 dump xmltree の出力を渡してください）"
        ));
    }
    let attrs = |ds: &[&Declaration]| -> Vec<(String, String, String)> {
        ds.iter()
            .filter_map(|d| d.android.as_ref().map(|a| (d, a)))
            .flat_map(|(d, a)| {
                a.main_activity_attributes
                    .iter()
                    .map(|(k, v)| (d.id.clone(), k.clone(), v.clone()))
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let reached_attrs = attrs(reached);
    let unreached_attrs = attrs(&unreached);
    for el in mains {
        for (key, _) in MAIN_ACTIVITY_ATTRIBUTES {
            let Some(v) = el.attrs.get(*key) else {
                continue;
            };
            if reached_attrs.iter().any(|(_, k, rv)| k == key && rv == v) {
                continue;
            }
            match unreached_attrs.iter().find(|(_, k, uv)| k == key && uv == v) {
                Some((id, _, _)) => hits.push(format!(
                    "選ばなかった機能 {id} の MainActivity 属性 `{key}=\"{v}\"` が成果物に含まれています"
                )),
                None => hits.push(format!(
                    "MainActivity の属性 `{key}=\"{v}\"` は選んだ機能の宣言にありません"
                )),
            }
        }
    }
    hits
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
    let mut hits = scan(&forbidden, observed);
    if os == "android" {
        hits.extend(android_structure(&reached, &catalog_decls, observed));
    }
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
    fn parses_aapt2_xmltree() {
        let t = "      E: uses-permission (line=157)\n        A: http://schemas.android.com/apk/res/android:name(0x01010003)=\"android.permission.BLUETOOTH_SCAN\" (Raw: \"android.permission.BLUETOOTH_SCAN\")\n        A: http://schemas.android.com/apk/res/android:usesPermissionFlags(0x01010644)=0x00010000\n      E: activity (line=9)\n        A: http://schemas.android.com/apk/res/android:supportsPictureInPicture(0x010104f7)=true\n+--- x:y:1\n";
        let els = android_elements(t);
        assert_eq!(els.len(), 2);
        assert_eq!(els[0].tag, "uses-permission");
        assert_eq!(els[0].attrs["name"], "android.permission.BLUETOOTH_SCAN");
        assert_eq!(
            parse_u32(&els[0].attrs["usesPermissionFlags"]),
            Some(0x10000)
        );
        assert_eq!(els[1].attrs["supportsPictureInPicture"], "true");
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
