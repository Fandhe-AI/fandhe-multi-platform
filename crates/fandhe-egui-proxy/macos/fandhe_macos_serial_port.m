// Phase 11 の拡充ループ（親 #131）型サンプル「シリアルポート」: macOS 実装（Issue #670）。
// IOKit でシリアルポート（callout の `/dev/cu.*`）を列挙し、POSIX の端末入出力で選んだポートを
// 固定設定（9600 bps・8N1・フロー制御なし）で開いて読み書きする。送る内容・往復の手順・期限は
// Rust 側 `fandhe_device::serial_port`（`round_trip`）の責務で、ここは 1 回ごとの OS 呼び出しだけを持つ。
//
// `crates/fandhe-egui-proxy/src/device/macos_serial_port.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルし、IOKit をリンクする。
// Info.plist・entitlement は足さない（サンドボックスなしで動かす前提。TCC の許可ダイアログは無い
// 見込みだが推測で、Mac 目視 #667 で確かめる）。
//
// 契約:
//   戻りコード: 0 = 成功 / 1 = errno が有効（*out_errno。Rust 側 `failure_from_errno` が失敗種別へ写す。
//   写像はそこ 1 か所）/ 2 = 内部失敗（引数不正・IOKit の失敗など）。0 以外のとき出力は信用しない。
//
//   fandhe_macos_serial_list(out)
//     callout デバイスのパスを最大 FANDHE_SERIAL_MAX_PORTS 件、各 FANDHE_SERIAL_NAME_BYTES バイトの
//     NUL 終端で out->names へ詰め、件数を out->count に書く。0 件は成功（count = 0）。
//   fandhe_macos_serial_open(path, out_errno)
//     open(O_RDWR|O_NOCTTY|O_NONBLOCK) → TIOCEXCL（排他）→ 変更前の termios を保存 → raw・9600 bps・
//     8N1・フロー制御なし・VMIN=0/VTIME=0 → 設定の反映を再読込で確認 → 受信・送信バッファを破棄。
//     途中で失敗したら termios を戻して閉じ、NULL と errno を返す。パスの検証は Rust 側が先に行う。
//   fandhe_macos_serial_write / fandhe_macos_serial_read
//     最大 wait_ms だけ select で待ってから 1 回だけ write / read する。待ち切れなければ 0 バイトの成功。
//     read が準備完了のあとで 0 バイトを返したら切断（ENXIO）として扱う。
//   fandhe_macos_serial_close(handle)
//     保存した termios を戻し、排他を解いて閉じ、handle を解放する。NULL を許す。
//
// メモリ安全・FFI 規約: 全関数を `@autoreleasepool` で囲む。呼び出し元のポインタは保持しない。
// バッファ長は Rust 側から渡され、本ファイルは長さを超えて書かない。パス・errno・受信内容は
// ログに出さない（NSLog しない）。

#import <Foundation/Foundation.h>
#import <IOKit/IOKitLib.h>
#import <IOKit/serial/IOSerialKeys.h>
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/select.h>
#include <sys/time.h>
#include <termios.h>
#include <unistd.h>

#define FANDHE_SERIAL_MAX_PORTS 32
#define FANDHE_SERIAL_NAME_BYTES 128

// Rust 側 `macos_serial_port.rs` の `RawList` と同じレイアウト。
typedef struct {
    int32_t count;
    char names[FANDHE_SERIAL_MAX_PORTS][FANDHE_SERIAL_NAME_BYTES];
} FandheSerialListRaw;

typedef struct {
    int fd;
    struct termios saved;
} FandheSerialHandle;

int32_t fandhe_macos_serial_list(FandheSerialListRaw *out) {
    if (out == NULL) {
        return 2;
    }
    @autoreleasepool {
        memset(out, 0, sizeof(*out));
        CFMutableDictionaryRef matching = IOServiceMatching(kIOSerialBSDServiceValue);
        if (matching == NULL) {
            return 2;
        }
        CFDictionarySetValue(matching, CFSTR(kIOSerialBSDTypeKey), CFSTR(kIOSerialBSDAllTypes));
        io_iterator_t iterator = IO_OBJECT_NULL;
        // matching は IOServiceGetMatchingServices が消費する（成否によらず解放不要）。
        kern_return_t kr = IOServiceGetMatchingServices(MACH_PORT_NULL, matching, &iterator);
        if (kr != KERN_SUCCESS) {
            return 2;
        }
        int32_t count = 0;
        io_object_t service;
        while ((service = IOIteratorNext(iterator)) != IO_OBJECT_NULL) {
            if (count < FANDHE_SERIAL_MAX_PORTS) {
                CFTypeRef path = IORegistryEntryCreateCFProperty(
                    service, CFSTR(kIOCalloutDeviceKey), kCFAllocatorDefault, 0);
                if (path != NULL) {
                    if (CFGetTypeID(path) == CFStringGetTypeID() &&
                        CFStringGetCString((CFStringRef)path, out->names[count],
                                           FANDHE_SERIAL_NAME_BYTES, kCFStringEncodingUTF8)) {
                        count++;
                    } else {
                        // 変換できない・長すぎる項目は捨てる（部分的に詰まった分を消す）。
                        memset(out->names[count], 0, FANDHE_SERIAL_NAME_BYTES);
                    }
                    CFRelease(path);
                }
            }
            IOObjectRelease(service);
        }
        IOObjectRelease(iterator);
        out->count = count;
        return 0;
    }
}

