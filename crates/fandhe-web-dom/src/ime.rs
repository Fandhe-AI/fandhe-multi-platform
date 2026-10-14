//! DOM の composition イベント（W3C UI Events）を `fandhe-core` の IME 境界型
//! （[`fandhe_core::ui::ImeEvent`]）へ変換するターゲット非依存レイヤ（REQ-8 / TASK-8.5-S21、
//! Issue #202）。
//!
//! `docs/architecture/ime-architecture.md` §4.5「Web（`compositionevent`、S21〜S22）」の
//! 実装。デスクトップ向け `fandhe-winit-ime`（S7〜S14）と同様に、本モジュールは
//! **純粋な変換関数のみ**を提供し、DOM イベントの購読・配線（`web-sys` の
//! `CompositionEvent` は `wasm32` 専用型）は [`super::mount`] が担う。変換関数自体は
//! フェーズ列挙 + `&str` という target 非依存の入力で設計してあるため、native
//! （`cargo test`、ホスト環境）でもユニットテストできる（coding-rust.md「クロスターゲットに
//! 関わる変更は対象ターゲットのビルド確認まで行う」の前提を満たしつつ、変換ロジック自体は
//! wasm32 専用ビルドを経由せず検証できるようにする狙い）。
//!
//! # 対応表（§4.5 準拠）
//!
//! | DOM イベント | 方向 | [`fandhe_core::ui::ImeEvent`] |
//! |--------------|------|--------------------------------|
//! | `compositionstart` | DOM → コア | `Enabled` |
//! | `compositionupdate`（`data`） | DOM → コア | `Preedit { text: data, cursor: None }` |
//! | `compositionend`（`data`） | DOM → コア | `Commit(data)` |
//!
//! `cursor` が常に `None` なのは DOM の compositionevent がカーソル範囲（選択範囲）を
//! 一切公開しないためであり（[W3C UI Events: Composition
//! Events](https://w3c.github.io/uievents/#events-compositionevents)、`CompositionEvent` は
//! `data` 文字列のみを運ぶ）、winit（byte offset）・iOS/Android（UTF-16 offset）と異なり
//! 変換すべきオフセットそのものが存在しない（§4.6「文字インデックス正規化方針」参照）。
//!
//! `ImeRequest::SetEnabled` に対応する DOM API はない（ブラウザの IME 制御はネイティブ
//! `<input>`/`<textarea>` 要素の存在に暗黙依存する、§4.5 表の最終行）。本モジュールは
//! `ImeRequest` 側の変換関数を提供しない（[`super::mount`] 側で no-op として扱う）。
//!
//! `ImeEvent::Disabled` に対応する DOM イベントは存在しない（blur 時はブラウザが
//! `compositionend` を発火する契約であり、`Disabled` の合成は行わない。フォーカス遷移調停
//! は S24・#205 のスコープ、§4.7）。
//!
//! # 二重表示の回避（配線判断、§4.5 既存整合の節）
//!
//! `fandhe-web-dom` は DOM `<input>` へブラウザの IME を素通しする方式であり、`<input>`
//! 要素自体が preedit をネイティブ表示する。したがって [`super::mount`] は
//! `ime_event_from_composition` が返す `Enabled` / `Preedit` をコア描画へ反映せず
//! （no-op）、`Commit` 到達時のみ既存の commit 経路（`TextFieldWidget::set` 相当）を呼ぶ。
//! これは既存の「`isComposing` 中は state 反映をスキップし `compositionend` で commit する」
//! 判断（PR #341、TASK-4.4-S2）を変更しない（§4.5「既存 `fandhe-web-dom/src/mount.rs` との
//! 整合」）。
//!
//! # ブラウザ差異（S22・#203 実機検証への申し送り）
//!
//! - Chromium・Safari を含め、`compositionend` 後に `isComposing: false` の後続
//!   `input` イベントが必ず発火するとは限らない（`mount.rs` の既存モジュールコメント
//!   参照。本モジュールの変換関数自体には影響しないが、呼び出し側の配線契約に関わる）。
//! - IME 変換キャンセル時（例: Esc キー）は `compositionend.data` が空文字列になりうる。
//!   `ime_event_from_composition` はこれを `Commit(String::new())` としてそのまま返し、
//!   空文字列の扱い（"除去後空なら composition クリアのみ" 契約）はコア側
//!   `TextFieldWidget::apply_ime` が吸収する（本モジュールは意味づけを行わない）。
//!
//! # セキュリティ考慮（§7、`.claude/rules/security.md` 準拠）
//!
//! - `CompositionEvent.data` は外部入力として扱い、本モジュールは文字列を一切加工・検証
//!   しない（制御文字除去等は `TextFieldWidget::apply_ime` の Commit 契約側に委ねる、
//!   `fandhe-winit-ime` と同じ分担）。
//! - ログ出力を一切行わない（`ImeEvent` の Debug マスク契約〈`Preedit`/`Commit` は文字数のみ
//!   出力〉を本モジュールがさらに漏らす経路を追加しない）。
//! - 未知の DOM イベント型名は `None` を返す（panic を API 契約にしない、coding-rust.md 準拠）。

