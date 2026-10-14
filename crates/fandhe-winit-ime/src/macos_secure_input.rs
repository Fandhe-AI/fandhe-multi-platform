//! macOS 専用: Carbon HIToolbox の SecureEventInput（`NSSecureTextField` が内部で使う機構と
//! 同じ API）で、パスワードフィールドのフォーカス中に IME の学習・予測変換・他プロセスの
//! キー入力傍受を抑止する（Issue #586、`crate::sensitive_input::SensitiveInputTracker` から
//! 呼ばれる。呼び出し側は enable/disable を 1:1 で対にする契約を守る責務を持つ）。
//!
//! # unsafe 境界
//!
//! クレートルート（`lib.rs`）は `#![deny(unsafe_code)]` を採用し、本モジュールのみ
//! `#![allow(unsafe_code)]` で例外的に許可する（`fandhe-swiftui-bridge` の `ffi` モジュールと
//! 同じ方式、coding-rust.md「最小範囲の関数・モジュールに閉じる」準拠）。3 関数はいずれも
//! 引数を取らずグローバルなプロセス状態を切り替えるのみで、生ポインタ・メモリ寿命の懸念は
//! 発生しない。各呼び出しに `// SAFETY:` を付す。
#![allow(unsafe_code)]

#[allow(non_camel_case_types)]
type OSStatus = i32;
#[allow(non_camel_case_types)]
type Boolean = u8;

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    // 3 関数とも Carbon HIToolbox（CarbonEvents.h）が公開する引数なしの C 関数。
    // https://developer.apple.com/documentation/coreservices/1613606-enablesecureeventinput
    fn EnableSecureEventInput() -> OSStatus;
    fn DisableSecureEventInput() -> OSStatus;
    fn IsSecureEventInputEnabled() -> Boolean;
}

/// [`enable`] が `EnableSecureEventInput()` の失敗を検知したことを表すエラー
/// （codex-review P1 対応、Issue #586）。権限不足等で有効化が実効を持たなかった場合に
/// 「パスワード入力が保護された」という誤った前提で処理を続行させないための型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("EnableSecureEventInput failed (OSStatus={0})")]
pub struct EnableSecureEventInputError(pub OSStatus);

/// SecureEventInput を有効化する（プロセス全体に効き、他プロセスからのイベントタップ傍受も
/// 遮断される。`docs/architecture/ime-known-constraints.md` §3.3 macOS 節参照）。
/// `OSStatus` を検査し、成功時のみ `Ok(())` を返す（戻り値を無条件に破棄すると、
/// 権限・実行環境要因で有効化に失敗してもパスワード入力が保護された前提で処理が
/// 続行され、失敗後も呼び出し元が対の `disable()` を発行する非対称な状態になり得る）。
/// エラーコードを見たリトライは行わない
/// （Carbon の該当エラーは主にプロセス権限起因でリトライしても解消しないため）。
/// 呼び出し元（[`crate::sensitive_input::SensitiveInputTracker`]）は失敗時、
/// 有効化されなかった実態に合わせて内部状態を戻し、対応しない `disable()` を
/// 発行しない契約を負う（`SensitiveInputTracker::record_apply_result` 参照）。
pub fn enable() -> Result<(), EnableSecureEventInputError> {
    // SAFETY: 引数なし・戻り値は OSStatus のみを返す C 関数呼び出し。スレッド制約なし
    // （Carbon HIToolbox のドキュメント上メインスレッド限定の記載はない）。
    // 生ポインタを扱わず、呼び出し自体がメモリ安全性へ影響しない。
    let status = unsafe { EnableSecureEventInput() };
    if status == 0 {
        Ok(())
    } else {
        Err(EnableSecureEventInputError(status))
    }
}

/// [`disable`] が `DisableSecureEventInput()` の失敗を検知したことを表すエラー
/// （codex-review P1 対応、Issue #586）。無効化に失敗した場合、プロセス全体の
/// SecureEventInput ロックが（他アプリの IME 入力も含め）残存しうるため、
/// 成功したものとして状態機械を進めてはならない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("DisableSecureEventInput failed (OSStatus={0})")]
pub struct DisableSecureEventInputError(pub OSStatus);

/// SecureEventInput を無効化する（`enable` と対で呼ぶこと、呼び出し元は
/// `SensitiveInputTracker`）。`OSStatus` を検査し、成功時のみ `Ok(())` を返す
/// （戻り値を無条件に破棄すると、無効化が実際には失敗していても呼び出し元・
/// [`crate::sensitive_input::SensitiveInputTracker`] は成功済みとみなし Drop でも
/// 再試行されず、プロセス全体の SecureEventInput ロックが他アプリの IME 入力も含め
/// 残存しうる）。呼び出し元は失敗時、`active` 状態を維持し次の機会
/// （次の状態遷移・`Drop`）で再試行する契約を負う
/// （`SensitiveInputTracker::record_apply_result` 参照）。
pub fn disable() -> Result<(), DisableSecureEventInputError> {
    // SAFETY: enable() と同様、引数なし・副作用が OS 側グローバル状態の切り替えのみ。
    let status = unsafe { DisableSecureEventInput() };
    if status == 0 {
        Ok(())
    } else {
        Err(DisableSecureEventInputError(status))
    }
}

/// 現在 SecureEventInput が有効かを問い合わせる（主にテスト・診断用途）。
pub fn is_enabled() -> bool {
    // SAFETY: 引数なし・戻り値は 0/1 の Boolean のみを返す照会関数。
    unsafe { IsSecureEventInputEnabled() != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_disable_round_trip_toggles_state() {
        // ウィンドウサーバーに接続できないヘッドレス CI 環境では enable が実効を持たず
        // is_enabled が false のまま変化しない場合がある（macOS の仕様上の制約）。
        // その場合は往復自体（panic しないこと）のみを確認し、状態遷移の断定は行わない。
        //
        // ここでの disable() は対応する enable() がまだ一度もないままの呼び出しであり、
        // Carbon 側が対のない無効化要求を OSStatus エラー（paramErr 相当）として拒否する
        // 環境がある。あくまで前回テスト実行の残留状態を掃除するベストエフォートの
        // クリーンアップのため、戻り値は無視する（対のある呼び出しの結果は下の
        // enable → disable 往復で検査する）。
        let _ = disable();
        let before = is_enabled();
        // ヘッドレス CI 環境でも OSStatus 自体は成功（0）を返す想定（is_enabled が
        // 実効を持つかは別、コメント参照）。失敗した場合はテスト自体を落として検知する。
        enable().expect("EnableSecureEventInput が OSStatus エラーを返した");
        let after_enable = is_enabled();
        disable().expect("DisableSecureEventInput が OSStatus エラーを返した");
        let after_disable = is_enabled();

        if after_enable != before {
            // 実効を持つ環境では enable → true、disable → false になる。
            assert!(after_enable);
            assert!(!after_disable);
        }
    }
}
