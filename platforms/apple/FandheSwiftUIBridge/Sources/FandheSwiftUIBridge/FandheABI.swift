import CFandheBridge

/// ABI 世代の実行時照合を提供する。`FandheStore.init` が内部で利用するほか、
/// TASK-3.3 のアプリ起動シーケンスが `FandheStore` 生成前にゲートとして単独で
/// 呼べるよう公開 API としても提供する（A08 整合性の不備対策、設計文書
/// §3.1「命名・ABI 規約」参照）。
public enum FandheABI {
    /// 実行時にリンクされた `fandhe-swiftui-bridge` の ABI 世代が、本ラッパが
    /// 期待する世代（`FandheBridgeContract.expectedAbiVersion`）と一致するか検証する。
    /// 不一致の場合は `FandheBridgeError`（`.abiMismatch`。詳細メッセージに
    /// 実際の世代値を含める）を throw する。
    ///
    /// ハンドルを一切生成しない検査のため、失敗時に解放すべき資源が存在しない
    /// （`FandheStore.init` 側で「生成 → 検査 → 失敗時に解放」という二重解放の
    /// リスクを持つ経路を作らないための設計。本メソッドを `store_new` より先に呼ぶ）。
    public static func verify() throws {
        let actual = fandhe_abi_version()
        guard actual == FandheBridgeContract.expectedAbiVersion else {
            throw FandheBridgeError(
                code: .abiMismatch,
                detail: "ABI 世代不一致: expected=\(FandheBridgeContract.expectedAbiVersion), actual=\(actual)"
            )
        }
    }
}