use fandhe_core::ui::ImeEvent;

/// DOM の composition イベント種別（`compositionstart`/`compositionupdate`/`compositionend`）を
/// target 非依存に表す列挙。`web-sys::CompositionEvent`（`wasm32` 専用型）に依存しないための
/// 中間表現であり、[`composition_phase_from_event_type`] が DOM イベント型名から解釈する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionPhase {
    /// `compositionstart`（コンポジション開始）。
    Start,
    /// `compositionupdate`（コンポジション中の未確定文字列更新）。
    Update,
    /// `compositionend`（コンポジション確定）。
    End,
}

/// DOM イベント型名（`Event::type_()` が返す文字列）を [`CompositionPhase`] へ解釈する。
///
/// W3C UI Events の定義済み 3 種類（`"compositionstart"`/`"compositionupdate"`/
/// `"compositionend"`）以外は `None` を返す（未知イベント型名を panic させない、
/// coding-rust.md「panic を API 契約にしない」準拠）。
pub fn composition_phase_from_event_type(event_type: &str) -> Option<CompositionPhase> {
    match event_type {
        "compositionstart" => Some(CompositionPhase::Start),
        "compositionupdate" => Some(CompositionPhase::Update),
        "compositionend" => Some(CompositionPhase::End),
        _ => None,
    }
}

/// [`CompositionPhase`] と `CompositionEvent.data`（`&str`）を、コア契約の [`ImeEvent`] へ
/// 変換する（`ime-architecture.md` §4.5 対応表の実装）。
///
/// `data` はいずれのフェーズでも一切加工しない（サニタイズはコア側 `apply_ime` の
/// Commit 契約に委ねる、モジュール doc「セキュリティ考慮」参照）。`Start` では `data` の
/// 値によらず `ImeEvent::Enabled` を返す（compositionstart の `data` は仕様上 IME
/// 実装依存で意味を持たないため、§4.5 表どおり読み捨てる）。
pub fn ime_event_from_composition(phase: CompositionPhase, data: &str) -> ImeEvent {
    match phase {
        CompositionPhase::Start => ImeEvent::Enabled,
        CompositionPhase::Update => ImeEvent::Preedit {
            text: data.to_string(),
            cursor: None,
        },
        CompositionPhase::End => ImeEvent::Commit(data.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_phase_from_event_type_maps_known_names() {
        assert_eq!(
            composition_phase_from_event_type("compositionstart"),
            Some(CompositionPhase::Start)
        );
        assert_eq!(
            composition_phase_from_event_type("compositionupdate"),
            Some(CompositionPhase::Update)
        );
        assert_eq!(
            composition_phase_from_event_type("compositionend"),
            Some(CompositionPhase::End)
        );
    }

    #[test]
    fn composition_phase_from_event_type_returns_none_for_unknown_name() {
        assert_eq!(composition_phase_from_event_type("input"), None);
        assert_eq!(composition_phase_from_event_type(""), None);
        assert_eq!(composition_phase_from_event_type("CompositionStart"), None);
    }

    #[test]
    fn ime_event_from_composition_maps_start_to_enabled() {
        assert_eq!(
            ime_event_from_composition(CompositionPhase::Start, ""),
            ImeEvent::Enabled
        );
        // compositionstart の data は仕様上意味を持たないため、値によらず Enabled。
        assert_eq!(
            ime_event_from_composition(CompositionPhase::Start, "ignored"),
            ImeEvent::Enabled
        );
    }

    #[test]
    fn ime_event_from_composition_maps_update_to_preedit_with_none_cursor() {
        assert_eq!(
            ime_event_from_composition(CompositionPhase::Update, "かん"),
            ImeEvent::Preedit {
                text: "かん".to_string(),
                cursor: None,
            }
        );
    }

    #[test]
    fn ime_event_from_composition_maps_empty_update_to_empty_preedit() {
        // 空 Preedit はコンポジションのクリアを表す契約（コア側 apply_ime が解釈する）。
        // 本モジュールは意味づけを行わず、素通しするのみ。
        assert_eq!(
            ime_event_from_composition(CompositionPhase::Update, ""),
            ImeEvent::Preedit {
                text: String::new(),
                cursor: None,
            }
        );
    }

    #[test]
    fn ime_event_from_composition_maps_end_to_commit() {
        assert_eq!(
            ime_event_from_composition(CompositionPhase::End, "漢字"),
            ImeEvent::Commit("漢字".to_string())
        );
    }

    #[test]
    fn ime_event_from_composition_maps_empty_end_to_empty_commit() {
        // IME キャンセル（Esc 等）時、compositionend.data が空文字列になりうる
        // （モジュール doc「ブラウザ差異」参照）。空文字列のまま Commit として返し、
        // 意味づけ（除去後空なら composition クリアのみ）はコア側契約に委ねる。
        assert_eq!(
            ime_event_from_composition(CompositionPhase::End, ""),
            ImeEvent::Commit(String::new())
        );
    }
}
