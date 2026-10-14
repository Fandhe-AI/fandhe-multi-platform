# 端末機能（Phase 8）の切り離し手順

spec 要件化が否決された場合に、Phase 8 の端末機能（`fandhe-device` crate・feature `device`・各画面）を取り除く手順を定める。

Phase 8（親 #752）は端末機能 3 型（ローカル保存 → HTTP → カメラ）を画面レジストリ方式で
検証するための土台で、spec 要件化が否決された場合に crate と feature を
消すだけで元に戻せる構成にしています（Issue #746）。

1. `Cargo.toml` の `members` から `crates/fandhe-device` を、`[workspace.dependencies]`
   から `ureq` を削除し、`crates/fandhe-device/` ディレクトリを削除する
2. `crates/fandhe-egui-proxy/Cargo.toml` の `default = ["device"]` / `device = [...]`、
   optional な `fandhe-device` / `serde_json` 依存行、`crates/fandhe-egui-proxy/src/device.rs`
   一式（`lib.rs` の対応する `pub mod device;` 行込み）を削除する
3. `crates/fandhe-core/src/screens/{storage_note,http_fetch,camera_capture}.rs` と
   `screens/mod.rs` の対応する `pub mod` 3 行を削除する（`storage_note` は #747、
   `http_fetch` は #748、`camera_capture` は #749 で追加済み）。
   `crates/fandhe-egui-proxy/src/device.rs`（`http_fetch`/`camera_capture` サブモジュール
   含む）と `lib.rs` の `pub mod device;` 行（`#[cfg(all(feature = "device",
   not(target_arch = "wasm32")))]` 付き）も削除する（マクロの `init`/`effect` 区画自体は
   他画面が使う可能性がある汎用機能のため `screens.rs` に残置してよい。`app.rs` の
   `EffectFn`/`with_effect` は #745 の apply API と同様、Phase 8 に依存しない汎用
   フックのため残置してよい）
4. `crates/fandhe-egui-proxy/src/screens/registry.rs` の
   `#[cfg(all(feature = "device", not(target_arch = "wasm32")))]` 付き `init`/`effect` 行
   （`storage_note`/`http_fetch`/`camera_capture`）を削除する
