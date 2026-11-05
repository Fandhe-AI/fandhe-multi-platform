// 拡張のエントリポイント。複数機能のウィジェットを束ねるアプリ側の入口で、機能 crate には置かない
// （生成は #1176）。
// - `FandheTextWidget`: home-widget（`crates/fandhe-cap-home-widget/apple/widget/FandheTextWidget.swift`）
// - `FandheCountControl`: quick-settings-tile（同ディレクトリの `FandheCountControl.swift`、macOS 26+）
import SwiftUI
import WidgetKit

/// 拡張のエントリポイント。テキストウィジェットに加え、macOS 26+ ではカウントコントロールを出す。
/// コントロール（`FandheCountControl.swift`）は macOS 専用の構成のため `#if os(macOS)` で囲む。
@main
struct FandheWidgets: WidgetBundle {
    var body: some Widget {
        FandheTextWidget()
        #if os(macOS)
        if #available(macOS 26.0, *) {
            FandheCountControl()
        }
        #endif
    }
}
