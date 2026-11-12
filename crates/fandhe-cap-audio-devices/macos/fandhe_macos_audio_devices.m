// Phase 11 の拡充ループ（親 #1136）型サンプル「音声デバイス」: macOS 実装（Issue #1330）。
// Core Audio で接続中のオーディオデバイスを列挙し、入力（input scope にチャンネルを持つ）と
// 出力（output scope にチャンネルを持つ）の名前を同期で読む。I/O は開かない
// （`AudioDeviceCreateIOProcID` を呼ばない）ので TCC のマイク確認は掛からず、権限の宣言も増やさない。
// デバイス ID・UID は読まない。hidden のデバイスは `kAudioHardwarePropertyDevices` に含まれない
// （`AudioHardwareBase.h`）ため `kAudioDevicePropertyIsHidden` は判定しない。
//
// `crates/fandhe-egui-proxy/src/device/macos_audio_devices.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_audio_devices_list(FandheAudioDevicesRaw *out) -> int32_t
//     UI スレッドから同期で呼ぶ。0 = 成功、非 0 = 失敗（out は無効）。
//     input_count / output_count は 0..=64（各方向 64 件に達したらそれ以上は書かない）。
//     名前は NUL 終端の UTF-8（最大 511 バイト、文字境界で切る）。取れなければ空文字。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。

#import <CoreAudio/CoreAudio.h>
#import <Foundation/Foundation.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define FANDHE_AUDIO_MAX 64
#define FANDHE_AUDIO_NAME_BYTES 512

typedef struct {
    int32_t input_count;
    int32_t output_count;
    char inputs[FANDHE_AUDIO_MAX][FANDHE_AUDIO_NAME_BYTES];
    char outputs[FANDHE_AUDIO_MAX][FANDHE_AUDIO_NAME_BYTES];
} FandheAudioDevicesRaw;

// 指定 scope のチャンネル有無。1 = あり、0 = なし、-1 = 読み取り失敗（「なし」と区別して全体を失敗にする。契約表 6）。
static int fandhe_has_channels(AudioObjectID device, AudioObjectPropertyScope scope) {
    AudioObjectPropertyAddress addr = {kAudioDevicePropertyStreamConfiguration, scope,
                                       kAudioObjectPropertyElementMain};
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(device, &addr, 0, NULL, &size) != noErr ||
        size < sizeof(UInt32)) {  // バッファ 0 本の方向は mNumberBuffers のみ（4 バイト）で返る
        return -1;
    }
    AudioBufferList *list = (AudioBufferList *)malloc(size);
    if (list == NULL) {
        return -1;
    }
    int found = -1;
    if (AudioObjectGetPropertyData(device, &addr, 0, NULL, &size, list) == noErr) {
        found = 0;
        for (UInt32 i = 0; i < list->mNumberBuffers; i++) {
            if (list->mBuffers[i].mNumberChannels > 0) {
                found = 1;
                break;
            }
        }
    }
    free(list);
    return found;
}

// デバイス名を dst（FANDHE_AUDIO_NAME_BYTES）へ書く。取れなければ空文字。
static void fandhe_copy_name(AudioObjectID device, char *dst) {
    dst[0] = '\0';
    AudioObjectPropertyAddress addr = {kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
                                       kAudioObjectPropertyElementMain};
    CFStringRef name = NULL;
    UInt32 size = sizeof(name);
    if (AudioObjectGetPropertyData(device, &addr, 0, NULL, &size, &name) != noErr || name == NULL) {
        return;
    }
    NSString *s = (__bridge NSString *)name;
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
    CFRelease(name);
}

int32_t fandhe_macos_audio_devices_list(FandheAudioDevicesRaw *out) {
    if (out == NULL) {
        return 1;
    }
    @autoreleasepool {
        out->input_count = 0;
        out->output_count = 0;

        AudioObjectPropertyAddress addr = {kAudioHardwarePropertyDevices,
                                           kAudioObjectPropertyScopeGlobal,
                                           kAudioObjectPropertyElementMain};
        UInt32 size = 0;
        if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &addr, 0, NULL, &size) !=
            noErr) {
            return 2;
        }
        AudioObjectID *ids = (AudioObjectID *)malloc(size > 0 ? size : 1);
        if (ids == NULL) {
            return 3;
        }
        if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &addr, 0, NULL, &size, ids) !=
            noErr) {
            free(ids);
            return 4;
        }
        UInt32 count = size / sizeof(AudioObjectID);
        for (UInt32 i = 0; i < count; i++) {
            int in_ch = fandhe_has_channels(ids[i], kAudioObjectPropertyScopeInput);
            int out_ch = fandhe_has_channels(ids[i], kAudioObjectPropertyScopeOutput);
            if (in_ch < 0 || out_ch < 0) {
                free(ids);
                out->input_count = 0;
                out->output_count = 0;
                return 5;
            }
            if (out->input_count < FANDHE_AUDIO_MAX && in_ch) {
                fandhe_copy_name(ids[i], out->inputs[out->input_count]);
                out->input_count++;
            }
            if (out->output_count < FANDHE_AUDIO_MAX && out_ch) {
                fandhe_copy_name(ids[i], out->outputs[out->output_count]);
                out->output_count++;
            }
        }
        free(ids);
    }
    return 0;
}
