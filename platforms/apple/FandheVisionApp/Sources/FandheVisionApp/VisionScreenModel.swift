import Foundation
import FandheSwiftUIBridge
import Observation

/// visionOS 画面（1 ウィンドウ＋1 操作要素）の状態保持モデル（REQ-3 / TASK-3.3-S4、Issue #68）。
///
/// `FandheStore`（`FandheSwiftUIBridge` が公開する型安全ラッパ、TASK-3.2-S5 / Issue #62）を
/// 生成・保持し、`fandhe_core::sample_screen()` の描画スナップショットから
/// 「最初のテキストノードの文言」と「最初のボタンノードのラベル」を導出して `ContentView` へ渡す。
/// ボタンタップ時は `activateElement()` を通じて `FandheEvent.buttonClicked` をディスパッチし、
/// 再描画が必要であれば表示値を更新する。
///
/// `ContentView` は表示専任とし、状態の生成・スナップショット取得・イベントディスパッチは
/// 本モデルに閉じ込める（watchOS 側 `WatchScreenModel`、Issue #65 と同じ責務分割を踏襲する）。
@Observable
final class VisionScreenModel {
    /// 画面に表示するラベル文言。取得成功時は Rust コアのスナップショット由来、
    /// 失敗時は日本語のフォールバック文言（エラーを UI 契約にしない、coding-rust.md 準拠）。
    private(set) var labelText: String

    /// 操作要素（ボタン）のラベル。スナップショットに `.button` ノードが含まれない、
    /// またはスナップショット取得自体に失敗した場合は nil とし、`ContentView` 側で
    /// ボタンを無効化表示させる（存在しない操作を押せる状態にしない）。
    private(set) var buttonLabel: String?

    /// `VisionScreenModel` がフォールバック表示に切り替えた理由（#69 の起動確認での切り分け用）。
    /// 成功時は nil。
    private(set) var lastError: VisionScreenModelError?

    /// `FandheStore` の生成に失敗した場合でも画面自体はクラッシュさせずフォールバック表示する
    /// （REQ-3 の受け入れ基準はまず「画面が起動し要素が出ること」であり、
    /// スナップショット取得・イベントディスパッチはベストエフォートとして扱う）。
    private let store: FandheStore?

    init() {
        do {
            let store = try FandheStore()
            self.store = store
            do {
                let snapshot = try store.renderSnapshot()
                self.labelText = Self.firstText(in: snapshot.nodes) ?? Self.fallbackText
                self.buttonLabel = Self.firstButtonLabel(in: snapshot.nodes)
                self.lastError = self.buttonLabel == nil ? .noButtonNodeInSnapshot : nil
            } catch {
                self.labelText = Self.fallbackText
                self.buttonLabel = nil
                self.lastError = .snapshotDecodeFailed(error)
            }
        } catch {
            self.store = nil
            self.labelText = Self.fallbackText
            self.buttonLabel = nil
            self.lastError = .storeInitializationFailed(error)
        }
    }

    /// 操作要素（ボタン）を活性化する。`buttonLabel` が確定している場合のみ
    /// `FandheEvent.buttonClicked(label:)` をディスパッチし、戻り値（再描画要否）が
    /// true であればスナップショットを再取得して表示値を更新する。
    ///
    /// `store` が nil（初期化失敗）または `buttonLabel` が nil（ボタン不在）の場合は
    /// 何もしない（`ContentView` 側でボタンが無効化されている前提だが、二重の安全策として
    /// ここでも guard する）。
    func activateElement() {
        guard let store, let buttonLabel else { return }
        do {
            let shouldRefresh = try store.dispatch(.buttonClicked(label: buttonLabel))
            guard shouldRefresh else { return }
            let snapshot = try store.renderSnapshot()
            self.labelText = Self.firstText(in: snapshot.nodes) ?? Self.fallbackText
            self.buttonLabel = Self.firstButtonLabel(in: snapshot.nodes)
            self.lastError = self.buttonLabel == nil ? .noButtonNodeInSnapshot : nil
        } catch {
            // ディスパッチ・再取得の失敗は表示済みの値を維持したまま要因のみ記録する
            // （ボタン操作の失敗で画面全体をフォールバック表示に戻すと、直前まで
            // 見えていた内容が消え体験を悪化させるため）。
            self.lastError = .dispatchFailed(error)
        }
    }

    /// スナップショット取得・デコードに失敗した場合に表示する日本語フォールバック文言。
    /// クラッシュさせず、ユーザーには最小限「表示できない」ことのみを伝える
    /// （詳細は `lastError` 側に保持し、UI 本文には含めない。A05 対策）。
    private static let fallbackText = "画面を表示できません"

    /// スナップショットの `nodes` から最初の `.text` ノードの文言を取り出す。
    private static func firstText(in nodes: [FandheRenderNode]) -> String? {
        for node in nodes {
            if case let .text(text) = node {
                return text
            }
        }
        return nil
    }

    /// スナップショットの `nodes` から最初の `.button` ノードのラベルを取り出す。
    /// `sample_screen()`（`fandhe-core`）の構成（`crates/fandhe-core/src/ui/sample.rs`）に
    /// ボタンが含まれることを前提とするが、含まれない場合は nil を返し呼び出し側で
    /// フォールバック扱いとする。
    private static func firstButtonLabel(in nodes: [FandheRenderNode]) -> String? {
        for node in nodes {
            if case let .button(label) = node {
                return label
            }
        }
        return nil
    }
}

/// `VisionScreenModel` がフォールバック表示に切り替えた、または操作に失敗した理由
/// （#69 のシミュレータ起動確認での切り分け用）。
enum VisionScreenModelError: Error {
    /// `FandheStore` の生成（Rust ⇔ Swift のハンドル確立）自体に失敗した。
    case storeInitializationFailed(Error)
    /// スナップショットの取得・JSON デコードに失敗した。
    case snapshotDecodeFailed(Error)
    /// スナップショットは取得できたが、`.button` ノードが 1 つも含まれていなかった。
    case noButtonNodeInSnapshot
    /// ボタン活性化時のイベントディスパッチ・再スナップショット取得に失敗した。
    case dispatchFailed(Error)
}
