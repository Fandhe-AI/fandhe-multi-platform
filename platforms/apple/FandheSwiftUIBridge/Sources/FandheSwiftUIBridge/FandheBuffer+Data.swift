import CFandheBridge
import Foundation

/// `CFandheBridge.FandheBuffer`（Rust 確保の JSON バイト列）を安全に Swift 側の値へ
/// 変換するためのヘルパ。
///
/// **所有権規約（設計文書 §3.1）**: `FandheBuffer` を返す全 FFI 呼び出し
/// （`fandhe_render_snapshot` / `fandhe_last_error_message`）の戻り値は、ここに集約した
/// ヘルパ経由でのみ消費する。呼び出し側が個別に `fandhe_buffer_free` を呼ぶ経路を作らない
/// ことで、早期 return・throw を含むあらゆる経路で解放漏れが起きないことを 1 箇所で保証する
/// （呼び出し元がこのモジュール外で `fandhe_buffer_free` を直接呼ばないことをレビュー観点とする）。
///
/// 名前を C 側の構造体 `CFandheBridge.FandheBuffer` とは区別して `FandheBufferBridge` と
/// する。同一モジュール内で C 型と同名の型を再宣言すると、シャドーイングにより
/// `CFandheBridge.FandheBuffer` への明示修飾が常に必要になり可読性を損なうため。
enum FandheBufferBridge {
    /// `buffer` を UTF-8 文字列として消費する。`buffer.ptr` が NULL または `len == 0` の場合は
    /// `nil` を返す（「バッファなし」は呼び出し元によって「エラーなし」「詳細メッセージなし」等、
    /// 意味が異なるため、ここでは判定せず `nil` として中立に返す）。
    ///
    /// UTF-8 として不正なバイト列を検出した場合も `nil` を返す（クラッシュさせず、
    /// 呼び出し元が `FandheBridgeError` へ変換できるようにする。A03 対策）。
    /// いずれの経路でも `defer` により必ず `fandhe_buffer_free` を呼ぶ。
    static func consumingIntoString(_ buffer: CFandheBridge.FandheBuffer) -> String? {
        defer { fandhe_buffer_free(buffer) }
        guard let ptr = buffer.ptr, buffer.len > 0 else {
            return nil
        }
        let data = Data(bytes: ptr, count: buffer.len)
        return String(data: data, encoding: .utf8)
    }

    /// `buffer` を生バイト列（`Data`）として消費する。JSON デコードなど、文字列化を経ずに
    /// バイト列のまま扱いたい呼び出し元向け。`consumingIntoString` と同様、必ず
    /// `fandhe_buffer_free` を呼んでから返す。
    static func consumingIntoData(_ buffer: CFandheBridge.FandheBuffer) -> Data? {
        defer { fandhe_buffer_free(buffer) }
        guard let ptr = buffer.ptr, buffer.len > 0 else {
            return nil
        }
        return Data(bytes: ptr, count: buffer.len)
    }
}
