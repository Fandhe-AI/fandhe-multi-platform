// Phase 10（親 #827）型サンプル「傾きで玉が転がる」: iOS 側の実センサ実装（Issue #832）。
//
// `crates/fandhe-egui-proxy/src/device/ios.rs`（`IosMotion`）から C ABI 経由で
// 呼ばれる。CoreMotion は Rust バインディングを持たないため、この 1 ファイルの
// ObjC コードに実装を閉じ、`extern "C"` 宣言 1 点のみを Rust 側へ公開する
// （`fandhe_ios_camera.m` と同型、coding-rust.md の FFI 例外規定）。
//
// `build.rs` が iOS ターゲット（`CARGO_CFG_TARGET_OS == "ios"`）かつ feature
// `device` 有効時のみ `cc` でコンパイルし、`fandhe_ios_motion` という静的ライブラリへ
// リンクする（他ターゲットのビルドには一切関与しない）。
//
// 契約（`ios.rs` のコメントと対）:
//   fandhe_ios_motion_read(&out_x, &out_y) -> 0 で成功。*out_x / *out_y に
//   -1.0..=1.0 の傾き（現在の UI 向き基準の座標系、符号変換は ios.rs 側の契約）を
//   書く。device motion のサンプルがまだ届いていない起動直後は (0.0, 0.0) を書いて
//   0 を返す。1 = 端末が device motion 非対応（シミュレータ等）で out 引数は
//   書き換えない。呼び出しは egui の update ループ（iOS ではメインスレッド）から
//   のみを想定する（`CMMotionManager` の起動・停止は main スレッドから行う
//   CoreMotion の作法に合わせる。`UIApplication.connectedScenes` の読み出しも
//   main スレッド専用のため同じ前提に乗せる）。
//
// `CMDeviceMotion.gravity` は端末そのものの物理座標系（portrait 直立時の x: 画面
// 右方向・y: 画面上方向）を返し、UI が landscape / 回転済みでも回転しない
// （Apple のドキュメント上の既定仕様）。アプリは landscape・iPad 回転を許可して
// いるため、現在の `UIInterfaceOrientation` を都度読み、gravity ベクトルを
// 「現在の UI 向きでの画面右・画面上」基準へ回転してから返す（Bugbot 指摘
// Medium、PR #844: portrait 前提の Y 反転のみだと landscape で玉が誤った軸へ
// 転がる）。
//
// センサ値はプロセス内の `CMMotionManager` の最新スナップショットを読むだけで、
// ファイル保存・ネットワーク送信・ログ出力は一切行わない（security.md A01）。
// バックグラウンド遷移時の明示停止は行わない: iOS はアプリが suspend されると
// センサ更新も止まるため、フォアグラウンド復帰以外の要件が出るまでは
// `stopDeviceMotionUpdates` を呼ばない
// （ponytail: 明示停止なしの単純化。バックグラウンドでの電力最適化が要件化したら
// `UIApplicationDidEnterBackgroundNotification` で止める）。

#import <CoreMotion/CoreMotion.h>
#import <UIKit/UIKit.h>

// 現在フォアグラウンドの `UIWindowScene` の `interfaceOrientation` を返す
// （main スレッド専用 API、`fandhe_ios_motion_read` の呼び出し前提と同じ）。
// 該当シーンが見つからない場合（`Info.plist` に `UIApplicationSceneManifest` が
// 無い旧ライフサイクル構成、または起動直後で `connectedScenes` がまだ空の 1
// フレーム目）は、旧ライフサイクルでも動く `UIApplication.statusBarOrientation`
// （非 Scene 構成の唯一のフォールバック。deprecated だが値の型は同じ
// `UIInterfaceOrientation` で変換不要）へ落ちる（codex-review 指摘 P1、PR #844:
// この経路が無いと常に portrait 扱いになり横向きで傾きが誤った軸に乗る）。
static UIInterfaceOrientation FandheCurrentInterfaceOrientation(void) {
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if (![scene isKindOfClass:[UIWindowScene class]]) {
            continue;
        }
        if (scene.activationState != UISceneActivationStateForegroundActive) {
            continue;
        }
        return ((UIWindowScene *)scene).interfaceOrientation;
    }
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    return UIApplication.sharedApplication.statusBarOrientation;
#pragma clang diagnostic pop
}

