// Phase 11 の拡充ループ（親 #932）: ゲームコントローラ入力を Apple（iOS / macOS）へ
// 橋渡しする（Issue #934）。GameController.framework（`GCController`）は iOS と
// macOS で同一 API のため、`apple/fandhe_apple_location.m` と同じく 1 ファイルを
// 両 OS で共用する。
//
// `crates/fandhe-egui-proxy/src/device/apple_gamepad.rs` から C ABI 経由で呼ばれる。
// `build.rs` が iOS・macOS 両ターゲット（feature `device` 有効時）で `cc` コンパイルし、
// GameController フレームワークをリンクする。
//
// 設計判断: スナップショットの読み取りは通知キャッシュではなく **pull 型**にする
// （Windows 実装 `windows_gamepad.rs`・iOS `fandhe_ios_motion.m` と同じ形）。状態を
// 一切キャッシュしないため、「切断後も最後の値を返し続ける」事故が構造上起こらない
// （Issue #859 型の「嘘の成功」の再発防止、`fandhe_device::gamepad` モジュール doc
// 参照）。
//
// ただし `GCControllerDidConnectNotification` を一度も購読しないと、GameController
// フレームワークが内部の検出処理を開始せず、アプリ起動前から接続済みだったコント
// ローラが `[GCController controllers]` に反映されないことがある（フレームワークの
// 既知の挙動）。そのため `fandhe_apple_gamepad_ensure_discovery_started` で検出処理の
// 起動だけを目的に通知を購読する（`dispatch_once` で 1 回のみ）。通知ハンドラは
// no-op であり、読み取りは引き続き `fandhe_apple_gamepad_read` 呼び出しのたびに
// `[GCController controllers]` を直接 pull する（Issue #934 レビュー指摘対応）。
//
// 契約:
//   int32_t fandhe_apple_gamepad_read(float *out_axes /* 6 要素 */,
//                                      uint16_t *out_buttons);
//   戻り値: 0 = スナップショットを書いた / 1 = 使えるコントローラが未接続 /
//           2 = main スレッド以外から呼ばれた / 3 = 引数が NULL。
//   0 以外のとき out 引数は書き換えない。
//
//   out_axes の並び（固定）: [0] 左 X / [1] 左 Y / [2] 右 X / [3] 右 Y /
//   [4] 左トリガ / [5] 右トリガ。**OS の生値（y 上が正）をそのまま書く**。
//   符号反転・クランプ・NaN 置換は Rust 側（`apple_gamepad.rs::result_from_code`）が
//   行う（テスト可能な側へ寄せる、`windows_gamepad.rs::reading_to_snapshot` と同じ
//   責務配置）。
//
//   out_buttons のビット位置は `fandhe_device::gamepad::BUTTON_*` と同一
//   （bit0 A / 1 B / 2 X / 3 Y / 4 LB / 5 RB / 6 左スティック押込み /
//   7 右スティック押込み / 8-11 十字キー上下左右 / 12 メニュー / 13 ビュー）。
//
//   「先頭 1 台」の解釈: `[GCController controllers]` を先頭から走査し、
//   `extendedGamepad != nil` の最初の 1 台を読む。スティックを持たないプロファイル
//   のみの機器は「使えるコントローラが未接続」（戻り値 1）として扱い、全 0 を
//   接続中として返さない（`fandhe_device::gamepad` モジュール doc の契約）。
//
//   main スレッド検査: `[NSThread isMainThread]`（`fandhe_apple_text_scale.m` と
//   同じ判断で、macOS 用に Rust 側へ `pthread_main_np` の unsafe 宣言を増やさない）。
//
// Apple 実装は `Unsupported`（戻り値のレパートリーに無い）を返さない: iOS 17 /
// macOS 11 の両方で GameController.framework は常に存在するため（実装計画 §3.3）。
//
// 入力値（スティック値・ボタン名・コントローラ名）をログへ出力しない
// （security.md A09）。
//
// ponytail: 固定間隔ポーリング（呼び出し元 effect ドライバの間隔、最大 200ms 程度の
// 接続検知遅延）。通知購読は検出処理の起動トリガーのみに使い、repaint 要求はしていない。
// 遅延が要件上問題になったら、通知ハンドラから repaint 要求を出す形へ上げる。

#import <Foundation/Foundation.h>
#import <GameController/GameController.h>

// `GCControllerDidConnectNotification` の購読を 1 回だけ行い、フレームワークの内部
// 検出処理を起動する（ファイル冒頭コメントの設計判断を参照）。ハンドラ自体は状態を
// 持たない no-op。
static void fandhe_apple_gamepad_ensure_discovery_started(void) {
    static dispatch_once_t once_token;
    dispatch_once(&once_token, ^{
        [[NSNotificationCenter defaultCenter]
            addObserverForName:GCControllerDidConnectNotification
                        object:nil
                         queue:nil
                    usingBlock:^(NSNotification *_Nonnull __unused note) {
                      // no-op: 購読すること自体がフレームワークの検出処理を起動する
                      // ために必要。実際の状態読み取りは毎回 pull で行う。
                    }];
    });
}

int32_t fandhe_apple_gamepad_read(float *out_axes, uint16_t *out_buttons) {
    if (out_axes == NULL || out_buttons == NULL) {
        return 3;
    }
    if (![NSThread isMainThread]) {
        return 2;
    }

    fandhe_apple_gamepad_ensure_discovery_started();

    GCExtendedGamepad *pad = nil;
    for (GCController *controller in [GCController controllers]) {
        if (controller.extendedGamepad != nil) {
            pad = controller.extendedGamepad;
            break;
        }
    }
    if (pad == nil) {
        return 1;
    }

    out_axes[0] = pad.leftThumbstick.xAxis.value;
    out_axes[1] = pad.leftThumbstick.yAxis.value;
    out_axes[2] = pad.rightThumbstick.xAxis.value;
    out_axes[3] = pad.rightThumbstick.yAxis.value;
    out_axes[4] = pad.leftTrigger.value;
    out_axes[5] = pad.rightTrigger.value;

    uint16_t buttons = 0;
    if (pad.buttonA.isPressed) buttons |= (1 << 0);
    if (pad.buttonB.isPressed) buttons |= (1 << 1);
    if (pad.buttonX.isPressed) buttons |= (1 << 2);
    if (pad.buttonY.isPressed) buttons |= (1 << 3);
    if (pad.leftShoulder.isPressed) buttons |= (1 << 4);
    if (pad.rightShoulder.isPressed) buttons |= (1 << 5);
    // `leftThumbstickButton`/`rightThumbstickButton` は nullable（押込み非対応の
    // コントローラでは nil）。nil への Objective-C メッセージ送信は NO を返すため、
    // 明示 nil チェックをしなくても安全に「未押下」として扱える。
    if (pad.leftThumbstickButton.isPressed) buttons |= (1 << 6);
    if (pad.rightThumbstickButton.isPressed) buttons |= (1 << 7);
    if (pad.dpad.up.isPressed) buttons |= (1 << 8);
    if (pad.dpad.down.isPressed) buttons |= (1 << 9);
    if (pad.dpad.left.isPressed) buttons |= (1 << 10);
    if (pad.dpad.right.isPressed) buttons |= (1 << 11);
    if (pad.buttonMenu.isPressed) buttons |= (1 << 12);
    // `buttonOptions` も nullable（同上の理由で nil 安全）。
    if (pad.buttonOptions.isPressed) buttons |= (1 << 13);

    *out_buttons = buttons;
    return 0;
}
