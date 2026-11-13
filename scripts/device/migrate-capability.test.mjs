/**
 * migrate-capability.mjs の自己検査（Issue #1183）。純関数（名前変換・移動先・書き換え・目印ブロック）だけを確かめる。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  checkId, stemOf, pascalOf, moveTargets, rewriteSource, usedErrorVariants, unmappedVariants,
  renderErrorRs, replaceBlock, featureLines, gradleLines, renderUnsafeAttr, renderBuildRs, unsafeOsesOf,
  strErrorVariants, rewriteEdition2024, stripRustLiterals, refPatterns, missingEntry,
} from "./migrate-capability.mjs";

test("checkId: 不正な id を拒む", () => {
  assert.equal(checkId("device-identifier"), "device-identifier");
  for (const bad of ["", "A-b", "a--b", "../x", "a b", "a;b", undefined]) assert.throws(() => checkId(bad));
});

test("stem と Pascal", () => {
  assert.equal(stemOf("device-identifier"), "device_identifier");
  assert.equal(pascalOf("device_identifier"), "DeviceIdentifier");
  assert.equal(pascalOf("nfc-card-emulation"), "NfcCardEmulation");
});

test("moveTargets: 契約・ドライバ・OS・Java の移動先", () => {
  const m = Object.fromEntries(moveTargets("device-identifier", "device_identifier"));
  const c = "crates/fandhe-cap-device-identifier";
  assert.equal(m["crates/fandhe-device/src/device_identifier.rs"], `${c}/src/contract.rs`);
  assert.equal(m["crates/fandhe-egui-proxy/src/device/device_identifier.rs"], `${c}/src/driver.rs`);
  assert.equal(m["crates/fandhe-egui-proxy/src/device/ios_device_identifier.rs"], `${c}/src/ios.rs`);
  assert.ok(m["platforms/android/app/src/main/java/com/fandhe/platform/DeviceIdentifierHelper.java"].endsWith("/DeviceIdentifierHelper.java"));
});

test("rewriteSource: 共通部品・エラー型・契約パスの置換", () => {
  const out = rewriteSource(
    [
      "use crate::app::CompletionSender;",
      "use fandhe_device::DeviceError;",
      "use crate::DeviceError;",
      "use fandhe_device::device_identifier::{DeviceId};",
      "use crate::device::android::JniCallFailed;",
      'Err(DeviceError::Internal("x"))',
      "Err(DeviceError::Unsupported(_)) => 1,",
      "crate::android::android_app()",
      "crate::ios::request_repaint();",
    ].join("\n"),
    "device-identifier",
  );
  assert.equal(
    out,
    [
      "use fandhe_runtime::CompletionSender;",
      "use crate::error::DeviceIdentifierError;",
      "use crate::error::DeviceIdentifierError;",
      "use crate::contract::{DeviceId};",
      "use fandhe_platform_android::JniCallFailed;",
      'Err(DeviceIdentifierError::Internal("x"))',
      "Err(DeviceIdentifierError::Unsupported) => 1,",
      "fandhe_platform_android::android_app()",
      "fandhe_runtime::request_repaint();",
    ].join("\n"),
  );
});

test("バリアントの抽出と未写像の検出", () => {
  const vs = usedErrorVariants('DeviceError::Internal("a") DeviceError::Io(e) DeviceError::Internal("b")');
  assert.deepEqual(vs, ["Internal", "Io"]);
  assert.deepEqual(unmappedVariants(vs), ["Io"]);
  assert.match(renderErrorRs("x-y", ["Internal"]), /pub enum XYError \{\n[\s\S]*Internal,/);
});

test("replaceBlock: 目印の間だけ書き直し、冪等で、目印が無ければ失敗", () => {
  const t = "a\n  # BEGIN fandhe-cap\n  old\n  # END fandhe-cap\nb\n  # BEGIN fandhe-cap\n  # END fandhe-cap";
  const once = replaceBlock(t, 0, featureLines(["a", "b"]));
  assert.equal(
    once,
    'a\n  # BEGIN fandhe-cap\n  "dep:fandhe-cap-a",\n  "dep:fandhe-cap-b",\n  # END fandhe-cap\nb\n  # BEGIN fandhe-cap\n  # END fandhe-cap',
  );
  assert.equal(replaceBlock(once, 0, featureLines(["a", "b"])), once);
  assert.throws(() => replaceBlock("no markers", 0, featureLines([])));
  assert.throws(() => replaceBlock(t, 2, featureLines([])));
});

test("gradleLines: java と res がある分だけ出す", () => {
  const l = gradleLines([{ id: "a", java: true, res: false }, { id: "b", java: false, res: true }])("");
  assert.deepEqual(l.filter((x) => x.includes("srcDirs")), [
    'java.srcDirs("../../../crates/fandhe-cap-a/android/java")',
    'res.srcDirs("../../../crates/fandhe-cap-b/android/res")',
  ]);
});

test("rewriteSource: --module で id と違う stem の契約パスも書き換える", () => {
  assert.equal(
    rewriteSource("use fandhe_device::text_scale::TextScale;", "dynamic-type", "text_scale"),
    "use crate::contract::TextScale;",
  );
});

test("renderUnsafeAttr: OS 1 つは not(target_os)、2 つ以上は not(any(..))（unsafe-audit.sh の has_forbid_attr が通す形）", () => {
  assert.equal(renderUnsafeAttr([]), "#![forbid(unsafe_code)]");
  const one = renderUnsafeAttr(["ios"]);
  assert.ok(one.includes('#![cfg_attr(not(target_os = "ios"), forbid(unsafe_code))]'));
  assert.ok(one.includes('#![cfg_attr(target_os = "ios", deny(unsafe_code))]'));
  assert.ok(!one.includes("any("));
  assert.ok(renderUnsafeAttr(["ios", "android"]).includes('not(any(target_os = "ios", target_os = "android"))'));
});

test("renderBuildRs: .m は置き場の OS でだけ、その OS の framework でコンパイルする", () => {
  const mac = renderBuildRs(["macos/fandhe_macos_x.m"]);
  assert.ok(mac.includes("if target_is_macos() {"));
  assert.ok(mac.includes('compile_objc("macos/fandhe_macos_x.m", &["AppKit", "Foundation"]);'));
  assert.ok(!mac.includes("target_is_ios") && !mac.includes("UIKit"));
  const ios = renderBuildRs(["ios/fandhe_ios_x.m"]);
  assert.ok(ios.includes("if target_is_ios() {") && ios.includes('&["Foundation", "UIKit"]') && !ios.includes("AppKit"));
  const both = renderBuildRs(["ios/a.m", "macos/b.m"]);
  assert.ok(both.includes("use fandhe_platform_apple::build::{compile_objc, target_is_ios, target_is_macos};"));
});

test("unsafeOsesOf: apple は ios と macos の両方、OS 名はそのまま、OS に写せない名前は例外（apple を target_os と取り違えない）", () => {
  assert.deepEqual(unsafeOsesOf("apple"), ["ios", "macos"]);
  assert.deepEqual(unsafeOsesOf("android"), ["android"]);
  assert.throws(() => unsafeOsesOf("contract"), /target_os へ写せない/);
  assert.throws(() => unsafeOsesOf("driver"));
  const attr = renderUnsafeAttr(unsafeOsesOf("apple"));
  assert.ok(attr.includes('not(any(target_os = "ios", target_os = "macos"))'));
  assert.ok(!attr.includes('"apple"'));
});

test("renderErrorRs: 写像するバリアントが 0 件ならテストを出さない（空配列は型を推論できずコンパイルできない）", () => {
  for (const vs of [[], ["Io"]]) {
    const rs = renderErrorRs("foo-bar", vs);
    assert.ok(!rs.includes("messages_are_fixed"));
    assert.ok(!rs.includes("for (e, expected) in [\n        ]"));
  }
  assert.ok(renderErrorRs("foo-bar", ["Internal"]).includes("messages_are_fixed"));
});

test("DeviceError: 機能名以外を渡すバリアントは引数を残し、機能名・_ だけなら単位バリアントへ", () => {
  const src = 'DeviceError::Internal("calendar save") DeviceError::Unsupported("cal") DeviceError::Unsupported(_) DeviceError::Cancelled(..)';
  assert.deepEqual([...strErrorVariants([src], "cal", "cal")], ["Internal"]);
  assert.equal(
    rewriteSource(src, "cal"),
    'CalError::Internal("calendar save") CalError::Unsupported CalError::Unsupported CalError::Cancelled',
  );
  // 識別子・変数・括弧なしの関数としての使用も文字列扱い。
  for (const u of ["DeviceError::Internal(MODEL_NOT_READY)", "DeviceError::Internal(stage)", ".map_err(DeviceError::Internal)"])
    assert.ok(strErrorVariants([u], "cal", "cal").has("Internal"), u);
  // 別ファイルで文字列を渡していれば、このファイルの Internal(_) も引数を残す（写像はファイル横断で揃える）。
  const vs = strErrorVariants(['DeviceError::Internal("x")', "DeviceError::Internal(_)"], "cal", "cal");
  assert.equal(rewriteSource("DeviceError::Internal(_)", "cal", "cal", vs), "CalError::Internal(_)");
});

test("renderErrorRs: 文字列を渡すバリアントは &'static str で出し、テストにも引数を渡す", () => {
  const rs = renderErrorRs("cal", ["Internal", "Unsupported"], new Set(["Internal"]));
  assert.ok(rs.includes('#[error("内部エラーが発生しました: {0}")]\n    Internal(&\'static str),'));
  assert.ok(rs.includes('(CalError::Internal("cal"), "内部エラーが発生しました: cal"),'));
  assert.ok(rs.includes("    Unsupported,\n"));
});

test("rewriteEdition2024: extern・no_mangle・gen を 2024 の書き方へ、文字列・コメント・既に直した箇所には触れない", () => {
  assert.equal(rewriteEdition2024('extern "C" {'), 'unsafe extern "C" {');
  assert.equal(rewriteEdition2024('unsafe extern "C" {'), 'unsafe extern "C" {');
  assert.equal(rewriteEdition2024('pub extern "C" fn f() {}'), 'pub extern "C" fn f() {}');
  assert.equal(rewriteEdition2024("#[no_mangle]"), "#[unsafe(no_mangle)]");
  assert.equal(rewriteEdition2024("let gen = 1;"), "let r#gen = 1;");
  assert.equal(rewriteEdition2024("fn f(gen: u64) {}"), "fn f(r#gen: u64) {}");
  for (const keep of ['"gen"', "// gen", "let r#gen = 1;", "generation", "gen_id"]) assert.equal(rewriteEdition2024(keep), keep);
  const lit = 'const S: &str = r#"extern "C" { #[no_mangle]"#;';
  assert.equal(rewriteEdition2024(lit), lit);
  assert.equal(rewriteEdition2024('log("gen"); let gen = 2; // gen'), 'log("gen"); let r#gen = 2; // gen');
});

test("refPatterns: stem・id・Pascal を語の境界で照合する（短い stem で別の機能・語を拾わない）", () => {
  const hit = (l) => refPatterns("ar", "ar").some((re) => re.test(l));
  for (const l of ["compile_ios_ar();", "pub mod ios_ar;", "device::ar::ar_show_effect", "ArHelper.java", "fandhe-cap-ar", "fandhe_ios_ar.m", "fandhe_egui_proxy::device::ar_show_effect"])
    assert.ok(hit(l), l);
  for (const l of ["compile_ios_ar_camera_view();", "Array", "solar", "charge", "fandhe-cap-ar-camera-view"]) assert.ok(!hit(l), l);
  // effect 名の接尾辞は `_effect` か `_<動詞 1 語>_effect` だけ。2 語以上は兄弟の機能名。
  const hitOf = (stem) => (l) => refPatterns(stem, stem).some((re) => re.test(l));
  for (const [stem, l] of [["nfc", "nfc_read_effect"], ["print", "print_page_effect"], ["location", "location_effect"]]) assert.ok(hitOf(stem)(l), l);
  for (const [stem, l] of [["nfc", "nfc_card_emulation_show_effect"], ["print", "print_screen_page_effect"]]) assert.ok(!hitOf(stem)(l), l);
});

test("missingEntry: crate ルートから公開された platform_<stem> だけを合格にする", () => {
  const def = "/// doc\n#[cfg(not(target_arch = \"wasm32\"))]\npub fn platform_x() -> u8 { 0 }\n";
  assert.equal(missingEntry("x", { "lib.rs": def }), null);
  assert.equal(missingEntry("x", { "lib.rs": "pub use driver::*;", "driver.rs": def }), null);
  assert.equal(missingEntry("x", { "lib.rs": "pub use driver::{a, platform_x, b};", "driver.rs": def }), null);
  assert.equal(missingEntry("x", { "lib.rs": "pub use driver::{\n    a,\n    platform_x,\n};", "driver.rs": def }), null);
  assert.ok(missingEntry("x", { "lib.rs": "mod driver;\npub use driver::{platform_x as other};", "driver.rs": def }));
  assert.ok(missingEntry("x", { "lib.rs": "mod driver;\npub use driver::platform_x as other;", "driver.rs": def }));
  assert.ok(missingEntry("x", { "lib.rs": "mod internal {\n    pub fn platform_x() {}\n}\n" }));
  assert.ok(missingEntry("x", { "lib.rs": "mod internal {\n    pub use driver::*;\n}\n", "driver.rs": def }));
  assert.ok(missingEntry("x", { "lib.rs": "mod driver;", "driver.rs": def }));
  assert.ok(missingEntry("x", { "lib.rs": "pub use driver::*;", "driver.rs": "pub(crate) fn platform_x() {}" }));
  assert.ok(missingEntry("x", { "lib.rs": "pub use driver::*;", "driver.rs": "fn platform_x() {}" }));
  assert.ok(missingEntry("x", { "lib.rs": "pub fn platform_passkey() {}" }) && missingEntry("passkey_signin", { "lib.rs": "pub fn platform_passkey() {}" }));
  assert.ok(missingEntry("x", { "lib.rs": "pub fn platform_x_y() {}" }));
  assert.ok(missingEntry("x", {}));
  // 深さ 0 にない定義・再公開は入口にしない（impl の関連関数・trait の関数・インライン mod）。
  assert.ok(missingEntry("x", { "lib.rs": "pub struct Factory;\nimpl Factory {\n    pub fn platform_x() {}\n}\n" }));
  assert.ok(missingEntry("x", { "lib.rs": "pub trait T {\n    fn platform_x() {}\n}\n" }));
  assert.ok(missingEntry("x", { "lib.rs": "mod inner { pub fn platform_x() {} }" }));
  assert.ok(missingEntry("x", { "lib.rs": "pub use driver::*;", "driver.rs": "impl D {\n    pub fn platform_x() {}\n}\n" }));
  assert.ok(missingEntry("x", { "lib.rs": "fn f() {\n    pub use driver::platform_x;\n}", "driver.rs": def }));
  assert.equal(missingEntry("x", { "lib.rs": "mod a { fn g() {} }\npub fn platform_x() { if true { } }\n" }), null);
  assert.equal(missingEntry("x", { "lib.rs": "mod driver;\npub use driver::platform_x;", "driver.rs": def }), null);
});

test("rewriteEdition2024: 複数行文字列・raw string・ブロックコメントの内側は変えない", () => {
  for (const keep of ['let s = "a\n  let gen = 1;\n";', 'let s = r#"x\nlet gen = 1;\n"#;', "/* \nlet gen = 1;\n*/", "let c = '\"'; // gen"]) assert.equal(rewriteEdition2024(keep), keep);
  assert.equal(rewriteEdition2024('let s = "a\nb";\nlet gen = 1;'), 'let s = "a\nb";\nlet r#gen = 1;');
  // ブロックコメントは入れ子になる。最初の `*/` で閉じると、コメント内の `"` から後ろを文字列とみなしてしまう。
  assert.equal(rewriteEdition2024('/* outer /* inner */ " */\nlet gen = 1;\nlet s = "x";'), '/* outer /* inner */ " */\nlet r#gen = 1;\nlet s = "x";');
  assert.equal(stripRustLiterals("/* a /* b */ c */x"), " ".repeat(17) + "x");
  // raw byte string・raw C string もリテラル全体を保護する。接頭辞付きの通常の文字列・文字・lifetime も中身に触れない。
  for (const keep of ['const S: &[u8] = br#"a " gen " b"#;', 'cr#"a " gen " b"#', 'br"gen"', 'b"a \\" gen"', 'c"gen"', "b'\"'"])
    assert.equal(rewriteEdition2024(keep), keep);
  assert.equal(rewriteEdition2024('br#"a " gen "#; let gen = 1;'), 'br#"a " gen "#; let r#gen = 1;');
  assert.equal(rewriteEdition2024("fn f<'a>(x: &'a str) { let gen = \"gen\"; }"), "fn f<'a>(x: &'a str) { let r#gen = \"gen\"; }");
});

test("missingEntry: コメント・文字列の中の pub fn は公開入口にしない", () => {
  assert.ok(missingEntry("x", { "lib.rs": "// pub fn platform_x() {}" }));
  assert.ok(missingEntry("x", { "lib.rs": "/*\npub fn platform_x() {}\n*/" }));
  assert.ok(missingEntry("x", { "lib.rs": 'const S: &str = "\npub fn platform_x() {}\n";' }));
  assert.ok(missingEntry("x", { "lib.rs": "pub use driver::*;", "driver.rs": "// pub fn platform_x() {}" }));
  assert.equal(stripRustLiterals("a // b\nc").length, 8);
});
