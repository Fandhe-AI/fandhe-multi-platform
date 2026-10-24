// Phase 11 の拡充ループ（親 #166）型サンプル「システム音量」: macOS 実装（Issue #785）。
// Core Audio で既定の出力機器の音量（VirtualMainVolume）と消音（Mute）を同期で読み、
// 指定した音量へ設定して、同じ呼び出しの中で読み戻す。I/O は開かないので TCC の確認は掛からず、
// 権限の宣言も増やさない。既定の出力機器は毎回 OS へ問い、デバイス ID・名前は保持しない・返さない。
//
// `crates/fandhe-egui-proxy/src/device/macos_volume_control.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_volume_control_read(FandheVolumeRaw *out) -> int32_t
//     UI スレッドから同期で呼ぶ。0 = 成功、非 0 = 失敗（out は無効）。
//     out->has_device: 既定の出力機器があるか。無ければ他のフィールドは 0。
//     out->has_level / level: VirtualMainVolume を持つか・その値（0.0〜1.0）。
//         音量の制御を持たない出力機器（HDMI 等）では has_level = 0。
//     out->settable: 音量を設定できるか（AudioObjectIsPropertySettable の結果）。
//     out->has_mute / muted: Mute プロパティを持つか・消音中か。
//   fandhe_macos_volume_control_set(float level, FandheVolumeRaw *out) -> int32_t
//     既定の出力機器を読み直して VirtualMainVolume を level へ設定し、同じ呼び出しの中で読み戻した
//     値を out に書く。0 = 成功、非 0 = 失敗（out は無効）。設定後の値は機器が選べる値へ丸められる。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。

#import <AudioToolbox/AudioToolbox.h>
#import <CoreAudio/CoreAudio.h>
#import <Foundation/Foundation.h>
#include <stdint.h>
#include <string.h>

typedef struct {
    int32_t has_device;
    int32_t has_level;
    float level;
    int32_t settable;
    int32_t has_mute;
    int32_t muted;
} FandheVolumeRaw;

static AudioObjectPropertyAddress fandhe_volume_address(void) {
    AudioObjectPropertyAddress addr = {kAudioHardwareServiceDeviceProperty_VirtualMainVolume,
                                       kAudioDevicePropertyScopeOutput,
                                       kAudioObjectPropertyElementMain};
    return addr;
}

static AudioObjectPropertyAddress fandhe_mute_address(void) {
    AudioObjectPropertyAddress addr = {kAudioDevicePropertyMute, kAudioDevicePropertyScopeOutput,
                                       kAudioObjectPropertyElementMain};
    return addr;
}

// 既定の出力機器を毎回 OS へ問う。取得に失敗したら非 0、機器が無ければ 0 で *device = kAudioObjectUnknown。
static int fandhe_default_output(AudioObjectID *device) {
    AudioObjectPropertyAddress addr = {kAudioHardwarePropertyDefaultOutputDevice,
                                       kAudioObjectPropertyScopeGlobal,
                                       kAudioObjectPropertyElementMain};
    UInt32 size = sizeof(AudioObjectID);
    *device = kAudioObjectUnknown;
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &addr, 0, NULL, &size, device) !=
        noErr) {
        return 1;
    }
    return 0;
}

// device から音量・可否・消音を読んで out に書く。0 = 成功。
static int fandhe_read_device(AudioObjectID device, FandheVolumeRaw *out) {
    memset(out, 0, sizeof(*out));
    if (device == kAudioObjectUnknown) {
        return 0;
    }
    out->has_device = 1;

    AudioObjectPropertyAddress vol = fandhe_volume_address();
    if (AudioObjectHasProperty(device, &vol)) {
        Float32 level = 0;
        UInt32 size = sizeof(level);
        if (AudioObjectGetPropertyData(device, &vol, 0, NULL, &size, &level) != noErr) {
            return 2;
        }
        Boolean settable = 0;
        if (AudioObjectIsPropertySettable(device, &vol, &settable) != noErr) {
            return 3;
        }
        out->has_level = 1;
        out->level = level;
        out->settable = settable ? 1 : 0;
    }

    AudioObjectPropertyAddress mute = fandhe_mute_address();
    if (AudioObjectHasProperty(device, &mute)) {
        UInt32 muted = 0;
        UInt32 size = sizeof(muted);
        if (AudioObjectGetPropertyData(device, &mute, 0, NULL, &size, &muted) != noErr) {
            return 4;
        }
        out->has_mute = 1;
        out->muted = muted ? 1 : 0;
    }
    return 0;
}

int32_t fandhe_macos_volume_control_read(FandheVolumeRaw *out) {
    if (out == NULL) {
        return 1;
    }
    @autoreleasepool {
        AudioObjectID device;
        if (fandhe_default_output(&device) != 0) {
            return 2;
        }
        return fandhe_read_device(device, out);
    }
}

int32_t fandhe_macos_volume_control_set(float level, FandheVolumeRaw *out) {
    if (out == NULL) {
        return 1;
    }
    @autoreleasepool {
        AudioObjectID device;
        if (fandhe_default_output(&device) != 0 || device == kAudioObjectUnknown) {
            return 2;
        }
        AudioObjectPropertyAddress vol = fandhe_volume_address();
        Boolean settable = 0;
        if (!AudioObjectHasProperty(device, &vol) ||
            AudioObjectIsPropertySettable(device, &vol, &settable) != noErr || !settable) {
            return 3;
        }
        Float32 value = level;
        if (AudioObjectSetPropertyData(device, &vol, 0, NULL, sizeof(value), &value) != noErr) {
            return 4;
        }
        // 要求値ではなく、同じ呼び出しの中で OS から読み戻した値を返す。
        return fandhe_read_device(device, out);
    }
}
