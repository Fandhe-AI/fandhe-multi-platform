//! Phase 11 の拡充ループ（親 #185、Issue #944）: MDM が配った管理アプリ設定のうち、固定の 3 キー
//! （[`KEY_BOOL`]・[`KEY_INTEGER`]・[`KEY_TEXT`]）だけを 1 回読む契約。`fandhe-egui-proxy` の
//! `device/managed_app_config.rs` が「管理設定を読む」ボタン押下ごとにワーカースレッドから
//! [`ManagedAppConfig::read`] を 1 回呼び、各 OS 実装（macOS: `CFPreferences`、iOS: #945、
//! Android: #946）が本トレイトを実装する。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 設定なし: `Ok(Report::NoConfig)`。3 キーとも未設定（管理されていない端末でも同じになるため
//!   「未管理」とは言わない）
//! - 設定あり: `Ok(Report::Config { .. })`。キーごとに [`KeyState`]（未設定・真偽・整数・文字数・型違い）
//! - 非対応: `Err(DeviceError::Unsupported(FEATURE))`
//! - 失敗: `Err(DeviceError::Internal(READ_FAILED))`（OS 呼び出しの失敗、識別子が取れない、
//!   OS 側の返す形が不正）
//!
//! 型違い（[`KeyState::WrongType`]）は未設定にも成功値にも畳まない。キャンセル・権限ダイアログの
//! 経路は無い。
//!
//! # OS 別の元の値
//!
//! - macOS: `CFPreferencesCopyAppValue` で読み、`CFPreferencesAppValueIsForced` が真の値だけを
//!   管理値とする（ユーザーが `defaults write` した値は未設定扱い）。`other_keys` は `None`
//! - iOS: `NSUserDefaults` の `com.apple.configuration.managed` 辞書。「無い」と「辞書でない」は
//!   同じ nil で返る
//! - Android: `RestrictionsManager.getApplicationRestrictions()`。制限が無いと `null`
//!
//! # 呼び出し契約
//!
//! ワーカースレッドから押下ごとに 1 回。変更通知の購読・定期 pull はしない。Android は
//! ディスク I/O を伴うため main thread では呼ばない。期限は設けない。
//!
//! # ワイヤ形式（OS 側 → [`parse_wire`]）
//!
//! OS 側ネイティブコードは ASCII 文字列を返す。4 欄を U+001F で区切る:
//! `<bool>␟<integer>␟<text>␟<other>`。各キー欄の `-` は未設定、`?` は型違い。値は bool が `1`/`0`、
//! integer が 10 進 i64、text が文字数（10 進 u32）。other 欄は `-`（数えない）か件数（10 進 u32）。
//! 全体は 128 バイトまで。運ぶのは数値と記号だけで、文字列の中身・固定外のキー名は読まず、運ばず、
//! ログにも出さない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 嘘の成功を作らない（失敗・型違い・未設定の区別）、中身を出さない、1 プロセス内の読み取り。
//! 守らない: プロファイルが重なったときの勝敗（OS に任せる）、読み取り後の変更の反映（再押下まで）、
//! 文字数の数え方が OS 間で揃うこと、OS 呼び出しが戻らない場合の期限。
//! 並行経路は「UI の effect → ワーカー 1 本」だけで、集約すべきものは無い。

use crate::DeviceError;

/// 対応表の機能 ID（`Unsupported` の識別子）。
pub const FEATURE: &str = "managed_app_config";
/// 失敗時の固定文言。
pub const READ_FAILED: &str = "managed_app_config read";
/// 真偽を読むキー。
pub const KEY_BOOL: &str = "fandheSampleBool";
/// 整数を読むキー。
pub const KEY_INTEGER: &str = "fandheSampleInteger";
/// 文字列を読むキー（画面には文字数だけ出す）。
pub const KEY_TEXT: &str = "fandheSampleText";

/// ワイヤ文字列の最大バイト数。
const MAX_WIRE_BYTES: usize = 128;
/// ワイヤの欄区切り（U+001F）。
const SEP: char = '\u{1f}';

/// 1 キーの状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// 管理値が無い。
    Unset,
    /// 真偽の管理値。
    Bool(bool),
    /// 整数の管理値。
    Integer(i64),
    /// 文字列の管理値。中身は持たず文字数（Unicode スカラー値）だけ。
    Text {
        /// 文字数。
        chars: u32,
    },
    /// 値はあるがキーの型と違う。
    WrongType,
}

/// 固定 3 キーの状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigKeys {
    /// [`KEY_BOOL`]。
    pub boolean: KeyState,
    /// [`KEY_INTEGER`]。
    pub integer: KeyState,
    /// [`KEY_TEXT`]。
    pub text: KeyState,
}

/// 読み取り結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// 3 キーとも未設定（他のキーも無い）。
    NoConfig,
    /// 何らかの設定がある。
    Config {
        /// 固定 3 キーの状態。
        keys: ConfigKeys,
        /// 固定外のキーの件数。数えない OS は `None`。
        other_keys: Option<u32>,
    },
}

/// 管理アプリ設定の読み取り。
pub trait ManagedAppConfig: Send + Sync {
    /// 固定 3 キーを 1 回読む。契約はモジュール doc。
    fn read(&self) -> Result<Report, DeviceError>;
}

