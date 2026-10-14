//! 外部トークン定義ファイル読み込み・シリアライズ境界（TASK-8.4-S17、#173、
//! feature `token-file`）。
//!
//! `docs/architecture/design-token-architecture.md` §6・
//! `docs/architecture/token-resolution-architecture.md` §3.1〜§3.2・§5.3 が
//! 定める契約を実装する:
//!
//! - **参照構文の集約**: DTCG 風の `{group.token}` 参照文字列パースは本モジュールに
//!   集約する。解決エンジン（`resolver`）・スキーマ（`schema`）は構造化済みの
//!   [`super::schema::TokenRef`] のみを受け渡し、生文字列を境界の外へ渡さない
//!   （§3.1）。
//! - **DoS 耐性**: ファイルサイズ・トークン件数・ネスト深さ/セグメント数・
//!   セグメント長・参照文字列長のそれぞれに上限を設ける（下記 `pub const`、
//!   §3.2「上限を設けること」の具体化、security.md の DoS 耐性観点）。
//! - **S18（#174）との境界**: 本モジュールは構文レベルの検証（JSON 形状・
//!   文字集合・上限・値域）のみを行い、fail-fast の単一 `Result<_, TokenError>`
//!   で返す。未定義参照・循環・型不一致といった意味レベルの検証、複数エラーの
//!   集約・報告形式は S18（#174）のスコープであり、本モジュールは resolver へ
//!   直結する API を持たない（読み込んだ `TokenSet` は S18 の検証を経てから
//!   `TokenResolver` へ渡す前提、§5.3）。
//!
//! ## ファイル形式（DTCG 部分準拠）
//!
//! ネストされたグループオブジェクト + `$type`/`$value` キーを持つトークン
//! オブジェクトという DTCG 風の形状を採用するが、以下の点で **部分準拠**
//! （逸脱あり）:
//!
//! - 参照は `$value` 全体が `"{group.token}"` 形式の文字列であるトークンとして
//!   表現し、[`super::schema::TokenDefinition::Reference`] へ変換する（`category`
//!   は `$type` から決定する）
//! - `$type` は各トークンに必須（DTCG のグループレベル `$type` 継承は非対応 =
//!   簡略化した逸脱）
//! - `$description` / `$extensions` は受理するが読み捨てる（値の検証は行わない）。
//!   未知の `$type` 値・想定外の値形状は [`TokenError::ParseError`] で拒否する
//!
//! カテゴリ別のエンコーディング表（`TokenCategory` 12 種すべてに対応）は
//! 実装計画 §2.2 を参照。値域検証（非有限値・負値・制御文字等）は既存の
//! 検証付きコンストラクタ（`ColorValue::from_hex` 等）へ委譲し、二重実装しない。
//!
//! ## 依存関係
//!
//! `serde_json::Value` を走査してドメイン型へ手動変換する（`serde` derive は
//! 使わない）。ドメイン型に serde 属性を付けないことで、エラー位置の詳細報告・
//! ネスト深さ/件数上限の自前制御を独立に保つ（実装計画 §2.1）。
//!
//! ## セキュリティ（OWASP A08「整合性の不備」）
//!
//! 外部トークン定義ファイルは**信頼できない入力**として扱う。本モジュールが
//! 返す `TokenSet` は S18（#174）のバリデーションを経てから `TokenResolver` へ
//! 渡す前提であり、ネットワーク経由のテーマ配信（OTA、Web 限定）を行う場合は
//! 別途署名検証が必須である（security.md A08、本モジュールのスコープ外）。

use std::fs;
use std::path::Path;

use serde_json::{Map, Number, Value};

use super::animation::{AnimationValue, DurationValue, EasingValue};
use super::border::{BorderStyleValue, BorderValue};
use super::color::ColorValue;
use super::dimension::{DimensionValue, ElevationValue};
use super::error::TokenError;
use super::schema::{TokenCategory, TokenDefinition, TokenPath, TokenRef, TokenSet, TokenValue};
use super::shadow::ShadowValue;
use super::spacing::SpacingValue;
use super::typography::{FontStyleValue, FontWeightValue, TypographyValue};

/// 外部定義ファイルとして受理する最大バイト数（1 MiB）。
///
/// `load_token_set_file` が `fs::metadata` で読み込み前に検査する（DoS 対策、
/// security.md）。トークン定義ファイルは通常数十 KiB 程度に収まる想定であり、
/// この上限は明らかな異常値（誤って巨大なファイルを指定した場合等）を弾く
/// 目安として定めた暫定値。
pub const MAX_TOKEN_FILE_BYTES: u64 = 1024 * 1024;

/// 1 ファイルあたりに許容するトークン定義数の上限。
///
/// フレームワーク全体のトークン数は 12 カテゴリ × 実用的なスケール数を
/// 大きく超えないと見込まれるため、想定利用を十分に超える値として定めた
/// 暫定値（DoS 対策）。
pub const MAX_TOKEN_COUNT: usize = 4096;

/// トークンパスのセグメント数（グループのネスト深さを兼ねる）の上限。
pub const MAX_PATH_SEGMENTS: usize = 16;

/// トークンパスの各セグメントに許容する最大バイト長。
pub const MAX_SEGMENT_LEN: usize = 64;

/// 参照文字列（`"{group.token}"` 全体）に許容する最大バイト長。
pub const MAX_REFERENCE_LEN: usize = 256;

/// `ParseError`（パスなし = ルート起因）を生成する。
fn root_parse_error(reason: impl Into<String>) -> TokenError {
    TokenError::ParseError {
        path: String::new(),
        reason: reason.into(),
    }
}

