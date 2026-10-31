// GameActivity サブクラス（TASK-9.3-S1 / REQ-9 / M4、Issue #242）。
//
// android-activity（"android-game-activity" feature、crates/fandhe-egui-proxy・
// fandhe-winit-ime の Cargo.toml 参照）は GameActivity バックエンドを使うが、
// GameActivity 自体は Java/Kotlin 側でサブクラス化しないと動作しない
// （com.google.androidgamesdk.GameActivity は抽象的な起動口のみを提供し、
// NativeActivity と異なりネイティブライブラリの自動ロードを行わない。
// developer.android.com/games/agdk/game-activity/get-started の
// 「Implement GameActivity in your game」節、取得日 2026-08-01。
// 決定記録: docs/architecture/android-activity-method.md）。
//
// 本クラスの責務は「ライブラリロードの宣言」「window insets の JNI 中継」に加え、
// Phase 8（端末機能、Issue #750）のカメラ Intent 中継、Phase 10（Issue #831）の
// 重力センサ中継、Phase 11（Issue #859）のクリップボード中継、Phase 11（Issue #866）の
// 読み上げ（TTS）中継、Phase 11（Issue #874）のファイル選択（SAF）中継、
// Phase 11（Issue #890）の位置情報（`LocationManager`）中継、
// Phase 11（Issue #935）のゲームコントローラ（`InputManager`/`dispatchKeyEvent`/
// `dispatchGenericMotionEvent`）中継、Phase 11（Issue #1218）のカスタム URL scheme
// （`fandhe://`、`onCreate` の `getIntent()` / `onNewIntent`）受信中継、
// Phase 11（Issue #607）の共有テキスト受信（`ShareReceiveHelper.deliver` を同じ経路から呼ぶ）
// （いずれも feature `device` 切り離し時に削除）。
// window insets 中継の経緯（Issue #579: `AndroidApp::content_rect()` は GameActivity
// 経路（android-activity 0.6.1、Pixel 10a / Android 16 実機）で insets を反映せず常に
// 画面全体を返すことを logcat で実測確認した。`GameActivity_getWindowInsets` C API は
// `GameActivity*` 構造体ポインタを要求するが、Rust 公開 API（`activity_as_ptr()` =
// JNI jobject）からは得られないため、Java 側で `WindowInsetsCompat` を取得し
// native メソッドで Rust 側 static へ橋渡しする以外に手段がない）。
// これ以外の実描画・入力処理はすべて cdylib 側（fandhe-egui-proxy が公開する
// android_main、crates/fandhe-egui-proxy/src/android.rs 参照）に委譲する
// （Java/Kotlin コードを最小限に保つ方針は維持）。
// AndroidManifest.xml の android:name はこのクラスを指す。
package com.fandhe.androidapp;

import android.Manifest;
import android.app.PendingIntent;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothManager;
import android.bluetooth.le.BluetoothLeScanner;
import android.bluetooth.le.ScanCallback;
import android.bluetooth.le.ScanRecord;
import android.bluetooth.le.ScanResult;
import android.bluetooth.le.ScanSettings;
import android.content.ActivityNotFoundException;
import android.content.BroadcastReceiver;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.IntentSender;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.hardware.input.InputManager;
import android.icu.text.BreakIterator;
import android.location.Location;
import android.location.LocationListener;
import android.location.LocationManager;
import android.media.MediaRecorder;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.ParcelFileDescriptor;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.os.SystemClock;
import android.os.VibrationEffect;
import android.os.Vibrator;
import android.os.VibratorManager;
import android.provider.CalendarContract;
import android.provider.ContactsContract;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.speech.RecognitionListener;
import android.speech.RecognizerIntent;
import android.speech.SpeechRecognizer;
import android.speech.tts.TextToSpeech;
import android.speech.tts.UtteranceProgressListener;
import android.view.DragAndDropPermissions;
import android.view.DragEvent;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.Surface;
import android.view.View;
import android.view.ViewGroup;
import android.view.ViewParent;
import android.view.accessibility.AccessibilityEvent;
import android.view.accessibility.AccessibilityManager;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.PickVisualMediaRequest;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.biometric.BiometricManager;
import androidx.biometric.BiometricPrompt;
import androidx.core.app.NotificationChannelCompat;
import androidx.core.app.NotificationCompat;
import androidx.core.app.NotificationManagerCompat;
import androidx.core.content.ContextCompat;
import androidx.core.graphics.Insets;
import androidx.core.location.LocationManagerCompat;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.security.crypto.EncryptedSharedPreferences;
import androidx.security.crypto.MasterKey;
import androidx.work.ExistingWorkPolicy;
import androidx.work.OneTimeWorkRequest;
import androidx.work.WorkManager;
import com.google.androidgamesdk.GameActivity;
import java.io.File;
import java.io.FileDescriptor;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.security.GeneralSecurityException;
import java.util.ArrayList;
import java.util.Calendar;
import java.util.GregorianCalendar;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.atomic.AtomicInteger;

public class MainActivity extends GameActivity {
    static {
        // ライブラリ名は AndroidManifest.xml の android.app.lib_name meta-data
        // （値 "fandhe_egui_proxy"）および crates/fandhe-egui-proxy/Cargo.toml の
        // [lib] name と一致させること（3 箇所の SSOT はこのライブラリ名文字列）。
        System.loadLibrary("fandhe_egui_proxy");
    }

    // Rust 側（crates/fandhe-egui-proxy/src/android.rs）の JNI エクスポート関数。
    // シンボル名はパッケージ名 com.fandhe.androidapp とクラス名 MainActivity に
    // 固定で紐づく（JNI の命名規約。クラス移動・rename 時は Rust 側も追随が必要）。
    private static native void nativeOnWindowInsetsChanged(
            int left, int top, int right, int bottom);

    // Rust 側（crates/fandhe-egui-proxy/src/device/android.rs）の JNI エクスポート関数
    // （Issue #750、Phase 8 の端末機能に限り追加。切り離し時に削除する）。
    // requestId: `requestCameraCapture(long)` が受け取った値をそのまま返す
    // （codex-review 指摘、PR #790。Rust 側の mailbox が要求 ID の一致を検査し、
    // タイムアウト後に届いた古い要求のコールバックを新しい要求の結果と取り違えないため）。
    // status: 0=成功／1=権限拒否／2=カメラアプリなし／3=キャンセル／4=旧要求が未完了のため拒否／
    // 5=画像サイズが上限超過（Rust 側 `decode_capture_result` と対で決める、codex-review 指摘 PR #790）／
    // 6=launch() 自体が想定外の実行時例外で失敗／7=Activity 再生成により中断
    // （再生成前から保留中だった権限ダイアログ・カメラアプリの結果を、新インスタンスの
    // launcher コールバックが実際に受け取り得るため、先回りして決着させ、後から届く
    // 実結果は `pendingCaptureInterrupted` で破棄する。`onCreate` の復元処理 doc 参照。
    // cursor Bugbot 指摘、PR #790）。
    private static native void nativeOnCameraResult(
            long requestId, byte[] rgba, int width, int height, int status);

    // Rust 側（crates/fandhe-egui-proxy/src/device/android.rs）の JNI エクスポート関数
    // （Issue #831、Phase 10 型サンプル「傾けると玉が転がる」の Android 実センサ経路に
    // 限り追加。Phase 8 の feature `device` 切り離し時に削除する。docs/architecture/
    // device-feature-removal.md 参照）。
    // x, y: 重力センサ（TYPE_GRAVITY、無ければ TYPE_ACCELEROMETER）の生値
    // （SensorEvent#values[0]/[1]、m/s^2）。rotation: 呼び出し時点の画面回転
    // （Surface.ROTATION_*）。Rust 側 `gravity_to_tilt` が画面座標の傾きへ写像する。
    private static native void nativeOnMotionChanged(float x, float y, int rotation);

    // Rust 側（crates/fandhe-egui-proxy/src/device/android_gamepad.rs）の JNI
    // エクスポート関数 5 本（Issue #935、Phase 11 型サンプル「ゲームコントローラ」の
    // Android 経路に限り追加。Phase 8 の feature `device` 切り離し時に削除する）。
    // `reportGamepadPresence()` が `InputDevice.getDeviceIds()` を再走査した結果を
    // begin → (見つかった deviceId ごとに) scanId → end の 3 段で通知する。1 台の
    // 物理コントローラがボタン用・ジョイスティック用の 2 つの deviceId に分かれて見える
    // 機種（split pad、Joy-Con 等）は Android 上で別々の InputDevice として扱われ、
    // 公開 API に再結合の手段が無いため束ねない（`getControllerNumber()` は機器ごとに
    // 固有の番号で、片割れ間の共通識別には使えない）。Rust 側の状態機械
    // （`AndroidGamepadState`）は走査で見つかった先頭 1 台のみを追跡する
    // （codex-review 指摘 P0/P1、PR #939 で束ね機能を撤去）。
    private static native void nativeOnGamepadPresenceScanBegin();
    private static native void nativeOnGamepadPresenceScanId(int deviceId);
    private static native void nativeOnGamepadPresenceScanEnd();
    // `dispatchGenericMotionEvent` の最新サンプル（x/y/z/rz はスティック軸、
    // leftTrigger/rightTrigger はトリガ軸、hatX/hatY は十字キー軸、flat* は各軸の
    // `MotionRange#getFlat()`）を Rust 側デッドゾーン処理へそのまま渡す。
    private static native void nativeOnGamepadMotion(
            int deviceId,
            int source,
            float x,
            float y,
            float z,
            float rz,
            float leftTrigger,
            float rightTrigger,
            float hatX,
            float hatY,
            float flatX,
            float flatY,
            float flatZ,
            float flatRz);
    // `dispatchKeyEvent` の DOWN/UP（keyCode は `KeyEvent.KEYCODE_BUTTON_*`/
    // `KEYCODE_DPAD_*`）。
    private static native void nativeOnGamepadKey(int deviceId, int source, int keyCode, boolean down);

    // Rust 側（crates/fandhe-egui-proxy/src/device/android.rs）の JNI エクスポート関数
    // （Issue #859、Phase 11 型サンプル「クリップボード」の Android 経路に限り追加。
    // Phase 8 の feature `device` 切り離し時に削除する）。
    // requestId: `clipboardRequest(long, int, String)` が受け取った値をそのまま返す
    // （Rust 側 mailbox が ID 一致を検査し、タイムアウト後の遅延到着を新要求の結果と
    // 取り違えないため。カメラ経路と同じ理由）。
    // text: 読み出し結果（書き込み要求・テキスト不在では null）。
    // status: 0=成功／1=クリップボードにテキストが無い／2=内部エラー
    // （Rust 側 `decode_clipboard_result` と対で決める）。
    private static native void nativeOnClipboardResult(long requestId, String text, int status);

    // クリップボード要求の種別（Rust 側 `CLIPBOARD_OP_READ` / `CLIPBOARD_OP_WRITE` と対）。
    private static final int CLIPBOARD_OP_READ = 0;
    private static final int CLIPBOARD_OP_WRITE = 1;

    // Rust 側（crates/fandhe-egui-proxy/src/device/android.rs）の JNI エクスポート関数
    // （Issue #866、Phase 11 型サンプル「読み上げ」の Android 経路に限り追加。
    // Phase 8 の feature `device` 切り離し時に削除する）。
    // requestId: `ttsRequest(long, int, String)` が受け取った値をそのまま返す
    // （Rust 側 mailbox が ID 一致を検査し、タイムアウト後の遅延到着を新要求の結果と
    // 取り違えないため。クリップボード経路と同じ理由）。
    // status: 0=成功（キュー投入ではなく `UtteranceProgressListener#onDone` による
    // 真の発話完了、`utteranceListener` 参照。codex-review P1 是正、PR #869）／
    // 1=`TextToSpeech` を生成できない／2=その他の内部エラー（stop() が ERROR を
    // 返した場合を含む）／3=非同期初期化（onInit）が ERROR／
    // 4=（未使用。旧: 初期化待ちの要求が既にある。INITIALIZING 中の要求は
    // 拒否せず最新の要求で保留を置き換えるよう改めたため、Rust 側は互換のため
    // 引き続き decode_tts_result で内部エラーへ写像する）／
    // 5=speak() 自体が ERROR を返した（キュー投入失敗）、または投入後に
    // `onError`（発話自体の失敗）が届いた（PR #869 是正: 旧実装はキュー投入成功を
    // 発話成功と混同していた）／6=最大文字数超過／
    // 7=stop または後続 speak による保留中 speak 要求のキャンセル、あるいは
    // 発話開始後に `onStop`（ユーザー操作による中断）が届いた（いずれも失敗として
    // 扱わない、`decode_tts_result` 参照）
    // （Rust 側 `decode_tts_result` と対で決める）。
    private static native void nativeOnTtsResult(long requestId, int status);

    // Rust 側（crates/fandhe-egui-proxy/src/device/file_pick.rs の bridge モジュール）の
    // JNI エクスポート関数（Issue #874、Phase 11 型サンプル「ファイル選択」の Android 経路。
    // 案 B: OS 側は「提示・1 回だけの通知・後始末」のみを担い、`FilePicker` trait は通らない
    // （`fandhe_device::file_picker` モジュール doc の「モバイルは trait を通らない」参照）。
    // Phase 8 の feature `device` 切り離し時に削除する）。
    // requestId: `requestFilePick(long)` が受け取った値をそのまま返す。Rust 側
    // `bridge` モジュールが画面状態の generation ではなくプロセス内で一意な連番
    // `request_id` を発行して OS 側へ渡す（改訂 2、codex P1 是正: Activity 再生成で
    // 画面状態が作り直され generation が 1 から再開するため、generation をそのまま
    // OS へ渡すと旧要求の遅延完了が新要求に一致してしまう）。`bridge::deliver` が
    // 現在追跡中の requestId と不一致（＝プロセス死後の再配送・旧要求の遅延到着）なら
    // 黙って捨てる。
    // code: 0=選択（name/fd が有効）／1=キャンセル／2=失敗（プロバイダなし・名前解決不能・
    // `launch()` 例外・fd 取得失敗のいずれも一律ここへ畳む。詳細な原因分類は行わない）。
    // fd: `code == 0` のときのみ有効。`ParcelFileDescriptor.detachFd()` で所有権を
    // Rust へ渡した生 fd（Android は原本パスを持たないため、パス文字列ではなく fd を渡す。
    // 改訂 2「Android: fd を開き直さない」参照）。detach 後は Java 側で close しない。
    // 読み取り・8 MiB 判定は Rust 側 `fandhe_device::file_picker::read_picked_from` に
    // 一本化する（Java 側では読み取りループを書かない、仕様「共有 Rust 部分」参照）。
    private static native void nativeOnFilePickerResult(
            long requestId, int code, String name, int fd);

    // Rust 側（crates/fandhe-egui-proxy/src/device/android.rs）の JNI エクスポート関数
    // （Issue #1104、Phase 11 型サンプル「写真選択」の Android 経路。`nativeOnFilePickerResult`
    // と同型の pull 型で、`FilePicker`/`PhotoPicker` trait は通らない）。
    // requestId: `requestPhotoPick(long)` が受け取った値をそのまま返す（Rust 側 `bridge` が
    // 現在の要求と照合し、Activity 再生成後の遅延到着は黙って捨てる）。
    // code: 0=選択（name/fd が有効）／1=キャンセル／2=失敗（`launch()` 例外・fd 取得失敗・
    // JPEG 以外の変換失敗・復号不能・長辺超過のいずれも一律ここへ畳む）。
    // name: 固定値 "photo"（表示にもログにも出さない。`nativeOnFilePickerResult` と署名を
    // 揃えて Rust 側の `decode_file_pick_result` を共有するための引数）。
    // fd: `code == 0` のときのみ有効。`detachFd()` で所有権を Rust へ渡した生 fd。
    // 読み取り・8 MiB 判定・形式判定・復号確認はすべて Rust 側で行う。
    private static native void nativeOnPhotoPickerResult(
            long requestId, int code, String name, int fd);

    // Rust 側（crates/fandhe-egui-proxy/src/device/android.rs）の JNI エクスポート関数
    // （Issue #1222、Phase 11 型サンプル「連絡先選択」の Android 経路。`nativeOnPhotoPickerResult`
    // と同型の pull 型）。
    // requestId: `requestContactPick(long)` が受け取った値をそのまま返す（Rust 側 `bridge` が
    // 現在の要求と照合し、Activity 再生成後の遅延到着は黙って捨てる）。
    // code: 0=選択（name が有効）／1=キャンセル／3=失敗（`launch()` 例外・連絡先アプリ無し・
    // 読み取り失敗・名前なしを一律ここへ畳む。インテント経由に権限拒否は無く 2 は送らない）。
    // name: 選ばれた連絡先の表示名のみ。ログには出さない。
    private static native void nativeOnContactPickerResult(
            long requestId, int code, String name);

    // Rust 側（crates/fandhe-egui-proxy/src/device/android.rs）の JNI エクスポート関数
    // （Issue #890、Phase 11 型サンプル「位置情報」の Android 経路に限り追加。
    // Phase 8 の feature `device` 切り離し時に削除する）。継続 push 型
    // （`fandhe_device::location::SharedLocation` 契約）のため requestId を持たない。
    // latitude/longitude: 度（WGS84）。accuracy: 水平精度（メートル）。
    private static native void nativeOnLocationChanged(
            double latitude, double longitude, float accuracy);

    // 同上。code の対応表（Rust 側 `decode_location_failure` と対で決める）:
    // 1=権限拒否／2=位置情報サービス無効（設定 OFF・provider 無効）／
    // 3=その他の失敗（`requestLocationUpdates` の例外・`LocationManager` 取得不能等）。
    private static native void nativeOnLocationFailure(int code);

    // 失敗からの回復（権限再取得・provider 再有効化）に、直前の失敗状態を「未取得」へ
    // 戻す（cursor Bugbot 指摘、PR #894）。Rust 側 `SharedLocation::reset_to_waiting`
    // を呼ぶだけで、`nativeOnLocationChanged`/`nativeOnLocationFailure` と異なり値を
    // 運ばない。coarse/network provider は再登録後も新しい fix が届くとは限らないため、
    // 「再開した」事実そのものを届けて古い Denied/Disabled 表示を残留させない。
    // 呼び出し口は `notifyLocationRecoveredIfNeeded` のみ（`locationFailurePosted`
    // doc 参照）。平常な `onResume` 再登録（直前が `Fixed`、失敗を挟んでいない）では
    // 呼ばない。無条件に呼ぶと取得済みの Fix を破棄してしまう。Rust 側
    // `location_effect` は `LocationShowState.retry_seq`（「再試行」ボタン押下のみが
    // 進む世代番号）でしか再要求を出さないため、本メソッドの呼び出し自体は
    // 「利用者のボタン再試行」と誤認されない（codex-review P1 指摘 / cursor Bugbot
    // 指摘、PR #894）。
    private static native void nativeOnLocationRetrying();

    // 上記 `nativeOnLocationFailure` の code 定数（Java 側でも同じ表を参照するため）。
    private static final int LOCATION_FAILURE_PERMISSION_DENIED = 1;
    private static final int LOCATION_FAILURE_SERVICE_DISABLED = 2;
    private static final int LOCATION_FAILURE_FAILED = 3;

    // 位置情報の更新間隔（Issue #890、ponytail: 型サンプルの表示更新用途で強い実測
    // 根拠はない。頻度が問題になったら見直す）。
    private static final long LOCATION_UPDATE_INTERVAL_MS = 1000L;

    // Rust 側（crates/fandhe-egui-proxy/src/device/android.rs）の JNI エクスポート関数
    // （Issue #899、Phase 11 型サンプル「共有」の Android 経路）。`Share` trait は通らない
    // （`device/share.rs` モジュール doc の「macOS/iOS/Android は pull 型 bridge」参照）。
    // code: `device/share.rs::outcome_from_code` の表（iOS/macOS/Android 共通）参照。
    // 0=共有した（選択 broadcast 受信）／3=内部エラー（`launch()` 例外等）／
    // 4=結果不明（chooser は閉じたが選択 broadcast が届かなかった。§ requestShare 参照）。
    // 1（キャンセル）・2（共有先なし）は Android では使わない。
    private static native void nativeOnShareResult(long requestId, int code);

    // コールバック時に requestId を思い出すためだけの唯一の static（`filePickRequest`
    // と同じ理由。Activity 再生成を跨いで生存する必要がある、同 doc 参照）。id の
    // 保持・照合そのものは `RequestTracker`（Issue #944）へ委譲する。
    private static final RequestTracker shareRequest = new RequestTracker();

    // 選択完了 broadcast（`shareChosenReceiver`）の受信を、chooser 終了コールバック
    // （`shareChooser`）より先に検知できた場合に true にする（実装計画 §2.3）。
    // broadcast は chooser の finish より先に送られるが非同期のため、`shareChooser`
    // コールバックが先に走った場合は `SHARE_CHOSEN_GRACE_MS` だけ遅延させて broadcast の
    // 遅延到着を待ってから「不明」を確定する（下記 `shareChooser` 参照）。UI スレッドの
    // みで読み書きするため追加の同期は不要。
    private static final long SHARE_CHOSEN_GRACE_MS = 500L;

    // 選択完了 broadcast の action プレフィックス（自パッケージ限定、`setPackage` で
    // 外部からの偽装を防ぐ。実装計画 §2.3・§6 A01/A05）。要求ごとに一意な action にする
    // （reviewer P1 是正、PR #903）: 全要求で同一 action・requestCode を再利用すると、
    // 要求 A の broadcast が遅延中に要求 B が開始した場合、A の遅延 broadcast が B の
    // 受信器（`registerShareChosenReceiver` が要求ごとに新規登録する IntentFilter）へ
    // 誤って配送され、B が未選択のまま Shared 扱いになってしまう（IntentFilter は
    // action 文字列で照合されるため、要求ごとに action を変えれば別要求の broadcast は
    // 届かない。`shareChosenAction` 参照）。
    private static final String SHARE_CHOSEN_ACTION_PREFIX =
            "com.fandhe.androidapp.SHARE_CHOSEN.";

    // 要求 ID ごとの一意な action 文字列（上記 `SHARE_CHOSEN_ACTION_PREFIX` 参照）。
    // `requestShare`（PendingIntent 生成）と `registerShareChosenReceiver`
    // （IntentFilter 登録）の両方から同じ requestId で呼ぶことで対を一致させる。
    private static String shareChosenAction(long requestId) {
        return SHARE_CHOSEN_ACTION_PREFIX + requestId;
    }

    // `shareChosenReceiver` は Activity 再生成を跨いで生存させる必要があるため
    // Application Context へ登録する static（`filePickRequest` と同じ理由:
    // Activity 単位の receiver は再生成の `onDestroy` で消えてしまう）。
    private static BroadcastReceiver shareChosenReceiver;

    // コールバック時に requestId を思い出すためだけの唯一の static（仕様書「決定事項」
    // 参照）。「進行中か」「中断されたか」を表すフラグ・ロックは持たない（単一飛行は
    // コアが「ファイルを選ぶ」ボタンを隠すことで保証済み）。static にする理由（reviewer
    // P1 是正）: 画面回転等の Activity 再生成は同一プロセス内で新しい `MainActivity`
    // インスタンスを作るが、`ActivityResultRegistry` は再生成前から保留中だった選択結果を
    // 新インスタンスの `pickDocument` コールバックへ実際に配送し得る（`onCreate` の
    // カメラ復元コメント参照、同じ仕様）。requestId を instance field にすると新
    // インスタンスでは `RequestTracker.NONE` に初期化され、実際に届く結果を
    // 「`NONE` なら捨てる」判定で誤って破棄し、
    // コアが `Picking` のまま固着する（画面回転中に選択、という頻出シナリオ）。static なら
    // 同一プロセス内の再生成を跨いで生き残るため、新インスタンスのコールバックでも
    // requestId が一致し正しく反映される。プロセス死では `RequestTracker.NONE` に戻るため、死後に OS が
    // 新プロセスへ配送してきた旧結果は「`NONE` なら捨てる」判定で無視される（Rust 側
    // `bridge::PENDING` もプロセスと運命を共にし空になるため、二重に防御される）。
    // 書き込みは `requestFilePick` を呼ぶネイティブスレッド（`android_main`）。直後の
    // `runOnUiThread` の post が happens-before を張るため、UI スレッド（`pickDocument`
    // コールバック）から正しく見える。読み取りは UI スレッドのみ。id の保持・照合そのものは
    // `RequestTracker`（Issue #944）へ委譲する。
    private static final RequestTracker filePickRequest = new RequestTracker();

    // 写真選択（Issue #1104）の「いま待っている要求 id」。`filePickRequest` と同じく static。
    // ただし契約は「Activity 再生成で保留中の要求は復元せず破棄」: Rust 側
    // （`android_main`）が再生成のたびに受け皿を捨てるため、再生成をまたいで届いた結果は
    // Rust 側の id 照合で捨てられる。
    private static final RequestTracker photoPickRequest = new RequestTracker();
    private static final RequestTracker contactPickRequest = new RequestTracker();
    // 連絡先アプリを開いてから結果コールバックが来るまで true（Issue #1222）。static なので
    // Activity 再生成をまたいで残り、旧ピッカーの結果が新インスタンスへ届くまで新しい起動を断る。
    // 断らないと、再生成後に始めた要求 B の id を旧要求 A の結果が消費して B の成功に見える
    // （`awaitingCameraResult` と同じ理由）。読み書きは UI スレッドだけで行う。
    private static boolean contactPickerOpen = false;

    // 変換前の元データに許す上限（バイト）。Rust 側 `MAX_PHOTO_BYTES` と揃える。
    private static final long PHOTO_MAX_BYTES = 8L * 1024 * 1024;
    // 変換前に許す画像の長辺（画素）。Rust 側 `MAX_PHOTO_SIDE` と揃える。
    private static final int PHOTO_MAX_SIDE = 16384;
    // 変換前に許す総画素数。Rust 側 `MAX_PHOTO_PIXELS`（64 Mi 画素）と揃える。
    private static final long PHOTO_MAX_PIXELS = 64L * 1024 * 1024;
    // JPEG へ変換するときの長辺の目標（画素）。これ以下になるまで `inSampleSize` で縮める。
    private static final int PHOTO_CONVERT_SIDE = 4096;

    // Rust 側（crates/fandhe-egui-proxy/src/device/android.rs）の JNI エクスポート関数
    // （Issue #908、Phase 11 型サンプル「生体認証」の Android 経路）。`BiometricAuth`
    // trait は通らない（`device/biometric_auth.rs` モジュール doc の
    // 「macOS/iOS/Android は pull 型 bridge」参照）。code の意味（本ファイルが SSOT、
    // Rust 側 `outcome_from_android_code` の doc と対）:
    // 0=認証した／1=キャンセル／2=使えない（未登録・非対応等）／3=失敗（不一致・
    // ロックアウト等）／4=内部エラー（提示依頼自体の失敗）。
    private static native void nativeOnBiometricResult(long requestId, int code);

    // 上記 code 定数（`biometricCallback`/`requestBiometric` からも参照するため）。
    private static final int BIOMETRIC_RESULT_AUTHENTICATED = 0;
    private static final int BIOMETRIC_RESULT_CANCELLED = 1;
    private static final int BIOMETRIC_RESULT_UNAVAILABLE = 2;
    private static final int BIOMETRIC_RESULT_FAILED = 3;
    private static final int BIOMETRIC_RESULT_INTERNAL_ERROR = 4;

    // コールバック時に requestId を思い出すためだけの唯一の static（`shareRequest`
    // と同じ理由。Activity 再生成を跨いで生存する必要がある。`BiometricPrompt` は
    // `onCreate` で新インスタンスごとに作り直すが（下記 `biometricPrompt` 参照）、
    // 公式ガイダンス上、内部 Fragment 経由で旧インスタンスへの提示結果が新
    // インスタンスの callback へ再接続される契約のため、static であれば正しく
    // 反映される。プロセス死では `RequestTracker.NONE` に戻るため死後の再配送は
    // 無視される（`filePickRequest` と同じ理屈）。UI スレッドのみで読み書きするため
    // 追加の同期は不要。
    private static final RequestTracker biometricRequest = new RequestTracker();

    // ローカル通知（Issue #1006、Phase 11 型サンプル「通知」の Android 経路）。Rust 側
    // `device/notification.rs::bridge` と pull 型で橋渡しする（`biometricRequest`/
    // `requestBiometric` と同型）。Rust 側は `MainActivity.requestNotification(long,
    // String, String)`（提示依頼）を呼び、結果は `nativeOnNotificationResult` で
    // 非同期に返す。code の意味（本ファイルが SSOT、Rust 側 `notification::
    // outcome_from_code` の doc と対）: 0=送信した（`NotificationManagerCompat.notify`
    // が例外無しで完了）／1=キャンセル（Android では出ない。`outcome_from_code` の
    // doc 参照）／2=権限拒否（実行時許可の拒否・通知オフ・チャンネルブロックを
    // 区別せず畳む、`ActivityResultContracts.RequestPermission` が拒否とダイアログを
    // 閉じただけを区別できないのと同じ制約）／3=内部エラー（権限確認自体の失敗）／
    // 4=内部エラー（`notify()` 呼び出し自体の失敗）。
    private static native void nativeOnNotificationResult(long requestId, int code);

    // 予定の追加（Issue #1226）。Rust 側 `device/calendar.rs::bridge` と pull 型で橋渡しする。
    // 結果コード（本ファイルが SSOT、Rust 側 `calendar::outcome_from_android_code` の doc と
    // 対）: 0=編集画面へ渡した（保存の有無は分からない）, 1=カレンダーアプリ無し,
    // 2=非前景・破棄中で起動しなかった, 3=その他の起動失敗。
    private static native void nativeOnCalendarResult(long requestId, int code);

    private static final int CALENDAR_RESULT_HANDED = 0;
    private static final int CALENDAR_RESULT_NO_APP = 1;
    private static final int CALENDAR_RESULT_BACKGROUND = 2;
    private static final int CALENDAR_RESULT_LAUNCH_FAILED = 3;

    // 新しい要求に差し替えられた旧要求を UI スレッド側で捨てるための追跡。
    private static final RequestTracker calendarRequest = new RequestTracker();


    // 上記 code 定数（`requestNotificationPermission`/`requestNotification`/
    // `postNotification` からも参照するため）。
    private static final int NOTIFICATION_RESULT_SUBMITTED = 0;
    // Android では発生しない（`ActivityResultContracts.RequestPermission` が「拒否」と
    // 「ダイアログを閉じただけ」を区別できないため）。Rust 側 `outcome_from_code` の
    // 写像表との対応を示すためだけに宣言する。
    private static final int NOTIFICATION_RESULT_CANCELLED = 1;
    private static final int NOTIFICATION_RESULT_PERMISSION_DENIED = 2;
    private static final int NOTIFICATION_RESULT_AUTH_ERROR = 3;
    private static final int NOTIFICATION_RESULT_POST_ERROR = 4;

    // 歩数画面（Issue #1249）。実装は `PedometerHelper`（フィールド初期化子で生成し、
    // STARTED 前に許可ランチャーを登録する）。Rust の `android_pedometer.rs` が JNI で呼ぶ。
    private final PedometerHelper pedometerHelper = new PedometerHelper(this);

    void requestPedometerRead(long requestId) {
        pedometerHelper.read(requestId);
    }

    void cancelPedometerRead(long requestId) {
        pedometerHelper.cancel(requestId);
    }

    // ヘルスデータ（今日の歩数、Issue #1100）。実装は `HealthDataHelper`（フィールド初期化子で
    // 生成し、STARTED 前に許可ランチャーを登録する）。Rust の `android_health_data.rs` が JNI で呼ぶ。
    private final HealthDataHelper healthDataHelper = new HealthDataHelper(this);

    void requestHealthDataRead(long requestId) {
        healthDataHelper.read(requestId);
    }

    void cancelHealthDataRead(long requestId) {
        healthDataHelper.cancel(requestId);
    }

    // 端末内翻訳（Issue #537）。実装は `TranslationHelper`。Rust の `android_translation.rs` が JNI で呼ぶ。
    private final TranslationHelper translationHelper = new TranslationHelper(this);

    void requestTranslation(long requestId, String text, String source, String target) {
        translationHelper.request(requestId, text, source, target);
    }

    void cancelTranslation(long requestId) {
        translationHelper.cancel(requestId);
    }

    // 住所の座標変換（Issue #693）。実装は `GeocodingHelper`。Rust の `android_geocoding_search.rs` が JNI で呼ぶ。
    private final GeocodingHelper geocodingHelper = new GeocodingHelper();

    void requestGeocode(long requestId, String address, int maxResults) {
        geocodingHelper.request(getApplicationContext(), requestId, address, maxResults);
    }

    void cancelGeocode(long requestId) {
        geocodingHelper.cancel(requestId);
    }

    // スペルチェック（Issue #580）。実装は `SpellCheckHelper`。Rust の `android_spell_check.rs` が JNI で呼ぶ。
    private final SpellCheckHelper spellCheckHelper = new SpellCheckHelper(this);

    void requestSpellCheck(long requestId, String text, String language, int suggestionsLimit) {
        spellCheckHelper.request(requestId, text, language, suggestionsLimit);
    }

    void cancelSpellCheck(long requestId) {
        spellCheckHelper.cancel(requestId);
    }

    // 音楽ライブラリ画面（Issue #485）。実装は `MusicLibraryHelper`（許可ランチャーを STARTED 前に登録）。
    // Rust の `android_music_library.rs` が JNI で呼ぶ。
    private final MusicLibraryHelper musicLibraryHelper = new MusicLibraryHelper(this);

    void requestMusicLibraryList(long requestId) {
        musicLibraryHelper.read(requestId);
    }

    void cancelMusicLibraryList(long requestId) {
        musicLibraryHelper.cancel(requestId);
    }

    // Wi-Fi 情報画面（Issue #516）。実装は `WifiInfoHelper`（許可ランチャーを STARTED 前に登録）。
    // Rust の `android_wifi_info.rs` が JNI で呼ぶ。
    private final WifiInfoHelper wifiInfoHelper = new WifiInfoHelper(this);

    void requestWifiInfo(long requestId) {
        wifiInfoHelper.read(requestId);
    }

    void cancelWifiInfo(long requestId) {
        wifiInfoHelper.cancel(requestId);
    }

    // 指定した Wi-Fi への接続（Issue #1065）。実装は `WifiConnectHelper`。
    // Rust の `android_wifi_connect.rs` が JNI で呼ぶ。
    private final WifiConnectHelper wifiConnectHelper = new WifiConnectHelper(this);

    void requestWifiConnect(long requestId, String ssid, String password, boolean hexKey) {
        wifiConnectHelper.connect(requestId, ssid, password, hexKey);
    }

    // 無線の状態取得と切り替え（Issue #974）。実装は `RadioControlHelper`（ランチャーを STARTED 前に登録）。
    // Rust の `android_radio_control.rs` が JNI で呼ぶ。
    private final RadioControlHelper radioControlHelper = new RadioControlHelper(this);

    int radioControlRead(int kind) {
        return radioControlHelper.read(kind);
    }

    void requestRadioControlSet(long requestId, int kind, boolean on) {
        radioControlHelper.set(requestId, kind, on);
    }

    // 接続した機器からの取り込み（Issue #983）。実装は `ScannerDevicesHelper`。
    // Rust の `android_scanner_devices.rs` が JNI で呼ぶ。
    private final ScannerDevicesHelper scannerDevicesHelper = new ScannerDevicesHelper(this);

    void requestScannerDevicesImport(long requestId, long windowMs, int maxBytes) {
        scannerDevicesHelper.start(requestId, windowMs, maxBytes);
    }

    void cancelScannerDevicesImport(long requestId) {
        scannerDevicesHelper.cancel(requestId);
    }

