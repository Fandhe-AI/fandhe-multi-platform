// 端末内 LLM（FoundationModels）の Swift 薄層（Issue #345 macOS、Issue #346 iOS）。
//
// `LanguageModelSession` は Swift 専用 API で ObjC から呼べないため、`build.rs` の
// `compile_apple_on_device_llm` が swiftc で静的ライブラリ化し、Rust の
// `device/apple_on_device_llm.rs` が `@_cdecl` の C ABI 1 関数だけを呼ぶ。macOS と iOS で共用し、
// OS の分岐は `TARGET_OS` ではなく `canImport` と `#available` で行う。
//
// 契約（戻りコードの意味の SSOT は Rust 側の `match`）:
//   0 成功 / 1 非対応 / 2 Apple Intelligence が無効 / 3 モデル未取得 / 4 応答拒否 /
//   5 混雑・回数制限（前の生成が残っている場合を含む）/ 6 期限切れ / 7 その他 /
//   8 プロンプトが UTF-8 でない、または長すぎる
//
// - プロンプト長の SSOT は Rust の `validate_prompt`。ここの上限（4096 バイト）は Rust の検査を経ない
//   経路での大量コピーを防ぐ最後の砦で、`Data` へコピーする前に検査する。
// - 実行中の生成はプロセス内で 1 本に制限する。期限切れで戻っても Task は走り切るまで枠を持つため、
//   再要求は 5（混雑）になり、Task が積み上がるのは最大 1 本。
//
// - ポインタは呼び出しの間だけ参照する。Swift の Task へは持ち込まず、コピーした String だけを渡す。
// - 呼び出し元のワーカーは `deadline_ms` だけ待つ。期限切れで戻った後に届いた完了は 1 回きりの
//   箱だけを触り、Rust のバッファには書かない。
// - `dispatch_sync` と main queue は使わない（呼び出し元は専用ワーカー）。
// - プロンプト・応答・エラーの内容はログへ出さない。

import Foundation
#if canImport(FoundationModels)
import FoundationModels
#endif

/// 完了を 1 回だけ受け取る箱。Task とワーカーが共有するのはこれだけ。
private final class ResultBox: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false
    private var code: Int32 = 7
    private var text = ""

    func set(code: Int32, text: String) {
        lock.lock()
        defer { lock.unlock() }
        if done { return }
        done = true
        self.code = code
        self.text = text
    }

    func take() -> (Int32, String) {
        lock.lock()
        defer { lock.unlock() }
        return (code, text)
    }
}

/// 実行中の生成を 1 本に制限する枠。判定と更新は同じロックの内側で行う。
private final class InflightGate: @unchecked Sendable {
    private let lock = NSLock()
    private var busy = false

    func tryAcquire() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if busy { return false }
        busy = true
        return true
    }

    func release() {
        lock.lock()
        defer { lock.unlock() }
        busy = false
    }
}

private let inflightGate = InflightGate()
private let maxPromptBytes = 4096

#if canImport(FoundationModels)
@available(macOS 26.0, iOS 26.0, *)
private func respondImpl(prompt: String, deadlineMs: UInt64, maxTokens: Int32) -> (Int32, String) {
    // 使える状態でなければセッションを作らず、モデルのダウンロードも始めない。
    switch SystemLanguageModel.default.availability {
    case .available:
        break
    case .unavailable(let reason):
        switch reason {
        case .deviceNotEligible: return (1, "")
        case .appleIntelligenceNotEnabled: return (2, "")
        case .modelNotReady: return (3, "")
        @unknown default: return (7, "")
        }
    @unknown default:
        return (7, "")
    }

    // 利用不可の早期 return では枠を取らない。Task 側の defer が全経路で解放する。
    guard inflightGate.tryAcquire() else { return (5, "") }
    let box = ResultBox()
    let sem = DispatchSemaphore(value: 0)
    let task = Task.detached {
        // 枠の解放 → signal の順（結果を届ける前に状態を確定する）。
        defer {
            inflightGate.release()
            sem.signal()
        }
        do {
            let session = LanguageModelSession()
            let options = GenerationOptions(maximumResponseTokens: Int(maxTokens))
            let r = try await session.respond(to: prompt, options: options)
            box.set(code: 0, text: r.content)
        } catch let e as LanguageModelSession.GenerationError {
            switch e {
            case .assetsUnavailable: box.set(code: 3, text: "")
            case .guardrailViolation, .refusal: box.set(code: 4, text: "")
            case .rateLimited, .concurrentRequests: box.set(code: 5, text: "")
            default: box.set(code: 7, text: "")
            }
        } catch {
            box.set(code: 7, text: "")
        }
    }
    let waitMs = Int(min(deadlineMs, UInt64(Int32.max)))
    if sem.wait(timeout: .now() + .milliseconds(waitMs)) == .timedOut {
        task.cancel()
        return (6, "")
    }
    return box.take()
}
#endif

@_cdecl("fandhe_apple_on_device_llm_respond")
public func fandhe_apple_on_device_llm_respond(
    _ prompt: UnsafePointer<UInt8>?,
    _ len: Int,
    _ deadlineMs: UInt64,
    _ maxTokens: Int32,
    _ out: UnsafeMutablePointer<UInt8>?,
    _ cap: Int,
    _ outLen: UnsafeMutablePointer<Int>?
) -> Int32 {
    guard let prompt = prompt, let out = out, let outLen = outLen, len >= 0, cap > 0 else { return 7 }
    outLen.pointee = 0
    guard len <= maxPromptBytes else { return 8 }
    let data = Data(bytes: prompt, count: len)
    guard let text = String(data: data, encoding: .utf8) else { return 8 }

    #if canImport(FoundationModels)
    guard #available(macOS 26.0, iOS 26.0, *) else { return 1 }
    let (code, response) = respondImpl(prompt: text, deadlineMs: deadlineMs, maxTokens: maxTokens)
    if code != 0 { return code }
    // ponytail: 末尾から 1 文字ずつ削る素朴な切り詰め。応答は最大 256 トークンで小さいため。長くするなら二分探索へ変える。
    var s = response
    while s.utf8.count > cap { s.removeLast() }
    let bytes = Array(s.utf8)
    bytes.withUnsafeBufferPointer { buf in
        if let base = buf.baseAddress { out.update(from: base, count: bytes.count) }
    }
    outLen.pointee = bytes.count
    return 0
    #else
    return 1
    #endif
}