/// パス付きの `ParseError` を生成する。
fn parse_error(path: &TokenPath, reason: impl Into<String>) -> TokenError {
    TokenError::ParseError {
        path: path.to_string(),
        reason: reason.into(),
    }
}

/// `{group.token}` 形式の参照文字列を構造化済みの [`TokenRef`] へパースする。
///
/// `token-resolution-architecture.md` §3.1〜§3.2 が定める境界そのものであり、
/// 生の参照文字列を解決エンジン（`resolver`）・スキーマ（`schema`）へ渡す前に
/// 必ず本関数を通す契約とする。許可文字集合・`..` セグメント拒否の検証自体は
/// [`TokenPath::new`] の型不変条件へ委譲し、本関数では以下のみを追加検証する:
/// - 波括弧 `{` `}` で囲まれた形式であること
/// - 全体の長さが [`MAX_REFERENCE_LEN`] 以下であること
/// - セグメント数が [`MAX_PATH_SEGMENTS`] 以下、各セグメント長が
///   [`MAX_SEGMENT_LEN`] 以下であること（`TokenPath::new` は §3.2 の契約により
///   上限を検証しないため、本関数が担う）
///
/// # Errors
///
/// 上記いずれかの条件を満たさない場合 [`TokenError::InvalidReferenceSyntax`] を、
/// `TokenPath::new` が検証に失敗した場合はその [`TokenError::InvalidPath`] を
/// そのまま返す。
pub fn parse_token_reference(s: &str) -> Result<TokenRef, TokenError> {
    if s.len() > MAX_REFERENCE_LEN {
        return Err(TokenError::InvalidReferenceSyntax {
            reason: format!(
                "参照文字列が長すぎます（上限 {MAX_REFERENCE_LEN} バイト）: {} バイト",
                s.len()
            ),
        });
    }

    let inner = s
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .ok_or_else(|| TokenError::InvalidReferenceSyntax {
            reason: format!("参照は '{{group.token}}' 形式である必要があります: {s}"),
        })?;

    if inner.is_empty() {
        return Err(TokenError::InvalidReferenceSyntax {
            reason: "参照先のパスが空です".to_string(),
        });
    }

    let segments: Vec<String> = inner.split('.').map(str::to_string).collect();
    if segments.len() > MAX_PATH_SEGMENTS {
        return Err(TokenError::InvalidReferenceSyntax {
            reason: format!(
                "参照先のセグメント数が上限（{MAX_PATH_SEGMENTS}）を超えています: {}",
                segments.len()
            ),
        });
    }
    for segment in &segments {
        if segment.len() > MAX_SEGMENT_LEN {
            return Err(TokenError::InvalidReferenceSyntax {
                reason: format!(
                    "参照先のセグメント長が上限（{MAX_SEGMENT_LEN} バイト）を超えています: {segment}"
                ),
            });
        }
    }

    let path = TokenPath::new(segments)?;
    Ok(TokenRef::new(path))
}

/// `$value` 文字列が参照必須のフィールド（border/shadow の `color`）であることを
/// 検証したうえで [`parse_token_reference`] へ委譲する。
///
/// border/shadow の色フィールドは設計文書 §4.4「色を要求するフィールドは color
/// カテゴリのトークンのみを参照できる」制約により参照専用（直値を許容しない）。
fn parse_required_reference(s: &str, path: &TokenPath) -> Result<TokenRef, TokenError> {
    if !(s.starts_with('{') && s.ends_with('}')) {
        return Err(parse_error(
            path,
            format!(
                "color フィールドは '{{group.token}}' 形式の参照文字列である必要があります: {s}"
            ),
        ));
    }
    parse_token_reference(s)
}

/// JSON の `$type` 文字列からカテゴリへ変換する。
fn category_from_type_str(s: &str) -> Option<TokenCategory> {
    Some(match s {
        "color" => TokenCategory::Color,
        "dimension" => TokenCategory::Dimension,
        "text" => TokenCategory::Text,
        "typography" => TokenCategory::Typography,
        "spacing" => TokenCategory::Spacing,
        "size" => TokenCategory::Size,
        "radius" => TokenCategory::Radius,
        "elevation" => TokenCategory::Elevation,
        "border" => TokenCategory::Border,
        "shadow" => TokenCategory::Shadow,
        "duration" => TokenCategory::Duration,
        "animation" => TokenCategory::Animation,
        _ => return None,
    })
}

/// カテゴリから JSON の `$type` 文字列へ変換する（[`category_from_type_str`] の逆写像。
/// [`serialize_token_set_json`] が使う）。
fn category_type_str(category: TokenCategory) -> &'static str {
    match category {
        TokenCategory::Color => "color",
        TokenCategory::Dimension => "dimension",
        TokenCategory::Text => "text",
        TokenCategory::Typography => "typography",
        TokenCategory::Spacing => "spacing",
        TokenCategory::Size => "size",
        TokenCategory::Radius => "radius",
        TokenCategory::Elevation => "elevation",
        TokenCategory::Border => "border",
        TokenCategory::Shadow => "shadow",
        TokenCategory::Duration => "duration",
        TokenCategory::Animation => "animation",
    }
}

fn required_field<'a>(
    obj: &'a Map<String, Value>,
    field: &str,
    path: &TokenPath,
) -> Result<&'a Value, TokenError> {
    obj.get(field)
        .ok_or_else(|| parse_error(path, format!("必須フィールドがありません: {field}")))
}

fn str_field<'a>(
    obj: &'a Map<String, Value>,
    field: &str,
    path: &TokenPath,
) -> Result<&'a str, TokenError> {
    required_field(obj, field, path)?.as_str().ok_or_else(|| {
        parse_error(
            path,
            format!("フィールド {field} は文字列である必要があります"),
        )
    })
}

