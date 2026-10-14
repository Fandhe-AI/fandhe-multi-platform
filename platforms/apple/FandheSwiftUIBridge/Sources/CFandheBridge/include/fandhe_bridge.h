// `crates/fandhe-swiftui-bridge`（`src/ffi/mod.rs` の `#[no_mangle] extern "C"`）の
// エクスポート関数を宣言する C ヘッダ。SwiftUI 側（本ターゲットの上位、
// `FandheSwiftUIBridge` モジュール）が SPM の C ターゲット（Clang importer）経由で
// 取り込み、`FandheStore` 等の型安全なラッパを実装する土台とする。
//
// 契約の出典: `docs/architecture/swiftui-bridge-architecture.md` §3.3。
// 本ヘッダは §3.3 の凍結シグネチャを手書きした宣言であり（cbindgen 等で生成していない）、
// `ffi/mod.rs` のシグネチャ変更時は本ファイルも同時に更新する。
//
// `@_silgen_name` ではなく C ヘッダ経由を採用した理由: `fandhe_render_snapshot` /
// `fandhe_last_error_message` は集約型 `FandheBuffer` を値で返す。`@_silgen_name` は
// 非公式機能であり集約型の値返しに C 呼び出し規約を保証しないため、SPM の C ターゲット
// （Clang importer）を介して正しい ABI を保証する（PoC-2 の `@_silgen_name` 方式からの
// 意図的な差異）。

#ifndef FANDHE_BRIDGE_H
#define FANDHE_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/// 不透明ハンドル。実体は Rust 側（`fandhe-swiftui-bridge`）が所有し、Swift 側は
/// ポインタ値のみを保持する（内部を参照しない）。
typedef struct FandheStore FandheStore;

/// Rust 側が確保した JSON バイト列（描画スナップショット・エラー詳細メッセージ）を
/// Swift 側へ受け渡すための構造体。所有権は Rust 側に残り、使用後は必ず
/// `fandhe_buffer_free` で解放する（Swift 側で `free()` を直接呼ばない。
/// 設計文書 §3.1「所有権・寿命規約」参照）。
typedef struct FandheBuffer {
    /// バイト列の先頭ポインタ。バイト列が存在しない場合（エラー詳細なし等）は NULL。
    const uint8_t *ptr;
    /// バイト列の長さ（バイト数）。`ptr` が NULL の場合は 0。
    size_t len;
} FandheBuffer;

/// ビルド生成物の ABI 世代を返す。Swift 側リンク時に期待値と照合し、
/// 不整合バイナリの誤リンクによる未定義動作を実行時に検出する（A08）。
uint32_t fandhe_abi_version(void);

/// `FandheStore` ハンドルを新規生成する。失敗時（内部初期化エラー）は NULL を返す。
/// この時点ではハンドルが存在しないため、詳細エラーはスレッドローカル経路
/// （`fandhe_last_error_message(NULL)`）から取得する。
FandheStore *fandhe_store_new(void);

/// `store` を解放する。戻り値は外部レジストリへのメンバーシップ判定結果
/// （`0`: 成功、負値: 既に解放済み／無効なハンドル）。判定・レジストリからの除去・
/// 実体の解放はこの順で単一ミューテックスにより直列化される
/// （設計文書 §3.1「所有権・寿命規約」参照。二重解放・use-after-free 対策）。
int32_t fandhe_store_free(FandheStore *store);

/// 現在の描画スナップショットを JSON（UTF-8 バイト列）として取得する。
/// 無効ハンドルの場合は NULL ポインタ・長さ 0 のバッファを返す。
/// 戻り値は必ず `fandhe_buffer_free` で解放すること。
FandheBuffer fandhe_render_snapshot(const FandheStore *store);

/// イベント（JSON バイト列）を `store` へディスパッチする。戻り値は
/// 再描画要否・エラーコードを表す `i32`（`0` 以上: 成功、負値: エラー種別）。
/// `event_json` / `len` は本呼び出し中のみ有効とみなされ、Rust 側が呼び出し完了後に
/// 保持することはない（設計文書 §3.1「Swift → Rust のポインタ」参照）。
int32_t fandhe_dispatch_event(FandheStore *store, const uint8_t *event_json, size_t len);

/// `fandhe_render_snapshot` / `fandhe_last_error_message` が返した `FandheBuffer` を解放する。
/// Rust 確保バッファは必ずこの関数でのみ解放し、Swift 側で `free()` 等を直接呼ばない。
void fandhe_buffer_free(FandheBuffer buffer);

/// 直近のエラーの詳細メッセージ（UTF-8 バイト列）を取得する。`store` が NULL の場合、
/// または無効ハンドルの場合はスレッドローカルの最終エラーを参照する（設計文書 §3.3
/// `fandhe_last_error_message` 個別記述参照）。エラーが存在しない場合は NULL ポインタ・
/// 長さ 0 のバッファを返す（エラーではない）。戻り値は必ず `fandhe_buffer_free` で
/// 解放すること。本関数自体は呼び出しによって最終エラー状態を書き換えない
/// （読み取り専用アクセサ）。
FandheBuffer fandhe_last_error_message(const FandheStore *store);

#ifdef __cplusplus
}
#endif

#endif /* FANDHE_BRIDGE_H */
