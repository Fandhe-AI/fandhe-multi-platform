// Phase 11 の拡充ループ（親 #1136）型サンプル「音声デバイス」: iOS 実装（Issue #1331）。
// `AVAudioSession` を**読むだけ**で、入力候補（`availableInputs`）と現在の出力経路
// （`currentRoute.outputs`）の `portName` を返す。`setCategory` / `setActive` は呼ばない。
// 共有セッションは mic-record（`apple/fandhe_apple_mic_record.m`）が category を切り替えて
// 所有しており、2 つ目の機能が書き込むと所有権が割れるため。`portUID`・種別は読まない。
//
// `crates/fandhe-egui-proxy/src/device/ios_audio_devices.rs` から C ABI 経由で呼ばれる。
// `build.rs` の iOS 分岐が feature `device` 有効時にのみコンパイルする（AVFoundation はリンク済み）。
//
// 契約:
//   fandhe_ios_audio_devices_list(FandheIosAudioDevicesRaw *out) -> int32_t
//     UI スレッド（main thread）から同期で呼ぶ。0 = 成功、非 0 = 失敗（out は無効）。
//     category: 0 = その他・不明、1 = Record、2 = PlayAndRecord、3 = MultiRoute。
//     input_count / output_count: 0..=64、-1 = 配列が nil（取得不可）。
//     入力を意味ある値として扱うかの判定（category）は Rust 側で行う。
//     名前は NUL 終端の UTF-8（最大 511 バイト、文字境界で切る）。取れなければ空文字。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。ログは出さない。

#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#include <stdint.h>
#include <string.h>

#define FANDHE_AUDIO_MAX 64
#define FANDHE_AUDIO_NAME_BYTES 512

typedef struct {
    int32_t category;
    int32_t input_count;
    int32_t output_count;
    char inputs[FANDHE_AUDIO_MAX][FANDHE_AUDIO_NAME_BYTES];
    char outputs[FANDHE_AUDIO_MAX][FANDHE_AUDIO_NAME_BYTES];
} FandheIosAudioDevicesRaw;

// `portName` を dst へ UTF-8 で書く。取れなければ空文字。
static void fandhe_copy_name(NSString *s, char *dst) {
    dst[0] = '\0';
    if (s == nil) {
        return;
    }
    NSUInteger used = 0;
    if ([s getBytes:dst
             maxLength:FANDHE_AUDIO_NAME_BYTES - 1
            usedLength:&used
              encoding:NSUTF8StringEncoding
               options:0
                 range:NSMakeRange(0, s.length)
        remainingRange:NULL]) {
        dst[used] = '\0';
    } else {
        dst[0] = '\0';
    }
}

// ports を dst へ詰める（64 件まで）。nil なら -1。
static int32_t fandhe_fill(NSArray<AVAudioSessionPortDescription *> *ports,
                           char (*dst)[FANDHE_AUDIO_NAME_BYTES]) {
    if (ports == nil) {
        return -1;
    }
    int32_t n = 0;
    for (AVAudioSessionPortDescription *p in ports) {
        if (n >= FANDHE_AUDIO_MAX) {
            break;
        }
        fandhe_copy_name(p.portName, dst[n]);
        n++;
    }
    return n;
}

int32_t fandhe_ios_audio_devices_list(FandheIosAudioDevicesRaw *out) {
    if (out == NULL) {
        return 1;
    }
    @autoreleasepool {
        memset(out, 0, sizeof(*out));
        out->input_count = -1;
        out->output_count = -1;

        AVAudioSession *session = [AVAudioSession sharedInstance];
        NSString *cat = session.category;
        if ([cat isEqualToString:AVAudioSessionCategoryRecord]) {
            out->category = 1;
        } else if ([cat isEqualToString:AVAudioSessionCategoryPlayAndRecord]) {
            out->category = 2;
        } else if ([cat isEqualToString:AVAudioSessionCategoryMultiRoute]) {
            out->category = 3;
        }
        out->input_count = fandhe_fill(session.availableInputs, out->inputs);
        out->output_count = fandhe_fill(session.currentRoute.outputs, out->outputs);
    }
    return 0;
}
