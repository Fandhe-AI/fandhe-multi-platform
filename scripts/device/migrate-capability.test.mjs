/**
 * migrate-capability.mjs の自己検査（Issue #1183）。純関数（名前変換・移動先・書き換え・目印ブロック）だけを確かめる。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  checkId, stemOf, pascalOf, moveTargets, rewriteSource, usedErrorVariants, unmappedVariants,
  renderErrorRs, replaceBlock, featureLines, gradleLines, renderUnsafeAttr, renderBuildRs, unsafeOsesOf,
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
      "Err(DeviceIdentifierError::Internal)",
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