fn f64_field(obj: &Map<String, Value>, field: &str, path: &TokenPath) -> Result<f64, TokenError> {
    required_field(obj, field, path)?.as_f64().ok_or_else(|| {
        parse_error(
            path,
            format!("フィールド {field} は数値である必要があります"),
        )
    })
}

/// JSON 数値を非負整数として解釈する。`serde_json::Value::as_u64` は `2.0` のような
/// 小数点付きの整数値相当のリテラルを `None` として弾んでしまうため、Python の
/// `json` 等の外部ジェネレータが出力するこの形式も許容できるよう、整数表現に
/// フォールバックできない場合のみ小数表現を経由して整数値かどうか検証する
/// （Bugbot 指摘、PR #430）。
fn value_as_whole_u64(value: &Value) -> Option<u64> {
    if let Some(n) = value.as_u64() {
        return Some(n);
    }
    let f = value.as_f64()?;
    if f.is_finite() && f >= 0.0 && f.fract() == 0.0 && f <= u64::MAX as f64 {
        Some(f as u64)
    } else {
        None
    }
}

fn u64_field(obj: &Map<String, Value>, field: &str, path: &TokenPath) -> Result<u64, TokenError> {
    value_as_whole_u64(required_field(obj, field, path)?).ok_or_else(|| {
        parse_error(
            path,
            format!("フィールド {field} は非負整数である必要があります"),
        )
    })
}

fn decode_typography(value: &Value, path: &TokenPath) -> Result<TokenValue, TokenError> {
    let obj = value.as_object().ok_or_else(|| {
        parse_error(
            path,
            "typography の $value はオブジェクトである必要があります",
        )
    })?;
    let family = str_field(obj, "family", path)?;
    let size = f64_field(obj, "size", path)? as f32;
    let line_height = f64_field(obj, "lineHeight", path)? as f32;
    let weight_num = u64_field(obj, "weight", path)?;
    if weight_num > u64::from(u16::MAX) {
        return Err(parse_error(
            path,
            format!("weight が範囲外です: {weight_num}"),
        ));
    }
    let weight = FontWeightValue::new(weight_num as u16)?;
    let style = match str_field(obj, "style", path)? {
        "normal" => FontStyleValue::Normal,
        "italic" => FontStyleValue::Italic,
        other => {
            return Err(parse_error(
                path,
                format!("style は 'normal'/'italic' のいずれかである必要があります: {other}"),
            ));
        }
    };
    let typography = TypographyValue::new(family, size, line_height, weight, style)?;
    Ok(TokenValue::Typography(typography))
}

fn decode_border(value: &Value, path: &TokenPath) -> Result<TokenValue, TokenError> {
    let obj = value
        .as_object()
        .ok_or_else(|| parse_error(path, "border の $value はオブジェクトである必要があります"))?;
    let width = f64_field(obj, "width", path)? as f32;
    let style = match str_field(obj, "style", path)? {
        "solid" => BorderStyleValue::Solid,
        "dashed" => BorderStyleValue::Dashed,
        other => {
            return Err(parse_error(
                path,
                format!("style は 'solid'/'dashed' のいずれかである必要があります: {other}"),
            ));
        }
    };
    let color = parse_required_reference(str_field(obj, "color", path)?, path)?;
    let border = BorderValue::new(width, style, color)?;
    Ok(TokenValue::Border(border))
}

fn decode_shadow(value: &Value, path: &TokenPath) -> Result<TokenValue, TokenError> {
    let obj = value
        .as_object()
        .ok_or_else(|| parse_error(path, "shadow の $value はオブジェクトである必要があります"))?;
    let offset_x = f64_field(obj, "offsetX", path)? as f32;
    let offset_y = f64_field(obj, "offsetY", path)? as f32;
    let blur_radius = f64_field(obj, "blurRadius", path)? as f32;
    let color = parse_required_reference(str_field(obj, "color", path)?, path)?;
    let shadow = ShadowValue::new(offset_x, offset_y, blur_radius, color)?;
    Ok(TokenValue::Shadow(shadow))
}

fn decode_animation(value: &Value, path: &TokenPath) -> Result<TokenValue, TokenError> {
    let obj = value.as_object().ok_or_else(|| {
        parse_error(
            path,
            "animation の $value はオブジェクトである必要があります",
        )
    })?;
    let duration_ms = f64_field(obj, "duration", path)? as f32;
    let duration = DurationValue::new(duration_ms)?;
    let easing = match str_field(obj, "easing", path)? {
        "linear" => EasingValue::Linear,
        "ease-in" => EasingValue::EaseIn,
        "ease-out" => EasingValue::EaseOut,
        "ease-in-out" => EasingValue::EaseInOut,
        other => {
            return Err(parse_error(
                path,
                format!(
                    "easing は 'linear'/'ease-in'/'ease-out'/'ease-in-out' のいずれかである必要があります: {other}"
                ),
            ));
        }
    };
    Ok(TokenValue::Animation(AnimationValue::new(duration, easing)))
}

