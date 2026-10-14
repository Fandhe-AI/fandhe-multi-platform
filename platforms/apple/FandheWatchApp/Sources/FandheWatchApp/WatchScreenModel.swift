import Foundation
import FandheSwiftUIBridge
import Observation

/// watchOS 最小画面の状態保持モデル（REQ-3 / TASK-3.3-S1 → S2、Issue #65 → #66）。
///
/// `FandheStore`（`FandheSwiftUIBridge` が公開する型安全ラッパ、TASK-3.2-S5 / Issue #62）を
/// 生成・保持し、`fandhe_core::sample_screen()` の描画スナップショットから
/// 「最初のテキストノードの文言」と「最初のボタンノードのラベル」を導出して
/// `ContentView` へ渡す。加えてボタン操作 → イベントディスパッチ → スナップショット再取得
/// という往復のブリッジ接続を担う。
///
/// `ContentView` は表示専任とし、状態の生成・スナップショット取得・イベントディスパッチは
/// 本モデルに閉じ込める。
@Observable
final class WatchScreenModel {
    /// 画面に表示するラベル文言。取得成功時は Rust コアのスナップショット由来、
    /// 失敗時は日本語のフォールバック文言（エラーを UI 契約にしない、coding-rust.md 準拠）。
    private(set) var labelText: String

    /// 操作要素（ボタン）に表示するラベル文言。スナップショットに `.button` ノードが
    /// 含まれない場合は `nil` とし、`ContentView` 側でボタンを出さない分岐に用いる。
    private(set) var buttonLabel: String?

    /// `FandheStore` の生成・スナップショット取得・イベントディスパッチに失敗した場合の要因。
    /// フォールバック表示時のみ非 nil。#69 のシミュレータ実機確認・デバッグ用に、
    /// 失敗の種類を区別して保持する（advisor 指摘: フォールバック理由を 1 種類に潰さない）。
    private(set) var lastError: WatchScreenModelError?

    /// `FandheStore` の生成に失敗した場合でも画面自体はクラッシュさせずフォールバック表示する
    /// （REQ-3 の受け入れ基準はまず「画面が起動しラベルが出ること」であり、
    /// スナップショット取得・イベントディスパッチはベストエフォートとして扱う）。
    private let store: FandheStore?

    init() {
        do {
            let store = try FandheStore()
            self.store = store
            // Swift の確定初期化規則により、メソッド呼び出し（applySnapshot）の前に
            // 非 Optional の labelText の初期化が必要（Optional の buttonLabel / lastError
            // は暗黙で nil 初期化されるため対象外だが、意図を明示するため併記する）。
            // フォールバック文言で仮初期化し、直後の applySnapshot で実スナップショット
            // 由来の値へ上書きする。
            self.labelText = Self.fallbackText
            self.buttonLabel = nil
            self.applySnapshot(fetchingFrom: store)
        } catch {
            self.store = nil
            self.labelText = Self.fallbackText
            self.buttonLabel = nil
            self.lastError = .storeInitializationFailed(error)
        }
    }

    /// `store` からスナップショットを取得し、`labelText` / `buttonLabel` を再導出する。
    /// `init` と `tapPrimaryButton()` の両方から呼ばれる共通経路
    /// （初回描画・再描画のいずれも同じ導出ロジックを通すための集約）。
    private func applySnapshot(fetchingFrom store: FandheStore) {
        do {
            let snapshot = try store.renderSnapshot()
            if let text = Self.firstText(in: snapshot.nodes) {
                self.labelText = text
                self.lastError = nil
            } else {
                self.labelText = Self.fallbackText
                self.lastError = .noTextNodeInSnapshot
            }
            self.buttonLabel = Self.firstButtonLabel(in: snapshot.nodes)
        } catch {
            self.labelText = Self.fallbackText
            self.buttonLabel = nil
            self.lastError = .snapshotDecodeFailed(error)
        }
    }

    /// 操作要素（ボタン）がタップされた際に呼ばれる。`FandheStore.dispatch` で
    /// Rust コアへ `Event::ButtonClicked` を送り、戻り値が再描画要（`true`）であれば
    /// スナップショットを再取得して `labelText` / `buttonLabel` を更新する
    /// （Swift → Rust → 再スナップショット → 表示更新、というブリッジ接続の往復。
    /// TASK-3.3-S2 / #66 の中心的な追加分）。
    ///
    /// `buttonLabel` が導出できていない場合は送るラベルが存在しないため何もしない
    /// （`ContentView` 側もボタン非表示分岐と併せて二重に安全側へ倒す）。
    func tapPrimaryButton() {
        guard let store, let buttonLabel else { return }

        do {
            let needsRedraw = try store.dispatch(.buttonClicked(label: buttonLabel))
            if needsRedraw {
                applySnapshot(fetchingFrom: store)
            }
        } catch {
            // ディスパッチ失敗時も画面をクラッシュさせず、現在の表示を維持したまま
            // 失敗理由のみ記録する（#69 の切り分け用。UI 本文には出さない、A05 対策）。
            self.lastError = .dispatchFailed(error)
        }
    }

    /// スナップショット取得・デコードに失敗した場合に表示する日本語フォールバック文言。
    /// クラッシュさせず、ユーザーには最小限「表示できない」ことのみを伝える
    /// （詳細は `lastError` 側に保持し、UI 本文には含めない。A05 対策）。
    private static let fallbackText = "画面を表示できません"

    /// スナップショットの `nodes` から最初の `.text` ノードの文言を取り出す。
    /// `sample_screen()`（`fandhe-core`）は先頭に `Node::Text` を持つ構成（codec.rs のテスト参照）
    /// のため、本画面の構成（1 ラベル）ではこれで足りる。
    private static func firstText(in nodes: [FandheRenderNode]) -> String? {
        for node in nodes {
            if case let .text(text) = node {
                return text
            }
        }
        return nil
    }

    /// スナップショットの `nodes` から最初の `.button` ノードのラベルを取り出す。
    /// `sample_screen()` はボタンを 2 つ持つが、本画面の構成（1 操作要素）では
    /// 最初のボタンのみを watchOS 画面の操作要素として用いる。
    private static func firstButtonLabel(in nodes: [FandheRenderNode]) -> String? {
        for node in nodes {
            if case let .button(label) = node {
                return label
            }
        }
        return nil
    }
}

/// `WatchScreenModel` がフォールバック表示に切り替えた理由（#69 の起動確認での切り分け用）。
enum WatchScreenModelError: Error {
    /// `FandheStore` の生成（Rust ⇔ Swift のハンドル確立）自体に失敗した。
    case storeInitializationFailed(Error)
    /// スナップショットの取得・JSON デコードに失敗した（スキーマ世代不一致を含む）。
    case snapshotDecodeFailed(Error)
    /// スナップショットは取得できたが、`.text` ノードが 1 つも含まれていなかった。
    case noTextNodeInSnapshot
    /// `tapPrimaryButton()` からのイベントディスパッチ（Swift → Rust）に失敗した。
    /// #59（`ffi` モジュール）未実装のため、実行時には常にこの経路（もしくは
    /// リンクエラー）に到達する制約が残っている（検証レポート参照）。
    case dispatchFailed(Error)
}
