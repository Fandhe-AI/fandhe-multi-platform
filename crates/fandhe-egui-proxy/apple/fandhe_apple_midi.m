// Phase 11 の拡充ループ（親 #1137）型サンプル「MIDI 入力」: macOS（Issue #1398）・iOS（Issue #1399）共用の実装。
// CoreMIDI で接続中の MIDI 入力ソースを購読し、UMP（MIDI 1.0 プロトコル）のワード列を Rust へ渡す。
//
// `crates/fandhe-egui-proxy/src/device/apple_midi.rs` から C ABI 経由で呼ばれる。`build.rs::build_macos`
// （macOS）・`build_ios` がコンパイルし CoreMIDI をリンクする。iOS は iOS 14 以降で動き、
// それ未満は戻りコード 1（失敗）。iOS ではペアリング UI（CABTMIDICentralViewController）を使わず、
// ペア済み・他アプリで接続済みのソースだけを受ける。背景遷移中の受信継続は守らない範囲。
//
// 契約:
//   fandhe_apple_midi_start(uint64_t epoch, uint32_t *out_sources, uint32_t *out_failed) -> int32_t
//     epoch は Rust が購読ごとに採番する 0 以外の世代。ソースの接続時に connRefCon へ世代を
//     埋め込み、受信ブロックはそれを Rust へ渡して照合させる（接続時点で不変。停止→再開を
//     またいで遅れて走る古い受信ブロックのデータを Rust 側で捨てるため。refcon は値渡しで
//     解放が要らないので、進行中のコールバックの間も有効）。
//     0 = 成功（*out_sources に接続できたソース数、*out_failed に接続できなかった数。
//         0 台も成功。一部の接続失敗は受信を止めず、失敗数を Rust へ渡して画面に出す）。
//     1 = client / port の作成に失敗、または OS が古い。
//     2 = ソースが 1 台以上あるのに全部の接続に失敗（購読は残さない）。
//     OSStatus の生値は返さない（固定コードへ写す、security.md A09）。
//   fandhe_apple_midi_stop(void)
//     全ソースを切断する。冪等。client / port はプロセス寿命で 1 つだけ持ち、破棄しない
//     （Apple の注意に従う）。停止 = 全ソース切断。
//   fandhe_rust_midi_ump(const uint32_t *words, uint32_t count, uint64_t epoch)  -- Rust 側が公開
//     受信コールバック 1 回につき 1 回、全パケットの words を連結して渡す（UMP のメッセージは
//     パケットをまたがない）。
//   fandhe_rust_midi_sources(uint32_t count, uint32_t failed, uint64_t epoch)  -- Rust 側が公開
//     SetupChanged で接続数が変わったときに呼ぶ（failed = 現時点で未接続のソース数）。
//
// 並行性（device-bridge-contracts.md 11）: 接続済み集合と Connect / Disconnect の呼び出しは
// すべて 1 本のミューテックス g_lock の内側で行う。Rust への通知はロックの外で呼ぶ。
// 受信ブロックはロックを取らず、refcon の世代が 0 なら即 return する。SetupChanged で追加ソースの
// 接続に失敗しても購読は終了させず、接続数と失敗数を通知する（start 経路のみ、全滅時は失敗扱い）。main への dispatch_sync は
// 使わない（main 側が同じロックを取るとデッドロックするため）。
//
// 演奏内容（ノート・ベロシティ・バイト列）はログへ出力しない。権限・entitlement は不要の見込み
// （#1397 で確認）。MIDINetworkSession は有効にしない（ローカルネットワークの許可を求めない）。

#import <CoreMIDI/CoreMIDI.h>
#import <Foundation/Foundation.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>

extern void fandhe_rust_midi_ump(const uint32_t *words, uint32_t count, uint64_t epoch);
extern void fandhe_rust_midi_sources(uint32_t count, uint32_t failed, uint64_t epoch);

static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
// 購読中の世代。0 = 停止中。start / stop が g_lock の内側で更新する。
static _Atomic uint64_t g_epoch = 0;
static MIDIClientRef g_client = 0;
static MIDIPortRef g_port = 0;
static NSMutableSet<NSNumber *> *g_connected = nil;

// 受信ブロック。@autoreleasepool の内側で処理し、1 回のコールバックを Rust への 1 呼び出しにまとめる。
static void fandhe_deliver(const MIDIEventList *list, uint64_t epoch) {
    // 世代は接続時に埋めた refcon 由来（実行時の g_epoch は読まない）。
    if (epoch == 0 || list == NULL) {
        return;
    }
    @autoreleasepool {
        uint32_t stack_buf[256];
        uint32_t *buf = stack_buf;
        size_t cap = 256;
        size_t n = 0;
        BOOL heap = NO;
        BOOL ok = YES;
        const MIDIEventPacket *p = &list->packet[0];
        for (UInt32 i = 0; i < list->numPackets && ok; i++) {
            size_t wc = p->wordCount;
            if (n + wc > cap) {
                size_t newcap = cap * 2 > n + wc ? cap * 2 : n + wc;
                uint32_t *nb = heap ? realloc(buf, newcap * sizeof(uint32_t))
                                    : malloc(newcap * sizeof(uint32_t));
                if (nb == NULL) {
                    ok = NO;
                    break;
                }
                if (!heap) {
                    memcpy(nb, buf, n * sizeof(uint32_t));
                    heap = YES;
                }
                buf = nb;
                cap = newcap;
            }
            memcpy(buf + n, p->words, wc * sizeof(uint32_t));
            n += wc;
            p = MIDIEventPacketNext(p);
        }
        if (ok && n > 0) {
            fandhe_rust_midi_ump(buf, (uint32_t)n, epoch);
        }
        if (heap) {
            free(buf);
        }
    }
}