// 端末の物理座標系（portrait 直立基準）の gravity 成分 (gx, gy) を、現在の UI 向き
// での「画面右方向・画面上方向」基準へ回転する。Apple の定義では LandscapeLeft は
// ホームボタン/ノッチが左辺に来る向きのため、画面右・画面上方向は (gy, -gx) へ
// 変換する（LandscapeRight はその逆で (-gy, gx)。codex-review 指摘 P1、PR #844:
// 変換式が左右逆で、水平置き（gravity=(1,0)）が上方向加速と誤変換されていた）。
static void FandheRotateGravityToInterfaceOrientation(float gx, float gy,
                                                        UIInterfaceOrientation orientation,
                                                        float *out_x, float *out_y) {
    switch (orientation) {
        case UIInterfaceOrientationLandscapeLeft:
            *out_x = gy;
            *out_y = -gx;
            break;
        case UIInterfaceOrientationLandscapeRight:
            *out_x = -gy;
            *out_y = gx;
            break;
        case UIInterfaceOrientationPortraitUpsideDown:
            *out_x = -gx;
            *out_y = -gy;
            break;
        case UIInterfaceOrientationPortrait:
        case UIInterfaceOrientationUnknown:
        default:
            *out_x = gx;
            *out_y = gy;
            break;
    }
}

// プロセス内で 1 個だけ生成する `CMMotionManager`（`dispatch_once` で初期化、
// `fandhe_ios_camera.m` の `g_capture_in_flight` と同じくプロセス単位の単純化）。
static CMMotionManager *_Nullable g_motion_manager = nil;
// device motion 自体が非対応（シミュレータ等）だった場合に `true` を保持し、
// 以後の呼び出しでは `CMMotionManager` へ触れず毎回 1 を返す。
static BOOL g_motion_unavailable = NO;

static void FandheEnsureMotionManagerStarted(void) {
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
      CMMotionManager *manager = [[CMMotionManager alloc] init];
      if (!manager.isDeviceMotionAvailable) {
          g_motion_unavailable = YES;
          return;
      }
      manager.deviceMotionUpdateInterval = 1.0 / 60.0;
      // ハンドラ無しの pull 方式（`fandhe_ios_motion_read` が毎フレーム
      // `manager.deviceMotion` を読む、モジュール冒頭コメント参照）。既定の
      // reference frame で十分なため（gravity 成分のみ使う）、参照系を
      // 明示指定する `startDeviceMotionUpdatesUsingReferenceFrame:` は使わない。
      [manager startDeviceMotionUpdates];
      g_motion_manager = manager;
    });
}

int32_t fandhe_ios_motion_read(float *out_x, float *out_y) {
    FandheEnsureMotionManagerStarted();

    if (g_motion_unavailable) {
        return 1;
    }

    CMDeviceMotion *motion = g_motion_manager.deviceMotion;
    if (motion == nil) {
        // 起動直後、最初のサンプルがまだ届いていない区間（契約: (0.0, 0.0) を
        // 書いて成功として扱う）。
        *out_x = 0.0f;
        *out_y = 0.0f;
        return 0;
    }

    // `gravity` は端末の物理座標系（portrait 直立時の x: 画面右方向、y: 画面上方向
    // が正）の単位ベクトルで、UI の回転に追従しない。現在の UI 向きへ回転してから
    // 返す（ファイル冒頭コメント参照、Bugbot 指摘 Medium 是正）。符号変換（y 反転）
    // は呼び出し側（`ios.rs`、`tilt_ball` の画面座標系との対応）の契約のまま。
    UIInterfaceOrientation orientation = FandheCurrentInterfaceOrientation();
    FandheRotateGravityToInterfaceOrientation((float)motion.gravity.x, (float)motion.gravity.y,
                                               orientation, out_x, out_y);
    return 0;
}
