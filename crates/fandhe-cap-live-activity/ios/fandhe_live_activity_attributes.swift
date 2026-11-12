// ライブアクティビティの属性定義（Issue #295、親 #88 の b-ios）。
// アプリ側の薄層（`fandhe_ios_live_activity.swift`、`build.rs` が静的ライブラリ化）と
// Widget Extension（`platforms/apple/FandheLiveActivity`、`project.yml` が同じファイルを取り込む）の
// 両方でコンパイルする SSOT。両者の型が食い違うと OS が表示を描画できないため、複製しない。
import ActivityKit

public struct FandheLiveActivityAttributes: ActivityAttributes {
    public struct ContentState: Codable, Hashable {
        /// 表示する値（0〜9999。範囲検証は Rust 側 `LiveValue::new` が済ませている）。
        public var value: UInt32

        public init(value: UInt32) {
            self.value = value
        }
    }

    public init() {}
}
