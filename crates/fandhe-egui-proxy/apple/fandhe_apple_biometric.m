// Phase 11 の拡充ループ（親 #905）型サンプル「生体認証」: macOS 実装（Issue #906）・
// iOS 実装（Issue #908）。iOS / macOS 共用ファイル（`fandhe_apple_location.m`/
// `fandhe_apple_share.m` と同じ判断）。`LAContext.evaluatePolicy` は提示 VC を要求
// しないため、iOS/macOS で分岐せず同じ共通関数を呼ぶだけで完了する。
//
// `crates/fandhe-egui-proxy/src/device/apple_biometric.rs` から C ABI 経由で呼ばれる。
// `build.rs` が iOS 全体・macOS ターゲット（feature `device` 有効時）で `cc`
// コンパイルし、LocalAuthentication をリンクする。
//
// 契約（pull 型。`fandhe_apple_share.m` と同型、coding-rust.md の FFI 例外規定）:
//   fandhe_apple_biometric_authenticate(uint64_t request_id, const char *reason)
//     -> int32_t 戻り値: 0 = 提示依頼を受理（結果は非同期に
//        fandhe_rust_biometric_outcome で届く）。1 = 提示依頼自体の失敗（呼び出し元が
//        同期的に Internal エラーへ畳む）。reason は NUL 終端 UTF-8。関数内で即座に
//        NSString へコピーしてから return するため、呼び出し元は return 後に元の
//        文字列バッファを解放してよい（所有権は移らない）。
//   fandhe_rust_biometric_outcome(uint64_t request_id, int32_t success, int64_t la_code)
//     -> Rust 側（`apple_biometric.rs`）が公開する `extern "C"` 関数。`success` は
//        `LAContext.evaluatePolicy` reply の `success` をそのまま渡し、`la_code` は
//        `error.code`（`success` が真のときは 0）をそのまま渡す。結果コード表への
//        写像は Rust 側 `device/biometric_auth.rs::outcome_from_reply` に一本化する
//        （`.m` 側で二重に写像しない、実装計画の設計判断 3）。
//
// ポリシーは `LAPolicyDeviceOwnerAuthenticationWithBiometrics` 固定でパスコードへの
// フォールバックは使わない（`localizedFallbackTitle = @""` でフォールバックボタンを
// 隠す）。`canEvaluatePolicy:error:` で可否判定してから `evaluatePolicy` を呼ぶ
// （可否判定の YES を「認証した」にしない）。
//
// スレッド: `dispatch_async(main)` で提示自体を行う（`fandhe_apple_share.m` と同じ
// 方針）。reply は `LAContext` の private queue から直接 Rust 側 `extern "C"` 関数を
// 呼ぶ（`bridge` は `Mutex` のため任意スレッドから呼んでよい）。ただし
// `g_biometric_context` への書き込み（reply 内の解放）は main queue へ
// dispatch_async して行い、生成側と同じスレッドに閉じる（static へのアクセスを
// 1 スレッドに限定するため）。iOS は
// `apple_biometric.rs::fandhe_rust_biometric_outcome` が
// `crate::ios::request_repaint()` で明示的に次フレームを起こす。
//
// 情報非保持: `reason` と `success`/`la_code` 以外は Rust 側へ渡さない。
// `LAContext.evaluatedPolicyDomainState`（生体登録状態のダイジェスト）等は読まない。
// `NSLog` を含むログ出力は一切行わない（security.md A09）。
//
// 二重呼び出し防御: `g_biometric_context != nil` の間に呼ばれた場合は進行中の
// context を潰さず、`la_code = -10`（Rust 側で未知コードとして Internal エラーに
// 畳まれる、`invalidContext` 相当）で即座にエラー応答する。通常は Rust 側の状態機械
// （`Idle` 以外はボタン非表示）が二重要求自体を防ぐため、この分岐は防御的コードに留まる。
//
// `g_biometric_context` への書き込みはすべて main queue に閉じる: 生成は
// `dispatch_async(main)` ブロック内、reply コールバック（`LAContext` の private
// queue から呼ばれる）内の解放も `dispatch_async(main)` で main queue へ戻してから
// 行う（読み取り判定の分岐冒頭も同様に main queue 上）。全アクセスを 1 スレッドに
// 閉じることで、形式的なデータレースも作らない。

#import <TargetConditionals.h>
#import <LocalAuthentication/LocalAuthentication.h>

// Rust 側（`apple_biometric.rs`）が公開する `extern "C"` 関数。両 OS 共通。
extern void fandhe_rust_biometric_outcome(uint64_t request_id, int32_t success, int64_t la_code);

// reply が届くまでプロセス内で生存させる必要があるため static で強参照する
// （`LAContext` はブロック内から弱参照されるだけでは reply 前に解放されうる）。
static LAContext *g_biometric_context = nil;

static void FandheBiometricAuthenticateOnMainThread(uint64_t request_id, NSString *reason) {
    if (g_biometric_context != nil) {
        // 進行中の context を潰さない（doc の「二重呼び出し防御」参照）。
        fandhe_rust_biometric_outcome(request_id, 0, -10);
        return;
    }

    LAContext *context = [[LAContext alloc] init];
    // パスコードへのフォールバックボタンを隠す（実装計画の設計判断 4）。
    context.localizedFallbackTitle = @"";

    NSError *error = nil;
    if (![context canEvaluatePolicy:LAPolicyDeviceOwnerAuthenticationWithBiometrics error:&error]) {
        // 可否判定の失敗コードをそのまま渡す（`error` が nil の場合のみ
        // `biometryNotAvailable` 相当の -6 で代替する）。`canEvaluatePolicy` の
        // YES を「認証した」にしない設計（実装計画の設計判断 5）。
        NSInteger code = (error != nil) ? error.code : -6;
        fandhe_rust_biometric_outcome(request_id, 0, (int64_t)code);
        return;
    }

    g_biometric_context = context;
    [context evaluatePolicy:LAPolicyDeviceOwnerAuthenticationWithBiometrics
             localizedReason:reason
                       reply:^(BOOL success, NSError *_Nullable replyError) {
                         int64_t code = success ? 0 : (int64_t)(replyError != nil ? replyError.code : 0);
                         fandhe_rust_biometric_outcome(request_id, success ? 1 : 0, code);
                         // 通知後は自分を強参照している static を解放する。private
                         // queue から直接触らず main queue へ dispatch し、書き込みを
                         // 1 スレッドに閉じる（doc 冒頭のスレッド節参照）。
                         dispatch_async(dispatch_get_main_queue(), ^{
                           g_biometric_context = nil;
                         });
                       }];
}

int32_t fandhe_apple_biometric_authenticate(uint64_t request_id, const char *reason) {
    if (reason == NULL) {
        return 1;
    }
    NSString *nsReason = [NSString stringWithUTF8String:reason];
    if (nsReason == nil) {
        return 1;
    }

    dispatch_async(dispatch_get_main_queue(), ^{
      FandheBiometricAuthenticateOnMainThread(request_id, nsReason);
    });
    return 0;
}
