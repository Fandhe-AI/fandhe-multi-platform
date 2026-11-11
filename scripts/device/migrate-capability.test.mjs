/**
 * migrate-capability.mjs の自己検査（Issue #1183）。純関数（名前変換・移動先・書き換え・目印ブロック）だけを確かめる。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  checkId, stemOf, pascalOf, moveTargets, rewriteSource, usedErrorVariants, unmappedVariants,
  renderErrorRs, replaceBlock, featureLines, gradleLines,
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