/// カテゴリ別エンコーディング表（実装計画 §2.2）に従い `$value` を [`TokenValue`] へ
/// デコードする。値域検証は各カテゴリの検証付きコンストラクタへ委譲する。
fn decode_value_for_category(
    category: TokenCategory,
    value: &Value,
    path: &TokenPath,
) -> Result<TokenValue, TokenError> {
    match category {
        TokenCategory::Color => {
            let s = value
                .as_str()
                .ok_or_else(|| parse_error(path, "color の $value は文字列である必要があります"))?;
            Ok(TokenValue::Color(ColorValue::from_hex(s)?))
        }
        TokenCategory::Dimension => {
            let n = value.as_f64().ok_or_else(|| {
                parse_error(path, "dimension の $value は数値である必要があります")
            })?;
            Ok(TokenValue::Dimension(n))
        }
        TokenCategory::Text => {
            let s = value
                .as_str()
                .ok_or_else(|| parse_error(path, "text の $value は文字列である必要があります"))?;
            Ok(TokenValue::Text(s.to_string()))
        }
        TokenCategory::Typography => decode_typography(value, path),
        TokenCategory::Spacing => {
            let n = value
                .as_f64()
                .ok_or_else(|| parse_error(path, "spacing の $value は数値である必要があります"))?
                as f32;
            Ok(TokenValue::Spacing(SpacingValue::new(n)?))
        }
        TokenCategory::Size => {
            let n = value
                .as_f64()
                .ok_or_else(|| parse_error(path, "size の $value は数値である必要があります"))?
                as f32;
            Ok(TokenValue::Size(DimensionValue::new(n)?))
        }
        TokenCategory::Radius => {
            let n = value
                .as_f64()
                .ok_or_else(|| parse_error(path, "radius の $value は数値である必要があります"))?
                as f32;
            Ok(TokenValue::Radius(DimensionValue::new(n)?))
        }
        TokenCategory::Elevation => {
            let n = value_as_whole_u64(value).ok_or_else(|| {
                parse_error(path, "elevation の $value は非負整数である必要があります")
            })?;
            if n > u64::from(u8::MAX) {
                return Err(parse_error(
                    path,
                    format!("elevation は 0..=255 の範囲である必要があります: {n}"),
                ));
            }
            Ok(TokenValue::Elevation(ElevationValue::new(n as u8)))
        }
        TokenCategory::Border => decode_border(value, path),
        TokenCategory::Shadow => decode_shadow(value, path),
        TokenCategory::Duration => {
            let n = value
                .as_f64()
                .ok_or_else(|| parse_error(path, "duration の $value は数値である必要があります"))?
                as f32;
            Ok(TokenValue::Duration(DurationValue::new(n)?))
        }
        TokenCategory::Animation => decode_animation(value, path),
    }
}

/// トークン 1 件分（`$type` + `$value` を持つオブジェクト）を [`TokenDefinition`] へ
/// 変換する。`$value` 全体が `"{group.token}"` 形式の文字列の場合は参照
/// （[`TokenDefinition::Reference`]）として扱う（実装計画 §2.2「準拠度」）。
fn parse_token_definition(
    path: &TokenPath,
    obj: &Map<String, Value>,
) -> Result<TokenDefinition, TokenError> {
    let type_str = str_field(obj, "$type", path)?;
    let category = category_from_type_str(type_str)
        .ok_or_else(|| parse_error(path, format!("未知の $type です: {type_str}")))?;
    let value = required_field(obj, "$value", path)?;

    if let Some(s) = value.as_str() {
        if s.starts_with('{') && s.ends_with('}') {
            let reference = parse_token_reference(s)?;
            return Ok(TokenDefinition::Reference {
                category,
                reference,
            });
        }
    }

    let token_value = decode_value_for_category(category, value, path)?;
    Ok(TokenDefinition::Value { value: token_value })
}

/// JSON オブジェクトツリーを再帰的に走査し、`$type` を持つオブジェクトをトークン
/// 定義として、それ以外をグループとして扱う（実装計画 §4 ステップ 4）。
///
/// `$` で始まるキー（`$description` / `$extensions` 等）はグループ直下でも
/// スキップする（`TokenPath` の許可文字集合に `$` を含まないため、誤ってパス
/// セグメントとして扱うと必ず `InvalidPath` になってしまう。仕様上は無視する
/// 契約であるため、ここで除外する）。
fn walk_group(
    obj: &Map<String, Value>,
    segments: &mut Vec<String>,
    set: &mut TokenSet,
    count: &mut usize,
) -> Result<(), TokenError> {
    for (key, value) in obj {
        if key.starts_with('$') {
            continue;
        }
        if key.len() > MAX_SEGMENT_LEN {
            return Err(TokenError::ParseError {
                path: segments.join("."),
                reason: format!(
                    "セグメント長が上限（{MAX_SEGMENT_LEN} バイト）を超えています: {key}"
                ),
            });
        }

        segments.push(key.clone());
        if segments.len() > MAX_PATH_SEGMENTS {
            return Err(TokenError::ParseError {
                path: segments.join("."),
                reason: format!(
                    "ネスト深さ/セグメント数が上限（{MAX_PATH_SEGMENTS}）を超えています"
                ),
            });
        }

        let child_obj = value.as_object().ok_or_else(|| TokenError::ParseError {
            path: segments.join("."),
            reason: "トークン・グループはオブジェクトである必要があります".to_string(),
        })?;

        if child_obj.contains_key("$type") {
            *count += 1;
            if *count > MAX_TOKEN_COUNT {
                return Err(TokenError::ParseError {
                    path: segments.join("."),
                    reason: format!("トークン件数が上限（{MAX_TOKEN_COUNT}）を超えています"),
                });
            }
            let path = TokenPath::new(segments.clone())?;
            let definition = parse_token_definition(&path, child_obj)?;
            set.insert(path, definition);
        } else if child_obj.contains_key("$value") {
            // `$type` を持たず `$value` のみを持つノードはトークンとして不完全な
            // 形状であり、グループとして黙って走査すると `$value` がグループ直下
            // キー（`$` 始まり）としてスキップされ、トークンが失われたまま
            // parsing が成功してしまう。本モジュールの fail-fast な構文検証契約
            // （モジュール冒頭コメント §「S18 との境界」参照）に反するため、
            // ここで明示的にエラーとして拒否する。
            return Err(TokenError::ParseError {
                path: segments.join("."),
                reason: "'$value' を持つノードには '$type' が必須です".to_string(),
            });
        } else {
            walk_group(child_obj, segments, set, count)?;
        }

        segments.pop();
    }
    Ok(())
}