5. Android（#750）:
   - `crates/fandhe-egui-proxy/src/device/android.rs`（`DataDir` 解決・`AndroidCamera`・
     `nativeOnCameraResult` の JNI エクスポート）を削除する
   - `crates/fandhe-egui-proxy/src/device.rs` の `#[cfg(target_os = "android")] pub mod android;`
     行と `storage_note_state` の Android 分岐（`use fandhe_device::storage::FsStorage;` を
     含む `#[cfg(target_os = "android")] { ... }` ブロック）を削除する（iOS 分岐のみ残す）
   - `crates/fandhe-egui-proxy/src/device/camera_capture.rs` の `platform_camera()` を
     `DummyCamera` 直書きへ戻す（Android 分岐 `Arc::new(crate::device::android::AndroidCamera::new())`
     を削除し、`#[cfg(not(target_os = "android"))]` の条件分岐で分けていた
     `DUMMY_WIDTH`/`DUMMY_HEIGHT`/`use ... DummyCamera` の cfg も外す）。`CaptureMailbox`
     はターゲット非依存の汎用機能のため残置してよい
   - `crates/fandhe-egui-proxy/src/android.rs` の `ANDROID_APP` static・
     `set_android_app()`/`android_app()`・`android_main` 内の `set_android_app(app.clone())`
     の呼び出しは Phase 8 固有ではない汎用配線のため残置可（削除しても実害はない）
   - `crates/fandhe-egui-proxy/Cargo.toml` の android target 分岐の `jni` 依存行、
     ルート `Cargo.toml` の `[workspace.dependencies]` の `jni` 行を削除する
   - `platforms/android/app/src/main/java/com/fandhe/androidapp/MainActivity.java` の
     `requestCameraPermission`/`takePicturePreview`（`ActivityResultLauncher` 2 つ）・
     `nativeOnCameraResult` native 宣言・`requestCameraCapture`/`launchCameraCapture`/
     `deliverBitmap` メソッドを削除する
   - `crates/fandhe-egui-proxy/src/device/tilt.rs` の `gravity_to_tilt`（Issue #831）・
     `crates/fandhe-egui-proxy/src/device/android.rs` の `nativeOnMotionChanged` JNI
     エクスポートを削除する（`shared_motion`/`drive` 等ターゲット非依存の部分は残置）
   - `MainActivity.java` の `sensorManager`/`motionSensor` フィールド・`motionListener`・
     `nativeOnMotionChanged` native 宣言・`onResume`/`onPause` 内のセンサ登録/解除を
     削除する（Issue #831）
   - ゲームコントローラ（親 #932、Issue #935）: `crates/fandhe-egui-proxy/src/device/
     android_gamepad.rs`（`device.rs` の `pub mod android_gamepad;` 行込み。JNI
     エクスポート `nativeOnGamepadPresenceScanBegin`/`nativeOnGamepadPresenceScanId`/
     `nativeOnGamepadPresenceScanEnd`/`nativeOnGamepadMotion`/`nativeOnGamepadKey`
     の 5 関数を含む）・`crates/fandhe-egui-proxy/src/device/gamepad.rs` の Android
     分岐（`platform_gamepad()` の `#[cfg(target_os = "android")]` ブロックと
     `dead_code` 許可の `android` 追加分）を削除する。`MainActivity.java` の
     `inputManager` フィールド・`gamepadDeviceListener`・
     `reportGamepadPresence`/`axisFlat`・`nativeOnGamepad*` native 宣言 5 本・
     `dispatchKeyEvent`/`dispatchGenericMotionEvent` オーバーライド・
     `onResume`/`onPause` 内の `InputManager` 登録/解除を削除する
   - `platforms/android/app/src/main/AndroidManifest.xml` の `INTERNET`/`CAMERA`
     の `uses-permission` 2 行と `uses-feature` 3 行（`android.hardware.camera.any`/
     `android.hardware.camera`/`android.hardware.camera.autofocus`）を戻す
   - `platforms/android/app/build.gradle.kts` の `androidx.activity:activity` 依存行、
     `scripts/android/versions.env` の `ANDROIDX_ACTIVITY_VERSION` 行を削除する
   - バックグラウンド処理（親 #1042、Issue #1046）: `platforms/android/app/src/main/
     java/com/fandhe/androidapp/BackgroundTaskWorker.java`・`crates/fandhe-egui-proxy/
     src/device/android_background_task.rs`（`device.rs` の `pub mod
     android_background_task;` 行込み）を削除し、`device/background_task.rs::
     implementation()` の Android 分岐を外す。`MainActivity.java` の
     `BACKGROUND_TASK_WORK_NAME` 定数・`awaitOperation`/`backgroundTaskSchedule`/
     `backgroundTaskCancel` メソッドを削除する。
     `platforms/android/app/build.gradle.kts` の `androidx.work:work-runtime`
     依存行、`scripts/android/versions.env` の `WORK_RUNTIME_VERSION` 行を削除する
   - ホーム画面ウィジェット（親 #1064、Issue #1067）: `platforms/android/app/src/main/
     java/com/fandhe/androidapp/HomeWidgetProvider.java`・`res/layout/home_widget.xml`・
     `res/xml/home_widget_info.xml`・`crates/fandhe-egui-proxy/src/device/
     android_home_widget.rs`（`device.rs` の `pub mod android_home_widget;` 行込み）を削除し、
     `device/home_widget.rs::implementation()` の Android 分岐を外す。`res/values/strings.xml`
     の `home_widget_label`/`home_widget_description`、`AndroidManifest.xml` の
     `<receiver>`、`MainActivity.java` の `homeWidgetSetText`/`homeWidgetCurrentText`
     メソッドを削除する
   iOS（#751）: 以下を削除・復元する
   - `crates/fandhe-egui-proxy/ios/fandhe_ios_camera.m`・`crates/fandhe-egui-proxy/build.rs`
   - `crates/fandhe-egui-proxy/src/device/ios.rs`（`device.rs` の `pub mod ios;` 行込み）
   - ルート `Cargo.toml` の `[workspace.dependencies]` の `cc = "1"`、
     `crates/fandhe-egui-proxy/Cargo.toml` の `[build-dependencies]` の `cc`
   - `platforms/apple/FandheIosApp/Resources/Info.plist` の `NSCameraUsageDescription`
   - `crates/fandhe-egui-proxy/src/lib.rs` の iOS 分岐の `deny(unsafe_code)` 緩和
     （`#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]` を
     Android 単独へ戻す）
   - `crates/fandhe-device/src/lib.rs` の `DeviceError::Cancelled`
     （Android（#750）のキャンセル処理でも使う場合は残す）
   - 実機確認用の `FANDHE_ITEMS_URL`（`crates/fandhe-egui-proxy/src/device/http_fetch.rs`）、
     `scripts/ios/verify-device-launch.sh` の ATS 差し込み処理、
     `crates/fandhe-device/src/http.rs` の私設 IPv4 許可（`is_local_plain_http_host`、Issue #660）
   - 位置情報（親 #887、Issue #889）: `crates/fandhe-egui-proxy/apple/fandhe_apple_location.m`・
     `crates/fandhe-egui-proxy/macos/Info.plist`・`build.rs` の macOS 分岐・`lib.rs` の
     macOS `deny(unsafe_code)` 緩和・iOS `Info.plist` の `NSLocationWhenInUseUsageDescription`
6. `cargo build --workspace && cargo test --workspace --all-features` が通ることを確認する
   （#745 の汎用 apply API のみ残るが、それ自体は Phase 8 に依存しないため残置してよい）
