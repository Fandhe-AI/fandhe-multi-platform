// 端末内での画像生成（Image Playground）の Swift 薄層（Issue #975 macOS、iOS は #976）。
//
// `ImagePlaygroundViewController` は Swift 専用の型を含むため、`build.rs` の
// `compile_apple_image_generation` が swiftc で静的ライブラリ化し、Rust の
// `device/apple_image_generation.rs` が `@_cdecl` の C ABI 1 関数だけを呼ぶ。結果は Rust が渡した
// 関数ポインタ 2 本（begin・done）で返す。OS の分岐は `TARGET_OS` ではなく `canImport` と
// `#available` で行う。
//
// C ABI: fandhe_apple_image_generation_present(request_id, max_side, prompt, prompt_len, begin, done) -> Int32
//   - 戻り値 0: 受理（結果は main queue から `done` で 1 回だけ届く）。
//   - 戻り値 0 以外: 即時の失敗コードで、`begin`・`done` は呼ばない。
//   - `begin(request_id) -> Bool`: 生成画面を出す直前に main queue で呼ぶ。false なら出さない。
//   - `done(request_id, code, ptr, len, w, h)`: 画面を閉じた後に 1 要求につき 1 回だけ main queue で呼ぶ。
//     `ptr` は呼び出しの間だけ有効（RGBA8・行優先・`len == w*h*4`）。
//
// コード表（意味の SSOT は Rust の `fandhe_device::image_generation::outcome_from_apple_code`）:
//   0 生成 / 1 キャンセル / 2 非対応 / 3 表示先なし / 4 復号不可 / 5 大きすぎる / 6 前面でない（iOS 用に予約）/
//   7 その他
//
// - 寸法は復号の前に ImageIO のプロパティで読んで検査する（0 は 4、`max_side` 超は 5）。縮小して成功にしない。
// - 描画は白で塗った RGBA8 の bitmap context（premultipliedLast）へ行う。不透明が保証されるため、
//   乗算済みでも非乗算でも画素の値は変わらない。
// - `didCreateImageAt` の一時ファイルは成功・失敗・寸法超過の全経路で消す。
// - 画面は閉じてから `done` を呼ぶ。VC とデリゲートは `done` の後に解放する。`done` は `delivered` で 1 回に守る。
// - プロンプト・一時ファイルのパス・画素・エラーの内容はログへ出さない（print / NSLog を使わない）。
// - 権限・usage description は追加しない。人物の取り込み（personalization）は無効にする。

import Foundation
#if canImport(ImagePlayground) && os(macOS)
import AppKit
import CoreGraphics
import ImageIO
import ImagePlayground
#endif

public typealias FandheImageGenBegin = @convention(c) (UInt64) -> Bool
public typealias FandheImageGenDone = @convention(c) (UInt64, Int32, UnsafePointer<UInt8>?, Int, UInt32, UInt32) -> Void

private let maxPromptBytes = 256

#if canImport(ImagePlayground) && os(macOS)

/// 1 要求ぶんの提示と結果の受け渡し。デリゲートは弱参照のため `currentSession` が保持する。
@available(macOS 15.4, *)
@MainActor
private final class Session: NSObject, ImagePlaygroundViewController.Delegate {
    let requestId: UInt64
    let maxSide: Int
    let done: FandheImageGenDone
    var delivered = false
    var vc: ImagePlaygroundViewController?
    var sheet: NSWindow?
    weak var parent: NSWindow?

    init(requestId: UInt64, maxSide: Int, done: @escaping FandheImageGenDone) {
        self.requestId = requestId
        self.maxSide = maxSide
        self.done = done
    }

    /// 提示する。`begin` が false なら何もしない（Rust 側で既に確定している）。
    func present(prompt: String, begin: FandheImageGenBegin) {
        guard let parent = NSApplication.shared.keyWindow
            ?? NSApplication.shared.mainWindow
            ?? NSApplication.shared.windows.first
        else {
            deliver(code: 3)
            return
        }
        guard begin(requestId) else { return }
        let vc = ImagePlaygroundViewController()
        vc.concepts = [.text(prompt)]
        vc.allowedGenerationStyles = [.animation, .illustration, .sketch]
        vc.selectedGenerationStyle = .illustration
        // ponytail: 26.4 で非推奨（代替は options の Personalization）。15.4 以降で動く唯一の経路のため警告を許容する。
        vc.personalizationPolicy = .disabled
        vc.delegate = self
        self.vc = vc
        self.parent = parent
        // ponytail: winit の窓は contentViewController を持たない。`presentAsSheet` は使わず、
        // 窓を作って親窓の beginSheet で出す 1 経路にする。Mac 目視（#966）で出せなければ `no` へ下げる。
        let sheet = NSWindow(contentViewController: vc)
        self.sheet = sheet
        parent.beginSheet(sheet, completionHandler: nil)
    }

