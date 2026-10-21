// 端末内翻訳（Translation フレームワーク）の Swift 薄層（Issue #535 macOS、iOS は #536 が差し替える）。
//
// `TranslationSession` は Swift 専用 API で ObjC から呼べないため、`build.rs` の
// `compile_apple_translation` が swiftc で静的ライブラリ化し、Rust の
// `device/apple_translation.rs` が `@_cdecl` の C ABI 1 関数だけを呼ぶ。macOS と iOS で共用し、
// OS の分岐は `TARGET_OS` ではなく `canImport` と `#available` で行う。
// 26 専用 API（`TranslationSession(installedSource:target:)`）は旧 SDK（Xcode 16 は Swift 6.1 以下）に
// 無く `#available` では型検査を除外できないため、`compiler(>=6.2)`（Xcode 26 以降）でも囲み、
// 旧 SDK では 1（非対応）経路にする。
//
// 契約（戻りコードの意味の SSOT は Rust 側の `match`）:
//   0 成功 / 1 非対応 / 2 言語の組み合わせ非対応 / 3 言語パック未導入・取得中 / 4 期限切れ /
//   5 その他（出力が出力バッファを超える場合を含む。切り詰めない）/
//   6 入力が UTF-8 でない、または長すぎる
//
// - 原文・言語タグの長さの SSOT は Rust の `validate_request`。ここの上限は Rust の検査を経ない
//   経路での大量コピーを防ぐ最後の砦で、`Data` へコピーする前に検査する。
// - 言語パックのダウンロードは始めない。`.installed` のときだけセッションを作る。
// - 期限切れでは `session.cancel()` と `task.cancel()` を済ませてから 4 を返す。セッションの登録と
//   取り消しは同じロックで排他にし、取り消し後に始まった翻訳が残らないようにする。
//   ponytail: 実行中の本数は制限しない。期限切れの直後に何度も押すと Task が一時的に重なるが、最大で期限 1 回分。
// - `LanguageAvailability.status` は main の runloop が回っていないと返らない（実測）。実アプリでは
//   winit が runloop を回すため問題ないが、runloop の無い実行体（cargo test 等）では期限切れ（4）になる。
//   main thread はこの関数を呼ばない（呼び出し元は専用ワーカー）ので、待ち合わせでデッドロックしない。
// - ポインタは呼び出しの間だけ参照する。Swift の Task へは持ち込まず、コピーした String だけを渡す。
// - 呼び出し元のワーカーは `deadline_ms` だけ待つ。期限切れで戻った後に届いた完了は 1 回きりの
//   箱だけを触り、Rust のバッファには書かない。
// - `dispatch_sync` と main queue は使わない（呼び出し元は専用ワーカー）。
// - 原文・訳文・エラーの内容はログへ出さない。

import Foundation
#if canImport(Translation) && compiler(>=6.2)
import Translation
#endif

/// 完了を 1 回だけ受け取る箱。Task とワーカーが共有するのはこれだけ。
private final class ResultBox: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false
    private var code: Int32 = 5
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

private let maxTextBytes = 4096
private let maxTagBytes = 64

#if canImport(Translation) && compiler(>=6.2)
/// 取り消しと開始を排他にする箱（セッション登録前に取り消されたら、登録時に即 `cancel()` する）。
@available(macOS 26.0, iOS 26.0, *)
private final class SessionHolder: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false
    private var session: TranslationSession?

    func register(_ s: TranslationSession) {
        lock.lock()
        defer { lock.unlock() }
        session = s
        if cancelled { s.cancel() }
    }

    func cancel() {
        lock.lock()
        defer { lock.unlock() }
        cancelled = true
        session?.cancel()
    }
}

@available(macOS 26.0, iOS 26.0, *)
private func translateImpl(text: String, source: String, target: String, deadlineMs: UInt64) -> (Int32, String) {
    let box = ResultBox()
    let holder = SessionHolder()
    let sem = DispatchSemaphore(value: 0)
    let task = Task.detached {
        defer { sem.signal() }
        let src = Locale.Language(identifier: source)
        let dst = Locale.Language(identifier: target)
        // 未導入ならセッションを作らず、言語パックのダウンロードも始めない。
        let status = await LanguageAvailability().status(from: src, to: dst)
        switch status {
        case .installed:
            break
        case .supported:
            box.set(code: 3, text: "")
            return
        case .unsupported:
            box.set(code: 2, text: "")
            return
        @unknown default:
            box.set(code: 5, text: "")
            return
        }
        do {
            let session = TranslationSession(installedSource: src, target: dst)
            holder.register(session)
            let r = try await session.translate(text)
            box.set(code: 0, text: r.targetText)
        } catch {
            // 同じ意味のコードをすべて拾う。
            if TranslationError.unsupportedSourceLanguage ~= error
                || TranslationError.unsupportedTargetLanguage ~= error
                || TranslationError.unsupportedLanguagePairing ~= error
            {
                box.set(code: 2, text: "")
            } else if TranslationError.notInstalled ~= error {
                box.set(code: 3, text: "")
            } else {
                box.set(code: 5, text: "")
            }
        }
    }
    let waitMs = Int(min(deadlineMs, UInt64(Int32.max)))
    if sem.wait(timeout: .now() + .milliseconds(waitMs)) == .timedOut {
        holder.cancel()
        task.cancel()
        return (4, "")
    }
    return box.take()
}
#endif

@_cdecl("fandhe_apple_translation_translate")
public func fandhe_apple_translation_translate(
    _ text: UnsafePointer<UInt8>?,
    _ len: Int,
    _ src: UnsafePointer<UInt8>?,
    _ srcLen: Int,
    _ dst: UnsafePointer<UInt8>?,
    _ dstLen: Int,
    _ deadlineMs: UInt64,
    _ out: UnsafeMutablePointer<UInt8>?,
    _ cap: Int,
    _ outLen: UnsafeMutablePointer<Int>?
) -> Int32 {
    guard let text = text, let src = src, let dst = dst, let out = out, let outLen = outLen,
        len >= 0, srcLen >= 0, dstLen >= 0, cap > 0
    else { return 5 }
    outLen.pointee = 0
    guard len <= maxTextBytes, srcLen <= maxTagBytes, dstLen <= maxTagBytes else { return 6 }
    guard let t = String(data: Data(bytes: text, count: len), encoding: .utf8),
        let s = String(data: Data(bytes: src, count: srcLen), encoding: .utf8),
        let d = String(data: Data(bytes: dst, count: dstLen), encoding: .utf8)
    else { return 6 }

    #if canImport(Translation) && compiler(>=6.2)
    guard #available(macOS 26.0, iOS 26.0, *) else { return 1 }
    let (code, translated) = translateImpl(text: t, source: s, target: d, deadlineMs: deadlineMs)
    if code != 0 { return code }
    // 出力が cap を超えるときは切り詰めず失敗にする（部分的な訳文を成功として出さない）。
    let bytes = Array(translated.utf8)
    guard bytes.count <= cap else { return 5 }
    bytes.withUnsafeBufferPointer { buf in
        if let base = buf.baseAddress { out.update(from: base, count: bytes.count) }
    }
    outLen.pointee = bytes.count
    return 0
    #else
    return 1
    #endif
}