// 失敗時に設定を戻して閉じる共通処理。saved_ok は保存済み termios が有効か。
static void FandheSerialAbort(int fd, const struct termios *saved, int saved_ok) {
    if (saved_ok) {
        tcsetattr(fd, TCSANOW, saved);
    }
    ioctl(fd, TIOCNXCL);
    close(fd);
}

FandheSerialHandle *fandhe_macos_serial_open(const char *path, int32_t *out_errno) {
    if (path == NULL || out_errno == NULL) {
        return NULL;
    }
    @autoreleasepool {
        int fd = open(path, O_RDWR | O_NOCTTY | O_NONBLOCK);
        if (fd < 0) {
            *out_errno = errno;
            return NULL;
        }
        if (ioctl(fd, TIOCEXCL) != 0) {
            *out_errno = errno;
            close(fd);
            return NULL;
        }
        FandheSerialHandle *h = (FandheSerialHandle *)calloc(1, sizeof(FandheSerialHandle));
        if (h == NULL) {
            *out_errno = ENOMEM;
            FandheSerialAbort(fd, NULL, 0);
            return NULL;
        }
        h->fd = fd;
        if (tcgetattr(fd, &h->saved) != 0) {
            *out_errno = errno;
            FandheSerialAbort(fd, NULL, 0);
            free(h);
            return NULL;
        }
        struct termios t = h->saved;
        cfmakeraw(&t);
        cfsetspeed(&t, B9600);
        t.c_cflag |= (CS8 | CLOCAL | CREAD);
        t.c_cflag &= ~(tcflag_t)(CRTSCTS | PARENB | CSTOPB);
        t.c_cc[VMIN] = 0;
        t.c_cc[VTIME] = 0;
        int ok = tcsetattr(fd, TCSANOW, &t) == 0;
        int err = errno;
        if (ok) {
            // tcsetattr は一部の項目が反映されなくても 0 を返しうるため、読み直して確かめる。
            struct termios got;
            if (tcgetattr(fd, &got) != 0 || cfgetospeed(&got) != B9600 ||
                (got.c_cflag & CSIZE) != CS8 || (got.c_cflag & (PARENB | CSTOPB | CRTSCTS)) != 0) {
                ok = 0;
                err = EINVAL;
            }
        }
        if (ok && tcflush(fd, TCIOFLUSH) != 0) {
            ok = 0;
            err = errno;
        }
        if (!ok) {
            *out_errno = err;
            FandheSerialAbort(fd, &h->saved, 1);
            free(h);
            return NULL;
        }
        return h;
    }
}

// fd が準備完了になるまで最大 wait_ms 待つ。1 = 準備完了 / 0 = 待ち切れず / -1 = 失敗（errno 有効）。
static int FandheSerialWait(int fd, int for_write, uint32_t wait_ms) {
    if (fd < 0 || fd >= FD_SETSIZE) {
        errno = EBADF;
        return -1;
    }
    fd_set set;
    FD_ZERO(&set);
    FD_SET(fd, &set);
    struct timeval tv;
    tv.tv_sec = (time_t)(wait_ms / 1000);
    tv.tv_usec = (suseconds_t)((wait_ms % 1000) * 1000);
    int r = select(fd + 1, for_write ? NULL : &set, for_write ? &set : NULL, NULL, &tv);
    if (r < 0 && errno == EINTR) {
        return 0;
    }
    return r < 0 ? -1 : (r > 0 ? 1 : 0);
}

int32_t fandhe_macos_serial_write(FandheSerialHandle *h, const uint8_t *buf, size_t len,
                                  uint32_t wait_ms, size_t *written, int32_t *out_errno) {
    if (h == NULL || buf == NULL || written == NULL || out_errno == NULL) {
        return 2;
    }
    @autoreleasepool {
        *written = 0;
        int w = FandheSerialWait(h->fd, 1, wait_ms);
        if (w < 0) {
            *out_errno = errno;
            return 1;
        }
        if (w == 0) {
            return 0;
        }
        ssize_t n = write(h->fd, buf, len);
        if (n < 0) {
            if (errno == EAGAIN || errno == EINTR) {
                return 0;
            }
            *out_errno = errno;
            return 1;
        }
        *written = (size_t)n;
        return 0;
    }
}

int32_t fandhe_macos_serial_read(FandheSerialHandle *h, uint8_t *buf, size_t cap,
                                 uint32_t wait_ms, size_t *nread, int32_t *out_errno) {
    if (h == NULL || buf == NULL || nread == NULL || out_errno == NULL) {
        return 2;
    }
    @autoreleasepool {
        *nread = 0;
        int w = FandheSerialWait(h->fd, 0, wait_ms);
        if (w < 0) {
            *out_errno = errno;
            return 1;
        }
        if (w == 0) {
            return 0;
        }
        ssize_t n = read(h->fd, buf, cap);
        if (n < 0) {
            if (errno == EAGAIN || errno == EINTR) {
                return 0;
            }
            *out_errno = errno;
            return 1;
        }
        if (n == 0) {
            // VMIN=0 の端末は「準備完了なのに 0 バイト」を切断として返す。
            *out_errno = ENXIO;
            return 1;
        }
        *nread = (size_t)n;
        return 0;
    }
}

void fandhe_macos_serial_close(FandheSerialHandle *h) {
    if (h == NULL) {
        return;
    }
    @autoreleasepool {
        FandheSerialAbort(h->fd, &h->saved, 1);
        free(h);
    }
}
