//! Phase 8（端末機能の型検証、親 #752）の型サンプル「メモ保存」（Issue #747）。
//! 1 行入力 + 保存ボタン + 保存済み一覧という最小構成で、ローカル保存が
//! 再起動後も残ることを示す参照実装。`FANDHE_PROXY_APP=storage_note` で
//! `fandhe-egui-proxy` の `screens/registry.rs` の `init` 区画から呼ばれる。
//!
//! # 端末保存との接続方式
//!
//! `fandhe-core` は `fandhe-device` に依存しない（依存方向は最適化レイヤ → コアの
//! 一方向、coding-rust.md 準拠）ため、本モジュールは [`fandhe_device::storage::Storage`]
//! と同形の最小トレイト [`NoteStorage`] を独自に定義する。純粋なトレイト定義のみで
//! I/O を持たないため「端末依存」には当たらず、画面ファイルと一緒に削除できる
//! （README.md「Phase 8（端末機能）の切り離し手順」の範囲内）。
//!
//! `fandhe-egui-proxy` 側の `device.rs` が `fandhe_device::storage::FsStorage` を
//! newtype で [`NoteStorage`] へ橋渡しし、レジストリの `init` 行が渡す状態ファクトリ
//! （`fn() -> StorageNoteState`）が [`StorageNoteState::with_storage`] を呼んで
//! 構築時に保存済みメモを読み込む（＝再起動後の復元）。`Default` 実装は保存先を
//! 持たない（メモリ内のみ、ヘッドレステスト・非対応プラットフォーム向け）。
use crate::ui::{Node, Screen, WidgetId};

/// 保存先 key（固定値）。ユーザー入力を key・パス・コマンドへ展開しない
/// （A03 インジェクション対策、security.md 準拠）。
pub const NOTES_KEY: &str = "storage_note.txt";

/// 端末保存領域への読み書きで発生し得るエラー。`fandhe_device::DeviceError` を
/// 直接使わない（本クレートは `fandhe-device` に依存しない、coding-rust.md の
/// 二層アーキテクチャ準拠）。橋渡し側（`fandhe-egui-proxy::device::DeviceNoteStorage`）
/// が `DeviceError` の各バリアントをこの型へ変換する。表示文言は `Display` 経由で
/// `status` にそのまま流用する（`SAVE_FAILED_PREFIX`/`LOAD_FAILED_PREFIX` 参照）。
#[derive(Debug, thiserror::Error)]
pub enum NoteStorageError {
    /// ファイル入出力（読み書き自体の失敗）。
    #[error("入出力エラー: {0}")]
    Io(String),
    /// 保存先 key が許可文字集合・パストラバーサル制約に違反する。
    #[error("不正な key: {0}")]
    InvalidKey(String),
    /// 現在のプラットフォームでは保存先を提供できない。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(String),
    /// 上記に分類できないその他のエラー（橋渡し側の実装追加時の受け皿）。
    #[error("{0}")]
    Other(String),
}

/// 端末保存領域への読み書き契約。`fandhe_device::storage::Storage` と同じ形にし、
/// `fandhe-egui-proxy` 側がその実装を newtype で橋渡しする（本クレートは
/// `fandhe-device` を参照しない）。
pub trait NoteStorage: Send + Sync {
    /// `key` に対応する値を読む。未存在は `Ok(None)`。
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, NoteStorageError>;
    /// `key` に `bytes` を書く。既存値があれば置き換える。
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), NoteStorageError>;
}

/// 「メモ保存」画面の状態。`storage` が `None`（既定）のときは一覧をメモリ内でのみ
/// 保持し、永続化しない（ヘッドレステスト・非対応プラットフォーム向け）。
pub struct StorageNoteState {
    /// 入力欄の現在値。
    pub input: String,
    /// 保存済みメモ（最新が先頭）。
    pub notes: Vec<String>,
    /// 直近の操作結果・案内文言。
    pub status: String,
    /// 端末保存領域への橋渡し。`Box<dyn NoteStorage>` を保持するため `derive(Default)`
    /// は使えず、`Default` は手動実装する。
    storage: Option<Box<dyn NoteStorage>>,
}

/// 保存先が無いときの既定案内文言（`Default` とテストの二重管理を避けるための定数）。
pub const NO_STORAGE_STATUS: &str = "保存先なし（メモリ内のみ）";
/// 空メモを保存しようとしたときの案内文言。
pub const EMPTY_NOTE_STATUS: &str = "空のメモは保存できません";
/// 保存に失敗したときの案内文言の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const SAVE_FAILED_PREFIX: &str = "保存に失敗しました: ";
/// 読み込みに失敗したときの案内文言の接頭辞。
pub const LOAD_FAILED_PREFIX: &str = "読み込みに失敗しました: ";

impl Default for StorageNoteState {
    fn default() -> Self {
        Self {
            input: String::new(),
            notes: Vec::new(),
            status: NO_STORAGE_STATUS.to_string(),
            storage: None,
        }
    }
}

