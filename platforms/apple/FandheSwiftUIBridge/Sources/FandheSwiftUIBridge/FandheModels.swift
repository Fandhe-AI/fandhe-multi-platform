import Foundation

/// `fandhe_render_snapshot` / `fandhe_dispatch_event` が JSON でやり取りする構造化データの
/// Swift 側 `Codable` モデル。TASK-3.3 の SwiftUI 画面実装はこの型のみを介して
/// `crates/fandhe-core` の `Screen<S>` / `Node<S>` ツリーの内容を参照する
/// （ジェネリックな `Screen<S>` / `Node<S>` 自体は不透明ハンドルの向こう側に留まり、
/// Swift 側へは一切公開しない。設計文書 §3.1「ジェネリクスの取り扱い」参照）。
///
/// `crates/fandhe-swiftui-bridge/src/codec.rs` の `#[serde(tag = "kind",
/// rename_all = "snake_case")]`（internally tagged・snake_case）に対応する
/// （`FandheSwiftUIBridgeTests` のフィクスチャで一致を検証）。
public struct FandheRenderSnapshot: Decodable {
    /// スナップショット JSON のスキーマ世代番号（`codec::SNAPSHOT_SCHEMA_VERSION` に対応）。
    /// `FandheStore.renderSnapshot()` が `FandheBridgeContract.snapshotSchemaVersion` との
    /// 一致を検査する（A08 整合性対策、設計文書 §5）。
    public let schemaVersion: UInt32
    /// 画面タイトル（`Screen::title()`）。
    public let title: String
    /// スナップショット生成時点の `StateStore::version()`。再描画判定に用いる。
    public let stateVersion: UInt64
    /// 画面を構成するノード列（`Screen::iter()` の走査結果に対応）。
    public let nodes: [FandheRenderNode]

    private enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case title
        case stateVersion = "state_version"
        case nodes
    }
}

/// 描画ツリーの 1 ノード。`Node<S>` の各バリアントに対応する（関数ポインタフィールドは
/// C ABI を越えられないため含まない。状態アクセスは Rust 側で完結し、Swift 側には
/// 「現在の表示値」のみが JSON として渡ってくる。設計文書 §3.3 参照）。
///
/// 他のモジュール公開型（`FandheStore`/`FandheBridgeError`/`FandheBufferBridge` 等）との
/// 命名一貫性のため `Fandhe` 接頭辞を付ける。
public indirect enum FandheRenderNode: Decodable {
    /// 静的テキスト表示（`Node::Text` / `SnapshotNode::Text` に対応）。
    case text(String)
    /// テキスト入力欄（`Node::TextField` に対応）。ラベルと現在値のみを持つ
    /// （`get`/`set` 関数ポインタは Rust 側に残る）。
    case textField(label: String, value: String)
    /// ボタン（`Node::Button` に対応）。ラベルのみを持つ（`on_click` は Rust 側に残り、
    /// Swift 側はラベルを添えて `FandheEvent.buttonClicked` をディスパッチする）。
    case button(label: String)
    /// 子要素が展開済みのリスト（`Node::List::expand_children` 済みの走査結果）。
    case list([FandheRenderNode])

    /// codec.rs の `#[serde(tag = "kind", rename_all = "snake_case")]` に対応するタグキー・
    /// フィールド名（TASK-3.3-S2 で実出力と突き合わせ済み。設計文書 §3.3 参照）。
    private enum CodingKeys: String, CodingKey {
        case kind
        case text
        case label
        case value
        case children
    }

    /// `SnapshotNode` の internally tagged 表現（`{"kind":"text",...}` /
    /// `{"kind":"text_field",...}` / `{"kind":"button",...}` / `{"kind":"list",...}`）を
    /// デコードする。タグ値は codec 実装（`rename_all = "snake_case"`）に合わせて
    /// snake_case で受理する。
    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try container.decode(String.self, forKey: .kind)
        switch kind {
        case "text":
            let text = try container.decode(String.self, forKey: .text)
            self = .text(text)
        case "text_field":
            let label = try container.decode(String.self, forKey: .label)
            let value = try container.decode(String.self, forKey: .value)
            self = .textField(label: label, value: value)
        case "button":
            let label = try container.decode(String.self, forKey: .label)
            self = .button(label: label)
        case "list":
            let children = try container.decode([FandheRenderNode].self, forKey: .children)
            self = .list(children)
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .kind,
                in: container,
                debugDescription: "未知の FandheRenderNode 種別: \(kind)"
            )
        }
    }
}

/// `fandhe_dispatch_event` へ渡すイベント。`crates/fandhe-core/src/state/store.rs` の
/// `Event` enum（`ButtonClicked` / `TextFieldChanged`）に対応する。
///
/// `codec.rs` の `EventPayload`（`#[serde(tag = "type", rename_all = "snake_case",
/// deny_unknown_fields)]`）に対応し、internally tagged（`{"type":"button_clicked",
/// "label":...}`）へエンコードする。
public enum FandheEvent: Encodable {
    /// `Node::Button` 押下（`store.rs` の `Event::ButtonClicked` に対応）。
    case buttonClicked(label: String)
    /// `Node::TextField` への入力確定（`store.rs` の `Event::TextFieldChanged` に対応）。
    case textFieldChanged(label: String, value: String)

    private enum CodingKeys: String, CodingKey {
        case type
        case label
        case value
    }

    private enum TypeTag: String, Encodable {
        case buttonClicked = "button_clicked"
        case textFieldChanged = "text_field_changed"
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .buttonClicked(let label):
            try container.encode(TypeTag.buttonClicked, forKey: .type)
            try container.encode(label, forKey: .label)
        case .textFieldChanged(let label, let value):
            try container.encode(TypeTag.textFieldChanged, forKey: .type)
            try container.encode(label, forKey: .label)
            try container.encode(value, forKey: .value)
        }
    }
}
