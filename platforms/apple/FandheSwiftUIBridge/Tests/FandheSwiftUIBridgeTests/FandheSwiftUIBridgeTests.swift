import XCTest

@testable import FandheSwiftUIBridge

/// FFI 実リンク（staticlib `libfandhe_swiftui_bridge.a` との結合、TASK-3.4 のスコープ）を
/// 要しない範囲として、`FandheModels.swift` のデコード/エンコードが
/// `crates/fandhe-swiftui-bridge/src/codec.rs` の `#[cfg(test)]` が保証する JSON と
/// 一致することを検証する。
final class FandheSwiftUIBridgeTests: XCTestCase {
    /// codec.rs `encode_snapshot_reflects_title_nodes_and_current_state` が生成する
    /// スナップショット JSON 相当の実例（トップレベルの `schema_version`/`title`/
    /// `state_version` を含む internally tagged `kind` 表現）を `FandheRenderSnapshot` が
    /// 正しくデコードできることを検証する。
    func testDecodeRenderSnapshotMatchesCodecOutput() throws {
        let json = """
        {
            "schema_version": 1,
            "title": "サンプル画面",
            "state_version": 7,
            "nodes": [
                {"kind": "text", "text": "代表 1 画面サンプル"},
                {"kind": "text_field", "label": "メモ", "value": "こんにちは"},
                {"kind": "button", "label": "カウントを増やす"},
                {"kind": "button", "label": "カウントを減らす"},
                {"kind": "list", "children": [
                    {"kind": "text", "text": "item-1"},
                    {"kind": "text", "text": "item-2"}
                ]}
            ]
        }
        """.data(using: .utf8)!

        let snapshot = try JSONDecoder().decode(FandheRenderSnapshot.self, from: json)

        XCTAssertEqual(snapshot.schemaVersion, 1)
        XCTAssertEqual(snapshot.title, "サンプル画面")
        XCTAssertEqual(snapshot.stateVersion, 7)
        XCTAssertEqual(snapshot.nodes.count, 5)

        guard case let .text(text) = snapshot.nodes[0] else {
            return XCTFail("text であるべき: \(snapshot.nodes[0])")
        }
        XCTAssertEqual(text, "代表 1 画面サンプル")

        guard case let .textField(label, value) = snapshot.nodes[1] else {
            return XCTFail("textField であるべき: \(snapshot.nodes[1])")
        }
        XCTAssertEqual(label, "メモ")
        XCTAssertEqual(value, "こんにちは")

        guard case let .button(label) = snapshot.nodes[2] else {
            return XCTFail("button であるべき: \(snapshot.nodes[2])")
        }
        XCTAssertEqual(label, "カウントを増やす")

        guard case let .list(children) = snapshot.nodes[4] else {
            return XCTFail("list であるべき: \(snapshot.nodes[4])")
        }
        XCTAssertEqual(children.count, 2)
        guard case let .text(childText) = children[0] else {
            return XCTFail("text であるべき: \(children[0])")
        }
        XCTAssertEqual(childText, "item-1")
    }

    /// `FandheEvent.buttonClicked` のエンコード結果が、codec.rs
    /// `decode_event_button_clicked_dispatches_and_bumps_version` が受理する
    /// `{"type":"button_clicked","label":"カウントを増やす"}` と同一の構造になることを検証する。
    func testEncodeButtonClickedMatchesCodecInput() throws {
        let data = try JSONEncoder().encode(FandheEvent.buttonClicked(label: "カウントを増やす"))
        let decoded = try JSONDecoder().decode([String: String].self, from: data)

        XCTAssertEqual(decoded["type"], "button_clicked")
        XCTAssertEqual(decoded["label"], "カウントを増やす")
        XCTAssertEqual(decoded.count, 2)
    }

    /// `FandheEvent.textFieldChanged` のエンコード結果が、codec.rs
    /// `decode_event_text_field_changed_dispatches_and_bumps_version` が受理する
    /// `{"type":"text_field_changed","label":"メモ","value":"更新後"}` と一致することを検証する。
    func testEncodeTextFieldChangedMatchesCodecInput() throws {
        let data = try JSONEncoder().encode(
            FandheEvent.textFieldChanged(label: "メモ", value: "更新後")
        )
        let decoded = try JSONDecoder().decode([String: String].self, from: data)

        XCTAssertEqual(decoded["type"], "text_field_changed")
        XCTAssertEqual(decoded["label"], "メモ")
        XCTAssertEqual(decoded["value"], "更新後")
        XCTAssertEqual(decoded.count, 3)
    }
}