    // NFC タグ読み取り（Issue #1045）。実装は `NfcReadHelper`。Rust の `android_nfc.rs` が JNI で呼ぶ。
    private final NfcReadHelper nfcReadHelper = new NfcReadHelper(this);

    void requestNfcRead(long requestId, int maxBytes) {
        nfcReadHelper.start(requestId, maxBytes);
    }

    void cancelNfcRead(long requestId) {
        nfcReadHelper.cancel(requestId);
    }

    // 方位（Issue #1406）。実装は `CompassHelper`。Rust の `android_compass.rs` が同期 JNI で呼ぶ。
    private final CompassHelper compassHelper = new CompassHelper(this);

    int compassStart() {
        return compassHelper.start();
    }

    long compassRead() {
        return compassHelper.read();
    }

    void compassStop() {
        compassHelper.stop();
    }

    // 気圧・照度・近接（Issue #611）。実装は `EnvironmentSensorsHelper`。Rust の
    // `android_environment_sensors.rs` が同期 JNI で呼ぶ。
    private final EnvironmentSensorsHelper environmentSensorsHelper = new EnvironmentSensorsHelper(this);

    int environmentSensorsStart(int kind) {
        return environmentSensorsHelper.start(kind);
    }

    long[] environmentSensorsRead(int kind) {
        return environmentSensorsHelper.read(kind);
    }

    void environmentSensorsStop(int kind) {
        environmentSensorsHelper.stop(kind);
    }

    // ジャイロ・磁気・線形加速度の生値（Issue #901）。実装は `MotionRawSensorsHelper`。Rust の
    // `android_motion_raw_sensors.rs` が同期 JNI で呼ぶ。
    private final MotionRawSensorsHelper motionRawSensorsHelper = new MotionRawSensorsHelper(this);

    int motionRawSensorsStart(int kind) {
        return motionRawSensorsHelper.start(kind);
    }

    long[] motionRawSensorsRead(int kind) {
        return motionRawSensorsHelper.read(kind);
    }

    void motionRawSensorsStop(int kind) {
        motionRawSensorsHelper.stop(kind);
    }

    // 音声再生（Issue #270）。実装は `MediaPlaybackHelper`。Rust の `android_media_playback.rs` が同期 JNI で呼ぶ。
    private final MediaPlaybackHelper mediaPlaybackHelper = new MediaPlaybackHelper();

    int mediaPlaybackLoad(long generation, byte[] wav) {
        return mediaPlaybackHelper.load(generation, wav);
    }

    int mediaPlaybackControl(long generation, int op) {
        return mediaPlaybackHelper.control(generation, op);
    }

    long[] mediaPlaybackSnapshot() {
        return mediaPlaybackHelper.snapshot();
    }

    void mediaPlaybackShutdown() {
        mediaPlaybackHelper.shutdown();
    }

    // PDF 表示（Issue #472）。実装は PdfPageHelper。Rust の android_pdf_viewer.rs がワーカーから同期 JNI で呼ぶ（署名 ([BI)[B）。
    byte[] pdfRenderFirstPage(byte[] pdf, int maxSide) {
        return PdfPageHelper.renderFirstPage(getCacheDir(), pdf, maxSide);
    }

    // ファイル・動画のプレビューとサムネイル（Issue #920）。実装は `FilePreviewHelper`。
    // Rust の `android_file_preview.rs` が JNI で呼ぶ（サムネイルは同期 `(I[BIJ)[B`、プレビューは非同期 `(JI[B)V`）。
    private final FilePreviewHelper filePreviewHelper = new FilePreviewHelper(this);

    byte[] filePreviewThumbnail(int kind, byte[] data, int maxSide, long timeoutMs) {
        return FilePreviewHelper.thumbnail(getCacheDir(), kind, data, maxSide, timeoutMs);
    }

    void requestFilePreview(long requestId, int kind, byte[] data) {
        filePreviewHelper.preview(requestId, kind, data);
    }

    // 音声変換（Issue #314）。実装は `MediaTranscodeHelper`。Rust の `android_media_transcode.rs` がワーカースレッドから同期 JNI で呼ぶ。
    byte[] mediaTranscode(byte[] pcmLe, int sampleRate, long remainingMs) {
        return MediaTranscodeHelper.transcode(getCacheDir(), pcmLe, sampleRate, remainingMs);
    }

    // 動画の変換（Issue #1017）。実装は `VideoTranscodeHelper`。Rust の `android_video_transcode.rs` が同期 JNI で呼ぶ。
    byte[] videoTranscode(int sw, int sh, int fps, int frames, int tw, int th, long remainingMs) {
        return VideoTranscodeHelper.transcode(getCacheDir(), sw, sh, fps, frames, tw, th, remainingMs);
    }

    // 端末・アプリの真正性証明（Issue #678）。実装は `AppAttestHelper`。Rust の `android_app_attest.rs` がワーカースレッドから同期 JNI で呼ぶ。
    byte[] appAttestRandom(int len) {
        return AppAttestHelper.random(len);
    }

    byte[] appAttest(byte[] challenge, long remainingMs) {
        return AppAttestHelper.attest(challenge, remainingMs);
    }

    // OS の検索への登録（Issue #562）。実装は `SpotlightIndexHelper`。Rust の `android_spotlight_index.rs` がワーカースレッドから同期 JNI で呼ぶ。
    int spotlightIndex(String domain, String id, String title, String text, long remainingMs) {
        return SpotlightIndexHelper.index(getApplicationContext(), domain, id, title, text, remainingMs);
    }

    int spotlightRemoveAll(String domain, long remainingMs) {
        return SpotlightIndexHelper.removeAll(getApplicationContext(), domain, remainingMs);
    }

    // MIDI 入力（Issue #1400）。実装は `MidiHelper`。Rust の `android_midi.rs` が JNI で呼ぶ。
    private final MidiHelper midiHelper = new MidiHelper(this);

    int midiStart(long epoch) {
        return midiHelper.start(epoch);
    }

    void midiStop() {
        midiHelper.stop();
    }

    // スリープ・画面ロック・解除の通知（Issue #811）。実装は `SleepLockEventsHelper`。
    // Rust の `android_sleep_lock_events.rs` が JNI で呼ぶ。
    private final SleepLockEventsHelper sleepLockEventsHelper = new SleepLockEventsHelper(this);

    int sleepLockEventsStart(long id) {
        return sleepLockEventsHelper.start(id);
    }

    void sleepLockEventsStop(long id) {
        sleepLockEventsHelper.stop(id);
    }

    int sleepLockEventsRead() {
        return sleepLockEventsHelper.read();
    }

    // アプリの前面・背面の通知（Issue #895）。実装は `AppLifecycleHelper`。
    // Rust の `android_app_lifecycle.rs` が JNI で呼ぶ。
    private final AppLifecycleHelper appLifecycleHelper = new AppLifecycleHelper(this);

    int appLifecycleStart(long id) {
        return appLifecycleHelper.start(id);
    }

    void appLifecycleStop(long id) {
        appLifecycleHelper.stop(id);
    }

    int appLifecycleRead() {
        return appLifecycleHelper.read();
    }

    // メモリ逼迫・負荷の通知（Issue #913）。実装は `MemoryPressureHelper`。
    // Rust の `android_memory_pressure.rs` が JNI で呼ぶ。
    private final MemoryPressureHelper memoryPressureHelper = new MemoryPressureHelper(this);

    int memoryPressureStart(long id) {
        return memoryPressureHelper.start(id);
    }

    void memoryPressureStop(long id) {
        memoryPressureHelper.stop(id);
    }

    int memoryPressureRead() {
        return memoryPressureHelper.read();
    }

    // 外部ストレージの着脱（Issue #859）。実装は `RemovableStorageHelper`。
    // Rust の `android_removable_storage.rs` が JNI で呼ぶ。
    private final RemovableStorageHelper removableStorageHelper = new RemovableStorageHelper(this);

    int removableStorageSupport() {
        return removableStorageHelper.support();
    }

    int removableStorageStart(long id) {
        return removableStorageHelper.start(id);
    }

    void removableStorageStop(long id) {
        removableStorageHelper.stop(id);
    }

    String removableStorageList() {
        return removableStorageHelper.list();
    }

    // 音声の割り込み・セッション種別（Issue #842）。実装は `AudioSessionHelper`。
    // Rust の `android_audio_session.rs` が JNI で呼ぶ。
    private final AudioSessionHelper audioSessionHelper = new AudioSessionHelper(this);

    int audioSessionStart(long id, int kind) {
        return audioSessionHelper.start(id, kind);
    }

    int audioSessionStop(long id) {
        return audioSessionHelper.stop(id);
    }

    int audioSessionRead() {
        return audioSessionHelper.read();
    }

    // カメラのライブ映像（Issue #814）。実装は `CameraPreviewHelper`（許可ランチャーを STARTED 前に登録）。
    // Rust の `android_camera_preview_stream.rs` が JNI で呼ぶ。
    private final CameraPreviewHelper cameraPreviewHelper = new CameraPreviewHelper(this);

    int cameraPreviewStart(long id) {
        return cameraPreviewHelper.start(id);
    }

    void cameraPreviewStop(long id) {
        cameraPreviewHelper.stop(id);
    }

    float[] cameraPreviewZoomRange(long id) {
        return cameraPreviewHelper.zoomRange(id);
    }

    int cameraPreviewSetZoom(long id, float ratio) {
        return cameraPreviewHelper.setZoom(id, ratio);
    }

    // 通話 UI 統合（Issue #278）。実装は `CallUiHelper`（許可ランチャーを STARTED 前に登録）。
    // Rust の `android_call_ui.rs` が JNI で呼ぶ。
    private final CallUiHelper callUiHelper = new CallUiHelper(this);

    int callUiReport(long id) {
        return callUiHelper.report(id);
    }

    void callUiEnd(long id) {
        callUiHelper.end(id);
    }

    // アプリアイコンのバッジ（Issue #1362）。実装は `AppBadgeHelper`（許可ランチャーを STARTED 前に登録）。
    // Rust の `android_app_badge.rs` が JNI で呼ぶ。
    private final AppBadgeHelper appBadgeHelper = new AppBadgeHelper(this);

    void requestAppBadge(long requestId, int value) {
        appBadgeHelper.set(requestId, value);
    }

    // ライブアクティビティ（Issue #296）。実装は `LiveActivityHelper`（許可ランチャーを STARTED 前に登録）。
    // Rust の `android_live_activity.rs` が JNI で呼ぶ。
    private final LiveActivityHelper liveActivityHelper = new LiveActivityHelper(this);

    void requestLiveActivity(long requestId, int op, int value) {
        liveActivityHelper.submit(requestId, op, value);
    }

    void liveActivityDismissAll() {
        liveActivityHelper.dismissAll();
    }

    // メール・SMS 作成画面（Issue #1386）。実装は `ComposeHelper`。
    // Rust の `android_compose_mail_sms.rs` が JNI で呼ぶ。
    private final ComposeHelper composeHelper = new ComposeHelper(this);

    void requestCompose(long requestId, int kind, String to, String cc, String bcc,
            String subject, String body, byte[] attData, String attMime, String attName) {
        composeHelper.compose(requestId, kind, to, cc, bcc, subject, body, attData, attMime,
                attName);
    }

    // OS の設定画面を開く（Issue #800）。実装は `SystemSettingsHelper`。
    // Rust の `android_open_system_settings.rs` が JNI で呼ぶ。
    private final SystemSettingsHelper systemSettingsHelper = new SystemSettingsHelper(this);

    void requestOpenSystemSettings(long requestId, int page) {
        systemSettingsHelper.open(requestId, page);
    }

    // 電話発信・地図など他アプリの起動（Issue #863）。実装は `ExternalAppLaunchHelper`。
    // Rust の `android_external_app_launch.rs` が JNI で呼ぶ。
    private final ExternalAppLaunchHelper externalAppLaunchHelper = new ExternalAppLaunchHelper(this);

    void requestExternalAppLaunch(long requestId, int target, String uri) {
        externalAppLaunchHelper.launch(requestId, target, uri);
    }

    // OAuth 用 Web 認証セッション（Issue #534）。実装は `WebAuthSessionHelper`。
    // Rust の `android_web_auth_session.rs` が JNI で呼ぶ。
    private final WebAuthSessionHelper webAuthSessionHelper = new WebAuthSessionHelper(this);

    void requestWebAuthStart(long requestId, String url) {
        webAuthSessionHelper.start(requestId, url);
    }

    void requestWebAuthCancel(long requestId) {
        webAuthSessionHelper.cancel(requestId);
    }

    byte[] webAuthRandom(int len) {
        return WebAuthSessionHelper.random(len);
    }

    byte[] webAuthSha256(byte[] data) {
        return WebAuthSessionHelper.sha256(data);
    }

    // 付属機器のペアリング（Issue #741）。実装は `CompanionDeviceHelper`（結果ランチャーを STARTED 前に登録）。
    private final CompanionDeviceHelper companionDeviceHelper = new CompanionDeviceHelper(this);

    void requestCompanionDevicePick(long requestId) {
        companionDeviceHelper.pick(requestId);
    }

    // アプリ内ブラウザ（Issue #703）。実装は `InAppBrowserHelper`。
    // Rust の `android_in_app_browser.rs` が JNI で呼ぶ。
    private final InAppBrowserHelper inAppBrowserHelper = new InAppBrowserHelper(this);

    void requestInAppBrowserOpen(long requestId, String url) {
        inAppBrowserHelper.open(requestId, url);
    }

    // クイック設定タイル（Issue #720）。実装は `QuickSettingsTileStore`。Rust の
    // `android_quick_settings_tile.rs` が JNI（署名 `()Ljava/lang/String;` と `()J`）で呼ぶ。
    String quickSettingsTileCountRaw() {
        return QuickSettingsTileStore.readRaw(getApplicationContext());
    }

    long quickSettingsTileFailedPresses() {
        return QuickSettingsTileStore.failedPresses();
    }

    // 車載ディスプレイ（Issue #1117）。実装は `CarDisplayScreen`。Rust の
    // `android_car_display.rs` が JNI（署名 `(Ljava/lang/String;)V` と `()Z`）で呼ぶ。
    void carDisplaySetText(String text) {
        CarDisplayScreen.setText(text);
    }

    boolean carDisplayIsShowing() {
        return CarDisplayScreen.isShowing();
    }

    // 印刷ダイアログ（Issue #328）。実装は `PrintHelper`。
    // Rust の `android_print.rs` が JNI で呼ぶ。
    private final PrintHelper printHelper = new PrintHelper(this);

    void requestPrint(long requestId, String title, String body) {
        printHelper.print(requestId, title, body);
    }

    // 画面の内容の印刷（#1012）も同じヘルパー。画素は ARGB の int[]。
    void requestPrintImage(long requestId, int[] argb, int width, int height) {
        printHelper.printImage(requestId, argb, width, height);
    }

    // 画面録画（Issue #1337）。実装は `ScreenCaptureHelper` / `ScreenCaptureService`。
    // Rust の `android_screen_capture.rs` が JNI で呼ぶ。
    private final ScreenCaptureHelper screenCaptureHelper = new ScreenCaptureHelper(this);

    void requestScreenCaptureStart(long requestId, int maxMs) {
        screenCaptureHelper.start(requestId, maxMs);
    }

    void requestScreenCaptureStop(long requestId) {
        screenCaptureHelper.stop(requestId);
    }

    // 動画の撮影（Issue #823）。実装は `VideoCaptureHelper`。Rust の
    // `android_video_capture.rs` が JNI で呼ぶ。
    private final VideoCaptureHelper videoCaptureHelper = new VideoCaptureHelper(this);

    void requestVideoCapture(long requestId, int maxSecs) {
        videoCaptureHelper.capture(requestId, maxSecs);
    }

    // 画面スリープ抑止（Issue #1280）。実装は `ScreenWakeLockHelper`。Rust の
    // `android_screen_wake_lock.rs` が egui スレッドから同期で JNI 呼び出しする。
    private final ScreenWakeLockHelper screenWakeLockHelper = new ScreenWakeLockHelper(this);

    int screenWakeLockSet(boolean keep) {
        return screenWakeLockHelper.set(keep);
    }

    boolean screenWakeLockIsHeld() {
        return screenWakeLockHelper.isHeld();
    }

    // 画面輝度の上書き（Issue #635）。実装は `ScreenBrightnessHelper`。Rust の
    // `android_screen_brightness.rs` が egui スレッドから同期で JNI 呼び出しする。
    private final ScreenBrightnessHelper screenBrightnessHelper = new ScreenBrightnessHelper(this);

    int screenBrightnessSet(float level) {
        return screenBrightnessHelper.set(level);
    }

    int screenBrightnessClear() {
        return screenBrightnessHelper.clear();
    }

    float screenBrightnessRead() {
        return screenBrightnessHelper.read();
    }

    // 画面向きの取得・固定（Issue #657）。実装は `ScreenOrientationHelper`。Rust の
    // `android_screen_orientation.rs` が egui スレッドから同期で JNI 呼び出しする。
    private final ScreenOrientationHelper screenOrientationHelper = new ScreenOrientationHelper(this);

    int screenOrientationRequest(int lock) {
        return screenOrientationHelper.request(lock);
    }

    int[] screenOrientationRead() {
        return screenOrientationHelper.read();
    }

    // 画面キャプチャの防止・検知（Issue #584）。実装は `CaptureProtectionHelper`。Rust の
    // `android_capture_protection.rs` が egui スレッドから JNI 呼び出しする。
    private final CaptureProtectionHelper captureProtectionHelper = new CaptureProtectionHelper(this);

    int captureProtectionSet(boolean on) {
        return captureProtectionHelper.set(on);
    }

    int captureProtectionIsProtected() {
        return captureProtectionHelper.isProtected();
    }

    int captureProtectionSupport() {
        return captureProtectionHelper.support();
    }

    int captureProtectionRecording() {
        return captureProtectionHelper.recording();
    }

    long captureProtectionScreenshotCount() {
        return captureProtectionHelper.screenshotCount();
    }

    // 折りたたみの姿勢・ヒンジ（Issue #1098）。実装は `FoldablePostureHelper`。Rust の
    // `android_foldable_posture.rs` が egui スレッドから同期で JNI 呼び出しする。
    private final FoldablePostureHelper foldablePostureHelper = new FoldablePostureHelper(this);

    int[] foldablePostureRead() {
        return foldablePostureHelper.read();
    }

    // ライト（トーチ）（Issue #461）。実装は `FlashlightHelper`。Rust の `android_flashlight.rs` が
    // egui スレッドから同期で JNI 呼び出しする。
    private final FlashlightHelper flashlightHelper = new FlashlightHelper(this);

    int flashlightSet(boolean on) {
        return flashlightHelper.set(on);
    }

    int flashlightRead() {
        return flashlightHelper.read();
    }

    // システムの効果音（Issue #940）。実装は `SystemSoundHelper`。Rust の `android_system_sound.rs` が
    // egui スレッドから同期で JNI 呼び出しする（0=要求した／1=鳴らさなかった／2=失敗）。
    private final SystemSoundHelper systemSoundHelper = new SystemSoundHelper();

    int systemSoundBeep() {
        return systemSoundHelper.beep();
    }

    // システム音量（Issue #787）。実装は `VolumeControlHelper`。Rust の `android_volume_control.rs` が
    // egui スレッドから同期で JNI 呼び出しする。
    private final VolumeControlHelper volumeControlHelper = new VolumeControlHelper(this);

    int[] volumeControlRead() {
        return volumeControlHelper.read();
    }

    int volumeControlAdjust(int direction) {
        return volumeControlHelper.adjust(direction);
    }

    @Override
    protected void onStart() {
        super.onStart();
        appLifecycleHelper.onStart();
        flashlightHelper.onStart();
        captureProtectionHelper.onStart();
        foldablePostureHelper.onStart();
    }

    @Override
    protected void onStop() {
        appLifecycleHelper.onStop();
        flashlightHelper.onStop();
        captureProtectionHelper.onStop();
        foldablePostureHelper.onStop();
        blePeripheralHelper.onStop();
        super.onStop();
    }

    // ロック画面・OS のメディア操作（Issue #383）。実装は `NowPlayingHelper`。Rust の
    // `android_now_playing.rs` が egui スレッドから同期で JNI 呼び出しする。
    private final NowPlayingHelper nowPlayingHelper = new NowPlayingHelper(this);

    long nowPlayingPublish(String title, boolean playing, long positionMs, long durationMs) {
        return nowPlayingHelper.publish(title, playing, positionMs, durationMs);
    }

    void nowPlayingClear() {
        nowPlayingHelper.clear();
    }

    // LAN のサービス探索（Issue #489）。実装は `LanDiscoveryHelper`。Rust の `android_lan_discovery.rs` が JNI で呼ぶ。
    private final LanDiscoveryHelper lanDiscoveryHelper = new LanDiscoveryHelper(this);

    void requestLanDiscovery(long requestId, int durationMs) {
        lanDiscoveryHelper.request(requestId, durationMs);
    }

    // Bluetooth Classic（Issue #661）。実装は `BluetoothClassicHelper`（許可ランチャーを STARTED 前に登録）。
    // Rust の `android_bluetooth_classic.rs` が JNI で呼ぶ。
    private final BluetoothClassicHelper bluetoothClassicHelper = new BluetoothClassicHelper(this);

    void requestBluetoothClassic(long requestId, int durationMs) {
        bluetoothClassicHelper.request(requestId, durationMs);
    }

    // ペアリング済み一覧（シグネチャ `()Ljava/lang/String;`）。詰め方は `BluetoothClassicHelper.paired` 冒頭を参照。
    private String bluetoothClassicPaired() {
        return BluetoothClassicHelper.paired(getApplicationContext());
    }

    // BLE の周辺機器役（Issue #1123）。実装は `BlePeripheralHelper`（許可ランチャーを STARTED 前に登録）。
    // Rust の `android_ble_peripheral.rs` が JNI で呼ぶ。戻り値の意味は `BlePeripheralHelper.start` / `stop` を参照。
    private final BlePeripheralHelper blePeripheralHelper = new BlePeripheralHelper(this);

    int blePeripheralStart(long token, String serviceUuid, String characteristicUuid, String value) {
        return blePeripheralHelper.start(token, serviceUuid, characteristicUuid, value);
    }

    int blePeripheralStop() {
        return blePeripheralHelper.stop();
    }

    // 近くの端末との直接通信（Issue #513）。実装は `LocalPeerHelper`（許可ランチャーを STARTED 前に登録）。
    // Rust の `android_local_peer.rs` が JNI で呼ぶ。
    private final LocalPeerHelper localPeerHelper = new LocalPeerHelper(this);

    void requestLocalPeer(long requestId, String ownName, String outgoing, int windowMs) {
        localPeerHelper.request(requestId, ownName, outgoing, windowMs);
    }

    // OS 管理のバックグラウンドダウンロード（Issue #392）。実装は `BackgroundDownloadHelper`。Rust の
    // `android_background_download.rs` が同期 JNI で呼ぶ（コールバックは無い）。
    private final BackgroundDownloadHelper backgroundDownloadHelper = new BackgroundDownloadHelper(this);

    int backgroundDownloadStart(String url) {
        return backgroundDownloadHelper.start(url);
    }

    int backgroundDownloadCancel() {
        return backgroundDownloadHelper.cancel();
    }

    long[] backgroundDownloadStatus() {
        return backgroundDownloadHelper.status();
    }

    // OS 管理のバックグラウンド送信（Issue #837）。実装は `BackgroundUploadHelper`。Rust の
    // `android_background_upload.rs` が同期 JNI で呼び、状態変化は native コールバックで合図される。
    private final BackgroundUploadHelper backgroundUploadHelper = new BackgroundUploadHelper(this);

    int backgroundUploadStart(String url) {
        return backgroundUploadHelper.start(url);
    }

    int backgroundUploadCancel() {
        return backgroundUploadHelper.cancel();
    }

    long[] backgroundUploadStatus() {
        return backgroundUploadHelper.status();
    }

    int backgroundUploadObserve() {
        return backgroundUploadHelper.observe();
    }

    // ペン・筆圧入力（Issue #357）。実装は `PenStylusHelper`。
    private final PenStylusHelper penStylusHelper = new PenStylusHelper();

    // 全画面表示（Issue #1314）。実装は `FullscreenHelper`。Rust の
    // `android_fullscreen_immersive.rs` が egui スレッドから同期で JNI 呼び出しする。
    private final FullscreenHelper fullscreenHelper = new FullscreenHelper(this);

    int fullscreenSet(boolean on) {
        return fullscreenHelper.set(on);
    }

    // ピクチャ・イン・ピクチャ（Issue #366）。実装は `PictureInPictureHelper`。Rust の
    // `android_picture_in_picture.rs` が egui スレッドから同期 JNI で呼ぶ。
    private final PictureInPictureHelper pictureInPictureHelper = new PictureInPictureHelper(this);

    int pipPrepare(long generation, int w, int h) {
        return pictureInPictureHelper.prepare(generation, w, h);
    }

    int pipStart(long generation) {
        return pictureInPictureHelper.start(generation);
    }

    int pipStop(long generation) {
        return pictureInPictureHelper.stop(generation);
    }

    long[] pipSnapshot() {
        return pictureInPictureHelper.snapshot();
    }

    void pipShutdown() {
        pictureInPictureHelper.shutdown();
    }

