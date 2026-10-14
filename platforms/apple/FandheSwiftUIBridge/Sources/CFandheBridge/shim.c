// SPM の C ターゲットは最低 1 個の実装ファイルを要求するための空実装。
// 実体（`extern "C"` 関数群）は `crates/fandhe-swiftui-bridge`（S2: #59）が生成する
// staticlib 側にあり、本ターゲットは `include/fandhe_bridge.h` の宣言のみを
// Clang importer 経由で Swift 側（`FandheSwiftUIBridge` ターゲット）へ橋渡しする
// ヘッダオンリーターゲットである。staticlib とのリンクは、`Package.swift` が
// `Artifacts/FandheBridge.xcframework`（`scripts/apple/build-bridge-xcframework.sh` が
// 生成、TASK-3.4-S2 / #72）を `FandheBridgeBinary` という binaryTarget として条件付きに
// 取り込み、本ターゲット（`CFandheBridge`）の依存へ配線することで完成する
