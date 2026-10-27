// 端末内での画像生成（Image Playground）の Swift 薄層（Issue #975 macOS、#976 iOS）。
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
//   0 生成 / 1 キャンセル / 2 非対応 / 3 表示先なし / 4 復号不可 / 5 大きすぎる / 6 前面でない（iOS のみ）/
//   7 その他
//
// 提示方法:
//   - macOS: 親窓の beginSheet。
//   - iOS: UIKit で最前面の VC から present する。アプリが background なら 6、提示元が無い・他の画面を
//     出している最中なら 3、提示に失敗したら 7。Inactive（権限ダイアログ・通知センター）は中断扱いにしない。
//     スワイプで閉じた場合は `presentationControllerDidDismiss` でキャンセル（1）にする。
//     画面は dismiss が終わってから `done` を呼ぶ（契約「画面を閉じてから done」）。
//
// - 寸法は復号の前に ImageIO のプロパティで読んで検査する（0 は 4、`max_side` 超は 5）。縮小して成功にしない。
// - 描画は白で塗った RGBA8 の bitmap context（premultipliedLast）へ行う。不透明が保証されるため、
//   乗算済みでも非乗算でも画素の値は変わらない。
// - `didCreateImageAt` の一時ファイルは成功・失敗・寸法超過の全経路で消す。
// - 画面は閉じてから `done` を呼ぶ。VC とデリゲートは `done` の後に解放する。`done` は `delivered` で 1 回に守る。
// - プロンプト・一時ファイルのパス・画素・エラーの内容はログへ出さない（print / NSLog を使わない）。
// - 権限・usage description は追加しない。人物の取り込み（personalization）は無効にする。

import Foundation
#if canImport(ImagePlayground) && (os(macOS) || os(iOS))
#if os(macOS)
import AppKit
#else
import UIKit
#endif
import CoreGraphics
import ImageIO
import ImagePlayground
#endif

public typealias FandheImageGenBegin = @convention(c) (UInt64) -> Bool
public typealias FandheImageGenDone = @convention(c) (UInt64, Int32, UnsafePointer<UInt8>?, Int, UInt32, UInt32) -> Void

private let maxPromptBytes = 256

#if canImport(ImagePlayground) && (os(macOS) || os(iOS))

/// 1 要求ぶんの提示と結果の受け渡し。デリゲートは弱参照のため `currentSession` が保持する。
@available(macOS 15.4, iOS 18.4, *)
@MainActor
private final class Session: NSObject, ImagePlaygroundViewController.Delegate {
    let requestId: UInt64
    let maxSide: Int
    let done: FandheImageGenDone
    var delivered = false
    var vc: ImagePlaygroundViewController?
    #if os(macOS)
    var sheet: NSWindow?
    weak var parent: NSWindow?
    #endif

    init(requestId: UInt64, maxSide: Int, done: @escaping FandheImageGenDone) {
        self.requestId = requestId
        self.maxSide = maxSide
        self.done = done
    }