    @Override
    public void onPictureInPictureModeChanged(
            boolean isInPictureInPictureMode, android.content.res.Configuration newConfig) {
        super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig);
        pictureInPictureHelper.onPictureInPictureModeChanged(isInPictureInPictureMode);
    }

    // 通知チャンネル ID（`postNotification`/`onCreate` から参照）。
    private static final String NOTIFICATION_CHANNEL_ID = "fandhe_default";

    // コールバック時に requestId を思い出すためだけの唯一の static（`biometricRequest`
    // と同じ理由）。UI スレッドのみで読み書きするため追加の同期は不要。
    private static final RequestTracker notificationRequest = new RequestTracker();

    // 権限ダイアログ表示中に `requestNotificationPermission` コールバックが読む
    // タイトル・本文（`micRequestedMaxDurationMs` と同じ理由で退避する）。UI スレッド
    // のみで読み書きする。
    private String pendingNotificationTitle;
    private String pendingNotificationBody;

    // 通知の実行時許可要求（`requestMicPermission` と同型）。許可が下りたら
    // `postNotification` を呼ぶ。ダイアログ自体が Activity を pause させるため、
    // `requestMicPermission` と異なり `activityResumed` はここでは見ない: 通知の
    // 投稿は背景でも正当な操作であり、`micStartPendingResume` のような保留は不要
    // （実装計画 §4 参照）。
    private final ActivityResultLauncher<String> requestNotificationPermission =
            registerForActivityResult(new ActivityResultContracts.RequestPermission(), granted -> {
                long id = notificationRequest.take();
                if (id == RequestTracker.NONE) {
                    // 既に破棄済み（Activity 再生成等）。
                    return;
                }
                String title = pendingNotificationTitle;
                String body = pendingNotificationBody;
                pendingNotificationTitle = null;
                pendingNotificationBody = null;
                if (!granted) {
                    nativeOnNotificationResult(id, NOTIFICATION_RESULT_PERMISSION_DENIED);
                    return;
                }
                postNotification(id, title, body);
            });

    // ドラッグ & ドロップ受け取り（Issue #1016、親 #1013 の子 (b')）。受け取り側のみ
    // （送り側 `View.startDragAndDrop` は親 issue の方針でスコープ外）。Rust 側の
    // 集約器 `crates/fandhe-egui-proxy/src/device/drop_session.rs` へ、`dragListener`
    // が観測した 1 件ずつのイベントを push する（`biometricRequest`/`filePickRequest`
    // のような要求 1 件 → 応答 1 件の `RequestTracker` 型ではなく、iOS
    // （`fandhe_ios_drag_drop.m`）と同じ「セッション id を世代として push し続ける」
    // 型。`drop_session.rs` モジュール doc「Android 経路」参照）。
    //
    // 現在表示中の画面が drag_drop かどうかは Rust 側 `nativeDragDropIsActive()`
    // （`crate::android`、feature 無条件で 1 つだけ定義）に問い合わせる。`false` が
    // 返れば `ACTION_DRAG_STARTED` の時点でこのドラッグ全体を無視し、以降の
    // `nativeOnDragDropHover`/`Begin`/`Wants`/`Item`（`device/android.rs`、feature
    // 有効時のみ存在）を一切呼ばない（実装計画 §3-2「スタブを置かない理由」）。
    private static native boolean nativeDragDropIsActive();

    // `ACTION_DRAG_ENTERED`/`ACTION_DRAG_EXITED` から呼ぶ。`entered` は「領域内に
    // 入った」かどうか。
    private static native void nativeOnDragDropHover(boolean entered);

    // `ACTION_DROP` の冒頭から呼ぶ。`total` 件のドロップを開始し、払い出された
    // セッション id（世代番号を兼ねる）を返す。以降の `nativeDragDropWants`/
    // `nativeOnDragDropItem` はこの id を使う。
    private static native long nativeOnDragDropBegin(int total);

    // 読み取りスレッドが 1 件読む **前** に呼ぶ。`false` なら読み取らず break する
    // （旧世代への差し替え・上限 [`MAX_DROP_FILES`]（Rust 側 SSOT）超過分の両方を
    // ここで打ち切る、実装計画 §3-3）。
    private static native boolean nativeDragDropWants(long session, int index);

    // 読み取りスレッドが 1 件読み終える（またはフォルダ/読めないと判定する）たびに
    // 呼ぶ。code: 0=ファイル（name/fd 必須）／1=フォルダ／2=読めない（URI 無し・
    // 上限到達による埋め・`openFileDescriptor` 失敗等を一律ここへ畳む。詳細分類は
    // 行わない）。fd: code == 0 のときのみ有効。`ParcelFileDescriptor.detachFd()` で
    // 所有権を Rust へ渡した生 fd（detach 後は Java 側で close しない）。読み取り・
    // 8 MiB 判定は Rust 側 `fandhe_device::drag_drop::read_dropped_from` に一本化する
    // （`nativeOnFilePickerResult` と同じ方針）。
    private static native void nativeOnDragDropItem(
            long session, int index, int code, String name, int fd);

    private static final int DRAG_DROP_CODE_FILE = 0;
    private static final int DRAG_DROP_CODE_FOLDER = 1;
    private static final int DRAG_DROP_CODE_UNREADABLE = 2;

    // 同時に生きる読み取りスレッドの本数の上限（デスクトップ側 `MAX_READERS` と同じ値）。
    // ponytail: 固定の本数上限。応答しないプロバイダの読み取りが上限まで溜まると、
    // それらが返るまで新しいドロップは全 slot が「読めない」になる。I/O を中断できる
    // API へ移すときに見直す。
    private static final int DRAG_DROP_MAX_READERS = 4;
    private static final AtomicInteger dragDropActiveReaders = new AtomicInteger(0);

    // `ACTION_DRAG_STARTED` を受理した（`dragAccepted = true`）のに `ACTION_DROP` が
    // 一度も来ないまま `ACTION_DRAG_ENDED` に至ったか（＝領域外で離された/取り消された）
    // を判定するためのインスタンスフィールド（Activity 再生成時は新インスタンスの
    // `false` から自然に始まる。`onPause` は何もしない: 表示中でも pause 状態の
    // Activity がドロップを受け取るマルチウィンドウ構成が正常なため、実装計画 §3-5）。
    private boolean dragAccepted;
    private boolean dropReceived;

    /**
     * `getWindow().getDecorView()` に取り付ける {@link View.OnDragListener}
     * （`onCreate` で取り付け、Activity 再生成のたびに新インスタンスへ付け直される）。
     * `ACTION_DRAG_STARTED` で `nativeDragDropIsActive()` が false ならこのドラッグ
     * 全体を無視する（実装計画 §3-1）。
     */
    private final View.OnDragListener dragListener = (view, event) -> {
        switch (event.getAction()) {
            case DragEvent.ACTION_DRAG_STARTED:
                if (!nativeDragDropIsActive()) {
                    return false;
                }
                dragAccepted = true;
                dropReceived = false;
                return true;
            case DragEvent.ACTION_DRAG_ENTERED:
                nativeOnDragDropHover(true);
                return true;
            case DragEvent.ACTION_DRAG_EXITED:
                nativeOnDragDropHover(false);
                return true;
            case DragEvent.ACTION_DROP:
                dropReceived = true;
                handleDragDrop(event);
                return true;
            case DragEvent.ACTION_DRAG_ENDED:
                // ドロップされないまま終わった（領域外で離された/取り消された）場合
                // のみキャンセルを通知する。`ACTION_DROP` 済みなら `handleDragDrop` が
                // 別途完了を通知するため、ここでは何もしない（契約表 10「三値」）。
                if (dragAccepted && !dropReceived) {
                    nativeOnDragDropHover(false);
                }
                dragAccepted = false;
                return true;
            default:
                return true;
        }
    };

    /**
     * {@code ACTION_DROP} を受け取った UI スレッドから呼ぶ。権限取得とセッション開始・
     * 読み取り枠の確保は同期に行い（軽量な同期呼び出しのみ）、実読み取りは専用スレッドへ
     * 渡す（`deliverPickedDocument` と同じ「名前解決・fd 取得を UI スレッドでやらない」
     * 方針）。読み取り枠は必ずスレッド生成前に確保する。生成後の判定だと、短時間に
     * 多数のドロップが届いたとき枠を取れない要求にもスレッドが立ち上がり続け、
     * スレッド数・メモリ使用量が無制限に増える（codex レビュー指摘、PR #1022）。
     */
    private void handleDragDrop(DragEvent event) {
        ClipData clipData = event.getClipData();
        int total = clipData == null ? 0 : clipData.getItemCount();
        DragAndDropPermissions perms = requestDragAndDropPermissions(event);
        long session = nativeOnDragDropBegin(total);
        if (!acquireDragDropReaderSlot()) {
            // 上限に達している: スレッドを立てず、読み込み中のまま固まった表示にせず
            // 各 slot を「読めない」で埋める（実装計画 §3-3）。
            for (int i = 0; i < total && nativeDragDropWants(session, i); i++) {
                nativeOnDragDropItem(session, i, DRAG_DROP_CODE_UNREADABLE, null, -1);
            }
            if (perms != null) {
                perms.release();
            }
            return;
        }
        new Thread(() -> readDroppedItems(session, clipData, total, perms), "fandhe-drag-drop")
                .start();
    }

    /**
     * 読み取りスレッド本体（1 ドロップにつき 1 本、読み取り枠は呼び出し元
     * {@link #handleDragDrop} で確保済み）。{@code perms} は成否を問わず必ず
     * {@link DragAndDropPermissions#release()} する（契約表 9「失敗時のリソース解放」。
     * `perms` は null になり得る）。
     */
    private void readDroppedItems(
            long session, ClipData clipData, int total, DragAndDropPermissions perms) {
        try {
            try {
                for (int i = 0; i < total; i++) {
                    if (!nativeDragDropWants(session, i)) {
                        // 旧世代への差し替え、または上限超過分（`drop_session::wants`
                        // 側の判定、実装計画 §3-3）。
                        break;
                    }
                    deliverDropItem(session, i, clipData.getItemAt(i).getUri());
                }
            } finally {
                dragDropActiveReaders.decrementAndGet();
            }
        } finally {
            if (perms != null) {
                perms.release();
            }
        }
    }

    /** [`DRAG_DROP_MAX_READERS`] 未満なら枠を 1 つ確保する（デスクトップ `ReaderSlot` と同型）。 */
    private static boolean acquireDragDropReaderSlot() {
        while (true) {
            int current = dragDropActiveReaders.get();
            if (current >= DRAG_DROP_MAX_READERS) {
                return false;
            }
            if (dragDropActiveReaders.compareAndSet(current, current + 1)) {
                return true;
            }
        }
    }

    /**
     * 1 件ぶんの読み込み（フォルダ判定 → 名前解決 → fd 取得）を行い、結果を
     * {@link #nativeOnDragDropItem} で通知する。URI・ファイル名・例外メッセージは
     * ログへ出力しない（他アプリの機密文書になり得る、security.md A09）。
     */
    private void deliverDropItem(long session, int index, Uri uri) {
        if (uri == null) {
            nativeOnDragDropItem(session, index, DRAG_DROP_CODE_UNREADABLE, null, -1);
            return;
        }
        String type;
        try {
            type = getContentResolver().getType(uri);
        } catch (RuntimeException e) {
            nativeOnDragDropItem(session, index, DRAG_DROP_CODE_UNREADABLE, null, -1);
            return;
        }
        if (DocumentsContract.Document.MIME_TYPE_DIR.equals(type)) {
            nativeOnDragDropItem(session, index, DRAG_DROP_CODE_FOLDER, null, -1);
            return;
        }

        String name = resolveDisplayName(uri);
        // `detachFd()` の契約は `deliverPickedDocument` と同じ（コメント参照）。失敗時も
        // 解決済みの `name` を渡す（null に畳むと、複数ドロップ混在時にどの項目が失敗
        // したか利用者に伝わらない。Cursor Bugbot 指摘、PR #1022）。
        try (ParcelFileDescriptor pfd = getContentResolver().openFileDescriptor(uri, "r")) {
            if (pfd == null) {
                nativeOnDragDropItem(session, index, DRAG_DROP_CODE_UNREADABLE, name, -1);
                return;
            }
            int fd = pfd.detachFd();
            nativeOnDragDropItem(session, index, DRAG_DROP_CODE_FILE, name, fd);
        } catch (IOException | RuntimeException e) {
            nativeOnDragDropItem(session, index, DRAG_DROP_CODE_UNREADABLE, name, -1);
        }
    }

    // マイク録音（Issue #957、Phase 11 型サンプル「マイク録音」の Android 経路）。
    // `MicRecorder` trait は通らない（`device/mic_record.rs` モジュール doc の
    // 「macOS/Android は pull 型 bridge」参照）。Rust 側は
    // `MainActivity.requestMicRecordStart(long, int)`/`requestMicRecordStop(long)`
    // （提示依頼）を呼び、結果は `nativeOnMicRecordStarted`/`nativeOnMicRecordResult`
    // で非同期に返す（`biometricRequest`/`requestBiometric` と同型）。
    private static native void nativeOnMicRecordStarted(long requestId);

    // code の意味（本ファイルが SSOT、Rust 側 `mic_record::error_from_code` の doc と
    // 対）: 0=成功（bytes/mime/durationMs 有効）／1=権限拒否／2=内部エラー／
    // 3=二重開始（busy）／4=許可確認中にキャンセル／5=ライフサイクル中断
    // （`onPause`/`onDestroy`/Activity 再生成で破棄）。
    private static native void nativeOnMicRecordResult(
            long requestId, int code, byte[] bytes, String mime, long durationMs);

    private static final int MIC_RESULT_SUCCESS = 0;
    private static final int MIC_RESULT_PERMISSION_DENIED = 1;
    private static final int MIC_RESULT_INTERNAL_ERROR = 2;
    private static final int MIC_RESULT_BUSY = 3;
    private static final int MIC_RESULT_CANCELLED = 4;
    private static final int MIC_RESULT_INTERRUPTED = 5;

    // 「いま待っている要求 id」（`biometricRequest` と同じ位置づけ）。UI スレッドのみで
    // 読み書きするが、`MediaRecorder`（`micRecorder`/`micTempFile`/`micStartedAt`）と
    // まとめて `micLock` の内側で判定・更新する（並行性の契約を単一ロックへ集約する、
    // device-bridge-contracts.md「並行性・世代」節）。
    private static final RequestTracker micRecordRequest = new RequestTracker();

    // `micLock` の内側でのみ読み書きする状態（ライフサイクル・並行性の契約を単一
    // ロックへ集約する、device-bridge-contracts.md 参照）。フィールド初期化子で
    // 参照する `requestMicPermission`（下記）より先に宣言する（javac の前方参照
    // 制限、`awaitingCameraResult`/`requestCameraPermission` の宣言順と同じ）。
    private final Object micLock = new Object();
    private MediaRecorder micRecorder;
    private File micTempFile;
    private long micStartedAt;
    // 権限ダイアログ表示中に `requestMicPermission` コールバックが読む上限
    // （`requestMicRecordStart` が受け取った値をそのまま退避する）。
    private volatile int micRequestedMaxDurationMs;

    // 許可結果が非前景中に届いたため `startMicRecorder` の実行を次の `onResume` へ
    // 保留していることを示すフラグ（cursor Bugbot 指摘、PR #962。location の
    // `locationStartRequestPending` と同型）。`micLock` の内側でのみ読み書きする。
    private boolean micStartPendingResume;

    // マイク録音の実行時許可要求（`requestCameraPermission`/`requestLocationPermission`
    // と同型）。許可が下りたら `startMicRecorder` を呼ぶ。
    private final ActivityResultLauncher<String> requestMicPermission =
            registerForActivityResult(new ActivityResultContracts.RequestPermission(), granted -> {
                synchronized (micLock) {
                    long id = micRecordRequest.current();
                    if (id == RequestTracker.NONE) {
                        // 既に破棄済み（`onPause`/`onDestroy`/再生成、または二重開始で
                        // 弾かれた後）。
                        return;
                    }
                    if (!granted) {
                        micRecordRequest.take();
                        nativeOnMicRecordResult(id, MIC_RESULT_PERMISSION_DENIED, null, null, 0);
                        return;
                    }
                    if (isFinishing() || isDestroyed()) {
                        // 真の破棄。`onPause` 止まりの非前景とは区別する（直下参照）。
                        micRecordRequest.take();
                        nativeOnMicRecordResult(id, MIC_RESULT_INTERRUPTED, null, null, 0);
                        return;
                    }
                    if (!this.activityResumed) {
                        // cursor Bugbot 是正（PR #962）: `RequestPermission` ダイアログの
                        // 起動自体が Activity を pause するため、初回許可はほぼ必ず
                        // ここを通る。旧実装はこれを `MIC_RESULT_INTERRUPTED` として
                        // 畳み、正常な許可でも `startMicRecorder` を一度も呼んでい
                        // なかった。location の `locationStartRequestPending` と同型に
                        // 「保留」として扱い、次の `onResume`（`micStartPendingResume`
                        // 参照）で `startMicRecorder` を実行する。`this.` 限定子は必須
                        // （`activityResumed`（611 行目）がこのフィールド初期化子より
                        // 後に宣言されており、単純名参照だと javac が forward
                        // reference エラーにする）。
                        this.micStartPendingResume = true;
                        return;
                    }
                    if (micRecorder != null) {
                        // 権限確認中に別経路から既に録音が始まっている（通常到達しない
                        // 防御的分岐）。
                        return;
                    }
                    startMicRecorder(id, micRequestedMaxDurationMs);
                }
            });

    // 音声認識（Issue #1026、Phase 11 型サンプル「音声認識」の Android 経路）。
    // `SpeechRecognizer` trait は通らない（`device/speech_recognition.rs` モジュール
    // doc「macOS/Windows/Android は pull 型 bridge」参照）。Rust 側は
    // `MainActivity.requestSpeechRecognitionStart(long, int)`/
    // `requestSpeechRecognitionStop(long)`（提示依頼）を呼び、結果は
    // `nativeOnSpeechStarted`/`nativeOnSpeechResult`/`nativeOnSpeechError` で非同期に
    // 返す（`micRecordRequest`/`requestMicRecordStart` と同型）。
    private static native void nativeOnSpeechStarted(long requestId);

    // code の意味（本ファイルが SSOT、Rust 側 `speech_recognition::error_from_code` の
    // doc と対）: 0=成功（text 有効）／1=権限拒否／2=内部エラー／3=二重開始（busy）／
    // 4=許可確認中にキャンセル／5=ライフサイクル中断（`onPause`/`onDestroy`/非前景での
    // 開始）／6=`isRecognitionAvailable == false`。
    private static native void nativeOnSpeechResult(long requestId, int code, String text);

    // `RecognitionListener.onError(int)` の生値をそのまま渡す（Java 側で分岐せず、
    // Rust 側 `speech_recognition::outcome_from_android_error`（SSOT）へ写像を
    // 一元化する、device-bridge-contracts.md「FFI 境界」節）。
    private static native void nativeOnSpeechError(long requestId, int error);

    private static final int SPEECH_RESULT_SUCCESS = 0;
    private static final int SPEECH_RESULT_PERMISSION_DENIED = 1;
    private static final int SPEECH_RESULT_INTERNAL_ERROR = 2;
    private static final int SPEECH_RESULT_BUSY = 3;
    private static final int SPEECH_RESULT_CANCELLED = 4;
    private static final int SPEECH_RESULT_INTERRUPTED = 5;
    private static final int SPEECH_RESULT_UNAVAILABLE = 6;

    // 手動停止後、`onResults`/`onError` が届かないまま要求を固着させないための保険の
    // 期限（`TTS_ACCEPT_TIMEOUT_MS` と同型。PR #1030 レビュー指摘: `stopListening()` 後に
    // 認識サービスが応答しない場合、Rust 側の `tick` は `Stopping` では上限判定を
    // 進めないため、Android 側で決着させる必要がある）。
    private static final long SPEECH_STOP_RESULT_GRACE_MS = 5_000L;

    // `startListening()` 呼び出しから `onReadyForSpeech`/`onError` のどちらも届かない
    // まま画面が Listening 固着するのを防ぐ保険の期限（PR #1030 レビュー指摘）。
    // `onReadyForSpeech` 到達後は本来の聞き取り上限タイマー（`maxListenMs`）へ
    // `cancelSpeechTimeout()` 経由で切り替わるため、本タイマーは準備段階にのみ効く。
    private static final long SPEECH_READY_TIMEOUT_MS = 10_000L;

    // 「いま待っている要求 id」（`micRecordRequest` と同じ位置づけ）。UI スレッドのみで
    // 読み書きするが、`SpeechRecognizer`（`speechRecognizer`）とまとめて `speechLock` の
    // 内側で判定・更新する（並行性の契約を単一ロックへ集約する、
    // device-bridge-contracts.md「並行性・世代」節）。
    private static final RequestTracker speechRequest = new RequestTracker();

    // `speechLock` の内側でのみ読み書きする状態（ライフサイクル・並行性の契約を単一
    // ロックへ集約する、device-bridge-contracts.md 参照）。フィールド初期化子で
    // 参照する `requestSpeechPermission`（下記）より先に宣言する（javac の前方参照
    // 制限、`micRecorder`/`requestMicPermission` の宣言順と同じ）。
    private final Object speechLock = new Object();
    private SpeechRecognizer speechRecognizer;
    private final Handler speechTimeoutHandler = new Handler(Looper.getMainLooper());
    private Runnable speechTimeout;
    // `onReadyForSpeech` 到達済みか（`startSpeechRecognizer` で false に初期化し、
    // `onReadyForSpeech` で true にする）。「聞き始めていない段階の停止は Cancelled」
    // という `fandhe_device::speech_recognition` の契約を守るため、
    // `requestSpeechRecognitionStop` がこの前後で分岐する（device-bridge-contracts.md
    // 「キャンセル・正常終了・失敗の三値」節）。`speechLock` の内側でのみ読み書きする。
    private boolean speechStarted;
    // 停止要求（自動期限タイマー・`requestSpeechRecognitionStop` のいずれか）を既に
    // 認識器へ送ったか（`startSpeechRecognizer` で false に初期化する）。`speechStarted`
    // は結果到着まで true のままのため、この別フラグが無いと自動停止タイマーの発火後に
    // 手動「停止」が届いた場合（またはその逆）、同じ認識器へ `stopListening()` を二重に
    // 送ってしまい、かつ先に仕掛けた決着用の保険タイマー（`scheduleSpeechStopGrace`）を
    // 無条件に解除・再設定してしまう（PR #1030 レビュー指摘）。自動・手動どちらが先でも
    // 停止要求と保険タイマーの設定を一度きりにするための番人。`speechLock` の内側で
    // のみ読み書きする。
    private boolean speechStopRequested;
    // 権限ダイアログ表示中に `requestSpeechPermission` コールバックが読む上限
    // （`requestSpeechRecognitionStart` が受け取った値をそのまま退避する）。
    private volatile int speechRequestedMaxListenMs;
    // 許可結果が非前景中に届いたため `startSpeechRecognizer` の実行を次の `onResume` へ
    // 保留していることを示すフラグ（`micStartPendingResume` と同型）。`speechLock` の
    // 内側でのみ読み書きする。
    private boolean speechStartPendingResume;

    // 権限ダイアログを起動した時点の要求 id（`requestSpeechRecognitionStart` が launch
    // 直前にここへ退避する）。コールバックは `speechRequest.current()` を直接読まない
    // （ダイアログ表示中に「停止」で `speechRequest.take()` された後、許可結果が届く前に
    // 新しい「話す」要求が追跡されると、`current()` は新要求の id を返してしまい、旧
    // ダイアログへの応答が新要求へ誤って適用される）。コールバック内でこの退避値と
    // `current()` を照合し、一致する場合のみ自分が起動したダイアログの結果とみなす。
    // `speechLock` の内側でのみ読み書きする。
    private long speechPermissionRequestId = RequestTracker.NONE;

    // 権限ダイアログの結果がまだ届いていないか（launch 直前に true、コールバック冒頭で
    // 必ず false に戻す）。`speechPermissionRequestId` の単純な id 照合だけでは、
    // ダイアログ表示中に「停止」（`speechRequest.take()`）→ 新しい「話す」要求が追跡
    // される、という順でもう 1 回ダイアログを起動してしまうと、2 回目の launch が
    // `speechPermissionRequestId` を新要求の id で上書きし、まだ届いていない旧ダイアログの
    // コールバックが新要求の id と一致してしまい、旧ダイアログへの許可・拒否結果が新要求
    // のものとして誤適用される（PR #1030 レビュー指摘、device-bridge-contracts.md
    // 「並行性・世代」節）。本フラグが true の間は `requestSpeechRecognitionStart` が
    // 新しいダイアログを起動せず BUSY を返し、先行する権限要求が決着する（コールバックが
    // 届く）まで再起動を保留する。`speechLock` の内側でのみ読み書きする。
    private boolean speechPermissionPending;

    // 音声認識の実行時許可要求（`requestMicPermission` と同型）。許可が下りたら
    // `startSpeechRecognizer` を呼ぶ。
    private final ActivityResultLauncher<String> requestSpeechPermission =
            registerForActivityResult(new ActivityResultContracts.RequestPermission(), granted -> {
                synchronized (speechLock) {
                    long id = speechPermissionRequestId;
                    speechPermissionRequestId = RequestTracker.NONE;
                    // 届いたら即座に解除する。これ以降 `requestSpeechRecognitionStart` が
                    // 新しいダイアログを起動できるようになる（上記 `speechPermissionPending`
                    // doc 参照）。
                    speechPermissionPending = false;
                    if (id == RequestTracker.NONE || speechRequest.current() != id) {
                        // 自分が起動したダイアログの結果ではない（旧要求が「停止」で
                        // 破棄された後に新要求が追跡された、または既に破棄済み）。
                        // 新要求を壊さないよう何もしない。
                        return;
                    }
                    if (!granted) {
                        speechRequest.take();
                        nativeOnSpeechResult(id, SPEECH_RESULT_PERMISSION_DENIED, null);
                        return;
                    }
                    if (isFinishing() || isDestroyed()) {
                        // 真の破棄。`onPause` 止まりの非前景とは区別する（直下参照）。
                        speechRequest.take();
                        nativeOnSpeechResult(id, SPEECH_RESULT_INTERRUPTED, null);
                        return;
                    }
                    if (!this.activityResumed) {
                        // `RequestPermission` ダイアログの起動自体が Activity を pause
                        // するため、初回許可はほぼ必ずここを通る（`requestMicPermission`
                        // の同じ分岐、PR #962 是正と同じ理由）。「保留」として扱い、次の
                        // `onResume`（`speechStartPendingResume` 参照）で
                        // `startSpeechRecognizer` を実行する。`this.` 限定子は必須
                        // （`activityResumed` がこのフィールド初期化子より後に
                        // 宣言されており、単純名参照だと javac が forward reference
                        // エラーにする）。
                        this.speechStartPendingResume = true;
                        return;
                    }
                    if (speechRecognizer != null) {
                        // 権限確認中に別経路から既に聞き取りが始まっている（通常到達
                        // しない防御的分岐）。
                        return;
                    }
                    startSpeechRecognizer(id, speechRequestedMaxListenMs);
                }
            });

    // codex-review P1 是正（Issue #866 案A）: `ttsRequest` は SPEAK 要求が
    // `TextToSpeech` の初期化待ち（`TTS_STATE_NONE`/`TTS_STATE_INITIALIZING`）へ
    // 進む直前にこれを呼ぶ。Rust 側 `AndroidTts::speak` はこの通知だけを待って
    // 戻り、`onInit` 完了や `speak()` 呼び出し自体の結果は待たない
    // （`nativeOnTtsResult` を `wait_outcome` が別途待つ）。これにより
    // `speak`/`stop` が同じ FIFO（`device/tts.rs` の `worker_loop`）上で
    // 直列処理されていても、初期化待ち中に届いた stop 要求が速やかに Java へ
    // 届くようになる（旧実装は `speak` 自体が `onInit` 完了までブロックしていた
    // ため、stop がキューに残ったまま初期化完了後に「キャンセル済みのテキストを
    // 発話してから停止」してしまっていた）。STOP・即座に確定する分岐
    // （破棄済み Activity・未知の op・最大文字数超過・`FAILED`・`READY`）には
    // 送らない（いずれも二段階に分ける必要が無い一発応答のため、Rust 側の
    // `TTS_ACCEPT_MAILBOX` がそのまま最終結果を受理する）。
    private static native void nativeOnTtsAccepted(long requestId);

    // 読み上げ要求の種別（Rust 側 `TTS_OP_SPEAK` / `TTS_OP_STOP` と対）。
    private static final int TTS_OP_SPEAK = 0;
    private static final int TTS_OP_STOP = 1;
    // `nativeOnTtsResult` の status 7（stop による保留 speak 要求のキャンセル）。
    // Rust 側はキャンセルを失敗として扱わない（`decode_tts_result` 参照）。
    private static final int TTS_STATUS_CANCELLED = 7;

    // `nativeOnCameraResult` の JByteArray 確保前検査（`device/android.rs` の
    // `MAX_IMAGE_BYTES` と同値、64 MiB）を Java 側の配列確保前にも適用する
    // （codex-review 指摘、PR #790。大寸法 Bitmap で Rust 検査前に Java 側だけで
    // 追加配列を確保して OutOfMemoryError になるのを防ぐ）。
    private static final long MAX_IMAGE_BYTES = 64L * 1024 * 1024;

    // Activity 再生成後も撮影要求の状態を復元するための Bundle キー（Issue #750 / PR #790）。
    private static final String STATE_PENDING_CAPTURE_REQUEST_ID = "pendingCaptureRequestId";
    private static final String STATE_AWAITING_CAMERA_RESULT = "awaitingCameraResult";
    private static final String STATE_PENDING_CAPTURE_INTERRUPTED = "pendingCaptureInterrupted";

    // 進行中の撮影要求の ID（`requestCameraCapture(long)` で設定、Issue #750 / PR #790）。
    // GameActivity は 1 Activity インスタンスにつき 1 撮影のみ許容する契約
    // （Rust 側 `CaptureMailbox` の単一飛行）のため、フィールド 1 個で足りる。
    // onSaveInstanceState/onCreate で保存・復元する（Activity 再生成で新インスタンスの
    // 初期値（`RequestTracker.NONE`）に戻ると、復元後に届く Android の ActivityResult が
    // 誤った ID で配送されるため）。id の保持・照合そのものは `RequestTracker`
    // （Issue #944）へ委譲する。
    private final RequestTracker pendingCaptureRequest = new RequestTracker();

    // true の間は Android 側のコールバック（権限ダイアログ・カメラアプリ）待ちで、
    // 旧要求の結果が未回収であることを示す（codex-review 指摘、PR #790）。
    // Rust 側 `CaptureMailbox` はタイムアウトで Idle に戻り新要求 B の `begin()` を
    // 許可し得るが、Java 側でまだ旧要求 A の ActivityResult を待っている間に
    // `requestCameraCapture(B)` が呼ばれると、後から届く A の結果が
    // `pendingCaptureRequest`（B に上書き済み）で配送され、B の結果として誤って
    // 受理されてしまう。このフラグが true の間は新要求を即座に拒否することで防ぐ。
    // `pendingCaptureInterrupted` が true の間もこのフラグは true のまま維持する
    // （中断決着後も、旧コールバックが実際に届いて消費されるまで新要求を拒否し
    // 続けるため。onCreate の復元処理・両コールバックの doc 参照。個別 ID 識別ではなく
    // 「旧コールバック消費まで新要求拒否」で誤配送を防ぐ設計、codex-review 指摘 PR #790）。
    // 呼び出しスレッド（Rust 側 worker）と UI スレッドの双方から読み書きされるため
    // volatile にする。
    private volatile boolean awaitingCameraResult;

    // Activity 再生成で status 7 として中断決着させた「旧」要求に対し、後から
    // AndroidX ActivityResultRegistry が再配送してくる許可ダイアログ・カメラアプリの
    // 結果を無視して破棄するためのフラグ（codex-review / cursor Bugbot 指摘、PR #790）。
    // `registerForActivityResult` は Activity 再生成後も同一キーで新インスタンスへ
    // 再登録され、システム側に残っていた保留中の結果はその新インスタンスのコールバックへ
    // 配送される契約（androidx.activity:activity の ActivityResultRegistry 仕様。
    // 「新インスタンスには何も紐づかない」という前提は誤りだった）。
    // 中断時にこのフラグを立て、コールバック側で最初にこれを検査して消費（クリアして
    // return）することで、既に status 7 で決着済みの要求について
    // 二重の nativeOnCameraResult 配送・意図しないカメラアプリ再起動・新しい要求 ID への
    // 誤配送を防ぐ。
    //
    // このフラグを「新要求開始時」に早期クリアしない（旧: `requestCameraCapture` で
    // 無条件クリアしていたが、旧要求 A の実コールバックが未着のまま新要求 B が始まると
    // A の遅延結果が B の ID で誤配送される競合があった。codex-review 指摘 PR #790）。
    // 代わりに `awaitingCameraResult` を true のまま維持して新要求そのものを
    // `requestCameraCapture` の入口で拒否し、このフラグは旧コールバックが実際に
    // 届いて消費された時点でのみクリアする。
    // Activity が複数回連続で再生成されても中断状態を失わないよう Bundle へ保存・復元する
    // （cursor Bugbot 指摘、PR #790。未保存だと 2 回目以降の再生成で中断状態が消え、
    // 旧許可コールバックがカメラを誤って再起動しうる）。
    // onCreate・両コールバックいずれも UI スレッドからのみ読み書きするため
    // 追加の同期は不要（旧: `requestCameraCapture` の worker スレッドからも書き込んで
    // いたが、上記の再設計でその書き込みは不要になった）。
    private boolean pendingCaptureInterrupted;

    // `registerForActivityResult` は `onCreate` 完了前（フィールド初期化子）に呼ぶ契約
    // （AndroidX ActivityResultRegistry の制約）。CAMERA 権限のランタイム要求と、
    // 標準カメラアプリ（ACTION_IMAGE_CAPTURE）のサムネイル起動をそれぞれ担う。
    private final ActivityResultLauncher<String> requestCameraPermission =
            registerForActivityResult(new ActivityResultContracts.RequestPermission(), granted -> {
                if (pendingCaptureInterrupted) {
                    // 再生成前に status 7 で決着済みの旧要求への遅延配送（doc 参照）。
                    // 許可の可否に関わらずここで消費し、以後の分岐へ進めない。
                    // ここで初めて awaitingCameraResult を解除し、新要求を受け付け
                    // 可能にする（`pendingCaptureInterrupted` doc 参照）。
                    pendingCaptureInterrupted = false;
                    awaitingCameraResult = false;
                    return;
                }
                if (granted) {
                    launchCameraCapture();
                } else {
                    // `pendingCaptureRequest` は awaitingCameraResult を解除する前に
                    // ローカル変数へ退避する（codex-review 指摘、PR #790）。解除後は
                    // worker スレッドの次要求が `requestCameraCapture` でこのフィールドを
                    // 上書きし得るため、フィールドを直接 `nativeOnCameraResult` へ渡すと
                    // 別要求の ID を誤って配送する競合が生じる。
                    long resultRequestId = pendingCaptureRequest.current();
                    awaitingCameraResult = false;
                    nativeOnCameraResult(resultRequestId, null, 0, 0, 1);
                }
            });

    private final ActivityResultLauncher<Void> takePicturePreview =
            registerForActivityResult(new ActivityResultContracts.TakePicturePreview(), bitmap -> {
                if (pendingCaptureInterrupted) {
                    // 許可コールバック側と同じ理由（doc 参照）。
                    pendingCaptureInterrupted = false;
                    awaitingCameraResult = false;
                    return;
                }
                if (bitmap == null) {
                    // ユーザーがカメラアプリを戻る操作でキャンセルした場合。
                    // ID 退避の理由は requestCameraPermission コールバック側コメント参照。
                    long resultRequestId = pendingCaptureRequest.current();
                    awaitingCameraResult = false;
                    nativeOnCameraResult(resultRequestId, null, 0, 0, 3);
                    return;
                }
                deliverBitmap(bitmap);
            });

    // Storage Access Framework の「開く」ダイアログ（Issue #874）。`ACTION_OPEN_DOCUMENT`
    // + `CATEGORY_OPENABLE` を組み立てる `OpenDocument` contract を使う（`takePicturePreview`
    // と同じく `registerForActivityResult` はフィールド初期化子で呼ぶ契約）。
    //
    // Activity 再生成後も同一キーで再登録されるため、`ActivityResultRegistry` は
    // 旧インスタンスで保留中だった選択結果を新インスタンスのこのコールバックへ実際に
    // 配送し得る（androidx.activity:activity の仕様）。`filePickRequest.take()` で
    // 読み取りと同時に `RequestTracker.NONE` へ戻すことで、(1) プロセス死後の再配送
    // （新プロセスでは `NONE` のまま）と (2) 対応する要求が既に他の経路で消費済みの
    // 重複コールバックの両方を「`NONE` なら捨てる」一箇所の判定で弾く。requestId は
    // Rust 側の照合にそのまま渡すため、ここでは
    // 「追跡中か」以上の判定をしない（中断・再生成用の static フラグ・Bundle 保存は
    // 案 B では持たない、仕様書「決定事項」参照）。
    private final ActivityResultLauncher<String[]> pickDocument =
            registerForActivityResult(new ActivityResultContracts.OpenDocument(), uri -> {
                long requestId = filePickRequest.take();
                if (requestId == RequestTracker.NONE) {
                    return;
                }
                if (uri == null) {
                    // ユーザーがピッカーを戻る操作でキャンセルした場合。
                    nativeOnFilePickerResult(requestId, 1, null, -1);
                    return;
                }
                // 名前解決・fd 取得を UI スレッドでやらない（`deliverPickedDocument` 参照）。
                new Thread(() -> deliverPickedDocument(requestId, uri), "fandhe-file-pick")
                        .start();
            });

    // 写真選択（Issue #1104）。`PickVisualMedia` は端末が対応していれば ACTION_PICK_IMAGES
    // （権限不要のシステムピッカー）を、使えない端末では ACTION_OPEN_DOCUMENT（`image/*`）を
    // 自動で選ぶ。Activity 再生成後は `ActivityResultRegistry` が旧要求の結果を新インスタンスへ
    // 配送し得るが、`photoPickRequest.take()` が NONE なら捨て、そうでなくても Rust 側の
    // id 照合で捨てられる（`pickDocument` と同じ理由）。
    private final ActivityResultLauncher<PickVisualMediaRequest> pickPhoto =
            registerForActivityResult(new ActivityResultContracts.PickVisualMedia(), uri -> {
                long requestId = photoPickRequest.take();
                if (requestId == RequestTracker.NONE) {
                    return;
                }
                if (uri == null) {
                    nativeOnPhotoPickerResult(requestId, 1, null, -1);
                    return;
                }
                // 変換・fd 取得を UI スレッドでやらない（`deliverPickedPhoto` 参照）。
                new Thread(() -> deliverPickedPhoto(requestId, uri), "fandhe-photo-pick").start();
            });

    // 連絡先選択（Issue #1222）。`PickContact` は `ACTION_PICK`（`Contacts.CONTENT_TYPE`）で
    // 連絡先アプリを開く。選んだ 1 件の URI 読み取り権限だけが委譲されるため `READ_CONTACTS`
    // は不要。Activity 再生成後に旧要求の結果が配送されても、`take()` が NONE なら捨て、
    // そうでなくても Rust 側の id 照合で捨てられる（`pickPhoto` と同じ理由）。
    private final ActivityResultLauncher<Void> pickContact =
            registerForActivityResult(new ActivityResultContracts.PickContact(), uri -> {
                contactPickerOpen = false;
                long requestId = contactPickRequest.take();
                if (requestId == RequestTracker.NONE) {
                    return;
                }
                if (uri == null) {
                    nativeOnContactPickerResult(requestId, 1, null);
                    return;
                }
                // ContentResolver の読み取りを UI スレッドでやらない。
                new Thread(() -> deliverPickedContact(requestId, uri), "fandhe-contact-pick")
                        .start();
            });

    // Android の chooser（`Intent.createChooser`）は選択結果を返さない
    // （`StartActivityForResult` の結果は常に `RESULT_CANCELED`）ため、この
    // コールバックは「chooser が閉じた」ことしか意味しない（Issue #899、実装計画 §2.3）。
    // 選択完了そのものは `shareChosenReceiver`（`IntentSender` broadcast）が別途届ける。
    // broadcast は chooser の finish より先に送られるが非同期のため、ここでは即座に
    // 「不明」を確定させず `SHARE_CHOSEN_GRACE_MS` だけ遅延させて broadcast の遅延到着を
    // 待つ。`complete` が false なら broadcast 側が既に処理済みのため、遅延 Runnable は
    // 空振りする（両方 UI スレッドなのでロック不要）。
    private final ActivityResultLauncher<Intent> shareChooser =
            registerForActivityResult(new ActivityResultContracts.StartActivityForResult(), result -> {
                long id = shareRequest.current();
                if (id == RequestTracker.NONE) {
                    // broadcast が先に届いて既に消費済み（`shareChosenReceiver` 参照）。
                    return;
                }
                new Handler(Looper.getMainLooper()).postDelayed(() -> {
                    if (!shareRequest.complete(id)) {
                        // broadcast が遅延到着で先に消費済み（空振り）。
                        return;
                    }
                    unregisterShareChosenReceiver();
                    nativeOnShareResult(id, 4);
                }, SHARE_CHOSEN_GRACE_MS);
            });

    // 測位の開始・停止を UI スレッド上の resumed 状態と一致させるための可視性フラグ
    // （codex-review P1 指摘、PR #894）。`requestLocationUpdates()` は Rust スレッドから
    // JNI 経由で呼ばれ `runOnUiThread` へ開始要求を積むが、その Runnable の実行前に
    // `onPause` が `removeUpdates` を済ませてしまうと、Runnable が停止済み Activity へ
    // リスナーを再登録してしまい「バックグラウンドでは測位しない」契約
    // （`onPause` doc 参照）が崩れる。Runnable・権限結果コールバックの両方で本フラグを
    // 確認し、resumed でなければ登録・通知を行わず次の `onResume` まで保留する。
    // UI スレッドでのみ読み書きする（`onResume`/`onPause`/上記 Runnable 群はすべて
    // UI スレッド上）ため volatile は不要。
    // フィールド初期化子（`requestLocationPermission` 等のラムダ）から先読みされるため、
    // それらより前で宣言する必要がある（javac の forward reference 制限、PR #894）。
    private boolean activityResumed;

    // 位置情報のランタイム許可要求（Issue #890、`requestCameraPermission` と同型）。
    // 位置情報画面は `ACCESS_COARSE_LOCATION` のみを要求する（最小権限、security.md A05）。
    // ジオフェンス（Issue #1058）が半径より良い精度を要るため `locationPreciseWanted` が立った
    // ときだけ `ACCESS_FINE_LOCATION` を COARSE と同時に要求する（Android 12 以降は FINE の
    // 単独要求不可）。
    // 利用者が「おおよそ」だけ許可した場合も許可扱いで測位を始める（精度不足は Rust 側が
    // 登録時に Inaccurate として示す）。BACKGROUND は要求しない（`AndroidManifest.xml` 参照）。
    //
    // `awaitingLocationPermission` を解除した後、resumed 状態を確認してから結果を配送する
    // （codex-review P1 指摘: 開始側と結果側の両方で resumed を確認する、PR #894）。
    // paused 中に届いた場合はここでは何もしない。granted なら `locationWanted` が立った
    // ままなので次の `onResume` が `startLocationUpdates` を呼ぶ。denied なら
    // `awaitingLocationPermission` を解除済みのため次の `onResume` が通常どおり
    // `nativeOnLocationFailure` を送る（cursor Bugbot 指摘の解消経路と同じ）。
    private final ActivityResultLauncher<String[]> requestLocationPermission =
            registerForActivityResult(new ActivityResultContracts.RequestMultiplePermissions(), result -> {
                awaitingLocationPermission = false;
                if (!activityResumed) {
                    return;
                }
                // 取消（空 Map）は拒否扱い。FINE か COARSE のどちらかが許可されれば進む。
                // 昇格ダイアログの取消で空 Map が返っても、COARSE 許可済みなら拒否扱いにしない。
                boolean granted = Boolean.TRUE.equals(result.get(Manifest.permission.ACCESS_FINE_LOCATION))
                        || Boolean.TRUE.equals(result.get(Manifest.permission.ACCESS_COARSE_LOCATION))
                        || hasLocationPermission();
                if (granted) {
                    startLocationUpdates();
                } else {
                    notifyLocationFailure(LOCATION_FAILURE_PERMISSION_DENIED);
                }
            });

    // 実測位（Issue #890）。`LocationManager`（`android.location`、フレームワーク同梱、
    // Play services 不使用。対応表 `docs/device/capabilities/location.json` の
    // `api` 節参照）から `getSystemService` で取得する（`onCreate` で初期化）。
    private LocationManager locationManager;

    // 「測位を続けたい」という意思を Activity 再生成を跨いで保持する static フラグ
    // （`filePickRequest` と同じ理由。`requestLocationUpdates()`（JNI から呼ばれる）が
    // true にし、`onResume` がこれを見て再登録する）。`onPause` では `removeUpdates` する
    // だけで false へは戻さない（バックグラウンドから復帰したら測位を再開する契約）。
    private static boolean locationWanted;

    // 権限ダイアログの結果待ちを Activity 再生成を跨いで保持する static フラグ
    // （cursor Bugbot 指摘、PR #894）。`RequestPermission` ダイアログは configChanges の
    // 対象外で、Activity が再生成されてもダイアログ自体は表示され続ける
    // （`registerForActivityResult` は同一キーで新インスタンスへ結果を配送する契約、
    // `pendingCaptureRequest` doc 参照）。この間に新インスタンスの `onResume` が走ると
    // `checkSelfPermission` はまだ未許可のままのため、ユーザーが応答する前に
    // `PermissionDenied` を誤って push してしまう。このフラグが立っている間は
    // `onResume` の権限確認をスキップし、ダイアログの結果を待つ。
    private static boolean awaitingLocationPermission;

    // COARSE のみ許可済みの利用者へ FINE への昇格ダイアログを出し済みか（プロセス内）。
    // 繰り返し開始要求のたびに同じダイアログを出さないための印。UI スレッドでのみ読み書きする。
    private static boolean fineUpgradeAsked;

    // ジオフェンス画面が精密位置（FINE）を要求済みか。`requestLocationUpdates(true)` が立て、
    // プロセス内は下ろさない（位置情報画面へ戻っても FINE 許可は取り消さないため、測位の
    // 再開時に FINE を再要求し続けても利用者への追加ダイアログは `fineUpgradeAsked` で 1 回に
    // 抑える）。UI スレッドでのみ読み書きする。
    private static volatile boolean locationPreciseWanted;

    // `requestLocationUpdates()` が積んだ開始要求のうち、まだ「権限確認 → 必要なら
    // ダイアログ起動」まで実行できていないものがあるかの可視性フラグ
    // （codex-review P1 指摘 / cursor Bugbot 指摘、PR #894）。`onPause` が先に走って
    // Runnable が resumed 状態を見て早期 return した場合、この意思を握り潰さずに
    // 保持し、次の `onResume` で `ensureLocationPermissionAndStart()` を代わりに
    // 実行させる（`onResume` の通常経路は権限失効チェックのみで自動的にダイアログを
    // 出さない設計のため、区別が要る）。UI スレッドでのみ読み書きする。
    private static boolean locationStartRequestPending;

    // 直近で Rust 側へ失敗（権限拒否・サービス無効・その他失敗）を通知済みかどうか
    // （codex-review P1 指摘 / cursor Bugbot 指摘、PR #894）。`nativeOnLocationRetrying`
    // （`SharedLocation::reset_to_waiting`）は「失敗からの回復」専用のシグナルであり、
    // 平常の `onResume` 再登録（前回も `Fixed` のまま・失敗を挟んでいない）でまで
    // 呼ぶと、取得済みの Fix が理由なく破棄される。Rust 側 `location_effect` は
    // `LocationShowState.retry_seq`（「再試行」ボタン押下のみが進む世代番号）でしか
    // 再要求を出さないため、この Java 側の OS 起点回復通知が「利用者がボタンで
    // 再試行した」と誤認されて二重に測位開始要求を発行することはない
    // （`crates/fandhe-egui-proxy/src/device/location.rs` の `LAST_PROCESSED_RETRY_SEQ`
    // 契約参照）。本フラグは失敗通知の唯一の出口 `notifyLocationFailure` が立て、
    // 回復通知の出口 `notifyLocationRecoveredIfNeeded` が「直前が失敗のときだけ」
    // `nativeOnLocationRetrying` を呼んで下ろす。UI スレッドでのみ読み書きする
    // （`locationListener` の各コールバックは `requestLocationUpdates` の登録時に渡す
    // `Looper.getMainLooper()` 経由で UI スレッドへ届く）。
    private static boolean locationFailurePosted;

    // `nativeOnLocationFailure` の唯一の呼び出し口（`locationFailurePosted` doc 参照）。
    // 呼び出し元を横断してこの経由に一本化することで、フラグの立て忘れを防ぐ。
    private void notifyLocationFailure(int code) {
        locationFailurePosted = true;
        nativeOnLocationFailure(code);
    }

    // 「失敗からの回復」を確定できるときだけ `nativeOnLocationRetrying` を呼ぶ
    // （`locationFailurePosted` doc 参照）。直前に失敗を通知していなければ
    // （＝前回も `Fixed`/`Waiting` のまま）何もしない。`startLocationUpdates`
    // （権限再取得後・`onResume` 再登録の両方）と `onProviderEnabled` から呼ばれる。
    private void notifyLocationRecoveredIfNeeded() {
        if (locationFailurePosted) {
            locationFailurePosted = false;
            nativeOnLocationRetrying();
        }
    }

    // OS からの継続コールバック（Issue #890）。`hasAccuracy()` が偽のサンプルは
    // `Fix.accuracy_m`「有限・0 以上」の契約（`fandhe_device::location` モジュール doc）を
    // 偽装しないため捨てる（network provider が精度なしの結果を返す天井、
    // `location.json` の note 参照）。
    private final LocationListener locationListener = new LocationListener() {
        @Override
        public void onLocationChanged(Location location) {
            if (!location.hasAccuracy()) {
                return;
            }
            // 実測位が届いた時点で「失敗通知済み」ではなくなる（`locationFailurePosted`
            // doc 参照）。以後の平常な `onResume` 再登録が誤って回復通知を送らないよう
            // ここで明示的に下ろす。
            locationFailurePosted = false;
            nativeOnLocationChanged(
                    location.getLatitude(), location.getLongitude(), location.getAccuracy());
        }

        @Override
        public void onProviderDisabled(String provider) {
            notifyLocationFailure(LOCATION_FAILURE_SERVICE_DISABLED);
        }

        @Override
        public void onProviderEnabled(String provider) {
            // provider 再有効化時に古い ServiceDisabled 状態を残さない（cursor Bugbot
            // 指摘、PR #894）。listener は登録済みのままのため `startLocationUpdates`
            // は再度呼ばれず、ここで明示的に「未取得」へ戻さないと、coarse/network
            // provider が次の fix を返すまで（届かないことすらある）画面が Disabled の
            // まま固定されてしまう。`onProviderDisabled` を経ていなければ
            // （`locationFailurePosted` が false）何もしない。
            notifyLocationRecoveredIfNeeded();
        }

        @Override
        public void onStatusChanged(String provider, int status, Bundle extras) {
            // 非推奨 API（API 29 以降呼ばれない）。使わない。
        }
    };

    // 傾きセンサの取得元と最新値の Rust 側への中継（Issue #831）。`registerListener`/
    // `unregisterListener` は onResume/onPause で対にする（Android の推奨作法。
    // バックグラウンド中の無駄なセンサ計測・バッテリ消費を避ける）。
    private SensorManager sensorManager;
    private Sensor motionSensor;

    // 重力センサ（TYPE_GRAVITY）が無い端末向けの fallback。加速度センサは
    // ユーザー操作由来の線形加速度成分を含むため傾き検出にノイズが乗り得るが、
    // 実装は追加しない（気になる場合はローパスフィルタを足す、#831 実装計画 §8）。
    private final SensorEventListener motionListener = new SensorEventListener() {
        @Override
        public void onSensorChanged(SensorEvent event) {
            // 呼び出しごとに取得する（ponytail: 回転変化のたびに読み直すだけの
            // 軽量呼び出し。負荷が問題になったら onConfigurationChanged でキャッシュ化、
            // #831 実装計画 §5 ステップ 3）。
            @SuppressWarnings("deprecation") // minSdk 24 のため Activity#getDisplay()（API 30+）は使えない
            int rotation = getWindowManager().getDefaultDisplay().getRotation();
            nativeOnMotionChanged(event.values[0], event.values[1], rotation);
        }

        @Override
        public void onAccuracyChanged(Sensor sensor, int accuracy) {
            // 精度変化の通知は使わない。
        }
    };

    // ゲームコントローラの接続監視（Issue #935）。Android には軸の現在値を問い合わせる
    // API が無い（イベント駆動のみ）ため、`InputDevice.getDeviceIds()` を都度再走査して
    // 「先頭のゲームパッドの deviceId、または無し」を通知する（レベルトリガ、
    // #935 実装計画 §2.3）。`registerInputDeviceListener`/`unregisterInputDeviceListener`
    // は onResume/onPause で対にする（motionListener と同じ作法）。
    private InputManager inputManager;

    private final InputManager.InputDeviceListener gamepadDeviceListener =
            new InputManager.InputDeviceListener() {
                @Override
                public void onInputDeviceAdded(int deviceId) {
                    reportGamepadPresence();
                }

                @Override
                public void onInputDeviceRemoved(int deviceId) {
                    reportGamepadPresence();
                }

                @Override
                public void onInputDeviceChanged(int deviceId) {
                    reportGamepadPresence();
                }
            };

    // 接続中のゲームパッド／ジョイスティック（`isVirtual()` を除く）を漏れなく Rust 側へ
    // 通知する。見つかった deviceId を scanId で 1 件ずつ通知し、Rust 側
    // （`AndroidGamepadState::presence_scan_end`）が先頭 1 台のみを追跡対象として
    // 採用する。1 台の物理コントローラが複数 deviceId に分かれて見える機種
    // （split pad、Joy-Con 等）は Android 上で別々の InputDevice として扱われ、
    // 公開 API に再結合の手段が無いため束ねず、片側のみが操作対象になる
    // （codex-review 指摘 P0/P1、PR #939 で束ね機能を撤去）。
    // `onResume`・`gamepadDeviceListener` の added/removed/changed から呼ばれる。
    private void reportGamepadPresence() {
        if (inputManager == null) {
            return;
        }
        nativeOnGamepadPresenceScanBegin();
        for (int deviceId : inputManager.getInputDeviceIds()) {
            InputDevice device = inputManager.getInputDevice(deviceId);
            if (device == null || device.isVirtual()) {
                continue;
            }
            int sources = device.getSources();
            boolean isGamepad = (sources & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD;
            boolean isJoystick = (sources & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK;
            if (isGamepad || isJoystick) {
                nativeOnGamepadPresenceScanId(deviceId);
            }
        }
        nativeOnGamepadPresenceScanEnd();
    }

    // 読み上げ（TTS、Issue #866）。`TextToSpeech` は最初の要求で遅延生成し（`onCreate`
    // では作らない。読み上げを 1 度も使わない画面ではエンジン起動コストを払わない）、
    // `OnInitListener` による非同期初期化の完了まで要求を 1 件だけ保留する
    // （Rust 側 mailbox が単一飛行を保証するため保留は常に高々 1 件で足りる。
    // ttsRequest doc 参照）。フィールドはすべて UI スレッド（`runOnUiThread`）
    // からのみ読み書きする。
    private TextToSpeech tts;

    // `speakNow` が `speak()` へ渡す utteranceId のプレフィックス。`utteranceListener`
    // はこのプレフィックスを剥がして `requestId`（long）へ戻すことで、どの発話に
    // 対する完了・失敗・中断通知かを識別する（codex-review P1 是正、PR #869）。
    private static final String UTTERANCE_ID_PREFIX = "fandhe-";

    // 現在 `tts.speak()` へ投入済み（＝完了・失敗・中断のいずれかを
    // `utteranceListener` が確定させるまで Rust 側 `wait_outcome` が待ち続けている）
    // requestId（無ければ `RequestTracker.NONE`）。`speakNow`（UI スレッド）が
    // 設定し、`deliverUtteranceOutcome`（`utteranceListener` のコールバックスレッド、
    // UI スレッドとは別）が確定時にクリアする。異なるスレッドから読み書きするため
    // `RequestTracker`（内部 `AtomicLong`）にする（codex-review 指摘、PR #869 4 巡目
    // 是正。id の保持・照合の集約は Issue #944）。
    //
    // `onDestroy`（UI スレッド）がこれを読み、発話中のまま Activity が破棄される
    // ケースで `onStop` を待たず `nativeOnTtsResult` を確定させるための唯一の用途。
    // `take` で読み取りとクリアを 1 操作にすることで、直後に本物の `onStop`/
    // `onDone` が届いても（`deliverUtteranceOutcome` 側の `complete` が
    // 不一致で no-op になり）二重送信にならない。
    private final RequestTracker speakingRequest = new RequestTracker();

    // `speakNow` が `speak()` に登録する完了通知（Issue #866、PR #869 是正）。
    // `TextToSpeech.speak()` の戻り値（`SUCCESS`）はキュー投入成功に過ぎず発話完了を
    // 意味しない（codex-review P1 指摘）ため、真の最終結果はこのリスナーの
    // `onDone`/`onError`/`onStop` で確定させ、`nativeOnTtsResult` を呼ぶ。
    //
    // コールバックは `TextToSpeech` 内部のワーカースレッドから呼ばれる契約
    // （`UtteranceProgressListener` のドキュメント）。ここで触るのは
    // `nativeOnTtsResult`（Rust 側の mailbox 実装が呼び出しスレッドを問わず
    // 安全、`device/android.rs` の `Mailbox` 参照）と `utteranceId` の文字列解析
    // のみで、`pendingTtsRequest` 等の UI スレッド専用フィールドには一切触れない
    // ため、追加の同期は不要。
    private final UtteranceProgressListener utteranceListener =
            new UtteranceProgressListener() {
                @Override
                public void onStart(String utteranceId) {
                    // 発話開始の通知は使わない（受理は `nativeOnTtsAccepted` が別途担う）。
                }

                @Override
                public void onDone(String utteranceId) {
                    deliverUtteranceOutcome(utteranceId, 0);
                }

                @Override
                @Deprecated
                public void onError(String utteranceId) {
                    // API 21 相当の後方互換オーバーロード。API 23+ では下の
                    // `onError(String, int)` が呼ばれるため通常は到達しない。
                    deliverUtteranceOutcome(utteranceId, 5);
                }

                @Override
                public void onError(String utteranceId, int errorCode) {
                    // エラーコードの種別は問わず一律「speak 失敗」に畳む
                    // （Rust 側 `decode_tts_result` の既存分類に合わせる）。
                    deliverUtteranceOutcome(utteranceId, 5);
                }

                @Override
                public void onStop(String utteranceId, boolean interrupted) {
                    // 停止（`stop()`・後続 speak の QUEUE_FLUSH による中断）は
                    // 失敗として扱わない（`decode_tts_result` の status 7 と同じ判断）。
                    deliverUtteranceOutcome(utteranceId, TTS_STATUS_CANCELLED);
                }
            };

    // `UTTERANCE_ID_PREFIX` を剥がして `requestId` を復元し、`nativeOnTtsResult` を
    // 呼ぶ（`utteranceListener` の各コールバックから共通で使う）。プレフィックス
    // 不一致・数値変換失敗（契約違反を検出した経路。通常到達しない）は無視する。
    private void deliverUtteranceOutcome(String utteranceId, int status) {
        if (utteranceId == null || !utteranceId.startsWith(UTTERANCE_ID_PREFIX)) {
            return;
        }
        try {
            long requestId =
                    Long.parseLong(utteranceId.substring(UTTERANCE_ID_PREFIX.length()));
            // この requestId がまだ「発話中」として追跡されていれば追跡を外す
            // （一致しなければ no-op。`onDestroy` が `take` で先に確定・
            // クリア済みの場合や、QUEUE_FLUSH で追い越された旧 utterance の
            // 遅延コールバックの場合はここで不一致になる、`speakingRequest`
            // doc 参照）。
            speakingRequest.complete(requestId);
            nativeOnTtsResult(requestId, status);
        } catch (NumberFormatException e) {
            // 数値へ変換できない utteranceId（契約違反。通常到達しない）。
        }
    }

    // `OnInitListener` を必ず非同期で処理するための Handler（cursor Bugbot 指摘）。
    // `ttsRequest` は既に UI スレッドの `runOnUiThread` 内で走るため、
    // `Activity#runOnUiThread` をそのまま使うと「呼び出し元スレッドが UI スレッドなら
    // 即時同期実行する」契約により、エンジンがコンストラクタ内から `onInit` を
    // 同期的に呼ぶ実装（一部端末で確認）の場合、`tts = new TextToSpeech(...)` の
    // 代入が完了する前に `onTtsInit` が走り、`speakNow` が未代入の `tts`（null）へ
    // `speak()` を呼んで NPE になる。`Handler#post` はメインルーパーのキューへ
    // 積むだけで同一スレッドでも同期実行しないため、`onTtsInit` は必ず
    // `new TextToSpeech(...)` 呼び出し（代入含む）が完了した後に走る。
    private final Handler mainHandler = new Handler(Looper.getMainLooper());

    // Java 側だけが持つ安全弁（PR #869 3 巡目是正: Rust 側 `wait_outcome`
    // （`TTS_OUTCOME_MAILBOX.wait_or_ok`）は完了・失敗・キャンセル・世代置換でのみ
    // 終了し、時間による打ち切りを持たない。`nativeOnTtsAccepted` は初期化待ちへ
    // 進む直前に既に送出済みのため、Rust 側は本値に対応するタイムアウトを持たない
    // 独立の値）。`TextToSpeech` の初期化（`onInit`）が極端に遅延・欠落した場合に
    // 保留中の SPEAK 要求を無期限に持ち続けないための Java ローカルな期限
    // （`pendingTtsDeadlineUptimeMs`/`expireStaleTtsRequest` 参照）。期限到来時は
    // `nativeOnTtsResult(requestId, 3)` を送って `wait_outcome` を確定させる
    // ため、次の speak/stop を待たずに解決する（PR #869 4 巡目是正、実測根拠の
    // ない安全弁である点は変わらず。実機で誤発火の報告が出たら見直す）。
    private static final long TTS_INIT_TIMEOUT_MS = 15_000L;
    // Rust 側 `device/android.rs` の `TTS_ACCEPT_TIMEOUT`（5 秒）と揃える
    // （codex-review P1 是正・案A）。SPEAK は経路によらず `nativeOnTtsAccepted` →
    // `nativeOnTtsResult` の二相に統一されている（PR #869 3 巡目是正: 以前は
    // `READY` 経路が受理を送らない一発応答だったため、完了コールバック化後は
    // 必ずここでタイムアウトしていた不具合があった）ため、Rust 側 `speak` は
    // 常に `TTS_ACCEPT_MAILBOX` の受理待ち（`TTS_ACCEPT_TIMEOUT`）だけを見る。
    // `nativeOnTtsAccepted` を送らず一発で `nativeOnTtsResult` を返す分岐
    // （最大文字数超過・`TTS_STATE_FAILED`・未知の op・破棄済み Activity）でも、
    // `runOnUiThread` の実行遅延がこの期限を超えていれば呼び出し元は既に
    // タイムアウト失敗を返した後（`TTS_INIT_TIMEOUT_MS` ではなくこちらを使う）。
    private static final long TTS_ACCEPT_TIMEOUT_MS = 5_000L;
    private static final int TTS_STATE_NONE = 0;
    private static final int TTS_STATE_INITIALIZING = 1;
    private static final int TTS_STATE_READY = 2;
    private static final int TTS_STATE_FAILED = 3;
    private int ttsInitState = TTS_STATE_NONE;
    // 初期化待ち中に保留している SPEAK 要求の id（`RequestTracker.NONE` なら保留なし）。
    // 旧 `ttsRequestPending` boolean は `current() != RequestTracker.NONE` で表現する
    // （id の保持・照合の集約は Issue #944）。
    private final RequestTracker pendingTtsRequest = new RequestTracker();
    private String pendingTtsText;
    // 保留要求の実期限（`SystemClock.uptimeMillis()` 基準、codex-review 指摘 P1）。
    // `ttsRequest` の先頭（JNI 呼び出し受信時点。Rust 側 `TTS_MAILBOX.begin()` の
    // 起点とほぼ同時刻）で確定させ、`runOnUiThread` のキュー処理が UI スレッド
    // 混雑で遅延しても期限の起点がずれないようにする。`expireStaleTtsRequest` の
    // 予約（`postAtTime`）と `onTtsInit` の発話直前検査の両方がこの値を参照する。
    private long pendingTtsDeadlineUptimeMs;

    // `onDestroy` 完了後に届く TTS 要求を拒否するための破棄フラグ（codex-review
    // 指摘 P1）。`ttsRequest` は `runOnUiThread` で Runnable を UI スレッドキューへ
    // 積むため、その Runnable の実行前に `onDestroy` が完了すると、`onDestroy` は
    // まだキューに積まれただけで未実行の要求を検査できない。その後 Runnable が
    // 走ると `ttsInitState` は `onDestroy` が `NONE` へ戻した後のため、破棄済み
    // Activity 上で `TextToSpeech` を再生成して発話してしまい、このインスタンスには
    // 以後 `onDestroy` が来ないため接続も解放されない。フラグは `onDestroy` の
    // 冒頭（`super.onDestroy()` より前）で真にし、`ttsRequest` の Runnable と
    // `onTtsInit`（いずれも同じ UI スレッドの直列キュー上で実行されるため
    // volatile 不要）の先頭で検査して、破棄後の要求はエラーで完了させる。
    private boolean ttsActivityDestroyed;

    // `BiometricPrompt` はインスタンスフィールド（内部で `FragmentActivity` の
    // `FragmentManager` に紐づく Fragment を使うため、Activity 再生成のたびに
    // `onCreate` で作り直す必要がある。作り直しても `biometricRequest`/
    // `biometricCallback` の組で結果が再接続される、`biometricRequest` doc 参照）。
    private BiometricPrompt biometricPrompt;

    // `biometricPrompt` 生成時に渡す唯一のコールバックインスタンス。先頭で
    // `biometricRequest` を読んで `RequestTracker.NONE` なら無視する（first-wins。
    // 二重通知防止）。
    // `onAuthenticationFailed`（不一致・プロンプト継続）は最終結果にしない
    // （実装計画 §4 参照）。
    private final BiometricPrompt.AuthenticationCallback biometricCallback =
            new BiometricPrompt.AuthenticationCallback() {
                @Override
                public void onAuthenticationSucceeded(BiometricPrompt.AuthenticationResult result) {
                    long id = biometricRequest.take();
                    if (id == RequestTracker.NONE) {
                        return;
                    }
                    nativeOnBiometricResult(id, BIOMETRIC_RESULT_AUTHENTICATED);
                }

                @Override
                public void onAuthenticationError(int errorCode, CharSequence errString) {
                    long id = biometricRequest.take();
                    if (id == RequestTracker.NONE) {
                        return;
                    }
                    // errString はログへ出さない（画面へ渡す情報も結果コードのみに
                    // 絞る、security.md A09）。
                    nativeOnBiometricResult(id, biometricErrorToResultCode(errorCode));
                }

                @Override
                public void onAuthenticationFailed() {
                    // 不一致（プロンプト継続）。最終結果は Succeeded/Error 側で確定する
                    // ため、ここでは何もしない（実装計画 §4 参照）。
                }
            };

    // `BiometricPrompt.ERROR_*` → `nativeOnBiometricResult` の code への写像
    // （本ファイルがこの表の SSOT、`device/biometric_auth.rs::outcome_from_android_code`
    // の doc 参照）。未知のコード（TIMEOUT/UNABLE_TO_PROCESS/NO_SPACE/VENDOR 等）は
    // 失敗側へ倒す（偽のキャンセル・偽の成功を作らない）。
    private static int biometricErrorToResultCode(int errorCode) {
        switch (errorCode) {
            case BiometricPrompt.ERROR_USER_CANCELED:
            case BiometricPrompt.ERROR_NEGATIVE_BUTTON:
            case BiometricPrompt.ERROR_CANCELED:
                return BIOMETRIC_RESULT_CANCELLED;
            case BiometricPrompt.ERROR_LOCKOUT:
            case BiometricPrompt.ERROR_LOCKOUT_PERMANENT:
                return BIOMETRIC_RESULT_FAILED;
            case BiometricPrompt.ERROR_NO_BIOMETRICS:
            case BiometricPrompt.ERROR_HW_NOT_PRESENT:
            case BiometricPrompt.ERROR_HW_UNAVAILABLE:
            case BiometricPrompt.ERROR_SECURITY_UPDATE_REQUIRED:
            case BiometricPrompt.ERROR_NO_DEVICE_CREDENTIAL:
                return BIOMETRIC_RESULT_UNAVAILABLE;
            default:
                return BIOMETRIC_RESULT_FAILED;
        }
    }

    // カスタム URL scheme 受信（Issue #1218）。`onCreate` / `onNewIntent` から
    // `deliverUrlIntent` が呼ぶ。実体は `crates/fandhe-egui-proxy/src/device/
    // android_url_scheme.rs`（feature `device` 無効時の no-op は `android.rs`）。JNI 署名は
    // この宣言・no-op スタブ・Rust 実体の 3 か所で一致させる。検査（scheme・長さ・制御文字）は
    // Rust 側の共通層に一本化しており、ここでは受け取った文字列を加工せずに渡す。
    // URL はトークンを含みうるためログへ出さない。
    private static native void nativeOnUrlSchemeReceived(String url);

    // `ACTION_VIEW` のときだけ URL を Rust へ渡す（MAIN/LAUNCHER 起動は拒否件数に混ぜない）。
    // null の data もそのまま渡し、Rust 側が拒否として数える。
    private static void deliverUrlIntent(Intent intent) {
        if (intent == null || !Intent.ACTION_VIEW.equals(intent.getAction())) {
            return;
        }
        // Web 認証のコールバック（code・state を含む）は url-scheme 受信箱へ流さない（Issue #534）。
        if (WebAuthSessionHelper.isCallback(intent)) {
            return;
        }
        nativeOnUrlSchemeReceived(intent.getDataString());
    }

    // `launchMode="singleTask"`（AndroidManifest.xml 参照）により、起動中に届いた URL は
    // 新インスタンスではなくここへ来る。1 インテントにつき 1 回だけ呼ばれる。
    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        // Web 認証のコールバックはここで確定し、setIntent で code を含む intent を残さない（Issue #534）。
        if (webAuthSessionHelper.onNewIntent(intent)) {
            return;
        }
        setIntent(intent);
        deliverUrlIntent(intent);
        ShareReceiveHelper.deliver(intent);
    }

    /**
     * a11y 無効時の accesskit イベント送信を親 View で止める（Issue #2）。
     *
     * <p>役割: accesskit_android は `send_completed_event` で UI スレッドから
     * `host.getParent().requestSendAccessibilityEvent` を呼び、結果を `unwrap()` する。a11y
     * サービスが一時接続して adapter が活性化したまま a11y が OFF に戻ると、Android が
     * `IllegalStateException("Accessibility off")` を投げ、Rust の panic（SIGABRT）になる。
     * host（`mSurfaceView`）の親 FrameLayout で、Android 標準の `View.sendAccessibilityEvent`
     * と同じ `isEnabled()` 確認を行い、無効なら false を返して親への伝播（例外）を断つ。
     *
     * <p>守備範囲: UI スレッド直列・1 プロセス内。a11y OFF 中のイベント破棄は Android 標準と
     * 同じ扱い。upstream の accesskit_android が修正されたら外す。イベント内容はログに出さない。
     */
    private void installAccessibilityOffGuard() {
        ViewParent parent = mSurfaceView != null ? mSurfaceView.getParent() : null;
        if (!(parent instanceof ViewGroup)) {
            return;
        }
        final AccessibilityManager a11y = getSystemService(AccessibilityManager.class);
        ((ViewGroup) parent).setAccessibilityDelegate(new View.AccessibilityDelegate() {
            @Override
            public boolean onRequestSendAccessibilityEvent(
                    ViewGroup host, View child, AccessibilityEvent event) {
                if (a11y != null && !a11y.isEnabled()) {
                    return false;
                }
                return super.onRequestSendAccessibilityEvent(host, child, event);
            }
        });
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        backgroundUploadHelper.onCreate();
        companionDeviceHelper.init();
        screenBrightnessHelper.onCreate();
        captureProtectionHelper.onCreate();
        installAccessibilityOffGuard();
        fullscreenHelper.register();

        // コールドスタートの URL・共有テキストを届ける。二重配送を避けるため、再生成（savedInstanceState
        // あり）と履歴（Recents）からの再起動（元のインテントが再送される）では届けない。
        // 同一プロセス内の再生成は Rust 側 static の受信箱が保持しているので表示は保たれる。
        // 守らない範囲: プロセス死亡後の復元では前回の URL を再表示しない（新プロセスの
        // 受信箱は空。画面状態全体が失われるのと同じ扱い）。
        if (savedInstanceState == null
                && (getIntent().getFlags() & Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY) == 0) {
            deliverUrlIntent(getIntent());
            ShareReceiveHelper.deliver(getIntent());
        }

        // `onCreate` のたびに作り直す（`biometricPrompt` doc 参照）。
        biometricPrompt = new BiometricPrompt(this, ContextCompat.getMainExecutor(this), biometricCallback);

        // Rust 側 `android_main` は `biometric_auth::bridge::reset()` で保留中の要求を
        // 無条件に捨て、新しい画面状態（`BiometricCheckState::default()` = `Idle`）を
        // 作り直す（codex-review 指摘、PR #917）。`biometricRequest` は static のため
        // Activity 再生成をまたいで残り、`biometricPrompt` も `FragmentManager` 経由で
        // 旧認証操作へ再接続され得るため、放置すると新しい `biometricPrompt` の上に
        // 「宛先を失った」認証が動き続けてしまう。ここで明示的に打ち切り、対になる
        // Rust 側の破棄と揃える。`cancelAuthentication()` は `onAuthenticationError`
        // （`ERROR_CANCELED`）を呼ぶため `biometricRequest` は通常のコールバック経路で
        // `RequestTracker.NONE` に戻る（結果は Rust 側が既に破棄済みのため無視される、
        // no-op）。
        if (biometricRequest.current() != RequestTracker.NONE) {
            biometricPrompt.cancelAuthentication();
        }

        // ローカル通知（Issue #1006）: `notificationRequest` は static のため Activity
        // 再生成をまたいで残るが、権限ダイアログ待ち中に破棄された要求の宛先は失われる。
        // 保留中の要求をここで捨て、Rust 側 `android_main` の `notification::
        // reset_pending_tracking()` と対で「宛先を失った」要求を残さない
        // （`biometricRequest`/`micRecordRequest` と同じ位置づけ、順序非依存）。
        notificationRequest.take();
        pendingNotificationTitle = null;
        pendingNotificationBody = null;

        // マイク録音（Issue #957）: `micRecordRequest` は static のため Activity 再生成を
        // またいで残るが、`MediaRecorder`（`micRecorder`）はインスタンスフィールドで
        // 旧 Activity インスタンスとともに失われている（新インスタンスでは常に
        // `null`）。保留中の要求（権限ダイアログ待ち含む）をここで捨て、Rust 側
        // `android_main` の `mic_record::reset_pending_tracking()` と対で「宛先を
        // 失った」要求を残さない（`biometricRequest` と同じ位置づけ、順序非依存）。
        micRecordRequest.take();

        // 音声認識（Issue #1026）: `speechRequest` は static のため Activity 再生成を
        // またいで残るが、`SpeechRecognizer`（`speechRecognizer`）はインスタンス
        // フィールドで旧 Activity インスタンスとともに失われている（新インスタンス
        // では常に `null`）。保留中の要求（権限ダイアログ待ち含む）をここで捨て、
        // Rust 側 `android_main` の `speech_recognition::reset_pending_tracking()` と
        // 対で「宛先を失った」要求を残さない（`micRecordRequest` と同じ位置づけ、
        // 順序非依存）。
        speechRequest.take();

        // 連絡先選択（Issue #1222）: Rust 側 `android_main` の `contact_pick::
        // reset_pending_tracking()` と対で、保留中の要求 id を捨てる。旧ピッカーの結果は
        // 新インスタンスへ届き得るが、id が NONE なので捨てられる。`contactPickerOpen` は
        // その結果が届くまで true のまま残し、新しい起動を断る。
        contactPickRequest.take();

        sensorManager = getSystemService(SensorManager.class);
        if (sensorManager != null) {
            motionSensor = sensorManager.getDefaultSensor(Sensor.TYPE_GRAVITY);
            if (motionSensor == null) {
                motionSensor = sensorManager.getDefaultSensor(Sensor.TYPE_ACCELEROMETER);
            }
        }

        locationManager = getSystemService(LocationManager.class);
        inputManager = getSystemService(InputManager.class);

        // Activity 再生成後の状態復元（Issue #750 / PR #790）。
        // `registerForActivityResult` は再生成後の新インスタンスでも同一キーで
        // 再登録されるため、旧インスタンスで保留中だった権限ダイアログ・カメラアプリの
        // 結果は新インスタンスのコールバックへ実際に配送され得る契約
        // （androidx.activity:activity の ActivityResultRegistry 仕様）。よって
        // awaitingCameraResult を false へ戻して新要求を即座に受け付け可能にしてはならない:
        // 戻すと、まだ Android 側に残っている旧要求の実コールバックが後から届いた際、
        // その時点で新要求 B が既に始まっていれば `pendingCaptureRequest` が B の値へ
        // 上書き済みとなり、旧要求 A の結果が B の結果として誤って配送される
        // （codex-review 指摘、PR #790）。
        // 代わりに、中断された要求があれば復元直後に status 7 で即座に決着させつつ
        // （Rust 側 `CaptureMailbox::wait` の 300 秒タイムアウトを待たせない）、
        // `awaitingCameraResult` は true のまま維持して新要求を拒否し続ける。
        // 解除は `pendingCaptureInterrupted` doc 参照の各コールバックが旧結果を
        // 実際に消費した時点でのみ行う。
        //
        // `pendingCaptureInterrupted` 自体も Bundle へ保存・復元する（cursor Bugbot
        // 指摘、PR #790）。未保存だと、旧コールバック未消費のまま Activity が連続で
        // 再生成された場合に中断状態が失われ、(1) 同じ要求へ status 7 を多重通知し、
        // (2) 「まだ中断済み」の事実を見失い awaitingCameraResult のブロックが
        // 意図せず解除される。既に中断決着済み（pendingCaptureInterrupted 復元値が
        // true）なら、今回の再生成では再通知しない。
        if (savedInstanceState != null) {
            pendingCaptureRequest.track(savedInstanceState.getLong(STATE_PENDING_CAPTURE_REQUEST_ID, RequestTracker.NONE));
            awaitingCameraResult = savedInstanceState.getBoolean(STATE_AWAITING_CAMERA_RESULT);
            pendingCaptureInterrupted = savedInstanceState.getBoolean(STATE_PENDING_CAPTURE_INTERRUPTED);
            if (awaitingCameraResult && !pendingCaptureInterrupted) {
                pendingCaptureInterrupted = true;
                nativeOnCameraResult(pendingCaptureRequest.current(), null, 0, 0, 7);
            }

            // ファイル選択（Issue #874）は `filePickRequest` を Bundle へ保存・復元
            // しない（案 B の決定事項）。`filePickRequest` は static のため Activity
            // 再生成を跨いで値を保持しており（`filePickRequest` doc 参照）、
            // Bundle 経由の復元は不要。`pickDocument` が再生成後の新インスタンスへ届く
            // 実際の結果は、この static requestId との一致で正しく処理される。
        }

        // systemBars（ステータスバー・ナビゲーションバー）と displayCutout（ノッチ等）を
        // 物理 px で取得し、native 側 static へ渡す。insets は消費せず（`insets` を
        // そのまま返す）GameActivity 自身の insets 処理を妨げない。リスナーは
        // ウィンドウ生成直後の初回発火に加え、回転・折りたたみ等の変化時にも
        // 再発火する（View.OnApplyWindowInsetsListener の契約）。
        ViewCompat.setOnApplyWindowInsetsListener(getWindow().getDecorView(), (view, insets) -> {
            Insets bars = insets.getInsets(
                    WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout());
            nativeOnWindowInsetsChanged(bars.left, bars.top, bars.right, bars.bottom);
            fullscreenHelper.onInsets(insets);
            return insets;
        });

        // ドラッグ & ドロップ受け取り（Issue #1016）。`insets` リスナーと同じ view
        // （DecorView 全体）へ取り付ける。Activity 再生成のたびに新インスタンスへ
        // 付け直す必要があるため `onCreate` で毎回設定する（実装計画 §6「リスナーの
        // 付け先」: SurfaceView ではなく DecorView。実機で届かない場合は
        // `mSurfaceView` への付け替えを検討する、実機確認時の確認事項）。
        getWindow().getDecorView().setOnDragListener(dragListener);
    }

    // Activity 再生成前に撮影要求の状態を保存する（`onCreate` の復元と対、Issue #750 /
    // PR #790 codex-review 指摘）。
    @Override
    protected void onSaveInstanceState(Bundle outState) {
        super.onSaveInstanceState(outState);
        outState.putLong(STATE_PENDING_CAPTURE_REQUEST_ID, pendingCaptureRequest.current());
        outState.putBoolean(STATE_AWAITING_CAMERA_RESULT, awaitingCameraResult);
        outState.putBoolean(STATE_PENDING_CAPTURE_INTERRUPTED, pendingCaptureInterrupted);
    }

    // BLE スキャン（Issue #1062、`device/bluetooth_le.rs` の Android 経路）。
    // Rust の `request_ble_scan` が `requestBleScan(long, int)` を呼び、広告ごとに
    // `nativeOnBleScanResult`、終了時に 1 回だけ `nativeOnBleScanFinished` で通知する
    // （macOS / Windows と共通の `OBSERVED` / `bridge` へ pull 型で橋渡し）。
    // 同一 id の統合・RSSI 降順・上限は Rust 側 `normalize_scan` に任せ、ここでは並べ替えも
    // 重複除去もしない。完了コード → 契約のエラーの写像は Rust 側
    // `bluetooth_le::error_from_android_code` が SSOT（ここでは分岐しない）。
    //
    // 並行性（device-bridge-contracts.md 2・11 節）: 状態を触る経路は UI スレッド 1 本に
    // 集約する。`requestBleScan` は `runOnUiThread`、`ScanCallback` は main looper へ届く
    // （Handler 未指定の既定）、期限 `Handler` も main looper、ライフサイクルも UI
    // スレッドのため、ロックは持たない。通知は `bleScanRequest.complete(id)` が true の
    // ときだけ行い（first-wins）、二重完了と古い世代の通知を防ぐ。
    //
    // 期限: Rust の `SCAN_DURATION_MS` を `durationMs` で受けてそのまま使う（複製しない）。
    // `startScan` 成功時点から数え、権限ダイアログの回答待ちには掛けない（macOS と同じ
    // 既知の天井）。周辺機器の名前・アドレスはログへ出さない（security.md A09）。
    //
    // 結果コード（Rust 側 `error_from_android_code` と一致させる）: 0=成功／1=Bluetooth オフ／
    // 2=権限拒否／3=非搭載・LE 非対応／4=内部エラー／5=中断（`onPause`/`onDestroy`/非前景での
    // 開始）／6=位置情報サービスがオフ（API 30 以下。オフのままだと 0 件になり「0 台の成功」に
    // 化けるため失敗にする）。
    private static final int BLE_RESULT_OK = 0;
    private static final int BLE_RESULT_DISABLED = 1;
    private static final int BLE_RESULT_PERMISSION_DENIED = 2;
    private static final int BLE_RESULT_UNSUPPORTED = 3;
    private static final int BLE_RESULT_INTERNAL_ERROR = 4;
    private static final int BLE_RESULT_INTERRUPTED = 5;
    private static final int BLE_RESULT_LOCATION_OFF = 6;

    // `address` は識別子、`name` は広告名（無ければ null）、`rssi` は `ScanResult.getRssi()`
    // の生値（取得不能の番兵 127 も Rust 側が `None` へ写す）。
    private static native void nativeOnBleScanResult(
            long requestId, String address, String name, int rssi);

    private static native void nativeOnBleScanFinished(long requestId, int code);

    private final RequestTracker bleScanRequest = new RequestTracker();
    private final Handler bleHandler = new Handler(Looper.getMainLooper());
    private BluetoothLeScanner bleScanner;
    private ScanCallback bleScanCallback;
    private Runnable bleScanTimeout;
    private int bleScanDurationMs;
    // 権限ダイアログの回答待ち。表示中に来た再要求は新しい id を追跡し直すだけで、
    // 結果は「その時点の現在 id」へ適用する（要求する権限は同一のため）。
    private boolean blePermissionPending;
    // 許可結果が非前景中に届いたため `startBleScan` を次の `onResume` へ保留している
    // （正常な許可を中断扱いにせず、背景での開始も避ける、契約表 1 節）。
    private boolean bleStartPendingResume;

    // API 31 以上は `BLUETOOTH_SCAN` + `BLUETOOTH_CONNECT`、30 以下は `ACCESS_FINE_LOCATION`
    // （Manifest 参照。API 31 以上のみ CONNECT を要求するため minSdk 未満の分岐は不要）。
    private static String[] blePermissions() {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
                ? new String[] {
                    Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT
                }
                : new String[] {Manifest.permission.ACCESS_FINE_LOCATION};
    }

    private boolean bleGranted() {
        for (String p : blePermissions()) {
            if (ContextCompat.checkSelfPermission(this, p) != PackageManager.PERMISSION_GRANTED) {
                return false;
            }
        }
        return true;
    }

    private final ActivityResultLauncher<String[]> requestBlePermission =
            registerForActivityResult(new ActivityResultContracts.RequestMultiplePermissions(), result -> {
                boolean granted = true;
                for (String p : blePermissions()) {
                    granted &= Boolean.TRUE.equals(result.get(p));
                }
                this.blePermissionPending = false;
                long id = this.bleScanRequest.current();
                if (id == RequestTracker.NONE) {
                    return;
                }
                if (!granted) {
                    finishBle(id, BLE_RESULT_PERMISSION_DENIED);
                    return;
                }
                if (isFinishing() || isDestroyed()) {
                    finishBle(id, BLE_RESULT_INTERRUPTED);
                    return;
                }
                if (!this.activityResumed) {
                    this.bleStartPendingResume = true;
                    return;
                }
                startBleScan(id);
            });

    // 要求を 1 回だけ確定して通知する（追跡中の id と一致したときのみ）。
    private void finishBle(long id, int code) {
        if (id != RequestTracker.NONE && bleScanRequest.complete(id)) {
            nativeOnBleScanFinished(id, code);
        }
    }

    // タイマー解除 → `stopScan` → 参照解放を全終了経路（期限・失敗・置き換え・
    // `onPause`・`onDestroy`）で必ず通す。Bluetooth オフ時の `stopScan` が投げる
    // 例外は握る（契約表 9 節: 失敗経路でも資源を残さない）。
    private void stopBleScanQuietly() {
        if (bleScanTimeout != null) {
            bleHandler.removeCallbacks(bleScanTimeout);
            bleScanTimeout = null;
        }
        if (bleScanner != null && bleScanCallback != null) {
            try {
                bleScanner.stopScan(bleScanCallback);
            } catch (RuntimeException ignored) {
                // SecurityException / IllegalStateException（Bluetooth オフ）。
            }
        }
        bleScanner = null;
        bleScanCallback = null;
    }

    // Rust から呼ばれる開始依頼（Rust スレッド → UI スレッドへ集約）。
    public void requestBleScan(final long requestId, final int durationMs) {
        runOnUiThread(() -> {
            // 置き換え: 進行中のスキャンは止め、旧 id は通知せず破棄する（Rust 側は既に新世代）。
            stopBleScanQuietly();
            bleScanRequest.track(requestId);
            bleScanDurationMs = durationMs;
            if (blePermissionPending || bleStartPendingResume) {
                return;
            }
            if (isFinishing() || isDestroyed() || !activityResumed) {
                finishBle(requestId, BLE_RESULT_INTERRUPTED);
                return;
            }
            // 非搭載の判定を権限より先に行い、非対応端末で権限ダイアログを出さない。
            if (!getPackageManager().hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE)
                    || bleAdapter() == null) {
                finishBle(requestId, BLE_RESULT_UNSUPPORTED);
                return;
            }
            if (!bleGranted()) {
                blePermissionPending = true;
                try {
                    requestBlePermission.launch(blePermissions());
                } catch (RuntimeException e) {
                    blePermissionPending = false;
                    finishBle(requestId, BLE_RESULT_INTERNAL_ERROR);
                }
                return;
            }
            startBleScan(requestId);
        });
    }

    private BluetoothAdapter bleAdapter() {
        BluetoothManager manager = (BluetoothManager) getSystemService(Context.BLUETOOTH_SERVICE);
        return manager == null ? null : manager.getAdapter();
    }

    private static int bleFailureCode(int scanErrorCode) {
        return scanErrorCode == ScanCallback.SCAN_FAILED_FEATURE_UNSUPPORTED
                ? BLE_RESULT_UNSUPPORTED
                : BLE_RESULT_INTERNAL_ERROR;
    }

    private void startBleScan(final long id) {
        final BluetoothAdapter adapter = bleAdapter();
        if (adapter == null) {
            finishBle(id, BLE_RESULT_UNSUPPORTED);
            return;
        }
        // API 31+ の `isEnabled()` / `BluetoothDevice.getAddress()` は `BLUETOOTH_CONNECT` を
        // 要しうるため、実行時許可に含めている。それでも SecurityException が出たら落とさず権限拒否で確定する。
        final BluetoothLeScanner scanner;
        try {
            scanner = adapter.isEnabled() ? adapter.getBluetoothLeScanner() : null;
        } catch (SecurityException e) {
            finishBle(id, BLE_RESULT_PERMISSION_DENIED);
            return;
        }
        if (scanner == null) {
            finishBle(id, BLE_RESULT_DISABLED);
            return;
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            // サービスが取れない端末は位置 OFF と同じ扱い（null を渡すと NPE になる）。
            LocationManager lm = (LocationManager) getSystemService(Context.LOCATION_SERVICE);
            if (lm == null || !LocationManagerCompat.isLocationEnabled(lm)) {
                finishBle(id, BLE_RESULT_LOCATION_OFF);
                return;
            }
        }
        ScanCallback callback = new ScanCallback() {
            @Override
            public void onScanResult(int callbackType, ScanResult result) {
                if (bleScanRequest.current() != id || result.getDevice() == null) {
                    return;
                }
                // `BluetoothDevice.getName()` は API 31+ で BLUETOOTH_CONNECT を要するため
                // 使わず、広告の ScanRecord から名前を取る。
                ScanRecord record = result.getScanRecord();
                final String address;
                try {
                    address = result.getDevice().getAddress();
                } catch (SecurityException e) {
                    // スキャン中に権限が取り消された（API 31+）。止めて権限拒否で確定する。
                    stopBleScanQuietly();
                    finishBle(id, BLE_RESULT_PERMISSION_DENIED);
                    return;
                }
                nativeOnBleScanResult(
                        id, address, record == null ? null : record.getDeviceName(), result.getRssi());
            }

            @Override
            public void onScanFailed(int errorCode) {
                if (bleScanRequest.current() != id) {
                    return;
                }
                stopBleScanQuietly();
                finishBle(id, bleFailureCode(errorCode));
            }
        };
        ScanSettings settings =
                new ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build();
        try {
            scanner.startScan(null, settings, callback);
        } catch (SecurityException e) {
            finishBle(id, BLE_RESULT_PERMISSION_DENIED);
            return;
        } catch (RuntimeException e) {
            finishBle(id, BLE_RESULT_INTERNAL_ERROR);
            return;
        }
        bleScanner = scanner;
        bleScanCallback = callback;
        bleScanTimeout = () -> {
            bleScanTimeout = null;
            if (bleScanRequest.current() != id) {
                return;
            }
            // 途中で Bluetooth がオフになった場合は失敗にする（部分結果は Rust 側で捨てる）。
            boolean stillOn;
            try {
                stillOn = adapter.isEnabled();
            } catch (SecurityException e) {
                stillOn = false;
            }
            // API 30 以下は途中で位置情報サービスが OFF になると広告が届かず 0 台の偽の成功になる。
            boolean locationOff = false;
            if (stillOn && Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
                LocationManager lm = (LocationManager) getSystemService(Context.LOCATION_SERVICE);
                locationOff = lm == null || !LocationManagerCompat.isLocationEnabled(lm);
            }
            stopBleScanQuietly();
            finishBle(
                    id,
                    !stillOn
                            ? BLE_RESULT_DISABLED
                            : locationOff ? BLE_RESULT_LOCATION_OFF : BLE_RESULT_OK);
        };
        bleHandler.postDelayed(bleScanTimeout, bleScanDurationMs);
    }

    // `onPause`/`onDestroy` 共通: スキャン中なら止めて中断で確定する。権限ダイアログ待ち
    // （`bleScanCallback == null`）は破棄しない（ダイアログ自体が `onPause` を起こすため）。
    private void abortBleScanIfRunning() {
        if (bleScanCallback != null) {
            long id = bleScanRequest.current();
            stopBleScanQuietly();
            finishBle(id, BLE_RESULT_INTERRUPTED);
        }
    }

    // 傾きセンサのリスナー登録（Issue #831）。バックグラウンド中は登録しない
    // （onPause で解除する）ため、フォアグラウンド復帰のたびに再登録が必要。
    @Override
    protected void onResume() {
        super.onResume();
        appLifecycleHelper.onResume();
        activityResumed = true;
        webAuthSessionHelper.onResume();
        inAppBrowserHelper.onResume();
        compassHelper.onResume();
        environmentSensorsHelper.onResume();
        motionRawSensorsHelper.onResume();
        localPeerHelper.onResume();
        bluetoothClassicHelper.onResume();
        mediaPlaybackHelper.onResume();
        nowPlayingHelper.onResume();
        cameraPreviewHelper.onResume();
        if (sensorManager != null && motionSensor != null) {
            sensorManager.registerListener(
                    motionListener, motionSensor, SensorManager.SENSOR_DELAY_GAME);
        }
        // ゲームコントローラの接続監視（Issue #935）。登録＋現在の接続状況を都度再走査
        // する（onPause で解除済みのため、フォアグラウンド復帰のたびに要る。motionListener
        // と同型の対）。
        if (inputManager != null) {
            inputManager.registerInputDeviceListener(gamepadDeviceListener, null);
            reportGamepadPresence();
        }
        // 測位継続の意思（Issue #890）があり、かつ権限が既にあるなら再登録する
        // （`onPause` で解除済みのため、フォアグラウンド復帰のたびに要る。傾き（#831）
        // の motionListener と同型の対）。権限が無ければここでは要求しない
        // （ダイアログはユーザー操作起点の `requestLocationUpdates` 経由のみで出す）。
        //
        // 権限が無い場合は nativeOnLocationFailure を発行する（cursor Bugbot 指摘、
        // PR #894）。設定画面等でバックグラウンド中に権限が取り消された場合、
        // onPause で removeUpdates 済みのため以降 onLocationChanged/onLocationFailure
        // が届かず、Rust 側 `SharedLocation` が古い Fix を保持し続けてしまう
        // （`push_failure` が呼ばれて初めて古い Fix を破棄する契約、location.rs
        // モジュール doc 参照）。ここで明示的に PermissionDenied を push して
        // 画面が失効済みの古い座標を表示し続けないようにする。
        //
        // ただし `awaitingLocationPermission` が立っている間（権限ダイアログの
        // 結果待ち）は権限確認をスキップする（cursor Bugbot 指摘、PR #894）。
        // Activity 再生成中もダイアログ自体は表示され続けるため、ここで
        // `checkSelfPermission` の未許可を見て即座に PermissionDenied を push すると、
        // ユーザーがまだダイアログに応答していないのに拒否扱いになってしまう。
        // 結果は `requestLocationPermission` のコールバックが確定させる。
        if (locationWanted && !awaitingLocationPermission) {
            if (locationStartRequestPending) {
                // `requestLocationUpdates()` の Runnable が paused 中で早期 return し、
                // まだ権限確認・ダイアログ起動を一度も行えていない未実行要求
                // （codex-review P1 指摘 / cursor Bugbot 指摘、PR #894）。単純な失効
                // チェックではなく、実際に開始要求を再開する（未許可ならここで初めて
                // ダイアログを出す。ユーザーは実際には拒否していないため）。
                locationStartRequestPending = false;
                ensureLocationPermissionAndStart();
            } else if (hasLocationPermission()) {
                startLocationUpdates();
            } else {
                notifyLocationFailure(LOCATION_FAILURE_PERMISSION_DENIED);
            }
        }
        // マイク録音（Issue #957）: 許可結果が非前景中に届き `startMicRecorder` を
        // 保留していた要求を再開する（cursor Bugbot 指摘、PR #962。
        // `micStartPendingResume` doc 参照）。
        synchronized (micLock) {
            if (micStartPendingResume) {
                micStartPendingResume = false;
                long id = micRecordRequest.current();
                if (id != RequestTracker.NONE && micRecorder == null) {
                    startMicRecorder(id, micRequestedMaxDurationMs);
                }
            }
        }
        // 音声認識（Issue #1026）: 許可結果が非前景中に届き `startSpeechRecognizer` を
        // 保留していた要求を再開する（`micStartPendingResume` と同型）。
        synchronized (speechLock) {
            if (speechStartPendingResume) {
                speechStartPendingResume = false;
                long id = speechRequest.current();
                if (id != RequestTracker.NONE && speechRecognizer == null) {
                    startSpeechRecognizer(id, speechRequestedMaxListenMs);
                }
            }
        }
        // BLE スキャン（Issue #1062）: 許可結果が非前景中に届き開始を保留していた要求を再開する。
        if (bleStartPendingResume) {
            bleStartPendingResume = false;
            long id = bleScanRequest.current();
            if (id != RequestTracker.NONE && bleScanCallback == null) {
                startBleScan(id);
            }
        }
    }

    // バックグラウンド中のセンサ計測・バッテリ消費を避けるための解除（Issue #831）。
    @Override
    protected void onPause() {
        super.onPause();
        appLifecycleHelper.onPause();
        // `requestLocationUpdates`/`requestLocationPermission` の Runnable・コールバックが
        // ここより後に UI スレッドで実行される場合に、停止済み Activity へリスナーを
        // 再登録しないようにするための可視性フラグ（codex-review P1 指摘、PR #894）。
        // `removeUpdates` より先に false へ落としておく。
        activityResumed = false;
        penStylusHelper.onPause();
        nfcReadHelper.onPause();
        webAuthSessionHelper.onPause();
        inAppBrowserHelper.onPause();
        screenCaptureHelper.onPause();
        lanDiscoveryHelper.onPause();
        localPeerHelper.onPause();
        bluetoothClassicHelper.onPause();
        compassHelper.onPause();
        environmentSensorsHelper.onPause();
        motionRawSensorsHelper.onPause();
        mediaPlaybackHelper.onPause();
        nowPlayingHelper.onPause();
        cameraPreviewHelper.onPause();
        if (sensorManager != null) {
            sensorManager.unregisterListener(motionListener);
        }
        // バックグラウンド中の key up/切断の取りこぼしで「接続中」が張り付かないよう、
        // 登録解除に加えて明示的に「無し」を通知する（Issue #935、#935 実装計画 §2.3）。
        if (inputManager != null) {
            inputManager.unregisterInputDeviceListener(gamepadDeviceListener);
            nativeOnGamepadPresenceScanBegin();
            nativeOnGamepadPresenceScanEnd();
        }
        // 「使用中のみ」の権限のため、バックグラウンドでは測位しない（Issue #890）。
        if (locationManager != null) {
            locationManager.removeUpdates(locationListener);
        }
        // マイク録音（Issue #957）: バックグラウンド遷移で録音中なら破棄する（保留要求を
        // 残さない、device-bridge-contracts.md「ライフサイクル境界」節）。権限ダイアログ
        // 表示中（`micRecorder == null` だが `micRecordRequest` は追跡中）は誤破棄しない
        // （ダイアログ自体は Activity 再生成の対象外で表示され続ける契約、
        // `awaitingLocationPermission` と同じ理由）。
        synchronized (micLock) {
            if (micRecorder != null) {
                finishMicRecord(micRecordRequest.current(), MIC_RESULT_INTERRUPTED);
            }
        }
        // 音声認識（Issue #1026）: バックグラウンド遷移で聞き取り中なら破棄する
        // （`cancel()` で OS 側の結果確定を求めない。保留要求を残さない、
        // device-bridge-contracts.md「ライフサイクル境界」節）。権限ダイアログ表示中
        // （`speechRecognizer == null` だが `speechRequest` は追跡中）は誤破棄しない
        // （ダイアログ自体は Activity 再生成の対象外で表示され続ける契約、
        // `micRecorder`/`awaitingLocationPermission` と同じ理由）。
        synchronized (speechLock) {
            if (speechRecognizer != null) {
                abortSpeech(SPEECH_RESULT_INTERRUPTED);
            }
        }
        // BLE スキャン（Issue #1062）: バックグラウンド遷移でスキャン中なら止めて中断で確定する。
        abortBleScanIfRunning();
    }

    // ペン・筆圧の観測（Issue #357）。【契約】イベントは消費しない:
    // `super.dispatchTouchEvent` の戻り値をそのまま返し、GameActivity → winit → egui の
    // 既存のタッチ経路を変えない。
    @Override
    public boolean dispatchTouchEvent(MotionEvent event) {
        penStylusHelper.onTouch(event);
        return super.dispatchTouchEvent(event);
    }

    // ゲームコントローラのスティック・トリガ・十字キー軸の中継（Issue #935）。
    // 【契約】イベントは一切消費しない: `return super.dispatchGenericMotionEvent(event)`
    // の戻り値をそのまま返すことで、TV フォーカス（#280）のリモコン十字キー
    // （`KEYCODE_DPAD_*`、`dispatchKeyEvent` 経由）を含む既存の入力経路
    // （GameActivity → winit → `egui::Key::Arrow*`）を一切変えない
    // （#935 実装計画 §2.2、ソース判定は誤検出の防御でしかなく、消費しない設計自体が
    // #280 を構造的に守る）。最新サンプルのみを渡す（履歴サンプルは読まない）。
    @Override
    public boolean dispatchGenericMotionEvent(MotionEvent event) {
        int source = event.getSource();
        boolean isJoystick = (source & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK;
        if (isJoystick && event.getAction() == MotionEvent.ACTION_MOVE) {
            InputDevice device = event.getDevice();
            nativeOnGamepadMotion(
                    event.getDeviceId(),
                    source,
                    event.getAxisValue(MotionEvent.AXIS_X),
                    event.getAxisValue(MotionEvent.AXIS_Y),
                    event.getAxisValue(MotionEvent.AXIS_Z),
                    event.getAxisValue(MotionEvent.AXIS_RZ),
                    Math.max(
                            event.getAxisValue(MotionEvent.AXIS_LTRIGGER),
                            event.getAxisValue(MotionEvent.AXIS_BRAKE)),
                    Math.max(
                            event.getAxisValue(MotionEvent.AXIS_RTRIGGER),
                            event.getAxisValue(MotionEvent.AXIS_GAS)),
                    event.getAxisValue(MotionEvent.AXIS_HAT_X),
                    event.getAxisValue(MotionEvent.AXIS_HAT_Y),
                    axisFlat(device, source, MotionEvent.AXIS_X),
                    axisFlat(device, source, MotionEvent.AXIS_Y),
                    axisFlat(device, source, MotionEvent.AXIS_Z),
                    axisFlat(device, source, MotionEvent.AXIS_RZ));
        }
        return super.dispatchGenericMotionEvent(event);
    }

    // `InputDevice.getMotionRange(axis, source)` のデッドゾーン幅（`range` が無ければ
    // 0、Rust 側 `apply_flat` が NaN/負値と同様に安全側へ倒す）。
    private static float axisFlat(InputDevice device, int source, int axis) {
        if (device == null) {
            return 0f;
        }
        InputDevice.MotionRange range = device.getMotionRange(axis, source);
        return range == null ? 0f : range.getFlat();
    }

    // ゲームコントローラのボタン押下/解放の中継（Issue #935）。
    // 【契約】`dispatchGenericMotionEvent` と同じくイベントを一切消費しない
    // （`return super.dispatchKeyEvent(event)` の戻り値をそのまま返す）。ソース前段
    // フィルタ（GAMEPAD/JOYSTICK のみ）は、物理キーボードの打鍵（パスワード等）を
    // ゲームパッド経路へ誤って流さないための防御であり、消費しない設計自体が #280
    // を守っている点は上記と同じ。
    // HAT 軸を持たず十字キーを SOURCE_DPAD（KEYBOARD ビット無し）の KeyEvent でのみ
    // 通知する機種があるため、SOURCE_DPAD かつ SOURCE_KEYBOARD を含まない場合も
    // ゲームパッド経路へ通す（TV リモコンは SOURCE_KEYBOARD | SOURCE_DPAD で来る
    // ため誤って混ざらない、codex-review 指摘 PR #939）。
    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        int source = event.getSource();
        boolean isGamepad = (source & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD;
        boolean isJoystick = (source & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK;
        boolean isDpadOnly =
                (source & InputDevice.SOURCE_DPAD) == InputDevice.SOURCE_DPAD
                        && (source & InputDevice.SOURCE_KEYBOARD) != InputDevice.SOURCE_KEYBOARD;
        if (isGamepad || isJoystick || isDpadOnly) {
            int action = event.getAction();
            if (action == KeyEvent.ACTION_DOWN || action == KeyEvent.ACTION_UP) {
                nativeOnGamepadKey(
                        event.getDeviceId(), source, event.getKeyCode(), action == KeyEvent.ACTION_DOWN);
            }
        }
        return super.dispatchKeyEvent(event);
    }

    // Rust 側（`device/android.rs` の `AndroidClipboard::request`）から JNI 経由で呼ばれる
    // （Issue #859）。`android_main` は GameActivity 経路では Java の UI スレッドとは別の
    // ネイティブスレッドで走るため、`ClipboardManager` の操作を `runOnUiThread` へ委譲し、
    // 結果を `nativeOnClipboardResult` で返す（Rust 側は mailbox で待つ）。
    //
    // `ClipboardManager` を UI スレッドで扱うのは Android の作法（`getPrimaryClip` は
    // Android 10 以降アプリのフォーカス状態に依存し、OEM 実装によっては Looper を
    // 持たないスレッドからの取得が失敗する）。
    //
    // クリップボードの内容はそのまま native 側へ渡すだけで、ログ出力・保存は一切行わない
    // （クリップボードは他アプリのパスワード等が載り得る機密経路のため痕跡を残さない）。
    private void clipboardRequest(long requestId, int op, String text) {
        runOnUiThread(
                () -> {
                    ClipboardManager clipboard =
                            (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
                    if (clipboard == null) {
                        nativeOnClipboardResult(requestId, null, 2);
                        return;
                    }
                    try {
                        if (op == CLIPBOARD_OP_WRITE) {
                            clipboard.setPrimaryClip(ClipData.newPlainText(null, text));
                            nativeOnClipboardResult(requestId, null, 0);
                            return;
                        }
                        if (op != CLIPBOARD_OP_READ) {
                            // 未知の op（Rust 側との契約違反）。内容を推測せず内部エラーにする。
                            nativeOnClipboardResult(requestId, null, 2);
                            return;
                        }
                        ClipData clip = clipboard.getPrimaryClip();
                        if (clip == null || clip.getItemCount() == 0) {
                            nativeOnClipboardResult(requestId, null, 1);
                            return;
                        }
                        // `getText()` でテキストそのものだけを取る。`coerceToText` は
                        // Intent を `intent:` URI へ、Uri を文字列へ強制変換するため、
                        // Rust 側 `Clipboard` の「非テキスト内容は Ok(None)」契約と
                        // 食い違う（呼び出し側がテキストか否かを区別できなくなる。
                        // codex-review 指摘 P1、PR #860）。テキストが無ければ status=1。
                        CharSequence value = clip.getItemAt(0).getText();
                        if (value == null || value.length() == 0) {
                            nativeOnClipboardResult(requestId, null, 1);
                            return;
                        }
                        nativeOnClipboardResult(requestId, value.toString(), 0);
                    } catch (RuntimeException e) {
                        // SecurityException（フォーカス無しでの読み取り拒否）等を
                        // 取りこぼさず Rust 側へ内部エラーとして返す（例外メッセージは
                        // クリップボード内容を含み得るためログ出力しない）。
                        nativeOnClipboardResult(requestId, null, 2);
                    }
                });
    }

    // Rust 側の TTS ワーカースレッド（`device/android.rs` の `AndroidTts`）から
    // JNI 経由で呼ばれる（Issue #866）。`TextToSpeech` の操作は UI スレッドへ
    // 委譲する（クリップボードと同じ理由に加え、`OnInitListener` のコールバックが
    // UI スレッドで届く契約に揃えるため）。
    //
    // 初期化は非同期（`onInit` が後から呼ばれる）ため、初期化未完了時に本メソッドが
    // 偽の成功を返す経路を作らない: `NONE` なら生成して要求を保留し、`onInit` の
    // 結果が出た時点で初めて `nativeOnTtsResult` を呼ぶ（`onTtsInit` 参照）。
    // `INITIALIZING` 中に 2 件目の要求が来ることもある（Rust 側 `TTS_ACCEPT_MAILBOX` は
    // 受理だけで即座に空くため、初期化待ち中でも次の speak/stop が Rust 側から
    // 続けて届き得る。詳細は本体内コメント）。その場合は拒否せず、保留中の要求が
    // あればキャンセル（status 7）として畳んでから最新の要求で保留を置き換える。
    //
    // codex-review P1 是正（案A）: SPEAK が初期化待ち（`NONE`/`INITIALIZING`）へ
    // 進む直前に [`nativeOnTtsAccepted`] を呼ぶ。Rust 側 `AndroidTts::speak` は
    // この通知だけで戻るため、`speak`/`stop` を同じ FIFO で処理する Rust 側
    // ワーカーが `onInit` 完了を待たされず、直後の stop 要求を速やかに本メソッドへ
    // 届けられる（旧実装は `speak` 自体が `onInit` 完了までブロックしていた）。
    //
    // 例外メッセージは読み上げ内容を含み得るためログ出力しない（security.md A09）。
    private void ttsRequest(long requestId, int op, String text) {
        // JNI 呼び出しを受けた時点（Rust 側 `TTS_ACCEPT_MAILBOX.begin_at_if_idle()`
        // 直後）で期限の起点を確定させる（codex-review 指摘 P1）。`runOnUiThread`
        // の実行が UI スレッド混雑で遅延すると、期限の起点をそこに置いた場合
        // Rust 側の受理待ち（`TTS_ACCEPT_TIMEOUT`、5 秒、JNI 呼び出し後起点）より
        // Java 側の期限が後ろへずれ、Rust が既に受理タイムアウト失敗を返した
        // 要求を Java が後から `nativeOnTtsAccepted` してしまう（SPEAK が受理後の
        // 完了待ちに入ってしまうと、`wait_outcome` は時間で打ち切らないため
        // Rust 側は既にタイムアウト済みの要求として二度と拾わない）。
        long requestArrivalUptimeMs = SystemClock.uptimeMillis();
        runOnUiThread(
                () -> {
                    try {
                        // `runOnUiThread` で積んだ本 Runnable の実行前に `onDestroy` が
                        // 完了しているケース（codex-review 指摘 P1）。`tts` を触らず
                        // 内部エラーで畳む（破棄済み Activity 上で `TextToSpeech` を
                        // 再生成しない）。
                        if (ttsActivityDestroyed) {
                            nativeOnTtsResult(requestId, 2);
                            return;
                        }
                        if (op == TTS_OP_STOP) {
                            long canceledRequestId = pendingTtsRequest.take();
                            if (canceledRequestId != RequestTracker.NONE) {
                                // 初期化待ち中の保留 speak 要求をキャンセルとして畳む
                                // （P1 是正: stop がここへ届くようになったことで初めて
                                // 到達できる分岐。`onTtsInit` は後で必ず呼ばれるが、
                                // ここで先に `take()` しておくことで `hadPending` が
                                // false になり二重に結果を送らない）。
                                pendingTtsText = null;
                                nativeOnTtsResult(canceledRequestId, TTS_STATUS_CANCELLED);
                            }
                            if (tts != null && ttsInitState == TTS_STATE_READY) {
                                // `stop()` の失敗（ERROR）を握りつぶさない（codex-review
                                // 指摘）。戻り値を検査し、非 SUCCESS を内部エラーへ
                                // 変換して Rust 側へ伝える（status 2 = その他の内部
                                // エラー。既存の分類に「停止失敗」を新設するほどの
                                // 呼び分けは Rust 側に無いため、既存の catch-all を使う）。
                                int stopResult = tts.stop();
                                nativeOnTtsResult(requestId, stopResult == TextToSpeech.SUCCESS ? 0 : 2);
                                return;
                            }
                            // `READY` 未満（NONE/INITIALIZING/FAILED）はまだ一度も
                            // `speak()` を呼んでいないため OS 側で止めるものが無い。
                            // INITIALIZING 中に保留していた発話があれば、ここで破棄する
                            // （codex-review 指摘 P1: 停止済みの要求が遅延 onInit 後に
                            // 発話されてしまう問題）。`pendingTtsRequest` を `take()` で
                            // 落とすことで `onTtsInit` の `hadPending` 判定が false になり、
                            // 後から初期化が成功しても発話されない。
                            pendingTtsRequest.take();
                            pendingTtsText = null;
                            nativeOnTtsResult(requestId, 0);
                            return;
                        }
                        if (op != TTS_OP_SPEAK) {
                            // 未知の op（Rust 側との契約違反）。内容を推測せず内部エラーにする。
                            nativeOnTtsResult(requestId, 2);
                            return;
                        }

                        // ここから先は SPEAK。SPEAK は受理 → 結果の二相を経路によらず
                        // 統一する（codex-review P1 再指摘・Bugbot High 是正、PR #869
                        // 3 巡目: 「受理通知は初期化待ちへ進む場合のみ」という前回の
                        // 限定が誤りだった。`READY`（発話まで即座に進む経路）でも
                        // `nativeOnTtsAccepted` を送らずに `speakNow` していたため、
                        // 完了コールバック化（本 PR 1 巡目）後は `speak()` の投入成功が
                        // 何も返さず `onDone` まで待つようになった結果、Rust 側
                        // `TTS_ACCEPT_MAILBOX` の受理待ちが `TTS_ACCEPT_TIMEOUT`
                        // （5 秒）で必ずタイムアウトし、5 秒を超えて続く正常な発話が
                        // 失敗表示になっていた）。単相応答（`nativeOnTtsAccepted` を
                        // 送らず一発で `nativeOnTtsResult` を返す）は下記の早期 return
                        // する失敗系（最大文字数超過・`FAILED`）のみに限定する。
                        if (text.length() > TextToSpeech.getMaxSpeechInputLength()) {
                            // `getMaxSpeechInputLength()` は static メソッド（`tts`
                            // インスタンスの有無に関わらず呼べる）のため、初回要求
                            // （`tts` 未生成）でも検査できる。
                            nativeOnTtsResult(requestId, 6);
                            return;
                        }
                        if (ttsInitState == TTS_STATE_FAILED) {
                            nativeOnTtsResult(requestId, 1);
                            return;
                        }
                        if (ttsInitState == TTS_STATE_READY) {
                            // `READY` でも `runOnUiThread` 自体がキュー混雑で遅延し得るため、
                            // ここでも `requestArrivalUptimeMs` 起点の期限を検査する
                            // （codex-review 指摘 P1: READY 経路が期限未検査だった）。
                            // 期限の基準は受理まで（`TTS_ACCEPT_TIMEOUT`、5 秒、
                            // `TTS_ACCEPT_TIMEOUT_MS`）であり、`nativeOnTtsAccepted` を
                            // 送った後の発話完了待ちには掛からない（Rust 側
                            // `TTS_OUTCOME_MAILBOX.wait_or_ok` は完了・失敗・キャンセル・
                            // 世代置換でのみ終了し、時間では終了しない、PR #869 3 巡目
                            // 是正）。期限超過なら Rust 側 `TTS_ACCEPT_MAILBOX` は既に
                            // タイムアウトで諦めているため `nativeOnTtsAccepted`/
                            // `nativeOnTtsResult` のいずれも送らず破棄する
                            // （`expireStaleTtsRequest`/`onTtsInit` と同じ扱い）。
                            if (SystemClock.uptimeMillis()
                                    >= requestArrivalUptimeMs + TTS_ACCEPT_TIMEOUT_MS) {
                                return;
                            }
                            // 受理を通知してから発話する（codex-review P1 再指摘・
                            // Bugbot High 是正: 旧実装はここで受理を送らず一発応答扱い
                            // にしていたため、完了コールバック化後は `TTS_ACCEPT_MAILBOX`
                            // の受理待ちが必ずタイムアウトしていた）。`speak()` が
                            // `ERROR` を返した場合の `nativeOnTtsResult(5)`
                            // （`speakNow` 参照）は受理後の失敗として `TTS_OUTCOME_MAILBOX`
                            // 側へ届く（`nativeOnTtsResult` の accept→outcome
                            // ルーティング、`Java_..._nativeOnTtsResult` doc 参照）。
                            nativeOnTtsAccepted(requestId);
                            speakNow(requestId, text);
                            return;
                        }
                        // ここに来るのは NONE か INITIALIZING。案A 是正（本 PR）により
                        // `nativeOnTtsAccepted` 送出後の Rust 側 `AndroidTts::speak` は
                        // 受理のみで戻るため、`TTS_ACCEPT_MAILBOX` は初期化待ち中でも
                        // 即座に空く。つまり INITIALIZING 中に複数回ここへ到達し得る
                        // （cursor Bugbot 指摘: 従来は status 4 で拒否し続け、新規発話が
                        // 回復不能になっていた）。旧保留要求（あれば）は Rust 側
                        // `TTS_OUTCOME_MAILBOX` がまだ待っている可能性があるため、
                        // 黙って捨てず `TTS_STATUS_CANCELLED` として確定させてから
                        // 最新の要求で保留を置き換える（旧: 単一 mailbox 前提で
                        // 「もう待っていない」と仮定して応答を省略していたが、案Aでは
                        // 受理と結果待ちが分離されているためこの仮定が成り立たない）。
                        //
                        // ただしこの要求 (requestId) 自体が `runOnUiThread` の遅延で
                        // 既に Rust 側の受理待ち（`TTS_ACCEPT_TIMEOUT`、5 秒）を
                        // 超えて到着していることがある（codex-review P1 再指摘。
                        // `READY` 経路の同種チェックと同じ判断、`TTS_ACCEPT_TIMEOUT_MS`
                        // 参照）。この場合 Rust 側は既に `nativeOnTtsAccepted` を
                        // 待たずタイムアウト失敗を返し `TTS_OUTCOME_MAILBOX` の登録も
                        // 自ら畳んでいるため、ここで受理を送っても誰も待っていない
                        // （無害だが無意味）。既存の保留要求を巻き込んで置き換える前に
                        // 検査し、期限切れなら requestId 自身を何もせず破棄する
                        // （既存の保留要求には触れない）。
                        if (SystemClock.uptimeMillis()
                                >= requestArrivalUptimeMs + TTS_ACCEPT_TIMEOUT_MS) {
                            return;
                        }
                        long canceledRequestId = pendingTtsRequest.take();
                        if (canceledRequestId != RequestTracker.NONE) {
                            nativeOnTtsResult(canceledRequestId, TTS_STATUS_CANCELLED);
                        }
                        pendingTtsRequest.track(requestId);
                        pendingTtsText = text;
                        // 新しい保留要求の受理を通知する（案A、`nativeOnTtsAccepted` doc
                        // 参照）。`onInit` 完了・`TextToSpeech` 構築の成否はまだ関係ない。
                        nativeOnTtsAccepted(requestId);
                        // `TTS_INIT_TIMEOUT_MS`（Java 側だけが持つ安全弁、上記 doc 参照）
                        // を仕込む（codex-review 指摘 P1）。期限は `requestArrivalUptimeMs`
                        // （JNI 呼び出し受信時点）起点で確定済み（`ttsRequest` 冒頭・
                        // `pendingTtsDeadlineUptimeMs` doc 参照）。`postDelayed` では
                        // なく `postAtTime` で絶対時刻指定にすることで、この
                        // `runOnUiThread` 自体の実行遅延を期限へ持ち込まない。
                        // `onInit` がこの期限を超えて遅延した場合、既に受理済み
                        // （`nativeOnTtsAccepted` 送出済み）のため Rust 側
                        // `wait_outcome` は時間で諦めず待ち続けている。Java 側の
                        // 保留（`pendingTtsRequest`/`pendingTtsText`）を期限到来時点で
                        // 破棄し（`expireStaleTtsRequest` 参照）、`nativeOnTtsResult`
                        // で status 3（init 失敗相当）を送って `wait_outcome` を
                        // 確定させる（codex-review 指摘、PR #869 4 巡目是正: 送らないと
                        // 世代置換まで無期限に残る）。期限到来時点で `requestId` が
                        // 変わっていなければ（＝新しい要求で上書きされていなければ）
                        // 保留を破棄し、遅れて届く `onInit(SUCCESS)` が `speakNow` を
                        // 呼ばないようにする。
                        pendingTtsDeadlineUptimeMs = requestArrivalUptimeMs + TTS_INIT_TIMEOUT_MS;
                        mainHandler.postAtTime(
                                () -> expireStaleTtsRequest(requestId),
                                pendingTtsDeadlineUptimeMs);
                        if (ttsInitState == TTS_STATE_NONE) {
                            ttsInitState = TTS_STATE_INITIALIZING;
                            try {
                                tts = new TextToSpeech(
                                        getApplicationContext(),
                                        status -> mainHandler.post(() -> onTtsInit(status)));
                                // `onInit` 完了前に設定してよい（`speak()` を呼ぶまでは
                                // コールバックが発生しないため）。`speakNow` が使う
                                // utteranceId 経由の完了通知はこのリスナーが担う
                                // （`utteranceListener` doc 参照、PR #869 是正）。
                                tts.setOnUtteranceProgressListener(utteranceListener);
                            } catch (RuntimeException e) {
                                // コンストラクタ自体が例外を投げた（engine 未搭載端末等）。
                                ttsInitState = TTS_STATE_FAILED;
                                pendingTtsRequest.take();
                                pendingTtsText = null;
                                nativeOnTtsResult(requestId, 1);
                            }
                        }
                        // `INITIALIZING` 中はここで戻り、`onTtsInit`（または上記の
                        // stop によるキャンセル分岐）が保留要求を処理する。
                    } catch (RuntimeException e) {
                        // 受理を送出済みでも、最終結果は必ず送って outcome 側の
                        // 待ちを解放する（`nativeOnTtsAccepted` 後の想定外例外）。
                        pendingTtsRequest.take();
                        pendingTtsText = null;
                        nativeOnTtsResult(requestId, 2);
                    }
                });
    }

    // `ttsRequest` が `mainHandler.postDelayed` で `TTS_INIT_TIMEOUT_MS` 後に予約する
    // 保留要求の期限切れ処理（codex-review 指摘 P1）。`requestId` が積んだ時点から
    // 変わっていない（＝新しい要求で上書きされていない）場合のみ保留を破棄する。
    // `onTtsInit` が先に届いていれば `pendingTtsRequest` は既に `NONE` のため
    // `complete` は false になり無害。
    private void expireStaleTtsRequest(long requestId) {
        if (pendingTtsRequest.complete(requestId)) {
            pendingTtsText = null;
            // 受理済み（`nativeOnTtsAccepted` 送出済み）のため Rust 側
            // `wait_outcome`（`TTS_OUTCOME_MAILBOX.wait_or_ok`）は時間で
            // 打ち切らない。ここで送らないと世代置換（次の speak/stop）まで
            // 無期限に残ってしまうため、init 失敗相当（status 3）で確定させる
            // （codex-review 指摘、PR #869 4 巡目是正）。`onTtsInit` 側が
            // 同じ requestId を先に処理済みなら `pendingTtsRequest` は既に
            // `NONE` のためここへは来ない（二重送信にならない）。
            nativeOnTtsResult(requestId, 3);
        }
    }

    // `TextToSpeech` の非同期初期化コールバック（UI スレッドで実行、`ttsRequest` 参照）。
    // 保留していた要求（あれば）をここで初めて確定させる。
    private void onTtsInit(int status) {
        // `onDestroy` 完了後に届く初期化コールバック（codex-review 指摘 P1）。
        // 保留要求はあれば `onDestroy` が既にエラーで畳んで `tts` を shutdown/null
        // 済みのため、ここで `ttsInitState`/`tts` へ触れず素通しする。
        if (ttsActivityDestroyed) {
            return;
        }
        long requestId = pendingTtsRequest.take();
        boolean hadPending = requestId != RequestTracker.NONE;
        String text = pendingTtsText;
        pendingTtsText = null;

        // `postAtTime` で予約した `expireStaleTtsRequest` がまだキューで順番待ちの
        // 間に `onInit` が先に届くことがある（同じ UI スレッドキュー上でも、期限
        // ちょうどの前後では投入順・処理順が入れ替わり得る）。`hadPending`
        // だけに頼らず、発話直前に実期限を検査する（codex-review 指摘 P1:
        // 「初期化完了時・READY 経路の発話直前に検査する」）。期限超過の場合は
        // `expireStaleTtsRequest` と同じ扱い（status 3 で `wait_outcome` を
        // 確定させる、PR #869 4 巡目是正）にする。`pendingTtsRequest` は既に
        // 上で `take()` 済み（`NONE`）のため、後から実行される
        // `expireStaleTtsRequest`（未実行ならキューに残っている）は
        // `complete` が不一致になり二重送信にならない。
        if (hadPending && SystemClock.uptimeMillis() >= pendingTtsDeadlineUptimeMs) {
            hadPending = false;
            nativeOnTtsResult(requestId, 3);
        }

        if (status == TextToSpeech.SUCCESS) {
            ttsInitState = TTS_STATE_READY;
            if (hadPending) {
                speakNow(requestId, text);
            }
        } else {
            ttsInitState = TTS_STATE_FAILED;
            if (hadPending) {
                nativeOnTtsResult(requestId, 3);
            }
        }
    }

    // `tts` が `READY` になった後の実発話（`ttsRequest`/`onTtsInit` の共通経路）。
    // `QUEUE_FLUSH` で直前の発話を中断してから始める（`Tts::speak` の契約
    // 「直前の発話が続いていれば中断してから始める」に対応）。中断された前の発話は
    // `utteranceListener#onStop` が別途キャンセルとして確定させる。
    //
    // `speak()` の戻り値（`SUCCESS`）はキュー投入成功に過ぎず発話完了ではない
    // （codex-review P1 指摘、PR #869）。ここでは投入失敗（`ERROR`）のときのみ
    // 即座に `nativeOnTtsResult` を送る（コールバックが二度と来ないため）。
    // 投入成功時は何も送らず、真の完了・失敗・中断は `utteranceListener` が
    // utteranceId（`UTTERANCE_ID_PREFIX + requestId`）経由で後から確定させる。
    private void speakNow(long requestId, String text) {
        // `tts.speak()` 呼び出し前に「発話中」として記録する（`onDone` 等が
        // 呼び出し中・直後に別スレッドで届いても取りこぼさないよう、呼び出し後
        // ではなく前に track する。`speakingRequest` doc 参照）。
        speakingRequest.track(requestId);
        int result = tts.speak(
                text, TextToSpeech.QUEUE_FLUSH, null, UTTERANCE_ID_PREFIX + requestId);
        if (result != TextToSpeech.SUCCESS) {
            // 投入自体が失敗しコールバックは二度と来ないため、「発話中」の
            // 記録を戻す。
            speakingRequest.complete(requestId);
            nativeOnTtsResult(requestId, 5);
        }
    }

    // Rust 側の worker スレッド（`device/android.rs` の `AndroidCamera::capture`）から
    // JNI 経由で呼ばれる（Issue #750）。UI 操作を伴うため `runOnUiThread` へ委譲する。
    // `requestId` は先に `pendingCaptureRequest` へ保持してから `nativeOnCameraResult`
    // へ返す（`runOnUiThread` はワーカースレッドの呼び出しと非同期のため、フィールドへの
    // 書き込みをコールバック登録より先に済ませておく）。
    private void requestCameraCapture(long requestId) {
        if (awaitingCameraResult) {
            // 旧要求の Android コールバック（権限ダイアログ・カメラアプリ）が未回収
            // （`awaitingCameraResult` doc 参照、codex-review 指摘 PR #790）。
            // `pendingCaptureRequest` を上書きすると旧要求の遅延結果がこの新要求の
            // ID で誤って配送されるため、ここで即座に拒否する
            // （`runOnUiThread` を経由しない。UI 操作を伴わないため）。
            nativeOnCameraResult(requestId, null, 0, 0, 4);
            return;
        }
        // この時点で `awaitingCameraResult` は false（上の分岐で保証済み）であり、
        // それは `pendingCaptureInterrupted` も false であることを含意する
        // （`pendingCaptureInterrupted` は常に `awaitingCameraResult` の部分集合として
        // 維持され、解除は必ず両方同時に行う。doc 参照）。よってここで
        // `pendingCaptureInterrupted` を触る必要はない。
        pendingCaptureRequest.track(requestId);
        awaitingCameraResult = true;
        runOnUiThread(() -> {
            try {
                if (ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA)
                        == PackageManager.PERMISSION_GRANTED) {
                    launchCameraCapture();
                } else {
                    requestCameraPermission.launch(Manifest.permission.CAMERA);
                }
            } catch (RuntimeException e) {
                // `launch()` 自体が例外を送出した場合（onSaveInstanceState 後の呼び出し等、
                // ActivityResultRegistry の状態不整合）に awaitingCameraResult が true の
                // まま恒久的に固着し、以後すべての要求が status 4 で拒否され続けるのを防ぐ
                // （cursor Bugbot 指摘、PR #790）。ID 退避の理由は requestCameraPermission
                // コールバック側コメント参照。
                long resultRequestId = pendingCaptureRequest.current();
                awaitingCameraResult = false;
                nativeOnCameraResult(resultRequestId, null, 0, 0, 6);
            }
        });
    }

    private void launchCameraCapture() {
        try {
            takePicturePreview.launch(null);
        } catch (ActivityNotFoundException e) {
            // 標準カメラアプリが端末に存在しない（エミュレータ等）。
            long resultRequestId = pendingCaptureRequest.current();
            awaitingCameraResult = false;
            nativeOnCameraResult(resultRequestId, null, 0, 0, 2);
        } catch (RuntimeException e) {
            // `requestCameraPermission` の granted コールバック経由でここへ来た場合は
            // 上位（requestCameraCapture の runOnUiThread）の try/catch を経由しないため、
            // ここでも launch() 失敗によるフラグ固着を防ぐ必要がある（cursor Bugbot 指摘、
            // PR #790。ID 退避の理由は requestCameraPermission コールバック側コメント参照）。
            long resultRequestId = pendingCaptureRequest.current();
            awaitingCameraResult = false;
            nativeOnCameraResult(resultRequestId, null, 0, 0, 6);
        }
    }

    // `Bitmap.getPixels` の ARGB_8888 int[] を RGBA byte[]（fandhe_device::Image と同じ
    // 行優先・パディングなしレイアウト）へ詰め替えて native 側へ渡す。
    private void deliverBitmap(Bitmap bitmap) {
        // ID 退避の理由は requestCameraPermission コールバック側コメント参照
        // （codex-review 指摘、PR #790）。
        long resultRequestId = pendingCaptureRequest.current();
        awaitingCameraResult = false;
        int width = bitmap.getWidth();
        int height = bitmap.getHeight();

        // 配列確保前にサイズを検証する（codex-review 指摘、PR #790）。`long` で計算し、
        // `int` オーバーフロー（`width * height` や `* 4`）を経由せず Rust 側
        // `MAX_IMAGE_BYTES`（64 MiB）と同じ上限を先に確認する。外部カメラアプリが
        // 返す Bitmap の寸法は検証していないため、大寸法（例: 4097x4096）だと
        // Rust 側の検査（`decode_capture_result`）に届く前に Java 側だけで
        // 128 MiB 超の int[]/byte[] を確保してしまい、ヒープの小さい端末で
        // OutOfMemoryError になり得る。
        long pixelCount = (long) width * (long) height;
        long byteCount = pixelCount * 4;
        if (byteCount > MAX_IMAGE_BYTES) {
            nativeOnCameraResult(resultRequestId, null, width, height, 5);
            return;
        }

        int[] argb = new int[width * height];
        bitmap.getPixels(argb, 0, width, 0, 0, width, height);

        byte[] rgba = new byte[width * height * 4];
        for (int i = 0; i < argb.length; i++) {
            int pixel = argb[i];
            int offset = i * 4;
            rgba[offset] = (byte) ((pixel >> 16) & 0xFF); // R
            rgba[offset + 1] = (byte) ((pixel >> 8) & 0xFF); // G
            rgba[offset + 2] = (byte) (pixel & 0xFF); // B
            rgba[offset + 3] = (byte) ((pixel >> 24) & 0xFF); // A
        }
        nativeOnCameraResult(resultRequestId, rgba, width, height, 0);
    }

    // Rust 側 bridge モジュール（`device/file_pick.rs`）の `drive`（Android 分岐）から
    // JNI 経由で呼ばれる（Issue #874、案 B）。`android_main` は Java の UI スレッドとは
    // 別のネイティブスレッドで走るため、`ActivityResultLauncher` の起動を
    // `runOnUiThread` へ委譲する。呼び出し元は即時 return（FFI）で、結果は非同期に
    // `nativeOnFilePickerResult` で通知する（`FilePicker` trait の同期 `pick()` は通らない、
    // モジュール doc 参照）。
    private void requestFilePick(long requestId) {
        filePickRequest.track(requestId);
        runOnUiThread(() -> {
            try {
                pickDocument.launch(new String[] {"*/*"});
            } catch (RuntimeException e) {
                // ドキュメントプロバイダが端末に存在しない、または `launch()` 自体が
                // 例外を送出した場合（`onSaveInstanceState` 後の呼び出し等）。
                // ピッカーは開かれておらず `pickDocument` コールバックも来ないため、
                // ここで直接 code 2 を通知する（仕様書「OS 側の受け口」参照）。
                filePickRequest.take();
                nativeOnFilePickerResult(requestId, 2, null, -1);
            }
        });
    }

    // Rust 側 `device/photo_pick.rs`（Android 分岐）から JNI 経由で呼ばれる（Issue #1104、
    // 署名 `(J)V`）。`requestFilePick` と同じ「即時 return・結果は `nativeOnPhotoPickerResult`」。
    // 権限は要らず、マニフェスト・依存も追加しない。
    private void requestPhotoPick(long requestId) {
        photoPickRequest.track(requestId);
        runOnUiThread(() -> {
            try {
                pickPhoto.launch(new PickVisualMediaRequest.Builder()
                        .setMediaType(ActivityResultContracts.PickVisualMedia.ImageOnly.INSTANCE)
                        .build());
            } catch (RuntimeException e) {
                // ピッカーが開かれずコールバックも来ない経路（`requestFilePick` と同じ）。
                photoPickRequest.take();
                nativeOnPhotoPickerResult(requestId, 2, null, -1);
            }
        });
    }

    // Rust 側 `device/contact_pick.rs`（Android 分岐）から JNI 経由で呼ばれる（Issue #1222、
    // 署名 `(J)V`）。即時 return・結果は `nativeOnContactPickerResult`。権限・依存は追加しない。
    private void requestContactPick(long requestId) {
        runOnUiThread(() -> {
            // 再生成前に開いた連絡先アプリの結果がまだ届いていない。その結果がこの要求の id を
            // 消費しないよう、起動せずに失敗で返す（`contactPickerOpen` doc 参照）。
            if (contactPickerOpen) {
                nativeOnContactPickerResult(requestId, 3, null);
                return;
            }
            contactPickRequest.track(requestId);
            contactPickerOpen = true;
            try {
                pickContact.launch(null);
            } catch (RuntimeException e) {
                // 連絡先アプリ無し（ActivityNotFoundException）等。コールバックが来ない経路なので
                // ここで一度だけ通知する。
                contactPickerOpen = false;
                contactPickRequest.take();
                nativeOnContactPickerResult(requestId, 3, null);
            }
        });
    }

    // 選ばれた連絡先の表示名だけを読んで Rust へ渡す（ワーカースレッド）。projection は
    // DISPLAY_NAME の固定 1 列、selection は使わない。Uri・名前・例外メッセージはログへ出さない（A09）。
    private void deliverPickedContact(long requestId, Uri uri) {
        String name = null;
        try (Cursor c = getContentResolver().query(
                uri, new String[] {ContactsContract.Contacts.DISPLAY_NAME}, null, null, null)) {
            if (c != null && c.moveToFirst()) {
                name = c.getString(0);
            }
        } catch (RuntimeException e) {
            name = null;
        }
        if (name == null) {
            nativeOnContactPickerResult(requestId, 3, null);
        } else {
            nativeOnContactPickerResult(requestId, 0, name);
        }
    }

    // 選ばれた写真を Rust へ渡す（ワーカースレッド）。JPEG / PNG は fd をそのまま渡し、それ以外
    // （HEIC・GIF・WebP 等。Rust の復号器は JPEG / PNG のみ）は JPEG の一時ファイルへ変換して
    // 渡す。MIME は OS の自己申告で、最終的な形式判定は Rust 側 `validate_photo` が行う。
    // 一時ファイルは成功・失敗のどちらでも削除する（Rust は呼び出しの中で読み切る）。
    // 変換の失敗（復号不能・OOM・長辺超過）は code 2。Uri・例外メッセージはログへ出さない（A09）。
    private void deliverPickedPhoto(long requestId, Uri uri) {
        File converted = null;
        try {
            String type = getContentResolver().getType(uri);
            ParcelFileDescriptor pfd;
            // 変換前の元データが上限超なら変換せず fd をそのまま渡し、Rust 側の上限判定に
            // `InvalidInput` で拒否させる（申告サイズは信用せず、実際に読んで数える）。
            if ("image/jpeg".equals(type) || "image/png".equals(type) || photoExceedsLimit(uri)) {
                pfd = getContentResolver().openFileDescriptor(uri, "r");
            } else {
                converted = convertPhotoToJpeg(uri);
                pfd = converted == null
                        ? null
                        : ParcelFileDescriptor.open(converted, ParcelFileDescriptor.MODE_READ_ONLY);
            }
            if (pfd == null) {
                nativeOnPhotoPickerResult(requestId, 2, null, -1);
                return;
            }
            // `detachFd()` で所有権が Rust へ移った後の close は no-op（`deliverPickedDocument`
            // と同じ契約）。detach 前に失敗した場合は try-with-resources が閉じる。
            try (ParcelFileDescriptor owned = pfd) {
                int fd = owned.detachFd();
                nativeOnPhotoPickerResult(requestId, 0, "photo", fd);
            }
        } catch (IOException | RuntimeException | OutOfMemoryError e) {
            nativeOnPhotoPickerResult(requestId, 2, null, -1);
        } finally {
            if (converted != null) {
                converted.delete();
            }
        }
    }

    // 元データが PHOTO_MAX_BYTES を超えるか。最大 PHOTO_MAX_BYTES + 1 バイトまで読んで数える。
    private boolean photoExceedsLimit(Uri uri) throws IOException {
        try (InputStream in = getContentResolver().openInputStream(uri)) {
            if (in == null) {
                return false;
            }
            byte[] buf = new byte[64 * 1024];
            long total = 0;
            int n;
            while ((n = in.read(buf)) > 0) {
                total += n;
                if (total > PHOTO_MAX_BYTES) {
                    return true;
                }
            }
            return false;
        }
    }

    // 画像を JPEG（品質 90）の一時ファイルへ変換する。失敗は null（呼び出し側が code 2 にする）。
    // ponytail: 長辺を PHOTO_CONVERT_SIDE 以下へ縮めて復号する（変換したときだけ。報告される
    // 寸法は変換後の値）。透明は JPEG で黒になる。必要になったら背景色を指定する。
    private File convertPhotoToJpeg(Uri uri) throws IOException {
        // HEIC / WebP のデコーダはシーク可能な入力を要する。openInputStream は非シーク
        // （パイプ等）になり得るため、fd（シーク可能なら）か上限付き一時コピーから復号する。
        ParcelFileDescriptor pfd = getContentResolver().openFileDescriptor(uri, "r");
        if (pfd == null) {
            return null;
        }
        File copy = null;
        try {
            FileDescriptor src = pfd.getFileDescriptor();
            boolean seekable = true;
            try {
                Os.lseek(src, 0, OsConstants.SEEK_CUR);
            } catch (ErrnoException e) {
                seekable = false;
            }
            if (!seekable) {
                copy = File.createTempFile("fandhe_photo_src_", ".bin", getCacheDir());
                long total = 0;
                try (InputStream in = new ParcelFileDescriptor.AutoCloseInputStream(pfd);
                        FileOutputStream os = new FileOutputStream(copy)) {
                    byte[] buf = new byte[64 * 1024];
                    int n;
                    while ((n = in.read(buf)) > 0) {
                        total += n;
                        if (total > PHOTO_MAX_BYTES) {
                            return null;
                        }
                        os.write(buf, 0, n);
                    }
                }
                try (ParcelFileDescriptor cpfd =
                        ParcelFileDescriptor.open(copy, ParcelFileDescriptor.MODE_READ_ONLY)) {
                    return decodeAndEncodeJpeg(cpfd.getFileDescriptor());
                }
            }
            return decodeAndEncodeJpeg(src);
        } finally {
            try {
                pfd.close();
            } catch (IOException ignored) {
                // 二重 close（AutoCloseInputStream 経由）や close 失敗は無視してよい。
            }
            if (copy != null) {
                copy.delete();
            }
        }
    }

    // シーク可能な fd から復号して JPEG 一時ファイルへ書く。失敗は null。
    private File decodeAndEncodeJpeg(FileDescriptor fd) throws IOException {
        BitmapFactory.Options bounds = new BitmapFactory.Options();
        bounds.inJustDecodeBounds = true;
        BitmapFactory.decodeFileDescriptor(fd, null, bounds);
        int longSide = Math.max(bounds.outWidth, bounds.outHeight);
        if (longSide <= 0 || longSide > PHOTO_MAX_SIDE) {
            return null;
        }
        // 総画素数も復号前に検査する（Rust 側 `MAX_PHOTO_PIXELS`・iOS 実装と揃える）。
        if ((long) bounds.outWidth * (long) bounds.outHeight > PHOTO_MAX_PIXELS) {
            return null;
        }
        BitmapFactory.Options opts = new BitmapFactory.Options();
        // 既定値は 0（未設定）で、割り算の分母にできない。
        opts.inSampleSize = 1;
        while (longSide / opts.inSampleSize > PHOTO_CONVERT_SIDE) {
            opts.inSampleSize *= 2;
        }
        Bitmap bitmap = null;
        File out = null;
        boolean done = false;
        try {
            bitmap = BitmapFactory.decodeFileDescriptor(fd, null, opts);
            if (bitmap == null) {
                return null;
            }
            out = File.createTempFile("fandhe_photo_", ".jpg", getCacheDir());
            try (FileOutputStream os = new FileOutputStream(out)) {
                if (!bitmap.compress(Bitmap.CompressFormat.JPEG, 90, os)) {
                    return null;
                }
            }
            // 所有権を呼び出し側へ移す。以降の削除は呼び出し側の責務。
            done = true;
            return out;
        } finally {
            if (bitmap != null) {
                bitmap.recycle();
            }
            // 成功以外の全終了経路（OOM を含む）で一時ファイルを消す。
            if (!done && out != null) {
                out.delete();
            }
        }
    }

    // Rust 側 bridge モジュール（`device/share.rs`）の `present`（Android 分岐）から
    // JNI 経由で呼ばれる（Issue #899）。`requestFilePick` と同じ「即時 return・結果は
    // 非同期通知」の形だが、chooser 自体は結果を返さないため（上記 `shareChooser` doc
    // 参照）、選択完了は `IntentSender` broadcast（`shareChosenReceiver`）で別途検出する
    // （実装計画 §2.3）。`url` が無い場合（Rust 側 `ShareRequest::url()` が `None`）は
    // `null` が渡ってくる契約。
    //
    // 共有テキスト・URL・例外メッセージをログへ出力しない（security.md A09）。
    private void requestShare(long requestId, String text, String url) {
        shareRequest.track(requestId);
        runOnUiThread(() -> {
            try {
                registerShareChosenReceiver(requestId);

                Intent send = new Intent(Intent.ACTION_SEND);
                send.setType("text/plain");
                send.putExtra(Intent.EXTRA_TEXT, url == null ? text : text + "\n" + url);

                // `IntentSender`（API 22+、minSdk 24 で利用可）で選択完了 broadcast を
                // 受け取る（実装計画 §2.3）。`setPackage(自パッケージ)` で自プロセス以外
                // からの偽装 broadcast を受理しない（security.md A01/A05）。API 31+ は
                // system が extras を書き込むため `FLAG_MUTABLE` が必須（API < 31 では
                // 未知ビットとして無視される）。
                Intent chosenIntent =
                        new Intent(shareChosenAction(requestId)).setPackage(getPackageName());
                int flags = PendingIntent.FLAG_UPDATE_CURRENT
                        | (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ? PendingIntent.FLAG_MUTABLE : 0);
                PendingIntent pi = PendingIntent.getBroadcast(this, 0, chosenIntent, flags);

                Intent chooser = Intent.createChooser(send, null, pi.getIntentSender());
                shareChooser.launch(chooser);
            } catch (RuntimeException e) {
                // `launch()` 自体が例外を送出した場合（ドキュメントプロバイダ非対応の
                // ケースと同型、`requestFilePick` 参照）。chooser は開かれておらず
                // `shareChooser`/`shareChosenReceiver` コールバックも来ないため、
                // ここで直接 code 3（内部エラー）を通知する。
                shareRequest.take();
                unregisterShareChosenReceiver();
                nativeOnShareResult(requestId, 3);
            }
        });
    }

    // 選択完了 broadcast の受信器を Application Context へ登録する（`requestShare` から
    // 要求ごとに呼ばれる）。Activity 単位ではなく Application Context に登録する理由は
    // `shareChosenReceiver` doc 参照（Activity 再生成の `onDestroy` を跨いで生存させる
    // ため）。`RECEIVER_NOT_EXPORTED`（API 33+ の必須引数、`ContextCompat` 経由で
    // それ未満でも安全に動作する）で他アプリからの直接送信を遮断する。
    private void registerShareChosenReceiver(long requestId) {
        unregisterShareChosenReceiver();
        shareChosenReceiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                // `requestId` はこの受信器を登録した時点の要求（クロージャで固定）。
                // `shareRequest` static は他要求で書き換わり得るため、両者が一致する
                // ときだけ処理する（reviewer P1 是正、PR #903: action を要求ごとに一意に
                // したため通常はここへ他要求の broadcast は届かないが、念のための二重
                // チェック）。不一致なら古い通知として破棄する。
                if (!shareRequest.complete(requestId)) {
                    return;
                }
                unregisterShareChosenReceiver();
                nativeOnShareResult(requestId, 0);
            }
        };
        ContextCompat.registerReceiver(
                getApplicationContext(),
                shareChosenReceiver,
                new IntentFilter(shareChosenAction(requestId)),
                ContextCompat.RECEIVER_NOT_EXPORTED);
    }

    // `requestShare`/`shareChooser`/`shareChosenReceiver` のいずれの決着経路からも呼ぶ
    // 後始末（二重登録・リーク防止）。未登録（`null`）での呼び出しは no-op。
    private void unregisterShareChosenReceiver() {
        if (shareChosenReceiver == null) {
            return;
        }
        try {
            getApplicationContext().unregisterReceiver(shareChosenReceiver);
        } catch (IllegalArgumentException e) {
            // 未登録状態での unregister（二重呼び出し等）。無害なため握りつぶす。
        }
        shareChosenReceiver = null;
    }

    // Rust 側 bridge モジュール（`device/biometric_auth.rs`）の `present`（Android
    // 分岐）から JNI 経由で呼ばれる（Issue #908）。`requestShare` と同じ「即時
    // return・結果は非同期通知」の形。`reason` は Rust 側 `validate_reason`（空・NUL
    // 拒否）を通した非空文字列の契約。表示にのみ使い、ログへ出さない
    // （security.md A03/A09）。
    private void requestBiometric(long requestId, String reason) {
        biometricRequest.track(requestId);
        runOnUiThread(() -> {
            // `isFinishing()`/`isDestroyed()` は破棄処理中しか検出できず、
            // `onSaveInstanceState` 済み（state-save 後）の resumed でない状態は
            // すり抜ける（codex-review P1 指摘 / cursor Bugbot 指摘、PR #917）。
            // state-save 後に `BiometricPrompt.authenticate` を呼ぶと
            // `IllegalStateException` を送出せず例外・コールバックとも無しで
            // 打ち切られるため、catch でも回収できず Rust 側は `Authenticating`
            // のままポーリングし続けてしまう。`activityResumed` は `onPause`
            // （`onSaveInstanceState` より必ず先に走る、API 28 以降の
            // 「onStop 後に onSaveInstanceState」順でも同様）で false に落ちる
            // ため、ここで見れば state-save 後の呼び出しを確実に弾ける
            // （測位の resumed ゲートと同型、本ファイル `onPause`/`onResume` 参照）。
            if (isFinishing() || isDestroyed() || !activityResumed) {
                biometricRequest.take();
                nativeOnBiometricResult(requestId, BIOMETRIC_RESULT_INTERNAL_ERROR);
                return;
            }
            if (BiometricManager.from(this).canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG)
                    != BiometricManager.BIOMETRIC_SUCCESS) {
                // 未登録・非対応ハードウェア・OS 側要件未達等（可否判定の失敗を
                // 「認証した」にしない）。
                biometricRequest.take();
                nativeOnBiometricResult(requestId, BIOMETRIC_RESULT_UNAVAILABLE);
                return;
            }
            try {
                BiometricPrompt.PromptInfo promptInfo = new BiometricPrompt.PromptInfo.Builder()
                        .setTitle(reason)
                        .setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
                        // `DEVICE_CREDENTIAL` を許可しないため negative button は必須
                        // （OS 標準文字列を使い新規リソースは足さない）。
                        .setNegativeButtonText(getString(android.R.string.cancel))
                        .build();
                biometricPrompt.authenticate(promptInfo);
            } catch (RuntimeException e) {
                // 提示依頼自体が例外を送出した場合（`requestShare` の `launch()` 例外と
                // 同型）。コールバックは二度と来ないため、ここで直接内部エラーを通知する。
                biometricRequest.take();
                nativeOnBiometricResult(requestId, BIOMETRIC_RESULT_INTERNAL_ERROR);
            }
        });
    }

    // Rust 側 bridge モジュール（`device/mic_record.rs`）の `present`（Android 分岐）から
    // JNI 経由で呼ばれる（Issue #957）。`requestBiometric` と同じ「即時 return・結果は
    // 非同期通知」の形。`maxDurationMs` は Rust 側 `MAX_RECORD_MS`（30 秒固定）を
    // そのまま `MediaRecorder.setMaxDuration` へ渡す（期限を複製しない、
    // device-bridge-contracts.md「期限・キャンセル」節）。
    private void requestMicRecordStart(long requestId, int maxDurationMs) {
        runOnUiThread(() -> {
            synchronized (micLock) {
                if (micRecorder != null || micRecordRequest.current() != RequestTracker.NONE) {
                    // 既に録音中、または権限確認待ち中の二重開始（現在の要求は壊さない）。
                    nativeOnMicRecordResult(requestId, MIC_RESULT_BUSY, null, null, 0);
                    return;
                }
                micRecordRequest.track(requestId);
                micRequestedMaxDurationMs = maxDurationMs;
                if (isFinishing() || isDestroyed() || !activityResumed) {
                    // biometric/permission コールバックと同じ非前景ガード
                    // （`requestMicPermission` 参照）。ここで弾いておけば、
                    // 権限確認へ進んだ場合もダイアログ表示時点で前景が保証される。
                    micRecordRequest.take();
                    nativeOnMicRecordResult(requestId, MIC_RESULT_INTERRUPTED, null, null, 0);
                    return;
                }
                if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
                        == PackageManager.PERMISSION_GRANTED) {
                    startMicRecorder(requestId, maxDurationMs);
                    return;
                }
                try {
                    requestMicPermission.launch(Manifest.permission.RECORD_AUDIO);
                } catch (RuntimeException e) {
                    // `launch()` 自体が例外を送出した場合（`requestCameraCapture` と同じ
                    // 理由）。コールバックは二度と来ないため、ここで直接失敗を通知する。
                    micRecordRequest.take();
                    nativeOnMicRecordResult(requestId, MIC_RESULT_INTERNAL_ERROR, null, null, 0);
                }
            }
        });
    }

    // 権限確認後（`requestMicRecordStart`/`requestMicPermission` 許可コールバックの
    // いずれか）に呼ぶ。呼び出し元が `micLock` の内側であることを保証する。
    // `new MediaRecorder()`（無引数）は API 31 で `MediaRecorder(Context)` に非推奨化
    // されたが、`MediaRecorder(Context)` は API 31 未満（minSdk 24）の実機に存在しない
    // ため使えない（新規追加 API を分岐なしで呼ぶと旧 OS で `NoSuchMethodError`）。
    @SuppressWarnings("deprecation")
    private void startMicRecorder(long requestId, int maxDurationMs) {
        File tempFile;
        try {
            tempFile = File.createTempFile("fandhe_mic_", ".m4a", getCacheDir());
        } catch (IOException e) {
            micRecordRequest.take();
            nativeOnMicRecordResult(requestId, MIC_RESULT_INTERNAL_ERROR, null, null, 0);
            return;
        }
        MediaRecorder recorder = new MediaRecorder();
        try {
            recorder.setAudioSource(MediaRecorder.AudioSource.MIC);
            recorder.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4);
            recorder.setAudioEncoder(MediaRecorder.AudioEncoder.AAC);
            recorder.setAudioChannels(1);
            recorder.setAudioEncodingBitRate(64_000);
            recorder.setAudioSamplingRate(44_100);
            recorder.setOutputFile(tempFile.getAbsolutePath());
            recorder.setMaxDuration(maxDurationMs);
            // リスナーは `MediaRecorder` を生成したスレッド（本メソッドは常に
            // `runOnUiThread` 経由で呼ばれる、`requestMicRecordStart`/
            // `requestMicPermission` 参照）の Looper から届く契約。`finishMicRecord` は
            // `micLock` の内側であることを要求するため、ここでも明示的に取る
            // （UI スレッド前提に依存しない防御、device-bridge-contracts.md
            // 「並行性・世代」節）。
            recorder.setOnInfoListener((mr, what, extra) -> {
                if (what == MediaRecorder.MEDIA_RECORDER_INFO_MAX_DURATION_REACHED) {
                    synchronized (micLock) {
                        // `MEDIA_RECORDER_INFO_MAX_DURATION_REACHED` の時点で
                        // `MediaRecorder` は既に内部で停止済み（Cursor Bugbot 指摘）。
                        // ここで再度 `stop()` を呼ぶと `IllegalStateException` になり、
                        // 完成した録音を内部エラー扱いで捨ててしまう。
                        finishMicRecord(requestId, MIC_RESULT_SUCCESS, true);
                    }
                }
            });
            recorder.setOnErrorListener((mr, what, extra) -> {
                synchronized (micLock) {
                    finishMicRecord(requestId, MIC_RESULT_INTERNAL_ERROR);
                }
            });
            recorder.prepare();
            recorder.start();
        } catch (IOException | RuntimeException e) {
            recorder.release();
            deleteQuietly(tempFile);
            micRecordRequest.take();
            nativeOnMicRecordResult(requestId, MIC_RESULT_INTERNAL_ERROR, null, null, 0);
            return;
        }
        micRecorder = recorder;
        micTempFile = tempFile;
        micStartedAt = SystemClock.elapsedRealtime();
        nativeOnMicRecordStarted(requestId);
    }

    // Rust 側 `device/android.rs::request_calendar_insert` から JNI 経由で呼ばれる
    // （Issue #1226）。カレンダーアプリの編集画面（`ACTION_INSERT`）を開くだけで権限は
    // 使わない。結果コードは `startActivity` が例外なく戻った時点で返し、編集画面からの
    // 戻りは待たない（結果コードは保存の有無を表さないため）。件名・日時はログへ出さない。
    private void requestCalendarInsert(long requestId, String title,
            int sy, int smo, int sd, int sh, int smi,
            int ey, int emo, int ed, int eh, int emi) {
        calendarRequest.track(requestId);
        runOnUiThread(() -> {
            if (!calendarRequest.complete(requestId)) {
                return; // 新しい要求に差し替えられた旧要求
            }
            if (isFinishing() || isDestroyed() || !activityResumed) {
                // 背景からの起動は OS に黙って抑止されうる（嘘の成功の防止）。
                nativeOnCalendarResult(requestId, CALENDAR_RESULT_BACKGROUND);
                return;
            }
            // th_TH 等で仏暦になる Calendar.getInstance() を避け、西暦固定にする。
            // 1582 年より前もユリウス暦へ倒さず、Rust 側の検証（先発グレゴリオ暦）と暦を揃える。
            Calendar begin = new GregorianCalendar();
            ((GregorianCalendar) begin).setGregorianChange(new java.util.Date(Long.MIN_VALUE));
            begin.clear();
            begin.set(sy, smo - 1, sd, sh, smi);
            Calendar end = new GregorianCalendar();
            ((GregorianCalendar) end).setGregorianChange(new java.util.Date(Long.MIN_VALUE));
            end.clear();
            end.set(ey, emo - 1, ed, eh, emi);
            Intent intent = new Intent(Intent.ACTION_INSERT)
                    .setData(CalendarContract.Events.CONTENT_URI)
                    .putExtra(CalendarContract.Events.TITLE, title)
                    .putExtra(CalendarContract.EXTRA_EVENT_BEGIN_TIME, begin.getTimeInMillis())
                    .putExtra(CalendarContract.EXTRA_EVENT_END_TIME, end.getTimeInMillis());
            try {
                startActivity(intent);
                nativeOnCalendarResult(requestId, CALENDAR_RESULT_HANDED);
            } catch (ActivityNotFoundException e) {
                nativeOnCalendarResult(requestId, CALENDAR_RESULT_NO_APP);
            } catch (RuntimeException e) {
                nativeOnCalendarResult(requestId, CALENDAR_RESULT_LAUNCH_FAILED);
            }
        });
    }

    // 写真ライブラリ保存（Issue #1256）。Rust 側 `device/android_photo_library.rs` から JNI で
    // 呼ばれる。実処理は `PhotoSaveHelper`（Activity を握らない。再生成時の破棄は Rust 側 reset と対）。
    private void requestPhotoSave(long requestId, byte[] png) {
        PhotoSaveHelper.save(getApplicationContext().getContentResolver(), requestId, png);
    }

    // 壁紙・ロック画面の設定（Issue #971）。Rust 側 `device/android_wallpaper.rs` から JNI で
    // 呼ばれる。実処理は `WallpaperHelper`（Activity を握らない。再生成時の破棄は Rust 側 reset と対）。
    private void requestWallpaperSet(long requestId, byte[] png, int target) {
        WallpaperHelper.set(getApplicationContext(), requestId, png, target);
    }

    // Rust 側 bridge モジュール（`device/notification.rs`）の `present`（Android 分岐）から
    // JNI 経由で呼ばれる（Issue #1006）。`requestBiometric` と同じ「即時 return・結果は
    // 非同期通知」の形。`title`/`body` は Rust 側 `NotificationRequest::new`（空・NUL・
    // 長さ検証）を通した文字列の契約。表示にのみ使い、ログへ出さない
    // （security.md A03/A09）。
    private void requestNotification(long requestId, String title, String body) {
        runOnUiThread(() -> {
            if (Build.VERSION.SDK_INT < 33
                    || ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS)
                            == PackageManager.PERMISSION_GRANTED) {
                // API 33 未満は実行時許可自体が存在しないため要求しない
                // （device-bridge-contracts.md「対応表・権限」節）。
                postNotification(requestId, title, body);
                return;
            }
            if (isFinishing() || isDestroyed() || !activityResumed) {
                // 非前景でダイアログを出さない（`requestBiometric`/`requestMicRecordStart`
                // と同じ非前景ガード）。
                nativeOnNotificationResult(requestId, NOTIFICATION_RESULT_AUTH_ERROR);
                return;
            }
            notificationRequest.track(requestId);
            pendingNotificationTitle = title;
            pendingNotificationBody = body;
            try {
                requestNotificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS);
            } catch (RuntimeException e) {
                // `launch()` 自体が例外を送出した場合（`requestMicRecordStart` と同じ
                // 理由）。コールバックは二度と来ないため、ここで直接失敗を通知する。
                notificationRequest.take();
                pendingNotificationTitle = null;
                pendingNotificationBody = null;
                nativeOnNotificationResult(requestId, NOTIFICATION_RESULT_AUTH_ERROR);
            }
        });
    }

    // 権限確認後（`requestNotification`/`requestNotificationPermission` 許可コールバック
    // のいずれか）に呼ぶ。常に UI スレッドから呼ばれる契約（`requestNotification` の
    // `runOnUiThread`、`requestNotificationPermission` の `ActivityResultLauncher`
    // コールバックはいずれも UI スレッド）。
    private void postNotification(long requestId, String title, String body) {
        NotificationManagerCompat manager = NotificationManagerCompat.from(this);
        NotificationChannelCompat channel = new NotificationChannelCompat.Builder(
                        NOTIFICATION_CHANNEL_ID, NotificationManagerCompat.IMPORTANCE_DEFAULT)
                .setName(getString(R.string.notification_channel_name))
                .build();
        try {
            // `areNotificationsEnabled`/`createNotificationChannel`/`getNotificationChannelCompat`
            // は通知サービスとの IPC を伴い、サービス側の異常で `RuntimeException` を送出し得る
            // （Codex 指摘、PR #1010）。ここを try の外に置くと UI スレッドの未捕捉例外で
            // アプリが終了し、`nativeOnNotificationResult` へ失敗を返せなくなる。
            if (!manager.areNotificationsEnabled()) {
                // API 33 未満で利用者が設定から通知を切った場合を含む（嘘の成功を出さない、
                // device-bridge-contracts.md「嘘の成功」節）。
                nativeOnNotificationResult(requestId, NOTIFICATION_RESULT_PERMISSION_DENIED);
                return;
            }
            manager.createNotificationChannel(channel);
            NotificationChannelCompat created = manager.getNotificationChannelCompat(NOTIFICATION_CHANNEL_ID);
            if (created != null && created.getImportance() == NotificationManagerCompat.IMPORTANCE_NONE) {
                // チャンネル単位でブロックされている（設定アプリからチャンネルを個別に
                // オフにした場合）。
                nativeOnNotificationResult(requestId, NOTIFICATION_RESULT_PERMISSION_DENIED);
                return;
            }
        } catch (RuntimeException e) {
            nativeOnNotificationResult(requestId, NOTIFICATION_RESULT_POST_ERROR);
            return;
        }
        NotificationCompat.Builder builder = new NotificationCompat.Builder(this, NOTIFICATION_CHANNEL_ID)
                .setSmallIcon(android.R.drawable.sym_def_app_icon)
                .setContentTitle(title)
                .setContentText(body)
                .setAutoCancel(true);
        try {
            manager.notify((int) requestId, builder.build());
            nativeOnNotificationResult(requestId, NOTIFICATION_RESULT_SUBMITTED);
        } catch (SecurityException e) {
            nativeOnNotificationResult(requestId, NOTIFICATION_RESULT_PERMISSION_DENIED);
        } catch (RuntimeException e) {
            nativeOnNotificationResult(requestId, NOTIFICATION_RESULT_POST_ERROR);
        }
    }

    // Rust 側（`device/mic_record.rs` の「停止」ボタンハンドラ）から JNI 経由で呼ばれる
    // （Issue #957）。`requestId` が現在の要求と一致しない場合は no-op（旧要求の遅延
    // 到着、`apple_mic_record.rs::request_stop` の Android 版）。
    private void requestMicRecordStop(long requestId) {
        runOnUiThread(() -> {
            synchronized (micLock) {
                if (micRecordRequest.current() != requestId) {
                    return;
                }
                if (micRecorder == null) {
                    // 権限確認中（ダイアログ表示中）の「停止」。正常なキャンセル。
                    micRecordRequest.take();
                    nativeOnMicRecordResult(requestId, MIC_RESULT_CANCELLED, null, null, 0);
                    return;
                }
                finishMicRecord(requestId, MIC_RESULT_SUCCESS);
            }
        });
    }

    // 録音セッションを 1 つ決着させる（自動停止・明示停止・`onPause`/`onDestroy` の
    // いずれの経路からも呼ぶ、`micLock` の内側であることを呼び出し元が保証する）。
    // `code` は「録音が正常に走っていた場合」の決着コード（`MIC_RESULT_SUCCESS`/
    // `MIC_RESULT_INTERRUPTED`）。`stop()` が失敗すれば `MIC_RESULT_INTERNAL_ERROR` へ
    // 昇格する。
    private void finishMicRecord(long requestId, int code) {
        finishMicRecord(requestId, code, false);
    }

    // `recorderAlreadyStopped` は `MEDIA_RECORDER_INFO_MAX_DURATION_REACHED` 経路専用
    // （その時点で `MediaRecorder` は内部で停止済みのため `stop()` を呼ばない。
    // 呼ぶと `IllegalStateException` になり成功結果を内部エラーへ落としてしまう）。
    private void finishMicRecord(long requestId, int code, boolean recorderAlreadyStopped) {
        if (!micRecordRequest.complete(requestId)) {
            // 旧世代・二重完了（`take()` 済みの要求への遅延呼び出し）。
            return;
        }
        MediaRecorder recorder = micRecorder;
        File tempFile = micTempFile;
        long startedAt = micStartedAt;
        micRecorder = null;
        micTempFile = null;
        if (!recorderAlreadyStopped) {
            try {
                recorder.stop();
            } catch (RuntimeException e) {
                // 自動停止直後の `stop()` 例外等。0 バイト判定に委ねるため成功コードを
                // 維持しない。
                code = MIC_RESULT_INTERNAL_ERROR;
            }
        }
        recorder.release();

        byte[] bytes = null;
        String mime = null;
        long durationMs = 0;
        if (code == MIC_RESULT_SUCCESS) {
            try {
                bytes = readFileFully(tempFile);
                if (bytes.length == 0) {
                    code = MIC_RESULT_INTERNAL_ERROR;
                    bytes = null;
                } else {
                    mime = "audio/mp4";
                    durationMs = SystemClock.elapsedRealtime() - startedAt;
                }
            } catch (IOException e) {
                code = MIC_RESULT_INTERNAL_ERROR;
                bytes = null;
            }
        }
        deleteQuietly(tempFile);
        nativeOnMicRecordResult(requestId, code, bytes, mime, durationMs);
    }

    // `tempFile` を全読み込みする（`finishMicRecord` 専用）。`java.nio.file.Files` は
    // API 26 以降専用で minSdk 24/25 端末では `NoClassDefFoundError` になるため
    // （codex-review 指摘）、`FileInputStream` で読む。
    private static byte[] readFileFully(File file) throws IOException {
        long length = file.length();
        if (length < 0 || length > Integer.MAX_VALUE) {
            throw new IOException("読み込み対象のサイズが不正: " + length);
        }
        byte[] data = new byte[(int) length];
        try (FileInputStream in = new FileInputStream(file)) {
            int offset = 0;
            while (offset < data.length) {
                int read = in.read(data, offset, data.length - offset);
                if (read < 0) {
                    break;
                }
                offset += read;
            }
            if (offset != data.length) {
                throw new IOException("ファイルの読み込みが途中で終了: expected="
                        + data.length + " actual=" + offset);
            }
        }
        return data;
    }

    // `micTempFile` の後始末（`finishMicRecord`/`startMicRecorder` の失敗経路の両方から
    // 呼ぶ）。削除失敗はアプリ専有キャッシュ領域のため致命的ではなく、握りつぶす。
    private static void deleteQuietly(File file) {
        if (file != null) {
            file.delete();
        }
    }

    // Rust 側 bridge モジュール（`device/speech_recognition.rs`）の `present`
    // （Android 分岐）から JNI 経由で呼ばれる（Issue #1026）。`requestMicRecordStart`
    // と同じ「即時 return・結果は非同期通知」の形。`maxListenMs` は Rust 側
    // `MAX_LISTEN_MS`（30 秒固定）をそのまま `Handler.postDelayed` へ渡す（期限を
    // 複製しない、device-bridge-contracts.md「期限・キャンセル」節）。
    private void requestSpeechRecognitionStart(long requestId, int maxListenMs) {
        runOnUiThread(() -> {
            synchronized (speechLock) {
                if (speechRecognizer != null || speechRequest.current() != RequestTracker.NONE
                        || speechPermissionPending) {
                    // 既に聞き取り中、権限確認待ち中、または先行する権限ダイアログの
                    // 結果がまだ届いていない（`speechPermissionPending` doc 参照。
                    // 「停止」で `speechRequest` 側だけ先に解除されていても、ダイアログ
                    // 自体はまだ閉じていないため新規に起動しない）の二重開始（現在の
                    // 要求は壊さない）。
                    nativeOnSpeechResult(requestId, SPEECH_RESULT_BUSY, null);
                    return;
                }
                speechRequest.track(requestId);
                speechRequestedMaxListenMs = maxListenMs;
                if (isFinishing() || isDestroyed() || !activityResumed) {
                    // `requestMicRecordStart` と同じ非前景ガード。
                    speechRequest.take();
                    nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERRUPTED, null);
                    return;
                }
                // 非対応端末では権限ダイアログを出す前に弾く（実装計画 §2.4 手順 3。
                // 権限確認より先に判定する）。
                if (!SpeechRecognizer.isRecognitionAvailable(this)) {
                    speechRequest.take();
                    nativeOnSpeechResult(requestId, SPEECH_RESULT_UNAVAILABLE, null);
                    return;
                }
                if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
                        == PackageManager.PERMISSION_GRANTED) {
                    startSpeechRecognizer(requestId, maxListenMs);
                    return;
                }
                try {
                    // launch 直前に id を退避する（上記 `speechPermissionRequestId`
                    // doc 参照。コールバックはこの値で現在の要求と照合する）。
                    // `speechPermissionPending` も同時に立て、この結果が届くまで
                    // 次の要求が新しいダイアログを起動しないようにする。
                    speechPermissionRequestId = requestId;
                    speechPermissionPending = true;
                    requestSpeechPermission.launch(Manifest.permission.RECORD_AUDIO);
                } catch (RuntimeException e) {
                    // `launch()` 自体が例外を送出した場合（`requestMicRecordStart` と
                    // 同じ理由）。コールバックは二度と来ないため、ここで直接失敗を
                    // 通知し、保留フラグも解除する。
                    speechPermissionRequestId = RequestTracker.NONE;
                    speechPermissionPending = false;
                    speechRequest.take();
                    nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERNAL_ERROR, null);
                }
            }
        });
    }

    // 権限確認後（`requestSpeechRecognitionStart`/`requestSpeechPermission` 許可
    // コールバックのいずれか）に呼ぶ。呼び出し元が `speechLock` の内側であることを
    // 保証する。`SpeechRecognizer`/`RecognitionListener` のコールバックはいずれも
    // `createSpeechRecognizer` を呼んだスレッド（本メソッドは常に `runOnUiThread`
    // 経由で呼ばれる）の Looper から届く契約のため、UI スレッドに閉じて動作する
    // （`speechTimeoutHandler` も `Looper.getMainLooper()` で同じスレッド、
    // device-bridge-contracts.md「並行性・世代」節）。
    private void startSpeechRecognizer(long requestId, int maxListenMs) {
        SpeechRecognizer recognizer;
        try {
            recognizer = SpeechRecognizer.createSpeechRecognizer(this);
        } catch (RuntimeException e) {
            speechRequest.take();
            nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERNAL_ERROR, null);
            return;
        }
        recognizer.setRecognitionListener(new RecognitionListener() {
            @Override
            public void onReadyForSpeech(Bundle params) {
                synchronized (speechLock) {
                    if (speechRequest.current() == requestId) {
                        // 準備待ちの保険タイマー（`SPEECH_READY_TIMEOUT_MS`）を解除して
                        // から本来の聞き取り上限タイマーへ切り替える。解除せず
                        // `speechTimeout` を上書きするだけだと、保険タイマーの
                        // Runnable が Handler のキューに残り続け、聞き取り中に不意に
                        // 発火してしまう。
                        cancelSpeechTimeout();
                        speechStarted = true;
                        // 期限は `onReadyForSpeech`（認識サービスの準備完了）起点で
                        // 数える。Rust 側 `nativeOnSpeechStarted` も同じ到達を起点に
                        // 経過時間を数える契約のため、`startListening()` 直後を起点に
                        // すると準備待ちの遅延ぶん OS 側の期限が先に来てしまう
                        // （PR #1030 レビュー指摘）。
                        speechTimeout = () -> {
                            synchronized (speechLock) {
                                if (speechRequest.current() == requestId
                                        && speechRecognizer != null
                                        && !speechStopRequested) {
                                    // 手動 `requestSpeechRecognitionStop` と停止要求の
                                    // 送出を一度きりにする番人（PR #1030 レビュー指摘。
                                    // `speechStopRequested` doc 参照）。
                                    speechStopRequested = true;
                                    if (!stopListeningOrFail(requestId)) {
                                        return;
                                    }
                                    // 自動停止後も手動停止と同じ保険期限を掛ける
                                    // （PR #1030 レビュー指摘: 自動停止だけ
                                    // `onResults`/`onError` 未到達の決着期限が無く、
                                    // 要求が永久に残っていた）。
                                    scheduleSpeechStopGrace(requestId);
                                }
                            }
                        };
                        speechTimeoutHandler.postDelayed(speechTimeout, maxListenMs);
                        nativeOnSpeechStarted(requestId);
                    }
                }
            }

            @Override
            public void onResults(Bundle results) {
                synchronized (speechLock) {
                    ArrayList<String> matches =
                            results.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION);
                    String text = (matches == null || matches.isEmpty()) ? "" : matches.get(0);
                    if (finishSpeech(requestId)) {
                        nativeOnSpeechResult(requestId, SPEECH_RESULT_SUCCESS, text);
                    }
                }
            }

            @Override
            public void onError(int error) {
                synchronized (speechLock) {
                    if (finishSpeech(requestId)) {
                        nativeOnSpeechError(requestId, error);
                    }
                }
            }

            @Override
            public void onBeginningOfSpeech() {}

            @Override
            public void onRmsChanged(float rmsdB) {}

            @Override
            public void onBufferReceived(byte[] buffer) {}

            @Override
            public void onEndOfSpeech() {}

            @Override
            public void onPartialResults(Bundle partialResults) {}

            @Override
            public void onEvent(int eventType, Bundle params) {}
        });

        Intent intent = new Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH);
        intent.putExtra(
                RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM);
        intent.putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1);
        intent.putExtra(RecognizerIntent.EXTRA_CALLING_PACKAGE, getPackageName());
        try {
            recognizer.startListening(intent);
        } catch (RuntimeException e) {
            try {
                recognizer.destroy();
            } catch (RuntimeException ignored) {
                // 要求の解除と結果配送は続ける（`abortSpeech` と同じ方針）
            }
            speechRequest.take();
            nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERNAL_ERROR, null);
            return;
        }
        speechRecognizer = recognizer;
        speechStartPendingResume = false;
        // `onReadyForSpeech` 未到達を明示する初期値（聞き取り上限タイマーは同
        // コールバックで開始する。上記コメント参照）。
        speechStarted = false;
        // 新しい要求ごとに停止要求の送出有無をリセットする（`speechStopRequested`
        // doc 参照）。
        speechStopRequested = false;
        // 準備待ちの保険タイマー（`SPEECH_READY_TIMEOUT_MS` doc 参照）。
        // `onReadyForSpeech`/`onError` のどちらも届かないまま画面が Listening
        // 固着するのを防ぐ（PR #1030 レビュー指摘）。`onReadyForSpeech` 到達時は
        // 上の分岐で `cancelSpeechTimeout()` により解除される。
        speechTimeout = () -> {
            synchronized (speechLock) {
                if (speechRequest.current() == requestId && !speechStarted) {
                    if (finishSpeech(requestId)) {
                        nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERNAL_ERROR, null);
                    }
                }
            }
        };
        speechTimeoutHandler.postDelayed(speechTimeout, SPEECH_READY_TIMEOUT_MS);
    }

    // Rust 側（`device/speech_recognition.rs` の「停止」ボタンハンドラ）から JNI 経由で
    // 呼ばれる（Issue #1026）。「停止」＝「話し終わった」の合図として OS に結果の確定を
    // 求めるだけで、確定結果は `onResults`/`onError` が別途届ける
    // （`fandhe_device::speech_recognition` モジュール doc「停止ボタンの意味」節）。
    // `requestId` が現在の要求と一致しない場合は no-op（旧要求の遅延到着、
    // `requestMicRecordStop` の Android 版）。
    private void requestSpeechRecognitionStop(long requestId) {
        runOnUiThread(() -> {
            synchronized (speechLock) {
                if (speechRequest.current() != requestId) {
                    return;
                }
                if (speechRecognizer == null) {
                    // 権限確認中（ダイアログ表示中）の「停止」。正常なキャンセル。
                    speechRequest.take();
                    nativeOnSpeechResult(requestId, SPEECH_RESULT_CANCELLED, null);
                    return;
                }
                if (!speechStarted) {
                    // `startListening()` 直後〜`onReadyForSpeech` 到達前の「停止」。
                    // まだ聞き取りを始めていないためキャンセル扱い（契約は
                    // `fandhe_device::speech_recognition` の「聞き始めていない段階の
                    // 停止は Cancelled」）。`stopListening()` を呼ぶと後続の
                    // `onError` が Cancelled ではなくエラーへ写ってしまうため、
                    // `onPause`/`onDestroy` と同じ `cancel()`→`destroy()` で終える
                    // （PR #1030 レビュー指摘）。
                    abortSpeech(SPEECH_RESULT_CANCELLED);
                    return;
                }
                // 手動停止は結果確定を OS に求めるだけで、確定は `onResults`/`onError`
                // が届けて `finishSpeech` がタイマーを解除する。自動停止タイマーが
                // 既に発火して停止要求を送出済みの場合（`speechStopRequested`）は
                // 何もしない。ここで `stopListening()` を再送すると同じ認識器への
                // 二重呼び出しになり、かつ既に仕掛けた決着用の保険タイマーを
                // 無条件に解除・再設定してしまう（PR #1030 レビュー指摘）。
                if (speechStopRequested) {
                    return;
                }
                // 結果到着前に期限タイマーが発火すると同じ認識器へ
                // `stopListening()` を二重に呼んでしまう（PR #1030 レビュー指摘）
                // ため、手動停止の時点でタイマーを解除し、停止要求を一度だけ送る。
                speechStopRequested = true;
                cancelSpeechTimeout();
                if (!stopListeningOrFail(requestId)) {
                    return;
                }
                scheduleSpeechStopGrace(requestId);
            }
        });
    }

    // 停止要求（手動「停止」ボタン・自動停止タイマーの両方）の送出直後に呼ぶ、保険の
    // 期限（`SPEECH_STOP_RESULT_GRACE_MS`）を仕掛ける（`speechLock` の内側であることを
    // 呼び出し元が保証する）。認識サービスが `stopListening()` に対して
    // `onResults`/`onError` のどちらも返さない場合、要求が永久に完了せず画面が固着
    // する（PR #1030 レビュー指摘。Rust 側 `tick` は `Stopping` では上限判定を進めない
    // ため Android 側で決着させる必要がある。自動停止だけこの保険が無いと再発した
    // ため、手動停止と共用の 1 箇所にまとめる）。期限に達してもまだ現在の要求のままなら
    // 内部エラーとして畳んで破棄する。呼び出し元が既に `speechTimeout` を使い切っている
    // 前提（直前に `cancelSpeechTimeout()` 済み、または `stopListening()` 直前の
    // タイマー発火自身からの呼び出し）で上書きする。
    private void scheduleSpeechStopGrace(long requestId) {
        speechTimeout = () -> {
            synchronized (speechLock) {
                if (finishSpeech(requestId)) {
                    nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERNAL_ERROR, null);
                }
            }
        };
        speechTimeoutHandler.postDelayed(speechTimeout, SPEECH_STOP_RESULT_GRACE_MS);
    }

    // 聞き取りセッションを 1 つ決着させる（`onResults`/`onError`/`onPause`/`onDestroy`
    // のいずれの経路からも呼ぶ、`speechLock` の内側であることを呼び出し元が保証する）。
    // `speechRequest.complete(requestId)` が false（旧世代・二重完了）なら false を
    // 返し、呼び出し元は結果を届けない。結果を届ける前に `destroy`・タイマー解除・
    // フィールドの null 化を済ませる（device-bridge-contracts.md「並行性・世代」節
    // 「結果を届ける前に状態を確定させる」）。
    private boolean finishSpeech(long requestId) {
        if (!speechRequest.complete(requestId)) {
            return false;
        }
        cancelSpeechTimeout();
        try {
            speechRecognizer.destroy();
        } catch (RuntimeException e) {
            // 認識サービス側の障害で destroy が失敗しても、参照は捨てて結果は届ける
            // （ここで抜けると要求が永久に残る）。
        }
        speechRecognizer = null;
        speechStartPendingResume = false;
        return true;
    }

    // 聞き取り中の認識器を結果確定なしで破棄し、現在の要求を `resultCode` で完了させる
    // （「停止」の未開始分岐・`onPause`・`onDestroy` から、`speechLock` の内側で
    // `speechRecognizer != null` を確かめてから呼ぶ）。認識サービスの障害で `cancel()` /
    // `destroy()` が `RuntimeException` を送出しても、状態を解除して結果を必ず届ける
    // （ここで抜けると要求が永久に残り以後の開始が busy になる。PR #1030 レビュー指摘、
    // `finishSpeech` と同じ方針）。
    private void abortSpeech(int resultCode) {
        long id = speechRequest.take();
        cancelSpeechTimeout();
        try {
            speechRecognizer.cancel();
        } catch (RuntimeException e) {
            // destroy と結果配送は続ける
        }
        try {
            speechRecognizer.destroy();
        } catch (RuntimeException e) {
            // 参照は捨てて結果は届ける
        }
        speechRecognizer = null;
        speechStartPendingResume = false;
        nativeOnSpeechResult(id, resultCode, null);
    }

    // `stopListening()` を呼ぶ（手動停止・自動停止の両方から、`speechLock` の内側で呼ぶ）。
    // 認識サービスの障害で `RuntimeException` が出ると UI スレッドの処理が未捕捉例外で
    // 終了するため、捕捉して認識器とタイマーを破棄し、要求を内部エラーとして完了させる
    // （PR #1030 codex 指摘）。false を返したら呼び出し元は保険タイマーを掛けずに抜ける。
    private boolean stopListeningOrFail(long requestId) {
        try {
            speechRecognizer.stopListening();
            return true;
        } catch (RuntimeException e) {
            if (finishSpeech(requestId)) {
                nativeOnSpeechResult(requestId, SPEECH_RESULT_INTERNAL_ERROR, null);
            }
            return false;
        }
    }

    // `speechTimeout`（期限タイマー／手動停止後の保険タイマーの共用フィールド）を
    // 解除する（`speechLock` の内側であることを呼び出し元が保証する）。`speechTimeout`
    // は `onReadyForSpeech` 到達後にのみ設定されるため、到達前の停止・エラー経路
    // （`requestSpeechRecognitionStop` の未開始分岐・`onError` 経由の `finishSpeech`・
    // `onPause`/`onDestroy`）では null のままになる。`Handler.removeCallbacks(null)` を
    // 無条件に呼ばない（PR #1030 レビュー指摘）。
    private void cancelSpeechTimeout() {
        if (speechTimeout != null) {
            speechTimeoutHandler.removeCallbacks(speechTimeout);
            speechTimeout = null;
        }
    }

    // Rust 側（`device/location.rs::shared_location`）から JNI 経由で呼ばれる
    // （Issue #890）。「使用中のみ」の権限のため、開始要求は Rust 起点にする
    // （位置情報画面が動いているときにだけ測位・権限ダイアログを出す。傾き（#831）の
    // ように常時センサ登録はしない、実装計画 §2.3 参照）。結果を待たず即座に戻る
    // （`requestFilePick` と同じ形。実測位は `nativeOnLocationChanged`/
    // `nativeOnLocationFailure` で非同期に届く）。
    //
    // `precise` はジオフェンスからの要求で真（FINE が要る）、位置情報画面からは偽（COARSE のみ）。
    // JNI 署名は `(Z)V`（Rust 側 `device/android.rs::request_location_updates` と一致させる）。
    private void requestLocationUpdates(boolean precise) {
        // `locationWanted`・`locationStartRequestPending` より先に、同じ JNI スレッドで設定する
        // （UI スレッドへ遅れて立てると、先に走った `onResume` が COARSE だけで要求してしまう）。
        // 現在の開始要求が求める精度で毎回上書きする（ジオフェンスの後に位置情報画面から
        // 要求した場合は COARSE だけを求める、最小権限）。
        locationPreciseWanted = precise;
        locationWanted = true;
        // paused 中に Runnable が早期 return しても要求を握り潰さないための印
        // （`onResume` 側の分岐参照、codex-review P1 指摘 / cursor Bugbot 指摘、PR #894）。
        locationStartRequestPending = true;
        runOnUiThread(() -> {
            // この Runnable の実行前に `onPause` が走っていれば、停止済み Activity へ
            // リスナーを再登録しない（codex-review P1 指摘、PR #894）。`locationWanted`・
            // `locationStartRequestPending` は立てたままのため、次の `onResume` が
            // 改めて拾って `ensureLocationPermissionAndStart()` を実行する。
            //
            // この Runnable の実行前に `onResume` が先に `locationStartRequestPending`
            // を消費済み（権限確認・ダイアログ起動まで進めた）場合は、ここでは何もしない
            // （codex-review P1 指摘 / cursor Bugbot 指摘、PR #894）。`activityResumed` だけ
            // 見て無条件に `ensureLocationPermissionAndStart()` を呼ぶと、ユーザーが
            // まだ権限ダイアログに応答していないのに `requestPermissions` を再 launch し、
            // AOSP 実装が返す空の取消結果で `PermissionDenied` が誤通知される。
            if (!activityResumed || !locationStartRequestPending) {
                return;
            }
            locationStartRequestPending = false;
            ensureLocationPermissionAndStart();
        });
    }

    // 権限確認 → 未許可ならダイアログ起動、を行う（`requestLocationUpdates` の
    // Runnable と `onResume` の未実行要求再開の両方から呼ばれる共通処理、PR #894）。
    // UI スレッド・resumed 状態を呼び出し元が保証すること。
    // 「おおよそ」だけ許可済みの利用者にも、プロセスにつき 1 回 FINE への昇格ダイアログを
    // 出す（ジオフェンスの半径 100 m に FINE が要る。旧版で COARSE のみ許可した利用者が
    // `hasLocationPermission()` で素通りし Inaccurate 拒否が続くのを防ぐ、codex-review P1）。
    // 拒否されても COARSE が許可済みなら結果コールバックが測位を始める（精度不足は
    // Rust 側が Inaccurate で示す）。FINE 未許可のまま `onResume` が再要求することは
    // ない（本関数は新規開始要求と保留要求の再開だけから呼ばれる）。
    private void ensureLocationPermissionAndStart() {
        // FINE が要るのはジオフェンスの要求があったときだけ（位置情報画面は COARSE のみ要求）。
        // COARSE のみ許可済みで昇格ダイアログを既に出したプロセスでは再要求せず COARSE で
        // 続ける（開始要求のたびにダイアログを出し続けない。利用者が「おおよそ」のままを
        // 選んだ場合の精度不足は Rust 側が Inaccurate で示す）。何も許可されていない場合は
        // 従来どおり常に要求する。
        if (hasFineLocationPermission()
                || (hasLocationPermission() && (!locationPreciseWanted || fineUpgradeAsked))) {
            startLocationUpdates();
        } else {
            boolean askFine = locationPreciseWanted;
            awaitingLocationPermission = true;
            try {
                requestLocationPermission.launch(askFine
                        ? new String[] {
                            Manifest.permission.ACCESS_FINE_LOCATION,
                            Manifest.permission.ACCESS_COARSE_LOCATION
                        }
                        : new String[] {Manifest.permission.ACCESS_COARSE_LOCATION});
                // FINE を含むダイアログを実際に出せたら（初回・昇格とも）「精密位置を聞いた」とする。
                // 利用者が「おおよそ」を選んだ後は、ジオフェンスの再試行でも位置情報画面でも
                // 再度 FINE を求めない（launch 失敗では立てず、聞く機会を失わない）。
                if (askFine) {
                    fineUpgradeAsked = true;
                }
            } catch (RuntimeException e) {
                // `launch()` 自体が例外を送出した場合（`onSaveInstanceState` 後の
                // 呼び出し等、`requestCameraCapture` と同じ理由）。
                awaitingLocationPermission = false;
                notifyLocationFailure(LOCATION_FAILURE_FAILED);
            }
        }
    }

    private boolean hasFineLocationPermission() {
        return ContextCompat.checkSelfPermission(this, Manifest.permission.ACCESS_FINE_LOCATION)
                == PackageManager.PERMISSION_GRANTED;
    }

    // FINE か COARSE のどちらかが許可済みか（「おおよそ」のみ許可でも true）。
    // `onResume` の再登録分岐と `ensureLocationPermissionAndStart` が共有する。
    private boolean hasLocationPermission() {
        return ContextCompat.checkSelfPermission(this, Manifest.permission.ACCESS_FINE_LOCATION)
                        == PackageManager.PERMISSION_GRANTED
                || ContextCompat.checkSelfPermission(this, Manifest.permission.ACCESS_COARSE_LOCATION)
                        == PackageManager.PERMISSION_GRANTED;
    }

    // 権限確認後（`requestLocationUpdates`/`requestLocationPermission` 許可コールバック/
    // `onResume` 再登録）に呼ばれる。UI スレッド前提（Issue #890）。
    private void startLocationUpdates() {
        if (locationManager == null) {
            notifyLocationFailure(LOCATION_FAILURE_FAILED);
            return;
        }
        String provider = currentLocationProvider();
        boolean serviceEnabled = LocationManagerCompat.isLocationEnabled(locationManager)
                && locationManager.isProviderEnabled(provider);
        if (serviceEnabled) {
            // 権限再取得・`onResume` 再登録のたびに呼ばれる本関数の入口で、直前の失敗状態
            // （PermissionDenied 等）を「未取得」へ戻す（cursor Bugbot 指摘、PR #894）。
            // ただし「直前に失敗を通知済み」のときだけ行う（`locationFailurePosted` doc
            // 参照）。平常な `onResume` 再登録（直前が `Fixed`、失敗を挟んでいない）では
            // 何もしないため、取得済みの Fix を理由なく破棄しない（codex-review P1 指摘
            // / cursor Bugbot 指摘、PR #894）。
            notifyLocationRecoveredIfNeeded();
        } else {
            // サービスが無効なままの `onResume` では回復通知を出さない。先に
            // `notifyLocationRecoveredIfNeeded()` を呼んでから直後に
            // `notifyLocationFailure(SERVICE_DISABLED)` を呼ぶと、サービス無効のまま
            // `onResume` するたびに Disabled → 取得中 → Disabled と表示が明滅する
            // （cursor Low 指摘、PR #894）。サービス無効を通知したうえで登録自体は
            // 行う。有効化後に OS が自然に `onLocationChanged`/`onProviderEnabled` を
            // 届けるため回復できる（実装計画 §3 startLocationUpdates 参照）。
            notifyLocationFailure(LOCATION_FAILURE_SERVICE_DISABLED);
        }
        try {
            locationManager.requestLocationUpdates(
                    provider,
                    LOCATION_UPDATE_INTERVAL_MS,
                    0f,
                    locationListener,
                    Looper.getMainLooper());
        } catch (SecurityException | IllegalArgumentException e) {
            notifyLocationFailure(LOCATION_FAILURE_FAILED);
        }
    }

    // provider 選択（実装計画 §2.1）: API 31（S）以降は `FUSED_PROVIDER`、それ未満
    // （minSdk 24）は `NETWORK_PROVIDER`。GPS は使わない。FINE が許可されていれば
    // fused / network は精密な値を返し、COARSE のみなら間引かれた値になる。
    private static String currentLocationProvider() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            return LocationManager.FUSED_PROVIDER;
        }
        return LocationManager.NETWORK_PROVIDER;
    }

    // SAF の DISPLAY_NAME を解決する共通ヘルパー（`deliverPickedDocument`（Issue #874）と
    // ドラッグ & ドロップ（`deliverDropItem`、Issue #1016）の両方から呼ぶ。実装計画
    // §4「DISPLAY_NAME 解決を `resolveDisplayName(Uri)` に切り出して共有する」）。
    // プロバイダの `query` 自体が例外を送出しても `getLastPathSegment()` の fallback へ
    // 続ける（内容は不明なため名前解決の失敗として扱うのみで、まだ内部エラー確定は
    // しない）。ファイル名・Uri はログへ出力しない（security.md A09）。
    private String resolveDisplayName(Uri uri) {
        String name = null;
        try (Cursor cursor =
                getContentResolver().query(uri, new String[] {OpenableColumns.DISPLAY_NAME},
                        null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                int nameIndex = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (nameIndex >= 0) {
                    name = cursor.getString(nameIndex);
                }
            }
        } catch (RuntimeException e) {
            // 握りつぶして fallback へ続ける（上記コメント参照）。
        }
        if (name == null || name.isEmpty()) {
            name = uri.getLastPathSegment();
        }
        return name;
    }

    // `pickDocument` が選択を受け取った後、バックグラウンドスレッドで名前解決と fd 取得を
    // 行う（Issue #874、案 B、改訂 2「Android: fd を開き直さない」）。内容の読み取り・
    // 8 MiB 判定は Rust 側 `fandhe_device::file_picker::read_picked_from` に一本化するため、
    // ここでは `detachFd()` で所有権を渡した生 fd を渡すのみで読み取りループを持たない
    // （仕様書「共有 Rust 部分」参照）。`/proc/self/fd/N` の再 open は pipe 型プロバイダで
    // 権限再確認に失敗するため使わない（fd をそのまま借用させる）。
    //
    // 選択された Uri・ファイル名・例外メッセージはログへ出力しない（他アプリの機密文書に
    // なり得る、security.md A09）。
    private void deliverPickedDocument(long requestId, Uri uri) {
        String name = resolveDisplayName(uri);
        if (name == null || name.isEmpty()) {
            nativeOnFilePickerResult(requestId, 2, null, -1);
            return;
        }

        // `detachFd()` は fd の所有権を呼び出し元（ここでは Rust 側）へ移す。移した後は
        // この `ParcelFileDescriptor` の `close()`（try-with-resources の暗黙の呼び出しを
        // 含む）は no-op になる（Android の契約）ため、二重 close にはならない。
        // `detachFd()` 自体が例外を送出した場合（fd 未取得・既に detach 済み等）は
        // まだ所有権を渡していないため、try-with-resources が通常どおり `pfd` を閉じる。
        try (ParcelFileDescriptor pfd = getContentResolver().openFileDescriptor(uri, "r")) {
            if (pfd == null) {
                nativeOnFilePickerResult(requestId, 2, null, -1);
                return;
            }
            int fd = pfd.detachFd();
            nativeOnFilePickerResult(requestId, 0, name, fd);
        } catch (IOException | RuntimeException e) {
            nativeOnFilePickerResult(requestId, 2, null, -1);
        }
    }

    // Rust 側（`device/text_scale.rs` の Android 分岐、Issue #914）から JNI 経由で
    // 呼ばれる同期 getter（`requestBiometric` 等と異なり結果を待つ非同期通知は無い）。
    // `Configuration.fontScale` は「システム標準の何倍か」を表す倍率で、Rust 側の
    // `TextScale` 契約（`fandhe_device::text_scale` モジュール doc）とそのまま一致する。
    //
    // 文字サイズ変更時、`AndroidManifest.xml` の `configChanges` に `fontScale` を
    // 含めていないため Activity は再生成される（`onCreate` から再実行）。これにより
    // 次回呼び出し時の `getResources()` は新しい設定を反映済みで、明示的な
    // `onConfigurationChanged` の追加は不要（実装計画 §7、天井は
    // `docs/device/capabilities/dynamic-type.json` の `note` に記録）。
    private float textScaleFactor() {
        return getResources().getConfiguration().fontScale;
    }

    // Rust 側（`device/android_power_state.rs`、Issue #1276）から JNI 経由で egui スレッドが
    // 1 秒ごとに呼ぶ同期メソッド（シグネチャ `()J`）。詰め方は `PowerStateHelper` 冒頭を参照。
    private long powerStateRead() {
        return PowerStateHelper.read(getApplicationContext());
    }

    // Rust 側（`device/android_device_info.rs`、Issue #1306）から JNI 経由で egui スレッドが
    // ボタン押下ごとに呼ぶ同期メソッド（シグネチャ `()Ljava/lang/String;`）。詰め方は
    // `DeviceInfoHelper` 冒頭を参照。失敗は null。
    private String deviceInfoRead() {
        return DeviceInfoHelper.read(getApplicationContext());
    }

    // Rust 側（`device/android_device_identifier.rs`、Issue #904）から JNI 経由で egui スレッドが
    // ボタン押下ごとに呼ぶ同期メソッド（シグネチャ `()Ljava/lang/String;`）。失敗は null。
    private String deviceIdentifierRead() {
        return DeviceIdentifierHelper.read(getApplicationContext());
    }

    // Rust 側（`device/android_per_app_language.rs`、Issue #1075）から JNI 経由で egui スレッドが
    // ボタン押下ごとに呼ぶ同期メソッド（シグネチャ `()Ljava/lang/String;`）。符号化は
    // `PerAppLanguageHelper` 冒頭を参照。失敗は null。
    private String perAppLanguageRead() {
        return PerAppLanguageHelper.read(getApplicationContext());
    }

    // Rust 側（`device/android_alarm.rs`、Issue #1078）から JNI 経由でワーカースレッドが同期で呼ぶ
    // 3 本。シグネチャは `(J)I` / `()I` / `()J`。結果コードは `AlarmHelper` 冒頭を参照。
    private int alarmSchedule(long fireAtMs) {
        return AlarmHelper.schedule(this, fireAtMs);
    }

    private int alarmCancel() {
        return AlarmHelper.cancel(getApplicationContext());
    }

    private long alarmState() {
        return AlarmHelper.state(getApplicationContext());
    }

    // Rust 側（`device/android_audio_devices.rs`、Issue #1332）から JNI 経由で egui スレッドが
    // ボタン押下ごとに呼ぶ同期メソッド（シグネチャ `()Ljava/lang/String;`）。詰め方は
    // `AudioDevicesHelper` 冒頭を参照。失敗は null。
    private String audioDevicesList() {
        return AudioDevicesHelper.list(getApplicationContext());
    }

    // Rust 側（`device/android_managed_app_config.rs`、Issue #946）からワーカースレッドが JNI 経由で
    // 押下ごとに呼ぶ同期メソッド（シグネチャ `()Ljava/lang/String;`）。詰め方は
    // `ManagedAppConfigHelper` 冒頭を参照。失敗は null。ディスク I/O を伴うので UI スレッドから呼ばない。
    private String managedAppConfigRead() {
        return ManagedAppConfigHelper.read(getApplicationContext());
    }

    // Rust 側（`device/android_usb_device.rs`、Issue #507）から JNI 経由で egui スレッドが
    // ボタン押下ごとに呼ぶ同期メソッド（シグネチャ `()Ljava/lang/String;`）。詰め方は
    // `UsbDeviceHelper` 冒頭を参照。失敗は null、USB ホスト非搭載は空文字。
    private String usbDeviceList() {
        return UsbDeviceHelper.list(getApplicationContext());
    }

    // Rust 側（`device/android_local_fonts.rs`、Issue #1378）から JNI 経由で egui スレッドが
    // ボタン押下ごとに呼ぶ同期メソッド（シグネチャ `()[Ljava/lang/Object;`）。詰め方は
    // `LocalFontsHelper` 冒頭を参照。API 29 未満は長さ 0、失敗は null。
    private Object[] localFontsList() {
        return LocalFontsHelper.list();
    }

    // Rust 側（`device/android_display_refresh_hdr.rs`、Issue #872）から JNI 経由で egui
    // スレッドが押下ごとに呼ぶ同期メソッド（シグネチャ `()[F`）。詰め方は
    // `DisplayRefreshHdrHelper` 冒頭を参照。失敗は null。
    private float[] displayRefreshHdrRead() {
        return DisplayRefreshHdrHelper.read(this);
    }

    // Rust 側（`device/android_multi_display.rs`、Issue #1343）から JNI 経由で egui スレッドが
    // 1 秒ごとに呼ぶ同期メソッド（シグネチャ `()Ljava/lang/String;`）。詰め方は
    // `MultiDisplayHelper` 冒頭を参照。失敗は null。
    private String multiDisplayRead() {
        return MultiDisplayHelper.read(getApplicationContext());
    }

    // Rust 側（`device/android.rs` の `AndroidHaptics`、Issue #1092）から JNI 経由で
    // egui スレッドから呼ばれる同期メソッド（シグネチャ `()I`）。`Vibrator.vibrate` に
    // スレッド制約は無く、Java → Rust の native コールバックも持たないため no-op スタブは不要。
    // 戻り値（Rust 側 `decode_haptics_result` と対で決める契約）:
    //   0 = 要求を渡した（サイレント・システム触覚オフでは無反応で、検知できない）
    //   1 = 振動子なし（`Unsupported` へ写す）
    //   2 = 失敗（`SecurityException` 等。Java 例外を JNI 側へ残さず `Internal` へ写す）
    // 15ms は Web 実装（`navigator.vibrate(15)`）に揃えた軽い単発。
    @SuppressWarnings("deprecation")
    private int hapticsTap() {
        try {
            Vibrator vibrator;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                VibratorManager manager = getSystemService(VibratorManager.class);
                vibrator = manager == null ? null : manager.getDefaultVibrator();
            } else {
                vibrator = (Vibrator) getSystemService(Context.VIBRATOR_SERVICE);
            }
            if (vibrator == null || !vibrator.hasVibrator()) {
                return 1;
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                vibrator.vibrate(
                        VibrationEffect.createOneShot(15, VibrationEffect.DEFAULT_AMPLITUDE));
            } else {
                vibrator.vibrate(15);
            }
            return 0;
        } catch (RuntimeException e) {
            return 2;
        }
    }

    // 単語分割（Issue #1095）。egui スレッドから呼ばれる同期メソッド（シグネチャ
    // `(Ljava/lang/String;)Ljava/lang/String;`）。Rust 側 `device/android.rs::
    // AndroidWordSegmenter` から呼ばれ、Java → Rust の native コールバックを持たないため
    // no-op スタブは不要（`hapticsTap` と同じ判断）。`BreakIterator`（API 24 = minSdk）の
    // 語境界で切った部分文字列を出現順に U+0000 で連結して返す。入力は Rust 側
    // `validate_input` が NUL を拒否済みで、トークンは入力の部分文字列のため区切りは曖昧にならない。
    // 空白・記号だけのトークンの除去は Rust 側 `normalize_words` が行う。
    // トークン 0 件は空文字列、失敗（RuntimeException）は null（Java 例外を JNI 側へ残さず
    // Rust が `Internal` へ写す）。入力・トークンはログへ出さない。
    private String nlpSegment(String text) {
        try {
            BreakIterator it = BreakIterator.getWordInstance();
            it.setText(text);
            StringBuilder sb = new StringBuilder();
            boolean first = true;
            int start = it.first();
            for (int end = it.next(); end != BreakIterator.DONE; start = end, end = it.next()) {
                if (!first) {
                    sb.append('\0');
                }
                sb.append(text, start, end);
                first = false;
            }
            return sb.toString();
        } catch (RuntimeException e) {
            return null;
        }
    }

    // 資格情報保存（Issue #924）の op 定数。Rust 側 `device/android.rs::
    // AndroidSecureCredential::request` の同名定数と対で決める契約。
    private static final int SECURE_CREDENTIAL_OP_STORE = 0;
    private static final int SECURE_CREDENTIAL_OP_LOAD = 1;
    private static final int SECURE_CREDENTIAL_OP_DELETE = 2;

    // `EncryptedSharedPreferences` の保存ファイル名（固定）。
    private static final String SECURE_CREDENTIAL_FILE = "fandhe_secure_credential";

    // 保存値に付ける固定プレフィックス。`EncryptedSharedPreferences.getDecryptedObject`
    // （androidx.security:security-crypto の内部実装）は復号後の文字列が `"__NULL__"`
    // と一致すると null 番兵として扱い、`getString` の戻り値を強制的に null にする
    // （codex-review 指摘、PR #931）。値が偶然この文字列と一致すると LOAD が `Ok(None)`
    // になり Rust 側の「無い」と区別が付かなくなるため、任意の値が番兵と衝突しない
    // 可逆表現として全値にこのプレフィックスを付けて保存する。
    private static final String SECURE_CREDENTIAL_VALUE_PREFIX = "v1:";

    // 保存直前に呼ぶ。プレフィックスを付けるだけの純粋関数（テスト容易性のため分離）。
    private static String encodeSecureCredentialValue(String value) {
        return SECURE_CREDENTIAL_VALUE_PREFIX + value;
    }

    // 読み出し直後に呼ぶ。プレフィックスを検証して外す。プレフィックスが無い値は
    // 想定外（このプレフィックス方式導入前の値、または外部からの改変）のため
    // 「破損」として `null` ではなく例外で失敗させる。呼び出し元でそのまま
    // `Ok(None)` として扱われる（＝「無い」と誤認される）ことを防ぐ。
    private static String decodeSecureCredentialValue(String stored) {
        if (!stored.startsWith(SECURE_CREDENTIAL_VALUE_PREFIX)) {
            throw new IllegalStateException("secure credential unavailable");
        }
        return stored.substring(SECURE_CREDENTIAL_VALUE_PREFIX.length());
    }

    // 遅延生成した `EncryptedSharedPreferences` インスタンス。Activity 再生成
    // （設定変更等）を跨いで生き残るよう `static` に保持し、初回生成時の
    // Keystore 初期化コストを 1 回に抑える。`securePrefs()` の `synchronized`
    // が並行アクセス（複数の背景スレッドからの同時要求）に対する排他を担う。
    private static SharedPreferences securePrefsInstance;

    // `securePrefsInstance` を遅延生成して返す。呼び出しは Rust 側の背景スレッド
    // （`device/secure_credential.rs::drive` が `std::thread::spawn` した先、
    // UI スレッドではない）から到達するため、生成中のブロッキング（Keystore
    // 初期化）は画面を固めない。
    //
    // `res/xml/data_extraction_rules.xml` の除外設定が効かない経路（旧 OS・
    // 未対応の OEM 実装等）で万一 `fandhe_secure_credential.xml` だけが復元された
    // 場合、Keystore に対応する鍵が無いため `EncryptedSharedPreferences.create` は
    // `GeneralSecurityException`/`IOException` を投げる。この例外は
    // `secureCredentialRequest` の catch 節で `IllegalStateException` に変換され
    // Rust 側は `Err` として扱う契約のため、偽の成功（`Ok`）にはならない。
    private static synchronized SharedPreferences securePrefs(Context context)
            throws GeneralSecurityException, IOException {
        if (securePrefsInstance == null) {
            MasterKey masterKey =
                    new MasterKey.Builder(context)
                            .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
                            .build();
            securePrefsInstance =
                    EncryptedSharedPreferences.create(
                            context,
                            SECURE_CREDENTIAL_FILE,
                            masterKey,
                            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
                            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM);
        }
        return securePrefsInstance;
    }

    // Rust 側（`device/android.rs::AndroidSecureCredential::request`、Issue #924）から
    // JNI 経由で呼ばれる同期メソッド。`op` は上記 3 定数のいずれか。
    // 戻り値: STORE/DELETE は常に null。LOAD はヒット時の値、無い場合は null。
    // 内部エラー（Keystore/暗号化失敗等）は `IllegalStateException`（固定文言、
    // 値を含めない）を投げる。Rust 側はこの例外を `exception_clear` するだけで
    // 文言を読まない契約（security.md A09、秘密値・例外メッセージをログに残さない）。
    //
    // `EncryptedSharedPreferences`（androidx.security:security-crypto）は
    // 1.1.0-alpha07 で全 API が deprecated 済み（Android Keystore 直接利用が
    // 推奨）だが、動作はするため Issue #924 の明示要件どおり採用する
    // （Keystore 直接実装への置き換えはスコープ外、PR 本文参照）。
    @SuppressWarnings("deprecation")
    private String secureCredentialRequest(int op, String key, String value) {
        // pref キーは `"fandhe." + key`（Apple Keychain 実装の `kSecAttrAccount`
        // 接頭辞規約と同じ）。`key` は Rust 側 `validate_key` で検証済み
        // （英数字・`.`/`_`/`-` のみ）のためそのまま連結してよい。
        String prefKey = "fandhe." + key;
        try {
            SharedPreferences prefs = securePrefs(getApplicationContext());
            switch (op) {
                case SECURE_CREDENTIAL_OP_STORE:
                    // `commit()`（同期）を使う。`apply()` は非同期のため、直後の
                    // LOAD 要求で未反映になりうる（Rust 側は往復を保証しない契約）。
                    if (!prefs.edit()
                            .putString(prefKey, encodeSecureCredentialValue(value))
                            .commit()) {
                        throw new IllegalStateException("secure credential unavailable");
                    }
                    return null;
                case SECURE_CREDENTIAL_OP_LOAD:
                    String stored = prefs.getString(prefKey, null);
                    return stored == null ? null : decodeSecureCredentialValue(stored);
                case SECURE_CREDENTIAL_OP_DELETE:
                    // キーが無くても `remove().commit()` は成功する（冪等）。
                    if (!prefs.edit().remove(prefKey).commit()) {
                        throw new IllegalStateException("secure credential unavailable");
                    }
                    return null;
                default:
                    throw new IllegalStateException("secure credential unavailable");
            }
        } catch (GeneralSecurityException | IOException | RuntimeException e) {
            throw new IllegalStateException("secure credential unavailable");
        }
    }

    // バックグラウンド処理（Issue #1046）。`WorkManager` の一意名。予約の実体
    // （記録の書き込み）は `BackgroundTaskWorker` が `getFilesDir()` 直下へ直接行う
    // （Rust 側 JNI 呼び出しは `schedule`/`cancel`/`state` の橋渡しのみ、
    // `device/android_background_task.rs` モジュール doc「記録を書く主体」参照）。
    private static final String BACKGROUND_TASK_WORK_NAME = "fandhe.background-task";

    // `schedule`/`cancel` の結果（`Operation.getResult()`）を、確定するまで待つ。
    // 待ち上限を設けると、上限を過ぎた後に操作が確定して戻り値と実際の予約が
    // 食い違い、取り消しで巻き戻そうとすると置き換え前の予約まで失う（PR #1052
    // codex 指摘）。呼び出し元（Rust 側 `AndroidBackgroundTask`）は UI スレッド外の
    // 作業スレッドから呼ぶため、無期限に待っても画面は固まらない（システムトレイと
    // 同じ判断）。割込みでも中断せず
    // 確定まで待ち、割込み状態は最後に戻す。戻り値: 0 = 成功、1 = 失敗。
    static int awaitOperation(androidx.work.Operation operation) {
        boolean interrupted = false;
        try {
            while (true) {
                try {
                    operation.getResult().get();
                    return 0;
                } catch (InterruptedException e) {
                    interrupted = true;
                } catch (ExecutionException | RuntimeException e) {
                    return 1;
                }
            }
        } finally {
            if (interrupted) {
                Thread.currentThread().interrupt();
            }
        }
    }

    // Rust 側（`device/android_home_widget.rs::call_set`、Issue #1067）から JNI 経由で呼ばれる
    // 同期メソッド（ワーカースレッド上）。SharedPreferences へ `commit()` で保存し、成否を
    // 返す（`apply()` は非同期で直後の読み出しが古くなりうるため使わない）。契約上 `true` は
    // 「保存できた」ことだけを約束し、再描画要求はベストエフォート（`fandhe_device::home_widget`、
    // macOS 実装と同じ）。ウィジェット 0 個の空振りも更新要求の例外も `commit()` 成功なら `true`。
    // 保存値はログへ出さない。
    // JNI 署名: (Ljava/lang/String;)Z
    private boolean homeWidgetSetText(String text) {
        Context app = getApplicationContext();
        boolean committed =
                app.getSharedPreferences(HomeWidgetProvider.PREFS_NAME, Context.MODE_PRIVATE)
                        .edit()
                        .putString(HomeWidgetProvider.KEY_TEXT, text)
                        .commit();
        if (!committed) {
            return false;
        }
        try {
            HomeWidgetProvider.requestUpdate(app);
        } catch (RuntimeException e) {
            // 再描画要求の失敗は保存成功を覆さない（例外の型名だけ記録し、保存値は出さない）。
            android.util.Log.w("FandheHomeWidget", "requestUpdate failed: " + e.getClass().getSimpleName());
        }
        return true;
    }

    // Rust 側（`android_home_widget.rs::call_get`）から JNI 経由で呼ばれる。未保存は null。
    // JNI 署名: ()Ljava/lang/String;
    private String homeWidgetCurrentText() {
        return HomeWidgetProvider.readText(getApplicationContext());
    }

    // Rust 側（`device/android_background_task.rs::AndroidBackgroundTask::call_schedule`、
    // Issue #1046）から JNI 経由で呼ばれる同期メソッド。処理 1 件を一意名で予約する。
    // 二重予約は常に置き換える（`ExistingWorkPolicy.REPLACE`、
    // `fandhe_device::background_task` の契約）。戻り値: 0 = 成功、それ以外 = 失敗。
    private int backgroundTaskSchedule() {
        try {
            androidx.work.OneTimeWorkRequest request =
                    new OneTimeWorkRequest.Builder(BackgroundTaskWorker.class).build();
            return awaitOperation(
                    WorkManager.getInstance(getApplicationContext())
                            .enqueueUniqueWork(
                                    BACKGROUND_TASK_WORK_NAME,
                                    ExistingWorkPolicy.REPLACE,
                                    request));
        } catch (RuntimeException e) {
            return 1;
        }
    }

    // Rust 側（`AndroidBackgroundTask::call_cancel`）から JNI 経由で呼ばれる同期
    // メソッド。予約を取り消す。未予約でも `cancelUniqueWork` 自体は成功する
    // （べき等、`fandhe_device::background_task` の契約。Rust 側はこの戻り値
    // 0 をそのまま `Ok(())` として扱う）。戻り値: 0 = 成功、それ以外 = 失敗。
    private int backgroundTaskCancel() {
        try {
            return awaitOperation(
                    WorkManager.getInstance(getApplicationContext())
                            .cancelUniqueWork(BACKGROUND_TASK_WORK_NAME));
        } catch (RuntimeException e) {
            return 1;
        }
    }

    // Rust 側（`AndroidNetworkStatus::call_read`、JNI 署名 `()I`）から 1 秒間隔で呼ばれる
    // 同期メソッド（Issue #1284）。結果コードの意味は `NetworkStatusHelper` を参照。
    private int networkStatusRead() {
        return NetworkStatusHelper.read(getApplicationContext());
    }

    // Rust 側（`AndroidAccessibilityPrefs::call_read`、JNI 署名 `()I`）から 1 秒間隔で呼ばれる
    // 同期メソッド（Issue #1310）。結果コードの意味は `AccessibilityPrefsHelper` を参照。
    private int accessibilityPrefsRead() {
        return AccessibilityPrefsHelper.read(getApplicationContext());
    }

    // Rust 側（`AndroidAccentColor::call_read`、JNI 署名 `()I`）から 1 秒間隔で呼ばれる
    // 同期メソッド（Issue #422）。結果コードの意味は `SystemAccentColorHelper` を参照。
    private int systemAccentColorRead() {
        return SystemAccentColorHelper.read(getApplicationContext());
    }

    // Rust 側（`AndroidFocusStatus::call_read`、JNI 署名 `()I`）から 1 秒間隔で呼ばれる
    // 同期メソッド（Issue #439）。結果コードの意味は `FocusModeStatusHelper` を参照。
    private int focusModeStatusRead() {
        return FocusModeStatusHelper.read(getApplicationContext());
    }

    // Rust 側（`AndroidAppShortcuts::call_install`、JNI 署名 `(Ljava/lang/String;Ljava/lang/String;)I`）
    // からワーカースレッドで呼ばれる同期メソッド（Issue #426）。結果コードは `AppShortcutsHelper` を参照。
    private int appShortcutsInstall(String label, String url) {
        return AppShortcutsHelper.install(getApplicationContext(), label, url);
    }

    // `TextToSpeech` はプロセス内リソース（音声エンジンへの接続）を持つため、
    // Activity 破棄時に明示的に手放す（Issue #866）。保留中の要求（`INITIALIZING`
    // 中に Activity が破棄されたケース）は `onInit` が届かなくなるため、ここで
    // 内部エラーとして畳んでおく（Rust 側 mailbox がタイムアウトまで待たされずに済む）。
    // Activity 再生成後は次の要求で `ttsRequest` が作り直す（`android_app()` は
    // 最新の Activity を指すため、以降の JNI 呼び出しは新インスタンスへ届く）。
    @Override
    protected void onDestroy() {
        // 破棄状態をまず確定する（codex-review 指摘 P1）。以後に実行される
        // `ttsRequest` の Runnable・`onTtsInit` はこのフラグを見て要求を拒否する
        // （`ttsActivityDestroyed` doc 参照）。
        ttsActivityDestroyed = true;
        // 破棄中ウィンドウの insets 報告が新 Activity の状態を潰さないよう、super より前に確定する。
        fullscreenHelper.onDestroy();
        penStylusHelper.onDestroy();
        // UI キューを待つネイティブ終了処理（clear）が super.onDestroy() の後ろで詰まらないよう先に解放する。
        nowPlayingHelper.onDestroy();
        super.onDestroy();
        inAppBrowserHelper.onDestroy();
        companionDeviceHelper.onDestroy();
        pedometerHelper.onDestroy();
        healthDataHelper.onDestroy();
        translationHelper.onDestroy();
        geocodingHelper.onDestroy();
        spellCheckHelper.onDestroy();
        lanDiscoveryHelper.onDestroy();
        localPeerHelper.onDestroy();
        bluetoothClassicHelper.onDestroy();
        blePeripheralHelper.onDestroy();
        musicLibraryHelper.onDestroy();
        wifiInfoHelper.onDestroy();
        wifiConnectHelper.onDestroy();
        radioControlHelper.onDestroy();
        scannerDevicesHelper.onDestroy();
        nfcReadHelper.onDestroy();
        compassHelper.onDestroy();
        environmentSensorsHelper.onDestroy();
        motionRawSensorsHelper.onDestroy();
        mediaPlaybackHelper.onDestroy();
        pictureInPictureHelper.onDestroy();
        midiHelper.onDestroy();
        sleepLockEventsHelper.onDestroy();
        appLifecycleHelper.onDestroy();
        memoryPressureHelper.onDestroy();
        removableStorageHelper.onDestroy();
        audioSessionHelper.onDestroy();
        cameraPreviewHelper.onDestroy();
        callUiHelper.onDestroy();
        screenCaptureHelper.onDestroy();
        videoCaptureHelper.onDestroy();
        printHelper.onDestroy();
        screenWakeLockHelper.onDestroy();
        screenBrightnessHelper.onDestroy();
        screenOrientationHelper.onDestroy();
        captureProtectionHelper.onDestroy();
        foldablePostureHelper.onDestroy();
        flashlightHelper.onDestroy();
        systemSoundHelper.onDestroy();
        // マイク録音（Issue #957）: `onPause` で録音中なら既に破棄済みのため
        // `micRecorder == null` で no-op（`onPause` doc 参照）。`onPause` を経由せず
        // 直接 `onDestroy` へ来るケース（Android の契約上は稀）に備えて同じ処理を置く。
        synchronized (micLock) {
            if (micRecorder != null) {
                finishMicRecord(micRecordRequest.current(), MIC_RESULT_INTERRUPTED);
            }
        }
        // 音声認識（Issue #1026）: `onPause` で聞き取り中なら既に破棄済みのため
        // `speechRecognizer == null` で no-op（`onPause` doc 参照）。`onPause` を経由せず
        // 直接 `onDestroy` へ来るケース（Android の契約上は稀）に備えて同じ処理を置く。
        synchronized (speechLock) {
            if (speechRecognizer != null) {
                abortSpeech(SPEECH_RESULT_INTERRUPTED);
            }
        }
        // BLE スキャン（Issue #1062）: スキャン中・権限ダイアログ待ちを問わず止めて、追跡中の
        // 要求があれば中断で確定する（破棄後に結果を届ける先は無いが Rust 側を畳む）。
        stopBleScanQuietly();
        blePermissionPending = false;
        bleStartPendingResume = false;
        finishBle(bleScanRequest.current(), BLE_RESULT_INTERRUPTED);
        // ファイル選択（Issue #874、案 B）は Activity 破棄時に明示的な決着を送らない。
        // `Picking` のまま残る（OS がコールバックしないケースと同じ既知の天井、
        // 仕様書「固定タイムアウトを掛けない」節参照）。`filePickRequest` は static
        // のため再生成後も値を保持し（`filePickRequest` doc 参照）、新インスタンスの
        // `pickDocument` コールバックが実際に届いた旧要求の結果を正しく拾える。
        // プロセス死では static ごと消えるため、次プロセスでは `RequestTracker.NONE`
        // から再開する。
        long pendingTtsRequestId = pendingTtsRequest.take();
        if (pendingTtsRequestId != RequestTracker.NONE) {
            nativeOnTtsResult(pendingTtsRequestId, 2);
            pendingTtsText = null;
        }
        if (tts != null) {
            // 発話中の utterance があれば `onStop` を促してから破棄する。
            // `shutdown()` 単体で `onStop` が確実に呼ばれる保証は `TextToSpeech`
            // のドキュメントに無いため、明示的に `stop()` してから `shutdown()` する。
            tts.stop();
            // `stop()` が `onStop` を発火させる保証はないため、`shutdown()` の前に
            // 直接確定させる（codex-review 指摘、PR #869 4 巡目是正）。
            // `take()` で読み取りとクリアを 1 操作にすることで、この直後に
            // 本物の `onStop` が届いても `deliverUtteranceOutcome` 側の
            // `complete` が不一致になり二重送信にならない
            // （`speakingRequest` doc 参照）。status 7 = キャンセル
            // （`decode_tts_result` 参照。Activity 破棄による停止のため失敗として
            // 扱わない）。
            long stillSpeakingRequestId = speakingRequest.take();
            if (stillSpeakingRequestId != RequestTracker.NONE) {
                nativeOnTtsResult(stillSpeakingRequestId, TTS_STATUS_CANCELLED);
            }
            tts.shutdown();
            tts = null;
            ttsInitState = TTS_STATE_NONE;
        }
    }

    // Rust 側（`device/android_file_provider.rs`、Issue #1020）から JNI 経由で呼ばれる同期メソッド
    // （ワーカースレッド上）。OS のファイル画面へ一覧の変更を通知する。保留中の要求は持たない。
    // JNI 署名: ()Z
    private boolean fileProviderNotifyChanged() {
        return FileProviderDocumentsProvider.notifyChanged(getApplicationContext());
    }
}
