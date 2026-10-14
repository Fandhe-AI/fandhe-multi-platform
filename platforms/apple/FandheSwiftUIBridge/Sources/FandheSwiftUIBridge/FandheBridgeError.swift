import CFandheBridge

/// `fandhe-swiftui-bridge` の FFI 呼び出しが失敗した際に、SwiftUI 側（TASK-3.3 の
/// 画面実装）へ投げる Swift Error。`i32` エラーコード（`FandheBridgeContract.ErrorCode`）と、
/// `fandhe_last_error_message` から同一呼び出しフレーム内で同期的に取得した詳細メッセージを
/// 保持する（設計文書 §3.3「エラー伝播」: スレッドローカル参照を含むため取得を遅延させない）。
public struct FandheBridgeError: Error, CustomStringConvertible {
    /// FFI 戻り値由来のエラー種別。
    public let code: FandheBridgeContract.ErrorCode
    /// `fandhe_last_error_message` から取得した詳細メッセージ。
    /// 取得時点でエラーが記録されていなかった場合（空バッファ）は `nil` ではなく
    /// 判別可能な既定文言を入れる（呼び出し側が「詳細なし」を error 扱いしないため）。
    public let detail: String

    public var description: String {
        "FandheBridgeError(code: \(code), detail: \(detail))"
    }
}

extension FandheBridgeError {
    /// 生の `i32` 戻り値と、対応する `store`（詳細メッセージの取得元。ハンドルが
    /// まだ存在しない場合は `nil`）から `FandheBridgeError` を組み立てる。
    ///
    /// `fandhe_last_error_message` はハンドル非依存呼び出し（`store` が NULL）でも
    /// スレッドローカルの最終エラーへフォールバックする契約（設計文書 §3.3）のため、
    /// `store` が `nil` の場合は NULL ポインタを渡してそのまま呼び出す。
    static func fromFFI(code rawCode: Int32, store: OpaquePointer?) -> FandheBridgeError {
        let errorCode = FandheBridgeContract.ErrorCode(rawCode: rawCode)
        let rawBuffer = fandhe_last_error_message(store)
        let detail = FandheBufferBridge.consumingIntoString(rawBuffer) ?? "詳細なし"
        return FandheBridgeError(code: errorCode, detail: detail)
    }
}