    #if os(macOS)
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
        // 閉じるボタン・Esc などでシートが終わっても DidCancel は届かない。終了を観測してキャンセルを確定する。
        // 通常経路は `deliver` が先に `delivered` を立てるため、ここは 2 回目として無視される。
        parent.beginSheet(sheet) { [weak self] _ in
            MainActor.assumeIsolated { self?.deliver(code: 1) }
        }
    }
    #else
    /// 提示猶予（秒）。提示を拒まれると completion が来ず Session が残るため、この時間で出ていなければ 7 にする。
    private static let presentGrace = 5.0

    /// 提示する。`begin` が false なら何もしない（Rust 側で既に確定している）。
    func present(prompt: String, begin: FandheImageGenBegin) {
        // Inactive は権限ダイアログ・通知センターでも起きるため中断扱いにしない。background だけ 6 にする。
        if UIApplication.shared.applicationState == .background {
            deliver(code: 6)
            return
        }
        guard let top = topmostViewController(), top.view.window != nil, !top.isBeingPresented,
              !top.isBeingDismissed
        else {
            deliver(code: 3)
            return
        }
        guard begin(requestId) else { return }
        let vc = ImagePlaygroundViewController()
        vc.concepts = [.text(prompt)]
        vc.allowedGenerationStyles = [.animation, .illustration, .sketch]
        vc.selectedGenerationStyle = .illustration
        // ponytail: 非推奨になる OS がある。18.4 以降で動く唯一の経路のため警告を許容する。
        vc.personalizationPolicy = .disabled
        vc.delegate = self
        vc.presentationController?.delegate = self
        self.vc = vc
        top.present(vc, animated: true) { [weak self] in
            MainActor.assumeIsolated {
                guard let self = self else { return }
                if self.delivered {
                    vc.presentingViewController?.dismiss(animated: false)
                } else if vc.presentingViewController == nil {
                    self.deliver(code: 7)
                }
            }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + Session.presentGrace) { [weak self] in
            MainActor.assumeIsolated {
                if let self = self, !self.delivered, self.vc?.presentingViewController == nil {
                    self.deliver(code: 7)
                }
            }
        }
    }
    #endif

    func imagePlaygroundViewController(
        _ imagePlaygroundViewController: ImagePlaygroundViewController,
        didCreateImageAt imageURL: URL
    ) {
        let (code, pixels, w, h) = decode(url: imageURL)
        // 結果にかかわらず一時ファイルを消す。消せなかったときは成功にせず、URL を保持して次回再削除する。
        if removeTemp(imageURL) {
            deliver(code: code, pixels: pixels, width: w, height: h)
        } else {
            deliver(code: 7)
        }
    }

    /// 前回消せなかった一時ファイルを再削除し、`url` も消す。`url` を消せなければ false（URL は保持）。
    private func removeTemp(_ url: URL) -> Bool {
        pendingTempURLs.append(url)
        pendingTempURLs = pendingTempURLs.filter { u in
            do { try FileManager.default.removeItem(at: u); return false } catch {
                // 既に無いなら済み。
                return FileManager.default.fileExists(atPath: u.path)
            }
        }
        return !pendingTempURLs.contains(url)
    }

    func imagePlaygroundViewControllerDidCancel(_ imagePlaygroundViewController: ImagePlaygroundViewController) {
        deliver(code: 1)
    }

    /// 通知せずに閉じる（新しい要求に置き換わるとき）。古い要求は Rust 側で既に無効。
    func abandon() {
        if delivered { return }
        delivered = true
        #if os(macOS)
        close()
        #else
        vc?.presentingViewController?.dismiss(animated: false)
        #endif
        release()
    }

    /// 画面を閉じてから `done` を 1 回だけ呼び、保持を解く。
    private func deliver(code: Int32, pixels: [UInt8]? = nil, width: UInt32 = 0, height: UInt32 = 0) {
        if delivered { return }
        delivered = true
        // 画面を閉じ終えてから通知する。画素は値で捕まえ、呼び出しの間だけ有効なポインタで渡す。
        closeThen { [self] in
            if let pixels = pixels {
                pixels.withUnsafeBufferPointer { buf in
                    done(requestId, code, buf.baseAddress, buf.count, width, height)
                }
            } else {
                done(requestId, code, nil, 0, 0, 0)
            }
            release()
        }
    }

    #if os(iOS)
    /// 出ている VC を dismiss し、終わってから `then` を呼ぶ。出ていなければすぐ呼ぶ。
    private func closeThen(_ then: @escaping () -> Void) {
        if let presenter = vc?.presentingViewController {
            presenter.dismiss(animated: true) { MainActor.assumeIsolated { then() } }
        } else {
            then()
        }
    }
    #else
    private func closeThen(_ then: @escaping () -> Void) {
        close()
        then()
    }

    private func close() {
        if let sheet = sheet, let parent = parent {
            parent.endSheet(sheet)
        }
        sheet?.contentViewController = nil
        sheet?.orderOut(nil)
    }
    #endif

    private func release() {
        vc?.delegate = nil
        vc = nil
        #if os(macOS)
        sheet = nil
        parent = nil
        #endif
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

#if os(iOS)
@available(iOS 18.4, *)
extension Session: UIAdaptivePresentationControllerDelegate {
    /// スワイプで閉じられた。既に閉じているため `closeThen` はすぐ `done` を呼ぶ。
    func presentationControllerDidDismiss(_ presentationController: UIPresentationController) {
        deliver(code: 1)
    }
}

/// 最前面の VC。`ios/fandhe_ios_common.h` の `FandheTopmostViewController` と同じ規則を写したもの
/// （ObjC ヘッダは単一ファイルの swiftc から import できないため）。
@MainActor
private func topmostViewController() -> UIViewController? {
    var root: UIViewController?
    for case let scene as UIWindowScene in UIApplication.shared.connectedScenes
    where scene.activationState == .foregroundActive {
        root = scene.windows.first(where: { $0.isKeyWindow })?.rootViewController
        if root != nil { break }
    }
    if root == nil {
        // Scene 接続前や scene 外の window 用のフォールバック（deprecated だが共通ヘッダも同じ）。
        root = UIApplication.shared.windows.first(where: { $0.isKeyWindow })?.rootViewController
    }
    var top = root
    while let next = top?.presentedViewController { top = next }
    return top
}
#endif

/// 進行中の要求（1 本）。`Session` を強参照で保持する（デリゲートは弱参照のため）。
@MainActor private var currentSession: AnyObject?

/// 削除に失敗した一時画像。次の結果通知で再削除する（パスはログへ出さない）。
@MainActor private var pendingTempURLs: [URL] = []

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
    #if canImport(ImagePlayground) && (os(macOS) || os(iOS))
    if #available(macOS 15.4, iOS 18.4, *) {
        if !ImagePlaygroundViewController.isAvailable { return 2 }
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                (currentSession as? Session)?.abandon()
                let session = Session(requestId: requestId, maxSide: Int(maxSide), done: done)
                currentSession = session
                session.present(prompt: text, begin: begin)
                // 表示せずに終えた（提示元なし・background・begin が false）場合は保持を解く。
                if session.vc == nil { currentSession = nil }
            }
        }
        return 0
    }
    #endif
    // 非対応（OS が古い・SDK に ImagePlayground が無い）。
    _ = (requestId, text, begin, done)
    return 2
}
