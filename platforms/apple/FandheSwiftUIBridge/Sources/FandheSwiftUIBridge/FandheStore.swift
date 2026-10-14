import CFandheBridge
import Foundation

/// `crates/fandhe-swiftui-bridge` の不透明ハンドル（`CFandheBridge.FandheStore` へのポインタ）を
/// 包む型安全な公開 API。TASK-3.3 の SwiftUI 画面実装はこの型のみを通じて
/// Rust コア（`fandhe-core` の `Screen<SampleState>` / `StateStore<SampleState>`）を操作し、
/// 生ポインタ・エラーコード・手動メモリ管理を一切扱わない（設計文書
/// `docs/architecture/swiftui-bridge-architecture.md` §3.1 参照）。
///
/// **スレッド安全性**: 本型は `Sendable` に適合させない。単一スレッド（TASK-3.3 の
/// SwiftUI 画面から `@MainActor` 経由で呼ばれる想定）からのみ利用すること。Rust 側の
/// ハンドルレジストリは複数呼び出しの直列化を保証するが（設計文書 §3.1 参照）、
/// Swift 側で並行アクセスした場合の呼び出し順序・エラー詳細の取り違えまでは
/// 保証しない契約とする。
public final class FandheStore {
    /// 不透明ハンドル。`init` で生成し `deinit` でのみ解放する
    /// （二重解放経路を作らないための単一箇所ルール。下記 `deinit` 参照）。
    private let handle: OpaquePointer

    /// `store` を新規生成する。
    ///
    /// **ABI 照合を `fandhe_store_new` より先に行う**（`FandheABI.verify()`）。
    /// 生成後に検査して失敗時に解放する順序にすると、Swift の初期化失敗規則
    /// （designated initializer が全プロパティ代入後に throw すると `deinit` が
    /// 呼ばれる）と衝突し、`fandhe_store_free` が「検査失敗時の明示解放」と
    /// 「`deinit` からの解放」の 2 経路で呼ばれる二重解放になる。ABI 検査はハンドルを
    /// 一切必要としないため、生成前に済ませることでこの経路自体を作らない。
    ///
    /// `fandhe_store_new` が `NULL` を返した場合、ハンドルがまだ存在しないため
    /// 詳細エラーはスレッドローカルの最終エラー（`fandhe_last_error_message(nil)`）から
    /// 取得する（設計文書 §3.3 参照）。
    public init() throws {
        try FandheABI.verify()

        guard let newHandle = fandhe_store_new() else {
            throw FandheBridgeError.fromFFI(
                code: FandheBridgeContract.ErrorCode.initializationFailed.rawValue,
                store: nil
            )
        }
        handle = newHandle
    }

    /// `store` を解放する。本型で `fandhe_store_free` を呼ぶ唯一の箇所
    /// （所有権規約: ハンドル解放は `deinit` の 1 経路に集約する）。
    deinit {
        // 戻り値（二重解放・無効ハンドル検査結果）は `deinit` からは通知先がないため
        // 破棄する。`handle` は本インスタンスの生存期間中のみここで解放される
        // ため、通常経路で負値が返ることはない想定（設計文書 §3.1 参照）。
        _ = fandhe_store_free(handle)
    }

    /// 現在の描画スナップショットを取得する。`Screen::visit_expanded` の走査結果を
    /// JSON 経由でデコードした `FandheRenderSnapshot` を返す（`FandheModels.swift` 参照）。
    ///
    /// バッファ取得（`fandhe_render_snapshot`）は `FandheBufferBridge` 経由でのみ消費し、
    /// `defer` により早期 return を含むあらゆる経路で `fandhe_buffer_free` が呼ばれる
    /// ことを保証する。
    public func renderSnapshot() throws -> FandheRenderSnapshot {
        let rawBuffer = fandhe_render_snapshot(handle)
        guard let data = FandheBufferBridge.consumingIntoData(rawBuffer) else {
            // 設計文書 §3.3: 無効ハンドルの場合 NULL ポインタ・長さ 0 のバッファを返す契約。
            // 本インスタンスが生存中は `handle` は常に有効なはずだが、Rust 側の内部エラーで
            // 空バッファが返るケースも同じ経路でエラー化する（詳細はスレッドローカル/
            // per-handle いずれかに書き込まれている想定、`fromFFI` が両方を吸収する）。
            throw FandheBridgeError.fromFFI(
                code: FandheBridgeContract.ErrorCode.invalidHandle.rawValue,
                store: handle
            )
        }
        let snapshot: FandheRenderSnapshot
        do {
            snapshot = try JSONDecoder().decode(FandheRenderSnapshot.self, from: data)
        } catch {
            throw FandheBridgeError(
                code: .malformedPayload,
                detail: "スナップショット JSON デコードに失敗しました: \(error)"
            )
        }

        // Rust ⇔ Swift のスキーマ世代不整合を silent failure にせず明示的エラーとして検出する
        // （A08 整合性対策、`FandheBridgeContract.snapshotSchemaVersion` 参照）。
        guard snapshot.schemaVersion == FandheBridgeContract.snapshotSchemaVersion else {
            throw FandheBridgeError(
                code: .malformedPayload,
                detail:
                    "スナップショットのスキーマ世代が不一致です（期待値: \(FandheBridgeContract.snapshotSchemaVersion)、"
                    + "実際: \(snapshot.schemaVersion)）"
            )
        }
        return snapshot
    }

    /// イベントを `store` へディスパッチする。戻り値は再描画が必要かどうかを表す。
    ///
    /// 戻り値契約（`crates/fandhe-swiftui-bridge/src/ffi/mod.rs` の
    /// `fandhe_dispatch_event`）: `1` = 状態更新あり・再描画要、`0` = 該当ハンドラなし、
    /// 負値 = エラー。本メソッドは正値を `true` に写像する。
    @discardableResult
    public func dispatch(_ event: FandheEvent) throws -> Bool {
        let payload: Data
        do {
            payload = try JSONEncoder().encode(event)
        } catch {
            throw FandheBridgeError(
                code: .malformedPayload,
                detail: "イベント JSON エンコードに失敗しました: \(error)"
            )
        }

        // `withUnsafeBytes` のスコープ内でのみ有効なポインタを Rust 側へ渡す。
        // 呼び出し完了後に Rust 側がこのポインタを保持しないことは設計文書 §3.1
        // 「Swift → Rust のポインタ」規約として前提にしている。
        let result: Int32 = payload.withUnsafeBytes { rawBuffer in
            let ptr = rawBuffer.bindMemory(to: UInt8.self).baseAddress
            return fandhe_dispatch_event(handle, ptr, payload.count)
        }

        guard result >= 0 else {
            throw FandheBridgeError.fromFFI(code: result, store: handle)
        }
        return result > 0
    }
}
