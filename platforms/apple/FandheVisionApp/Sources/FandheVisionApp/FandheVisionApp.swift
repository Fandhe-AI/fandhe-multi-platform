// REQ-3 / TASK-3.3-S4（Issue #68）: visionOS アプリのエントリポイント。
//
// 役割: `WindowGroup` による「1 ウィンドウ」構成の骨格を提供する（REQ-3 の visionOS 側要件）。
// PoC-2（`docs/spec/03-poc/watchos-visionos/visionos-app/Sources/main.swift`）の
// `@main` App + `WindowGroup` 構成を踏襲するが、PoC は検証用の生 FFI（`@_silgen_name`）を
// 直接呼んでいたのに対し、本実装ではアプリ層から生ポインタ・生 FFI を一切扱わず、
// `FandheSwiftUIBridge` の型安全な公開 API のみに依存する（watchOS 側 S1・Issue #65 と同方針）。
//
// `VisionScreenModel` はアプリ起動時に 1 度だけ生成し、`ContentView` へ渡す
// （画面遷移が増えた場合でも、モデルのライフサイクルはこのエントリに閉じる）。
//
// スコープ外（意図的に含めない）:
// - `ImmersiveSpace` / volumetric ウィンドウスタイル: REQ-3 の要件は平面ウィンドウ 1 つであり、
//   空間没入表示は REQ の範囲外。
// - 複数ウィンドウ・シーン: `UIApplicationSupportsMultipleScenes` は false（Info.plist 参照）。
import SwiftUI

@main
struct FandheVisionApp: App {
    private let model = VisionScreenModel()

    var body: some Scene {
        WindowGroup {
            ContentView(model: model)
        }
    }
}