/// JSON 文字列から [`TokenSet`] を構築する（外部定義ファイル読み込みの構文
/// パース境界本体、TASK-8.4-S17）。
///
/// `TokenResolver` へ渡す前に S18（#174）のバリデーションを経る前提であり
/// （§5.3）、本関数自体は resolver を呼び出さない。
///
/// # Errors
///
/// JSON として解析できない、ルートがオブジェクトでない、トークン・グループの
/// 形状が不正、値域検証（各カテゴリの検証付きコンストラクタ）に失敗した、
/// いずれかの上限（ファイルサイズは呼び出し元の [`load_token_set_file`] が
/// 検査するため対象外・トークン件数・ネスト深さ/セグメント数・セグメント長・
/// 参照文字列長）を超えた場合に、対応する `TokenError` バリアントを返す。
pub fn parse_token_set_json(input: &str) -> Result<TokenSet, TokenError> {
    let root: Value = serde_json::from_str(input)
        .map_err(|e| root_parse_error(format!("JSON として解析できません: {e}")))?;
    let obj = root
        .as_object()
        .ok_or_else(|| root_parse_error("ルートはオブジェクトである必要があります"))?;

    let mut set = TokenSet::new();
    let mut segments = Vec::new();
    let mut count = 0usize;
    walk_group(obj, &mut segments, &mut set, &mut count)?;
    Ok(set)
}

/// サイズ上限を検査したうえでファイルを読み込み、[`parse_token_set_json`] へ
/// 委譲する（TASK-8.4-S17）。
///
/// # Errors
///
/// メタデータ取得・読み込みに失敗した場合、または [`MAX_TOKEN_FILE_BYTES`] を
/// 超える場合に [`TokenError::FileRead`] を返す。JSON の構文検証に失敗した
/// 場合は [`parse_token_set_json`] のエラーをそのまま返す。
pub fn load_token_set_file(path: &Path) -> Result<TokenSet, TokenError> {
    let metadata = fs::metadata(path).map_err(|e| TokenError::FileRead {
        reason: format!("{e}"),
    })?;
    if metadata.len() > MAX_TOKEN_FILE_BYTES {
        return Err(TokenError::FileRead {
            reason: format!(
                "ファイルサイズが上限（{MAX_TOKEN_FILE_BYTES} バイト）を超えています: {} バイト",
                metadata.len()
            ),
        });
    }

    let content = fs::read_to_string(path).map_err(|e| TokenError::FileRead {
        reason: format!("{e}"),
    })?;
    parse_token_set_json(&content)
}

fn number_value(n: f64, path: &TokenPath) -> Result<Value, TokenError> {
    Number::from_f64(n).map(Value::Number).ok_or_else(|| {
        parse_error(
            path,
            format!("数値をシリアライズできません（非有限値）: {n}"),
        )
    })
}

/// [`TokenValue`] を JSON の `$value`（または typography/border/shadow/animation の
/// フィールド群を含むオブジェクト）へ変換する。
fn serialize_value(value: &TokenValue, path: &TokenPath) -> Result<Value, TokenError> {
    match value {
        TokenValue::Color(color) => Ok(Value::String(color.to_hex())),
        TokenValue::Dimension(n) => number_value(*n, path),
        TokenValue::Text(s) => Ok(Value::String(s.clone())),
        TokenValue::Typography(typography) => {
            let mut obj = Map::new();
            obj.insert(
                "family".to_string(),
                Value::String(typography.family().to_string()),
            );
            obj.insert(
                "size".to_string(),
                number_value(f64::from(typography.size()), path)?,
            );
            obj.insert(
                "lineHeight".to_string(),
                number_value(f64::from(typography.line_height()), path)?,
            );
            obj.insert(
                "weight".to_string(),
                Value::Number(Number::from(typography.weight().value())),
            );
            obj.insert(
                "style".to_string(),
                Value::String(
                    match typography.style() {
                        FontStyleValue::Normal => "normal",
                        FontStyleValue::Italic => "italic",
                    }
                    .to_string(),
                ),
            );
            Ok(Value::Object(obj))
        }
        TokenValue::Spacing(spacing) => number_value(f64::from(spacing.value()), path),
        TokenValue::Size(size) => number_value(f64::from(size.value()), path),
        TokenValue::Radius(radius) => number_value(f64::from(radius.value()), path),
        TokenValue::Elevation(elevation) => Ok(Value::Number(Number::from(elevation.level()))),
        TokenValue::Border(border) => {
            let mut obj = Map::new();
            obj.insert(
                "width".to_string(),
                number_value(f64::from(border.width()), path)?,
            );
            obj.insert(
                "style".to_string(),
                Value::String(
                    match border.style() {
                        BorderStyleValue::Solid => "solid",
                        BorderStyleValue::Dashed => "dashed",
                    }
                    .to_string(),
                ),
            );
            obj.insert(
                "color".to_string(),
                Value::String(format!("{{{}}}", border.color())),
            );
            Ok(Value::Object(obj))
        }
        TokenValue::Shadow(shadow) => {
            let mut obj = Map::new();
            obj.insert(
                "offsetX".to_string(),
                number_value(f64::from(shadow.offset_x()), path)?,
            );
            obj.insert(
                "offsetY".to_string(),
                number_value(f64::from(shadow.offset_y()), path)?,
            );
            obj.insert(
                "blurRadius".to_string(),
                number_value(f64::from(shadow.blur_radius()), path)?,
            );
            obj.insert(
                "color".to_string(),
                Value::String(format!("{{{}}}", shadow.color())),
            );
            Ok(Value::Object(obj))
        }
        TokenValue::Duration(duration) => number_value(f64::from(duration.milliseconds()), path),
        TokenValue::Animation(animation) => {
            let mut obj = Map::new();
            obj.insert(
                "duration".to_string(),
                number_value(f64::from(animation.duration().milliseconds()), path)?,
            );
            obj.insert(
                "easing".to_string(),
                Value::String(
                    match animation.easing() {
                        EasingValue::Linear => "linear",
                        EasingValue::EaseIn => "ease-in",
                        EasingValue::EaseOut => "ease-out",
                        EasingValue::EaseInOut => "ease-in-out",
                    }
                    .to_string(),
                ),
            );
            Ok(Value::Object(obj))
        }
    }
}