/// 非対応環境用。常に `Unsupported`（ダミーの成功へ倒さない）。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedManagedAppConfig;

impl ManagedAppConfig for UnsupportedManagedAppConfig {
    fn read(&self) -> Result<Report, DeviceError> {
        Err(DeviceError::Unsupported(FEATURE))
    }
}

fn bad() -> DeviceError {
    DeviceError::Internal(READ_FAILED)
}

/// 1 欄を解く。`parse` は値の欄（`-`・`?` 以外）の解釈。
fn field(raw: &str, parse: impl FnOnce(&str) -> Option<KeyState>) -> Result<KeyState, DeviceError> {
    match raw {
        "-" => Ok(KeyState::Unset),
        "?" => Ok(KeyState::WrongType),
        v => parse(v).ok_or_else(bad),
    }
}

/// 10 進数字だけの欄か（`allow_sign` のときだけ先頭の `-` を 1 つ許す）。
fn digits(v: &str, allow_sign: bool) -> bool {
    let body = if allow_sign {
        v.strip_prefix('-').unwrap_or(v)
    } else {
        v
    };
    !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit())
}

/// OS 側が返したワイヤ文字列を [`Report`] へ解く。形が不正なら `Internal`（`NoConfig` へ畳まない）。
pub fn parse_wire(raw: &str) -> Result<Report, DeviceError> {
    if raw.is_empty() || raw.len() > MAX_WIRE_BYTES {
        return Err(bad());
    }
    let f: Vec<&str> = raw.split(SEP).collect();
    let [b, i, t, o] = f[..] else {
        return Err(bad());
    };
    let boolean = field(b, |v| match v {
        "1" => Some(KeyState::Bool(true)),
        "0" => Some(KeyState::Bool(false)),
        _ => None,
    })?;
    let integer = field(i, |v| {
        if digits(v, true) {
            v.parse().ok().map(KeyState::Integer)
        } else {
            None
        }
    })?;
    let text = field(t, |v| {
        if digits(v, false) {
            v.parse().ok().map(|chars| KeyState::Text { chars })
        } else {
            None
        }
    })?;
    let other_keys = match o {
        "-" => None,
        v if digits(v, false) => Some(v.parse::<u32>().map_err(|_| bad())?),
        _ => return Err(bad()),
    };
    let keys = ConfigKeys {
        boolean,
        integer,
        text,
    };
    let all_unset = [boolean, integer, text]
        .iter()
        .all(|k| *k == KeyState::Unset);
    if all_unset && other_keys.unwrap_or(0) == 0 {
        Ok(Report::NoConfig)
    } else {
        Ok(Report::Config { keys, other_keys })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(parts: [&str; 4]) -> String {
        parts.join("\u{1f}")
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedManagedAppConfig.read(),
            Err(DeviceError::Unsupported(FEATURE))
        ));
    }

    #[test]
    fn parses_values() {
        let r = parse_wire(&w(["1", "-42", "5", "-"])).unwrap();
        assert_eq!(
            r,
            Report::Config {
                keys: ConfigKeys {
                    boolean: KeyState::Bool(true),
                    integer: KeyState::Integer(-42),
                    text: KeyState::Text { chars: 5 },
                },
                other_keys: None,
            }
        );
        let r = parse_wire(&w(["0", "-", "-", "2"])).unwrap();
        assert!(matches!(
            r,
            Report::Config {
                keys: ConfigKeys {
                    boolean: KeyState::Bool(false),
                    ..
                },
                other_keys: Some(2),
            }
        ));
    }

    #[test]
    fn no_config_only_when_everything_unset() {
        assert_eq!(
            parse_wire(&w(["-", "-", "-", "-"])).unwrap(),
            Report::NoConfig
        );
        assert_eq!(
            parse_wire(&w(["-", "-", "-", "0"])).unwrap(),
            Report::NoConfig
        );
        assert!(matches!(
            parse_wire(&w(["-", "-", "-", "2"])).unwrap(),
            Report::Config {
                other_keys: Some(2),
                ..
            }
        ));
    }

    #[test]
    fn wrong_type_is_kept() {
        let r = parse_wire(&w(["?", "-", "-", "-"])).unwrap();
        assert!(matches!(
            r,
            Report::Config {
                keys: ConfigKeys {
                    boolean: KeyState::WrongType,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn malformed_is_internal() {
        let long = "1".repeat(129);
        let cases = [
            String::new(),
            "-".to_string(),
            w(["-", "-", "-", "-"]) + "\u{1f}-",
            w(["2", "-", "-", "-"]),
            w(["-", "99999999999999999999", "-", "-"]),
            w(["-", "-", "-5", "-"]),
            w(["-", "-", "abc", "-"]),
            w(["-", "-", "-", "x"]),
            w(["-", "-", "-", "99999999999"]),
            w(["-", "--1", "-", "-"]),
            long,
        ];
        for c in cases {
            assert!(
                matches!(parse_wire(&c), Err(DeviceError::Internal(READ_FAILED))),
                "{c:?}"
            );
        }
    }
}