impl StorageNoteState {
    /// `status` を上書きしたビルダー形式（プラットフォームレイヤが保存先未解決・
    /// 未配線の案内文言を `Default` の上に重ねるための最小 API。`storage` フィールド
    /// が非公開のため `..Self::default()` の構造体更新記法をクレート外から使えない）。
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = status.into();
        self
    }

    /// `storage` を保存先として構築し、既存の保存済みメモを読み込んで復元する
    /// （再起動後もメモが残る、受入 (a)/(b) の本体）。読み込みが失敗した場合は
    /// `storage` を保持せず（`None` のまま）メモリ内のみの状態にする。保持したまま
    /// 空一覧を返すと、次の `save` が「読み込めなかった既存メモ」ごと上書きしてしまう
    /// ため（読み込み成功が確認できるまで書き込みを許可しない設計）。
    pub fn with_storage(storage: Box<dyn NoteStorage>) -> Self {
        match storage.read(NOTES_KEY) {
            Ok(Some(bytes)) => Self {
                input: String::new(),
                notes: decode_notes(&bytes),
                status: String::new(),
                storage: Some(storage),
            },
            Ok(None) => Self {
                input: String::new(),
                notes: Vec::new(),
                status: String::new(),
                storage: Some(storage),
            },
            Err(e) => Self::default().with_status(format!("{LOAD_FAILED_PREFIX}{e}")),
        }
    }
}

/// メモ一覧をファイル表現へ符号化する（改行区切り）。
pub fn encode_notes(notes: &[String]) -> Vec<u8> {
    notes.join("\n").into_bytes()
}

/// ファイル表現をメモ一覧へ復号する（改行区切り、`decode_notes`/`encode_notes` は
/// 対で使う）。空バイト列は空一覧。`from_utf8_lossy` を使うため、破損した保存内容でも
/// panic せず読める範囲を復元する。
pub fn decode_notes(bytes: &[u8]) -> Vec<String> {
    if bytes.is_empty() {
        return Vec::new();
    }
    String::from_utf8_lossy(bytes)
        .split('\n')
        .map(str::to_string)
        .collect()
}

/// 「メモ」欄の表示値を返す（`Screen::text_field` の getter）。
pub fn get_input(state: &StorageNoteState) -> String {
    state.input.clone()
}

/// 「メモ」欄の入力を `state.input` へ反映する（`Screen::text_field` の setter）。
pub fn set_input(state: &mut StorageNoteState, value: String) {
    state.input = value;
}

/// 「保存」ボタンのハンドラ。空判定のみ trim して行う（前後の空白だけの入力は
/// 保存しない）が、保存値自体は前後の空白を保持する（契約表 8「入力どおりに保存」、
/// Issue #987 で是正）。改行はメモ 1 件 1 行の保存契約を守るため空白へ置換する
/// （ponytail: 構造化が必要になれば JSON 化する）。書込に失敗してもメモは一覧に残し
/// （メモリ内には反映済み）、`status` にエラーを表示する。
pub fn save(state: &mut StorageNoteState) {
    if state.input.trim().is_empty() {
        state.status = EMPTY_NOTE_STATUS.to_string();
        return;
    }
    let note = state.input.replace(['\n', '\r'], " ");
    state.notes.insert(0, note);
    state.input.clear();

    let Some(storage) = state.storage.as_ref() else {
        // 保存先なし（`Default` のまま、または読み込み失敗で `with_storage` が
        // `storage` を保持しなかった場合）。メモリ内には反映済みのため、直前の
        // エラー文言を残さず件数入りの案内へ更新する（P2/Bugbot 指摘対応）。
        state.status = format!("保存しました（メモリ内のみ、{} 件）", state.notes.len());
        return;
    };
    match storage.write(NOTES_KEY, &encode_notes(&state.notes)) {
        Ok(()) => state.status = format!("保存しました（{} 件）", state.notes.len()),
        Err(e) => state.status = format!("{SAVE_FAILED_PREFIX}{e}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &StorageNoteState) -> String {
    state.status.clone()
}

/// 保存済みメモ一覧を `Node::Text` へ展開する（`Screen::list` の `expand`）。
/// 0 件時は「メモはありません」を単一項目として表示する
/// （list_detail.rs / confirm_dialog.rs と同様、空一覧を無表示にしない）。
pub fn expand_notes(state: &StorageNoteState) -> Vec<Node<StorageNoteState>> {
    if state.notes.is_empty() {
        return vec![Node::Text("メモはありません".to_string())];
    }
    state.notes.iter().cloned().map(Node::Text).collect()
}

/// 「メモ保存」画面を組み立てる。`registry.rs` の `init` 行から
/// `FANDHE_PROXY_APP=storage_note` で呼ばれるエントリポイント。
pub fn storage_note_screen() -> Screen<StorageNoteState> {
    Screen::new("メモ保存")
        .text_field("メモ", get_input, set_input)
        .button(WidgetId::from_raw(1), "保存", save)
        .dynamic_text(status_text)
        .text("保存済みメモ（最新が上）")
        .list(expand_notes)
}