/// [`TokenDefinition`] を `$type`/`$value` を持つ JSON オブジェクトへ変換する。
fn serialize_definition(
    definition: &TokenDefinition,
    path: &TokenPath,
) -> Result<Value, TokenError> {
    let mut obj = Map::new();
    match definition {
        TokenDefinition::Reference {
            category,
            reference,
        } => {
            obj.insert(
                "$type".to_string(),
                Value::String(category_type_str(*category).to_string()),
            );
            obj.insert(
                "$value".to_string(),
                Value::String(format!("{{{reference}}}")),
            );
        }
        TokenDefinition::Value { value } => {
            obj.insert(
                "$type".to_string(),
                Value::String(category_type_str(value.category()).to_string()),
            );
            obj.insert("$value".to_string(), serialize_value(value, path)?);
        }
    }
    Ok(Value::Object(obj))
}

/// あるトークンパスが別のトークンパスの真の接頭辞である（ネスト表現不能）ことを
/// 表すエラーを生成する。
fn prefix_conflict_error(path: &TokenPath) -> TokenError {
    parse_error(
        path,
        "他のトークンパスの接頭辞であるため、ネストしたグループとして表現できません",
    )
}

/// ソート済みの 1 エントリをネストされたグループ構造へ挿入する（再帰）。
///
/// 呼び出し元（[`serialize_token_set_json`]）がキーをセグメント列の辞書式順序
/// でソート済みであることを前提とする。これにより、あるパスが別のパスの
/// 真の接頭辞である場合、短い方が必ず先に処理される（`Vec<String>` の辞書式
/// 順序は接頭辞を「より小さい」と扱うため）。長いパスを処理する際に短い方が
/// 既にリーフ（`$type` を持つオブジェクト）として存在していれば衝突として
/// 検出できる。
fn insert_definition(
    node: &mut Map<String, Value>,
    segments: &[String],
    definition: &TokenDefinition,
    full_path: &TokenPath,
) -> Result<(), TokenError> {
    let Some((head, rest)) = segments.split_first() else {
        // TokenPath::new が 1 つ以上のセグメントを保証するため到達しない。
        return Err(prefix_conflict_error(full_path));
    };

    if rest.is_empty() {
        if let Some(Value::Object(child)) = node.get(head) {
            if !child.is_empty() {
                return Err(prefix_conflict_error(full_path));
            }
        }
        let leaf = serialize_definition(definition, full_path)?;
        node.insert(head.clone(), leaf);
        return Ok(());
    }

    let entry = node
        .entry(head.clone())
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(child) = entry else {
        return Err(prefix_conflict_error(full_path));
    };
    if child.contains_key("$type") {
        return Err(prefix_conflict_error(full_path));
    }
    insert_definition(child, rest, definition, full_path)
}

