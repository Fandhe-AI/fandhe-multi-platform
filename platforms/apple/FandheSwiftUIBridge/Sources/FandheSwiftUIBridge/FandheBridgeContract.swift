/// `crates/fandhe-swiftui-bridge`（S2〜S4: #59〜#61）との契約のうち、
/// `docs/architecture/swiftui-bridge-architecture.md` §3.3 が値まで凍結していない
/// 部分（ABI バージョン番号・エラーコードの具体値・JSON タグ付け方式）の
/// Swift 側期待値を 1 ファイルに集約する。変更時は本ファイルと
/// `FandheModels.swift` に閉じる。
public enum FandheBridgeContract {
    /// 期待する ABI 世代。`fandhe_abi_version()`（`crates/fandhe-swiftui-bridge/src/ffi/mod.rs`）
    /// の戻り値 `1` と一致させる（`FandheStore.init` で検査）。
    static let expectedAbiVersion: UInt32 = 1

    /// 期待するスナップショット JSON のスキーマ世代。`crates/fandhe-swiftui-bridge/src/codec.rs`
    /// の `SNAPSHOT_SCHEMA_VERSION` と同値であり、
    /// `FandheStore.renderSnapshot()` が `FandheRenderSnapshot.schemaVersion` との一致を
    /// 検査する際の期待値として用いる（A08 整合性対策、設計文書 §5）。
    static let snapshotSchemaVersion: UInt32 = 1

    /// Swift 側が区別するエラー種別。設計文書 §3.3「エラー伝播」は
    /// 「`0` は成功、負値をエラー種別に割り当てる」とのみ規定する。
    /// Rust 側 `FandheErrorCode`（`crates/fandhe-swiftui-bridge/src/error.rs`）の
    /// 値割り当てとは 1:1 対応させておらず、成功/失敗判定は「`0` 以上か負か」のみで行う
    /// （具体値に依存したロジックを持ち込まない）設計にしてある。
    ///
    /// `FandheBridgeError.code` を通じてモジュール外（TASK-3.3）へ公開されるため
    /// `public` とする。
    public enum ErrorCode: Int32 {
        /// 不正なポインタ・NULL 引数。
        case invalidArgument = -1
        /// ハンドルが無効（解放済み、または元から不正）。
        case invalidHandle = -2
        /// 受け渡された JSON の UTF-8 検証・パースに失敗。
        case malformedPayload = -3
        /// Rust 側の panic を `catch_unwind` で捕捉した。
        case panicCaught = -4
        /// `FandheABI.verify()` が検出した ABI 世代不一致（Swift 側ローカル判定であり
        /// FFI の生戻り値としては現れない。`invalidArgument`/`invalidHandle` のいずれとも
        /// 意味が異なるための専用コード）。
        case abiMismatch = -5
        /// `fandhe_store_new` が `NULL` を返した場合の内部初期化失敗（Swift 側ローカル
        /// 判定であり FFI の生戻り値としては現れない）。`fandhe_store_new` は引数を
        /// 取らないため `invalidArgument`（NULL 引数向け）を誤用しないための専用コード
        /// （`abiMismatch` と同じ理由でヘッダ上「引数に起因しないローカル判定」を
        /// 別カテゴリとして区別する）。
        case initializationFailed = -6
        /// 上記いずれにも一致しない未知のエラー種別。Rust 側の割り当てと一致しない
        /// 負値を吸収する。
        case unknown = -99

        /// 生の `i32` から寛容にマッピングする。未知の負値は `.unknown` に丸め、
        /// 予期しない値であっても Swift 側が exhaustive でないケースへ落ちて
        /// クラッシュすることを避ける（A05 の関連対策、未定義動作を作らない）。
        init(rawCode: Int32) {
            self = ErrorCode(rawValue: rawCode) ?? .unknown
        }
    }
}