    func imagePlaygroundViewController(
        _ imagePlaygroundViewController: ImagePlaygroundViewController,
        didCreateImageAt imageURL: URL
    ) {
        let (code, pixels, w, h) = decode(url: imageURL)
        // 結果にかかわらず一時ファイルを消す。消せなくても通知はする。
        try? FileManager.default.removeItem(at: imageURL)
        deliver(code: code, pixels: pixels, width: w, height: h)
    }

    func imagePlaygroundViewControllerDidCancel(_ imagePlaygroundViewController: ImagePlaygroundViewController) {
        deliver(code: 1)
    }

    /// 通知せずに閉じる（新しい要求に置き換わるとき）。古い要求は Rust 側で既に無効。
    func abandon() {
        if delivered { return }
        delivered = true
        close()
        release()
    }

    /// 画面を閉じてから `done` を 1 回だけ呼び、保持を解く。
    private func deliver(code: Int32, pixels: [UInt8]? = nil, width: UInt32 = 0, height: UInt32 = 0) {
        if delivered { return }
        delivered = true
        close()
        if let pixels = pixels {
            pixels.withUnsafeBufferPointer { buf in
                done(requestId, code, buf.baseAddress, buf.count, width, height)
            }
        } else {
            done(requestId, code, nil, 0, 0, 0)
        }
        release()
    }

    private func close() {
        if let sheet = sheet, let parent = parent {
            parent.endSheet(sheet)
        }
        sheet?.contentViewController = nil
        sheet?.orderOut(nil)
    }

    private func release() {
        vc?.delegate = nil
        vc = nil
        sheet = nil
        parent = nil
        if currentSession === self { currentSession = nil }
    }

    /// 寸法を先に検査してから RGBA8 へ描く。失敗は (コード, nil, 0, 0)。
    private func decode(url: URL) -> (Int32, [UInt8]?, UInt32, UInt32) {
        guard let src = CGImageSourceCreateWithURL(url as CFURL, nil),
              let props = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any],
              let w = (props[kCGImagePropertyPixelWidth] as? NSNumber)?.intValue,
              let h = (props[kCGImagePropertyPixelHeight] as? NSNumber)?.intValue,
              w > 0, h > 0
        else { return (4, nil, 0, 0) }
        if w > maxSide || h > maxSide { return (5, nil, 0, 0) }
        guard let image = CGImageSourceCreateImageAtIndex(src, 0, nil) else { return (4, nil, 0, 0) }
        var buf = [UInt8](repeating: 0, count: w * h * 4)
        let ok = buf.withUnsafeMutableBytes { raw -> Bool in
            guard let ctx = CGContext(
                data: raw.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            ) else { return false }
            let rect = CGRect(x: 0, y: 0, width: w, height: h)
            ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
            ctx.fill(rect)
            ctx.draw(image, in: rect)
            return true
        }
        return ok ? (0, buf, UInt32(w), UInt32(h)) : (4, nil, 0, 0)
    }
}

/// 進行中の要求（1 本）。`Session` を強参照で保持する（デリゲートは弱参照のため）。
@MainActor private var currentSession: AnyObject?

#endif

@_cdecl("fandhe_apple_image_generation_present")
public func fandhe_apple_image_generation_present(
    _ requestId: UInt64,
    _ maxSide: UInt32,
    _ prompt: UnsafePointer<UInt8>?,
    _ promptLen: Int,
    _ begin: FandheImageGenBegin,
    _ done: FandheImageGenDone
) -> Int32 {
    guard maxSide > 0, let prompt = prompt, promptLen > 0, promptLen <= maxPromptBytes,
          let text = String(data: Data(bytes: prompt, count: promptLen), encoding: .utf8)
    else { return 7 }
    #if canImport(ImagePlayground) && os(macOS)
    if #available(macOS 15.4, *) {
        if !ImagePlaygroundViewController.isAvailable { return 2 }
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                (currentSession as? Session)?.abandon()
                let session = Session(requestId: requestId, maxSide: Int(maxSide), done: done)
                currentSession = session
                session.present(prompt: text, begin: begin)
                // 表示せずに終えた（親窓なし・begin が false）場合は保持を解く。
                if session.vc == nil { currentSession = nil }
            }
        }
        return 0
    }
    #endif
    // 非対応（OS が古い・SDK に ImagePlayground が無い・iOS は #976 が実装する）。
    _ = (requestId, text, begin, done)
    return 2
}