// g_lock を保持した状態で呼ぶ。現在のソースと接続済み集合の差分を接続 / 切断し、接続数を返す。
// *failed には接続に失敗した数を入れる。
static uint32_t fandhe_refresh_locked(uint32_t *failed) {
    NSMutableSet<NSNumber *> *now = [NSMutableSet set];
    ItemCount total = MIDIGetNumberOfSources();
    for (ItemCount i = 0; i < total; i++) {
        MIDIEndpointRef e = MIDIGetSource(i);
        if (e != 0) {
            [now addObject:@(e)];
        }
    }
    for (NSNumber *old in [g_connected copy]) {
        if (![now containsObject:old]) {
            MIDIPortDisconnectSource(g_port, (MIDIEndpointRef)old.unsignedIntValue);
            [g_connected removeObject:old];
        }
    }
    uint32_t bad = 0;
    for (NSNumber *e in now) {
        if (![g_connected containsObject:e]) {
            OSStatus st = MIDIPortConnectSource(
                g_port, (MIDIEndpointRef)e.unsignedIntValue,
                (void *)(uintptr_t)atomic_load(&g_epoch));
            if (st == noErr) {
                [g_connected addObject:e];
            } else {
                bad++;
            }
        }
    }
    if (failed != NULL) {
        *failed = bad;
    }
    return (uint32_t)g_connected.count;
}

static void fandhe_disconnect_all_locked(void) {
    for (NSNumber *e in [g_connected copy]) {
        MIDIPortDisconnectSource(g_port, (MIDIEndpointRef)e.unsignedIntValue);
    }
    [g_connected removeAllObjects];
}

// g_lock を保持した状態で呼ぶ。client / port が無ければ作る（プロセス寿命で 1 つ）。
// port 作成に失敗した client は *to_dispose へ退避する。MIDIClientDispose は実行中の通知
// ブロック（g_lock を取る）の完了を待つため、呼び出し側が g_lock を解放してから破棄する。
static BOOL fandhe_ensure_client_locked(MIDIClientRef *to_dispose) {
    if (g_client != 0 && g_port != 0) {
        return YES;
    }
    if (@available(macOS 11.0, iOS 14.0, *)) {
        g_connected = [NSMutableSet set];
        OSStatus st = MIDIClientCreateWithBlock(
            CFSTR("fandhe-midi"), &g_client, ^(const MIDINotification *msg) {
                if (msg->messageID != kMIDIMsgSetupChanged) {
                    return;
                }
                uint32_t count = 0;
                uint32_t failed = 0;
                pthread_mutex_lock(&g_lock);
                uint64_t epoch = atomic_load(&g_epoch);
                if (epoch != 0) {
                    count = fandhe_refresh_locked(&failed);
                }
                pthread_mutex_unlock(&g_lock);
                if (epoch != 0) {
                    fandhe_rust_midi_sources(count, failed, epoch);
                }
            });
        if (st != noErr) {
            g_client = 0;
            return NO;
        }
        st = MIDIInputPortCreateWithProtocol(
            g_client, CFSTR("fandhe-midi-in"), kMIDIProtocol_1_0, &g_port,
            ^(const MIDIEventList *list, void *refcon) {
                fandhe_deliver(list, (uint64_t)(uintptr_t)refcon);
            });
        if (st != noErr) {
            g_port = 0;
            // port 失敗時に client を残すと再試行で二重に作られるため破棄対象にする
            *to_dispose = g_client;
            g_client = 0;
            return NO;
        }
        return YES;
    }
    return NO;
}

int32_t fandhe_apple_midi_start(uint64_t epoch, uint32_t *out_sources, uint32_t *out_failed) {
    if (out_sources == NULL || out_failed == NULL || epoch == 0) {
        return 1;
    }
    int32_t code = 0;
    MIDIClientRef to_dispose = 0;
    pthread_mutex_lock(&g_lock);
    if (!fandhe_ensure_client_locked(&to_dispose)) {
        code = 1;
    } else {
        atomic_store(&g_epoch, epoch);
        uint32_t failed = 0;
        uint32_t count = fandhe_refresh_locked(&failed);
        if (count == 0 && failed > 0) {
            atomic_store(&g_epoch, 0);
            fandhe_disconnect_all_locked();
            code = 2;
        } else {
            *out_sources = count;
            *out_failed = failed;
        }
    }
    pthread_mutex_unlock(&g_lock);
    if (to_dispose != 0) {
        MIDIClientDispose(to_dispose);
    }
    return code;
}

void fandhe_apple_midi_stop(void) {
    pthread_mutex_lock(&g_lock);
    atomic_store(&g_epoch, 0);
    if (g_port != 0) {
        fandhe_disconnect_all_locked();
    }
    pthread_mutex_unlock(&g_lock);
}