/// [`TokenSet`] を決定的な（キーをソート済みの）JSON 文字列へシリアライズする
/// （TASK-8.4-S17）。
///
/// パスをネストされたグループへ展開する。あるトークンパスが別のトークンパスの
/// 真の接頭辞になる場合（ネスト表現不能）は [`TokenError::ParseError`] で拒否する。
///
/// # Errors
///
/// 接頭辞衝突を検出した場合、値が非有限で JSON 数値として表現できない場合
/// （`TokenValue::Dimension` は生の `f64` で検証を経ないため理論上あり得る）、
/// または `serde_json` のシリアライズに失敗した場合に返す。
pub fn serialize_token_set_json(set: &TokenSet) -> Result<String, TokenError> {
    let mut entries: Vec<(&TokenPath, &TokenDefinition)> = set.iter().collect();
    entries.sort_by(|a, b| a.0.segments().cmp(b.0.segments()));

    let mut root = Map::new();
    for (path, definition) in entries {
        insert_definition(&mut root, path.segments(), definition, path)?;
    }

    serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|e| root_parse_error(format!("JSON へのシリアライズに失敗しました: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::color::palette_token_set;

    #[test]
    fn parse_token_reference_accepts_multi_segment_path() {
        let reference = parse_token_reference("{color.palette.blue-500}").expect("正当な参照");
        assert_eq!(
            reference.path().segments(),
            &["color", "palette", "blue-500"]
        );
    }

    #[test]
    fn parse_token_reference_rejects_missing_braces() {
        let err = parse_token_reference("color.primary").expect_err("波括弧欠落は拒否される");
        assert!(matches!(err, TokenError::InvalidReferenceSyntax { .. }));
    }

    #[test]
    fn parse_token_reference_rejects_empty_path() {
        let err = parse_token_reference("{}").expect_err("空パスは拒否される");
        assert!(matches!(err, TokenError::InvalidReferenceSyntax { .. }));
    }

    #[test]
    fn parse_token_reference_rejects_disallowed_characters() {
        let err = parse_token_reference("{color/primary}").expect_err("許可外文字は拒否される");
        assert!(matches!(err, TokenError::InvalidPath { .. }));
    }

    #[test]
    fn parse_token_reference_rejects_path_traversal_segment() {
        let err = parse_token_reference("{color..primary}").expect_err("空セグメントは拒否される");
        assert!(matches!(err, TokenError::InvalidPath { .. }));

        let err = parse_token_reference("{a...}").expect_err("`..` セグメントは拒否される");
        assert!(matches!(err, TokenError::InvalidPath { .. }));
    }

    #[test]
    fn parse_token_reference_rejects_oversized_reference() {
        let long = format!("{{{}}}", "a".repeat(MAX_REFERENCE_LEN));
        let err = parse_token_reference(&long).expect_err("長さ超過は拒否される");
        assert!(matches!(err, TokenError::InvalidReferenceSyntax { .. }));
    }

    #[test]
    fn parse_token_reference_rejects_too_many_segments() {
        let inner = vec!["a"; MAX_PATH_SEGMENTS + 1].join(".");
        let err = parse_token_reference(&format!("{{{inner}}}"))
            .expect_err("セグメント数超過は拒否される");
        assert!(matches!(err, TokenError::InvalidReferenceSyntax { .. }));
    }

    #[test]
    fn parse_token_set_json_decodes_color_and_reference() {
        let json = r##"{
            "color": {
                "primary": { "$type": "color", "$value": "#3b82f6" },
                "accent": { "$type": "color", "$value": "{color.primary}" }
            }
        }"##;
        let set = parse_token_set_json(json).expect("正当な JSON は解析できる");
        assert_eq!(set.len(), 2);

        let primary = TokenPath::from_dotted("color.primary").expect("パスは有効");
        match set.get(&primary).expect("存在する") {
            TokenDefinition::Value {
                value: TokenValue::Color(color),
            } => assert_eq!(*color, ColorValue::from_hex("#3b82f6").expect("正当な色")),
            other => panic!("Color 値であるべきです: {other:?}"),
        }

        let accent = TokenPath::from_dotted("color.accent").expect("パスは有効");
        match set.get(&accent).expect("存在する") {
            TokenDefinition::Reference {
                category,
                reference,
            } => {
                assert_eq!(*category, TokenCategory::Color);
                assert_eq!(reference.path().segments(), &["color", "primary"]);
            }
            other => panic!("Reference であるべきです: {other:?}"),
        }
    }

    #[test]
    fn parse_token_set_json_decodes_all_twelve_categories() {
        let json = r##"{
            "color": { "c": { "$type": "color", "$value": "#000000" } },
            "dimension": { "d": { "$type": "dimension", "$value": 4.0 } },
            "text": { "t": { "$type": "text", "$value": "hello" } },
            "typography": { "t": { "$type": "typography", "$value": {
                "family": "sans-serif", "size": 16.0, "lineHeight": 24.0,
                "weight": 400, "style": "normal"
            } } },
            "spacing": { "s": { "$type": "spacing", "$value": 8.0 } },
            "size": { "s": { "$type": "size", "$value": 32.0 } },
            "radius": { "r": { "$type": "radius", "$value": 8.0 } },
            "elevation": { "e": { "$type": "elevation", "$value": 2 } },
            "border": { "b": { "$type": "border", "$value": {
                "width": 1.0, "style": "solid", "color": "{color.c}"
            } } },
            "shadow": { "sh": { "$type": "shadow", "$value": {
                "offsetX": 0.0, "offsetY": 2.0, "blurRadius": 4.0, "color": "{color.c}"
            } } },
            "duration": { "d": { "$type": "duration", "$value": 200.0 } },
            "animation": { "a": { "$type": "animation", "$value": {
                "duration": 200.0, "easing": "ease-in-out"
            } } }
        }"##;
        let set = parse_token_set_json(json).expect("12 カテゴリすべてを解析できる");
        assert_eq!(set.len(), 12);
    }

    #[test]
    fn parse_token_set_json_accepts_whole_float_for_integer_fields() {
        // Bugbot 指摘（PR #430）: Python の `json` 等が出力する `2.0` / `400.0` の
        // ような小数点付きの整数値相当のリテラルを、elevation・typography の
        // weight で拒否せず受理できることを確認する回帰テスト。
        let json = r##"{
            "typography": { "t": { "$type": "typography", "$value": {
                "family": "sans-serif", "size": 16.0, "lineHeight": 24.0,
                "weight": 400.0, "style": "normal"
            } } },
            "elevation": { "e": { "$type": "elevation", "$value": 2.0 } }
        }"##;
        let set = parse_token_set_json(json).expect("小数点付きの整数値相当は解析できる");

        let typography_path = TokenPath::from_dotted("typography.t").expect("パスは有効");
        match set.get(&typography_path).expect("存在する") {
            TokenDefinition::Value {
                value: TokenValue::Typography(typography),
            } => assert_eq!(typography.weight().value(), 400),
            other => panic!("Typography 値であるべきです: {other:?}"),
        }

        let elevation_path = TokenPath::from_dotted("elevation.e").expect("パスは有効");
        match set.get(&elevation_path).expect("存在する") {
            TokenDefinition::Value {
                value: TokenValue::Elevation(elevation),
            } => assert_eq!(elevation.level(), 2),
            other => panic!("Elevation 値であるべきです: {other:?}"),
        }
    }

    #[test]
    fn parse_token_set_json_rejects_unknown_type() {
        let json = r#"{ "x": { "y": { "$type": "unknown", "$value": 1 } } }"#;
        let err = parse_token_set_json(json).expect_err("未知の $type は拒否される");
        assert!(matches!(err, TokenError::ParseError { .. }));
    }

    #[test]
    fn parse_token_set_json_rejects_value_without_type() {
        // `$type` を持たず `$value` のみを持つノードをグループとして黙って走査
        // すると、トークンが失われたまま parsing が成功してしまう回帰を防ぐ
        // （Bugbot 指摘 f7407fe5-4d65-40ad-b6da-97df74f8a907）。
        let json = r##"{ "x": { "y": { "$value": "#ffffff" } } }"##;
        let err =
            parse_token_set_json(json).expect_err("'$type' の無い '$value' ノードは拒否される");
        assert!(matches!(err, TokenError::ParseError { .. }));
    }

    #[test]
    fn parse_token_set_json_ignores_description_and_extensions() {
        let json = r##"{
            "$description": "root description",
            "color": {
                "$description": "group description",
                "primary": {
                    "$type": "color",
                    "$value": "#ffffff",
                    "$description": "primary color",
                    "$extensions": { "vendor": "example" }
                }
            }
        }"##;
        let set = parse_token_set_json(json).expect("説明・拡張フィールドは無視される");
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn parse_token_set_json_rejects_non_object_root() {
        let err = parse_token_set_json("[]").expect_err("配列ルートは拒否される");
        assert!(matches!(err, TokenError::ParseError { .. }));
    }

    #[test]
    fn parse_token_set_json_rejects_invalid_json() {
        let err = parse_token_set_json("{ not json").expect_err("不正な JSON は拒否される");
        assert!(matches!(err, TokenError::ParseError { .. }));
    }

    #[test]
    fn parse_token_set_json_rejects_token_count_over_limit() {
        let mut body = String::from("{");
        for i in 0..=MAX_TOKEN_COUNT {
            if i > 0 {
                body.push(',');
            }
            body.push_str(&format!(
                r#""t{i}": {{ "$type": "dimension", "$value": 1.0 }}"#
            ));
        }
        body.push('}');
        let err = parse_token_set_json(&body).expect_err("上限超過は拒否される");
        assert!(matches!(err, TokenError::ParseError { .. }));
    }

    #[test]
    fn parse_token_set_json_rejects_nesting_over_limit() {
        let mut json = String::from(r#"{ "$type": "dimension", "$value": 1.0 }"#);
        for _ in 0..=MAX_PATH_SEGMENTS {
            json = format!(r#"{{ "g": {json} }}"#);
        }
        let err = parse_token_set_json(&json).expect_err("ネスト深さ超過は拒否される");
        assert!(matches!(err, TokenError::ParseError { .. }));
    }

    #[test]
    fn serialize_token_set_json_round_trips_palette() {
        let original = palette_token_set().expect("パレットは構築できる");
        let json = serialize_token_set_json(&original).expect("シリアライズできる");
        let parsed = parse_token_set_json(&json).expect("再パースできる");
        assert_eq!(original, parsed);
    }

    #[test]
    fn serialize_token_set_json_produces_sorted_deterministic_output() {
        let set = palette_token_set().expect("パレットは構築できる");
        let first = serialize_token_set_json(&set).expect("1 回目のシリアライズ");
        let second = serialize_token_set_json(&set).expect("2 回目のシリアライズ");
        assert_eq!(first, second);
    }

    #[test]
    fn serialize_token_set_json_rejects_prefix_conflict() {
        let mut set = TokenSet::new();
        let short = TokenPath::from_dotted("color").expect("パスは有効");
        let long = TokenPath::from_dotted("color.primary").expect("パスは有効");
        set.insert(
            short,
            TokenDefinition::Value {
                value: TokenValue::Color(ColorValue::rgb(0, 0, 0)),
            },
        );
        set.insert(
            long,
            TokenDefinition::Value {
                value: TokenValue::Color(ColorValue::rgb(255, 255, 255)),
            },
        );
        let err = serialize_token_set_json(&set).expect_err("接頭辞衝突は拒否される");
        assert!(matches!(err, TokenError::ParseError { .. }));
    }

    #[test]
    fn round_trip_preserves_reference_definition() {
        let mut set = TokenSet::new();
        let base = TokenPath::from_dotted("color.base").expect("パスは有効");
        let alias = TokenPath::from_dotted("color.alias").expect("パスは有効");
        set.insert(
            base.clone(),
            TokenDefinition::Value {
                value: TokenValue::Color(ColorValue::rgb(10, 20, 30)),
            },
        );
        set.insert(
            alias,
            TokenDefinition::Reference {
                category: TokenCategory::Color,
                reference: TokenRef::new(base),
            },
        );
        let json = serialize_token_set_json(&set).expect("シリアライズできる");
        let parsed = parse_token_set_json(&json).expect("再パースできる");
        assert_eq!(set, parsed);
    }

    #[test]
    fn round_trip_preserves_animation_and_duration() {
        let animation = AnimationValue::new(
            DurationValue::new(150.0).expect("正当な値"),
            EasingValue::EaseOut,
        );
        let mut set = TokenSet::new();
        let path = TokenPath::from_dotted("animation.standard").expect("パスは有効");
        set.insert(
            path,
            TokenDefinition::Value {
                value: TokenValue::Animation(animation),
            },
        );
        let json = serialize_token_set_json(&set).expect("シリアライズできる");
        let parsed = parse_token_set_json(&json).expect("再パースできる");
        assert_eq!(set, parsed);
    }
}
