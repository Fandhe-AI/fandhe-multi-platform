//! トークン参照・エイリアスの解決エンジン（TASK-8.4-S3、#159）。
//!
//! `docs/architecture/token-resolution-architecture.md`（TASK-8.4-S2、#158 の
//! 成果物）§4 の API 案・§5 の境界条件を実装契約として引き継ぐ。二層アーキテクチャ
//! （REQ-2）上は共通コア（`fandhe-core`）に閉じ、描画バックエンド等のプラットフォーム
//! 固有クレートには依存しない純粋なデータ変換として実装する。
//!
//! **データフロー契約（設計文書 §5.3）**: 本エンジンは S17（#173、外部定義ファイル
//! 読み込み）が読み込み、S18（#174、バリデーション）が検証済みの `TokenSet` を
//! 受け取る前提であり、生のデシリアライズ結果を直接受け取らない
//! （S17 → S18 → 解決エンジンの順序。この契約を迂回する経路を将来追加する際は
//! 本コメントをレビュー観点として維持すること）。

use std::collections::HashMap;
use std::collections::HashSet;

use super::error::TokenError;
use super::schema::{TokenDefinition, TokenPath, TokenRef, TokenSet, TokenValue};
use super::theme::{Theme, ThemeVariant};
use super::typed::{TokenKind, TypedTokenRef};

/// エイリアスチェーンの最大解決深さ（設計文書 §5.2 の DoS 対策）。
///
/// 通常のデザイントークン運用（2〜3 段のエイリアスが一般的）を大きく超える値として
/// 「数十段」を初期値とする設計文書の指示に基づき 32 とした。設定可能にするかは
/// §7 の残課題（固定値とする）。
pub const MAX_ALIAS_DEPTH: u32 = 32;

/// 解決済みの最終値（設計文件 §4 の `ResolvedValue` 型シグネチャ案）。
///
/// トークンのカテゴリを跨いだ暗黙変換を行わないため、`TokenValue` のバリアントを
/// そのまま保持する。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedValue(TokenValue);

impl ResolvedValue {
    /// 解決済みの値を参照で返す。
    pub fn value(&self) -> &TokenValue {
        &self.0
    }
}

/// トークン間の参照・エイリアスを解決するエンジン（設計文書 §4）。
///
/// 構築時にベーストークンセットとアクティブテーマの差分を受け取り、`resolve` で
/// 個別のトークン参照を最終値まで解決する。解決結果は `resolve_all` の呼び出しで
/// 内部キャッシュへ反映され、テーマ切替（S4/S5、#160/#161）時は `set_theme_overrides`
/// でアクティブテーマの差分を差し替えてキャッシュ全体を無効化し、`resolve_all` を
/// 再実行する契約とする（設計文書 §4.3。部分無効化はスコープ外）。
///
/// `set_active_theme`（S5、#161）はこれをより高レベルな契約として提供し、
/// [`ThemeVariant`] 単位でライト/ダーク両方の差分を保持したまま切替を行う
/// （`with_themes` で構築した場合。`new` 経由で構築した場合は互換維持のため
/// 単一の差分のみを保持し、もう一方のバリアントへの切替は空差分＝ベース値
/// フォールバックとして扱う。フィールド doc 参照）。
#[derive(Debug)]
pub struct TokenResolver {
    /// ベーストークンセット（テーマに依存しない既定値）。
    base: TokenSet,
    /// アクティブテーマの上書きマップ（同一キーに対しベースより優先される、§3.3）。
    ///
    /// `set_theme_overrides`（低レベル API）が直接差し替える対象であり、
    /// `set_active_theme`（高レベル API）はこのフィールドを `themes` 配列から
    /// 導出して更新する。両 API は独立しており、`set_theme_overrides` の呼び出しは
    /// `themes` 配列を更新しない（`set_theme_overrides` doc 参照）。
    theme_overrides: TokenSet,
    /// `resolve_all` が一括解決した結果のキャッシュ。個別の `resolve` 呼び出しは
    /// このキャッシュを可能な限り参照する（毎回チェーンを辿るコストを避けるため）。
    cache: HashMap<TokenPath, ResolvedValue>,
    /// 現在のアクティブテーマ区分（設計文書 §3.1）。
    active: ThemeVariant,
    /// テーマ世代カウンタ（設計文書 §3.2）。`state/store.rs` の
    /// `StateStore::version` とは独立した系統であり、テーマ切替（`set_active_theme`
    /// が異なるバリアントへ切り替わった場合のみ）でのみ増加する。
    generation: u64,
    /// `set_active_theme` が切替時に参照するライト/ダーク両方の差分
    /// （`ThemeVariant::as_index` でインデックスする、`[Light, Dark]` の順）。
    /// `new` で構築した場合は `[theme_overrides, TokenSet::new()]`
    /// （もう一方のバリアントは「差分なし＝ベース値フォールバック」として扱う、
    /// 互換維持のための既定値）。
    themes: [TokenSet; 2],
}

impl TokenResolver {
    /// ベーストークンセットとアクティブテーマの差分から解決エンジンを構築する。
    ///
    /// S19（#175、AI 型安全 DSL からのトークン参照 API）はこの構築をアプリ起動時・
    /// テーマ切替時に呼び出す想定（設計文書 §4）。テーマの切替トリガー・データ構造
    /// 自体は S4/S5（#160/#161）のスコープであり、本コンストラクタは「差分マップを
    /// 受け取る窓口」のみを担う。S19 は本エンジンを直接使うほか、[`Self::resolve_typed`]
    /// 経由で型付きトークン参照（`typed::TypedTokenRef`）からも利用する。
    ///
    /// 外部由来（S17、#173）のトークンセットは本コンストラクタを迂回せず
    /// `TokenResolver::validated` 経由で構築すること（設計文書 §5.3 のレビュー
    /// 観点、TASK-8.4-S18、#174）。本コンストラクタは内部・テスト用に未検証の
    /// セットで直接構築できる経路として残置する。
    pub fn new(base: TokenSet, theme_overrides: TokenSet) -> Self {
        Self {
            base,
            themes: [theme_overrides.clone(), TokenSet::new()],
            theme_overrides,
            cache: HashMap::new(),
            active: ThemeVariant::Light,
            generation: 0,
        }
    }

    /// `super::validation::validate_token_sets` による検証を通過した場合のみ
    /// 構築する、設計文書 §5.3 の「検証済みセットのみで解決エンジンを構築する」
    /// データフロー契約（S17 → S18 → 解決エンジン）を型レベルで導線化した
    /// コンストラクタ（TASK-8.4-S18、#174）。
    ///
    /// S17（#173、外部定義ファイル読み込み）が生成した `TokenSet` は、検証を
    /// 経ずに `TokenResolver::new` へ渡す経路を迂回しないこと（本コンストラクタが
    /// レビュー観点）。検証に失敗した場合は所有権を渡した引数をそのまま消費し
    /// `TokenValidationReport` を返す（`TokenResolver::new` は内部・テスト用の
    /// 経路として残置する）。
    ///
    /// **保証の範囲（TASK-8.4-S18、#174）**: `validate_token_sets`
    /// は `theme_overrides` に対してのみでなく、ベースがテーマ差分無しでも
    /// 単独で解決できることも検証する（同関数 doc の 3. 参照）。このため本
    /// コンストラクタが返す `Ok` は「この `(base, theme_overrides)` の組み合わせ」
    /// だけでなく「この `base` がどのようなテーマ差分（テーマ差分無しを含む）と
    /// 組み合わされても、ベース側に起因する解決エラーは起きない」ことまでを
    /// 保証する。ただし `theme_overrides` を丸ごと **別の未検証セット**へ差し替える
    /// 場合（`set_theme_overrides` 参照）は、その新しいセット自体が持ち込む
    /// 問題（新規キーの未定義参照・カテゴリ不一致等）まではこの構築時点の検証
    /// ではカバーされない。テーマ切替時は `set_validated_theme_overrides` を
    /// 経由し、新しい組み合わせを再検証すること。
    pub fn validated(
        base: TokenSet,
        theme_overrides: TokenSet,
    ) -> Result<Self, super::validation::TokenValidationReport> {
        super::validation::validate_token_sets(&base, &theme_overrides)?;
        Ok(Self::new(base, theme_overrides))
    }

    /// ベーストークンセットとライト/ダーク両方の [`Theme`] から解決エンジンを
    /// 構築する（設計文書 §3.1 の `new(tokens, initial_theme)` 読み替えを、
    /// 両テーマの差分を保持できる形で具体化した高レベルコンストラクタ、
    /// TASK-8.4-S5、#161）。
    ///
    /// [`Self::new`]（低レベル・単一差分のみ）と異なり、構築後に
    /// [`Self::set_active_theme`] でライト/ダーク間を行き来できる（両方の
    /// 差分をこのコンストラクタで受け取っているため）。`light` / `dark` の
    /// [`Theme::variant`] は呼び出し元がそれぞれ `ThemeVariant::Light` /
    /// `ThemeVariant::Dark` を渡す前提だが、本コンストラクタ自体は
    /// `variant()` の値を検証しない（`themes` 配列への格納順は `light` /
    /// `dark` という引数の位置のみで決まる。誤った variant を持つ `Theme` を
    /// 渡した場合の呼び出し元の契約違反は、S18（#174、バリデーション）の
    /// スコープに委ねる）。
    pub fn with_themes(
        base: TokenSet,
        light: Theme,
        dark: Theme,
        initial_theme: ThemeVariant,
    ) -> Self {
        let light_overrides = light.into_overrides();
        let dark_overrides = dark.into_overrides();
        let active_overrides = match initial_theme {
            ThemeVariant::Light => light_overrides.clone(),
            ThemeVariant::Dark => dark_overrides.clone(),
        };
        Self {
            base,
            theme_overrides: active_overrides,
            cache: HashMap::new(),
            active: initial_theme,
            generation: 0,
            themes: [light_overrides, dark_overrides],
        }
    }

    /// 現在のアクティブテーマ区分を返す（設計文書 §3.1）。
    pub fn active_theme(&self) -> ThemeVariant {
        self.active
    }

    /// アクティブテーマを切り替える（設計文書 §3.1・§4.2）。
    ///
    /// 処理契約:
    /// 1. `variant` が現在のアクティブテーマと同一の場合は何もせず即座に
    ///    `Ok(())` を返す（`theme_generation` を進めない冪等契約。高頻度切替
    ///    による無駄な再計算を避ける OWASP A04 対策、設計文書 §4.2・§8）
    /// 2. 異なる場合: アクティブテーマを更新し、`theme_overrides` を
    ///    `with_themes` が保持する該当バリアントの差分へ差し替え、解決結果
    ///    キャッシュを全件無効化した上で `resolve_all` を再実行する
    /// 3. `resolve_all` が `Err` を返した場合でも、テーマ自体はすでに
    ///    切り替わっているため `theme_generation` は進める（部分的な解決結果
    ///    であっても呼び出し元が再描画判定に使えるようにするため。設計文書
    ///    §4.2 の「呼び出し元が `theme_generation` を見て再描画を要求する」
    ///    契約と整合）
    ///
    /// クロージャを引数に取らない（REQ-13 整合）。panic しない
    /// （`Result` を返す、`.claude/rules/coding-rust.md` の panic 禁止方針）。
    ///
    /// # Errors
    ///
    /// 切替後の `resolve_all` が失敗した場合、集約されたエラー一覧をそのまま
    /// 返す（[`Self::resolve_all`] と同じ契約）。
    pub fn set_active_theme(&mut self, variant: ThemeVariant) -> Result<(), Vec<TokenError>> {
        if variant == self.active {
            return Ok(());
        }
        self.active = variant;
        self.theme_overrides = self.themes[variant.as_index()].clone();
        self.cache.clear();
        let result = self.resolve_all();
        self.generation += 1;
        result
    }

    /// テーマが切り替わるたびに増加する世代カウンタ（設計文書 §3.2）。
    ///
    /// `state/store.rs` の `StateStore::version`（アプリ状態の変更検知）とは
    /// 独立したカウンタである。同一テーマへの再切替（`set_active_theme` の
    /// 冪等契約）では進まない。プラットフォーム統合レイヤは両方を監視し、
    /// いずれかが変化したら再描画する想定（具体的な監視方法はプラットフォーム
    /// レイヤの実装詳細）。
    pub fn theme_generation(&self) -> u64 {
        self.generation
    }

    /// 単一のトークン参照を最終値まで解決する。
    ///
    /// キャッシュ（`resolve_all` が充填する）に解決済みの値があればそれを返し、
    /// ミスした場合はその場でチェーンを辿って解決する（`&self` のまま
    /// interior mutability を持ち込まない設計。キャッシュへの充填は `resolve_all`
    /// に集約する）。呼び出し元（S19 の DSL 参照 API・各種ウィジェットスタイル
    /// 適用箇所）は戻り値の `Result` を必ず処理し、エラー時に panic しない
    /// （coding-rust.md の panic 禁止方針）。エラー時に既定値へ自動フォールバック
    /// することはしない（設計文書 §4.1）。
    pub fn resolve(&self, reference: &TokenRef) -> Result<ResolvedValue, TokenError> {
        if let Some(cached) = self.cache.get(reference.path()) {
            return Ok(cached.clone());
        }
        self.resolve_chain(reference)
    }

    /// [`TypedTokenRef<K>`] を解決し、`K::Value` へ変換する（TASK-8.4-S19、#175）。
    ///
    /// 実体は [`TypedTokenRef::resolve`] への薄い委譲であり、解決ロジック自体は
    /// `typed.rs` の 1 箇所に集約する（`resolve` / `resolve_typed` は呼び出し側の
    /// 好みに合わせた 2 つの入口だが実体は 1 つという設計、`typed.rs` モジュール
    /// doc 参照）。エラー時に既定値へ自動フォールバックしない契約は [`Self::resolve`]
    /// と同一。
    ///
    /// # Errors
    ///
    /// [`TypedTokenRef::resolve`] と同じ（未定義参照・型不一致・循環参照・
    /// 深さ上限超過のいずれか）。
    pub fn resolve_typed<K: TokenKind>(
        &self,
        token_ref: &TypedTokenRef<K>,
    ) -> Result<K::Value, TokenError> {
        token_ref.resolve(self)
    }

    /// トークンセット全体（ベース・テーマ差分の両方のキー）を一括解決し、内部
    /// キャッシュへ反映する。アプリ起動時・テーマ切替時（S4/S5 からの通知）に
    /// 呼び出される契約（設計文書 §4.3）。
    ///
    /// 一部のキーが解決に失敗しても処理を中断せず、成功したキーはキャッシュへ
    /// 反映しつつ、全エラーを集約して返す（部分的な結果でも呼び出し元が活用
    /// できるようにするため）。
    pub fn resolve_all(&mut self) -> Result<(), Vec<TokenError>> {
        let mut keys: HashSet<TokenPath> = HashSet::new();
        keys.extend(self.base.keys().cloned());
        keys.extend(self.theme_overrides.keys().cloned());

        let mut errors = Vec::new();
        let mut resolved = HashMap::new();
        for path in keys {
            let reference = TokenRef::new(path.clone());
            match self.resolve_chain(&reference) {
                Ok(value) => {
                    resolved.insert(path, value);
                }
                Err(error) => errors.push(error),
            }
        }

        self.cache = resolved;

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// アクティブテーマの上書きマップを直接差し替え、内部キャッシュを無効化する
    /// 低レベル API（`resolver.rs` 冒頭 doc・mod.rs が定める「テーマ切替時に
    /// キャッシュ全体を無効化して `resolve_all` を再実行する」契約 §4.3 を、
    /// 既存インスタンスを作り直さずに満たすための公開 API）。
    ///
    /// [`Self::set_active_theme`]（S5、#161 が追加した高レベル API）とは独立
    /// しており、本メソッドは `themes` 配列（`with_themes` が保持するライト/
    /// ダーク両方の差分）を更新しない。`with_themes` で構築した後に本メソッドを
    /// 呼び出すと、`active_theme` が返す区分とアクティブな `theme_overrides` の
    /// 中身が食い違う状態になり得るため、`with_themes` 経由で構築した
    /// `TokenResolver` の切替には `set_active_theme` を使うこと。本メソッドは
    /// `new`（低レベルコンストラクタ）と対にした「アクティブテーマの差分を
    /// 直接差し替える」用途に限定して存続させる（S22、#178 の既存テストとの
    /// 後方互換維持）。
    ///
    /// 呼び出し元がテーマが切り替わった時点で本メソッドを呼び出し、続けて
    /// `resolve_all` を呼び出してキャッシュを再充填する契約とする。本メソッド
    /// 単体ではキャッシュを空にするのみで再計算は行わない（`resolve` は
    /// キャッシュミス時にその場でチェーンを辿るため呼び出し自体は安全だが、
    /// `resolve_all` を経ないと以後の呼び出しが毎回チェーンを辿る）。
    ///
    /// **再検証しない（TASK-8.4-S18、#174）**: 本メソッドは
    /// `theme_overrides` をそのまま差し替えるのみで、
    /// `super::validation::validate_token_sets` による再検証は行わない。
    /// `TokenResolver::validated` で構築したインスタンスであっても、本メソッドで
    /// 未検証の `theme_overrides` へ差し替えた場合はその新しい組み合わせの
    /// 正当性は保証されない（新しいテーマ差分自体が持ち込む未定義参照・
    /// カテゴリ不一致等が `resolve` 呼び出し時点で表面化し得る）。
    /// 再検証を伴うテーマ切替が必要な場合は `set_validated_theme_overrides` を
    /// 使うこと。
    pub fn set_theme_overrides(&mut self, theme_overrides: TokenSet) {
        self.theme_overrides = theme_overrides;
        self.cache.clear();
    }

    /// `set_theme_overrides` と同じ役割（アクティブテーマの上書きマップの
    /// 差し替え・キャッシュ無効化）を、`super::validation::validate_token_sets`
    /// による再検証を経てから行う（TASK-8.4-S18、#174）。
    ///
    /// `TokenResolver::validated` の「検証済みセットのみで構築する」契約を
    /// テーマ切替をまたいでも維持したい呼び出し元（S4/S5、#160/#161）向けの
    /// 経路。検証に失敗した場合は `self` を変更せず、所有権を渡した引数を
    /// そのまま消費して `TokenValidationReport` を返す（`validated` と同じ
    /// 「失敗時は変更しない」設計）。検証成功時は `set_theme_overrides` と
    /// 同様にキャッシュを空にするのみで、`resolve_all` の再実行は呼び出し元の
    /// 責務のままとする（§4.3。本メソッドが自動で呼び出すことはしない）。
    pub fn set_validated_theme_overrides(
        &mut self,
        theme_overrides: TokenSet,
    ) -> Result<(), super::validation::TokenValidationReport> {
        super::validation::validate_token_sets(&self.base, &theme_overrides)?;
        self.theme_overrides = theme_overrides;
        self.cache.clear();
        Ok(())
    }

    /// 明示的ループでエイリアスチェーンを辿り、値・エラーいずれかへ確定させる
    /// （設計文書 §5.2「再帰関数ではなく明示的なスタック（ループ）で実装する」）。
    ///
    /// 実体は `resolve_chain_in`（本モジュールの自由関数）へ委譲する薄い
    /// ラッパー（TASK-8.4-S18、#174 のリファクタ。挙動は変えない）。
    fn resolve_chain(&self, start: &TokenRef) -> Result<ResolvedValue, TokenError> {
        resolve_chain_in(&self.base, &self.theme_overrides, start)
    }
}

/// テーマ差分優先でトークン定義を探索する共有ロジック（設計文書 §3.3 の
/// 解決順序 (1)(2)）。
///
/// `TokenResolver::lookup` と `super::validation::validate_token_sets` の
/// 両方から呼ばれる（S18、#174 でロジックの二重管理を避けるために抽出した）。
/// 「各ステップの参照先ルックアップ自体もテーマ差分を優先する」契約に従い、
/// エイリアスチェーンを辿る各段でも本関数を経由する。
pub(super) fn lookup_in<'a>(
    base: &'a TokenSet,
    theme_overrides: &'a TokenSet,
    path: &TokenPath,
) -> Result<&'a TokenDefinition, TokenError> {
    theme_overrides
        .get(path)
        .or_else(|| base.get(path))
        .ok_or_else(|| TokenError::UndefinedReference {
            reference: path.to_string(),
        })
}

/// エイリアスチェーンを明示的ループで辿り、値・エラーいずれかへ確定させる
/// 共有ロジック（設計文書 §5.2「再帰関数ではなく明示的なスタック（ループ）で
/// 実装する」）。
///
/// `TokenResolver::resolve_chain` と `super::validation::validate_token_sets`
/// の両方から呼ばれる（S18、#174 でロジックの二重管理を避けるために抽出した。
/// 検証ゲート側も解決エンジンと全く同じチェーン走査・DoS 対策を適用できる）。
/// 再帰関数はスタックオーバーフロー時に `Result` を返せず即座に abort するため、
/// 上限チェックを関数呼び出しの外側で行える明示的ループを採用し panic しない
/// 契約と整合させる。訪問済みパスの `HashSet`（循環検出）と `MAX_ALIAS_DEPTH`
/// （深さ上限）の両方で異常なチェーンを打ち切る。
pub(super) fn resolve_chain_in(
    base: &TokenSet,
    theme_overrides: &TokenSet,
    start: &TokenRef,
) -> Result<ResolvedValue, TokenError> {
    // 解決順序 (3): テーマ適用後のエイリアス解決（設計文書 §3.3）。
    // チェーンの起点が宣言するカテゴリを「期待カテゴリ」として固定し、
    // 以降のリンクがこのカテゴリから逸脱した時点で型不一致とみなす
    // （色トークンがスペーシングトークンを参照する等のケースの検出、§4.4）。
    let entry_definition = lookup_in(base, theme_overrides, start.path())?;
    let expected_category = entry_definition.category();

    let mut visited: HashSet<TokenPath> = HashSet::new();
    let mut visit_order: Vec<TokenPath> = Vec::new();
    let mut current_path = start.path().clone();
    let mut depth: u32 = 0;

    loop {
        if !visited.insert(current_path.clone()) {
            visit_order.push(current_path.clone());
            let chain = visit_order
                .iter()
                .map(TokenPath::to_string)
                .collect::<Vec<_>>()
                .join(" -> ");
            return Err(TokenError::CircularReference { chain });
        }
        visit_order.push(current_path.clone());

        let definition = lookup_in(base, theme_overrides, &current_path)?;
        if definition.category() != expected_category {
            return Err(TokenError::TypeMismatch {
                origin: start.to_string(),
                reference: current_path.to_string(),
                expected: expected_category.name(),
                actual: definition.category().name(),
            });
        }

        match definition {
            TokenDefinition::Value { value } => {
                return Ok(ResolvedValue(value.clone()));
            }
            TokenDefinition::Reference { reference, .. } => {
                depth += 1;
                if depth > MAX_ALIAS_DEPTH {
                    return Err(TokenError::MaxDepthExceeded {
                        reference: start.to_string(),
                        limit: MAX_ALIAS_DEPTH,
                    });
                }
                current_path = reference.path().clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::color::ColorValue;
    use super::super::schema::TokenCategory;
    use super::*;

    fn path(s: &str) -> TokenPath {
        TokenPath::from_dotted(s).expect("テスト用の正当なパス")
    }

    fn value_def(value: TokenValue) -> TokenDefinition {
        TokenDefinition::Value { value }
    }

    fn reference_def(category: TokenCategory, target: &str) -> TokenDefinition {
        TokenDefinition::Reference {
            category,
            reference: TokenRef::new(path(target)),
        }
    }

    #[test]
    fn resolves_direct_value() {
        let mut base = TokenSet::new();
        base.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x00, 0x00, 0x00))),
        );
        let resolver = TokenResolver::new(base, TokenSet::new());
        let resolved = resolver
            .resolve(&TokenRef::new(path("color.primary")))
            .expect("直接値は解決できる");
        assert_eq!(
            resolved.value(),
            &TokenValue::Color(ColorValue::rgb(0x00, 0x00, 0x00))
        );
    }

    #[test]
    fn resolves_multi_step_alias_chain() {
        let mut base = TokenSet::new();
        base.insert(path("a"), reference_def(TokenCategory::Color, "b"));
        base.insert(path("b"), reference_def(TokenCategory::Color, "c"));
        base.insert(
            path("c"),
            value_def(TokenValue::Color(ColorValue::rgb(0xff, 0xff, 0xff))),
        );
        let resolver = TokenResolver::new(base, TokenSet::new());
        let resolved = resolver
            .resolve(&TokenRef::new(path("a")))
            .expect("2〜3 段のエイリアスチェーンは解決できる");
        assert_eq!(
            resolved.value(),
            &TokenValue::Color(ColorValue::rgb(0xff, 0xff, 0xff))
        );
    }

    #[test]
    fn theme_override_changes_alias_target() {
        // 設計文書 §3.3 のテーマ適用順序（ベース → テーマ差分 → エイリアス解決）が
        // 正しく機能することの確認。ライト/ダークでエイリアス先が異なるケース。
        let mut base = TokenSet::new();
        base.insert(
            path("button.background"),
            reference_def(TokenCategory::Color, "color.primary"),
        );
        base.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))),
        );

        let mut dark_overrides = TokenSet::new();
        dark_overrides.insert(
            path("button.background"),
            reference_def(TokenCategory::Color, "color.primary.dark"),
        );
        dark_overrides.insert(
            path("color.primary.dark"),
            value_def(TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))),
        );

        let resolver = TokenResolver::new(base, dark_overrides);
        let resolved = resolver
            .resolve(&TokenRef::new(path("button.background")))
            .expect("テーマ差分によるエイリアス先切替は解決できる");
        assert_eq!(
            resolved.value(),
            &TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))
        );
    }

    #[test]
    fn detects_self_reference_cycle() {
        let mut base = TokenSet::new();
        base.insert(path("a"), reference_def(TokenCategory::Color, "a"));
        let resolver = TokenResolver::new(base, TokenSet::new());
        let err = resolver
            .resolve(&TokenRef::new(path("a")))
            .expect_err("自己参照は循環として検出される");
        assert!(matches!(err, TokenError::CircularReference { .. }));
    }

    #[test]
    fn detects_indirect_reference_cycle() {
        let mut base = TokenSet::new();
        base.insert(path("a"), reference_def(TokenCategory::Color, "b"));
        base.insert(path("b"), reference_def(TokenCategory::Color, "a"));
        let resolver = TokenResolver::new(base, TokenSet::new());
        let err = resolver
            .resolve(&TokenRef::new(path("a")))
            .expect_err("間接循環も検出される");
        assert!(matches!(err, TokenError::CircularReference { .. }));
    }

    #[test]
    fn undefined_reference_is_reported() {
        let resolver = TokenResolver::new(TokenSet::new(), TokenSet::new());
        let err = resolver
            .resolve(&TokenRef::new(path("missing")))
            .expect_err("未定義参照はエラーになる");
        assert!(matches!(err, TokenError::UndefinedReference { .. }));
    }

    #[test]
    fn type_mismatch_across_categories_is_reported() {
        let mut base = TokenSet::new();
        base.insert(
            path("color.primary"),
            reference_def(TokenCategory::Color, "spacing.md"),
        );
        base.insert(path("spacing.md"), value_def(TokenValue::Dimension(8.0)));
        let resolver = TokenResolver::new(base, TokenSet::new());
        let err = resolver
            .resolve(&TokenRef::new(path("color.primary")))
            .expect_err("カテゴリ跨ぎ参照は型不一致になる");
        assert!(matches!(err, TokenError::TypeMismatch { .. }));
    }

    #[test]
    fn max_depth_exceeded_does_not_hang() {
        // 深さ上限を確実に超えるよう MAX_ALIAS_DEPTH より多い段数のチェーンを構築する。
        let mut base = TokenSet::new();
        let chain_len = MAX_ALIAS_DEPTH as usize + 8;
        for i in 0..chain_len {
            let key = format!("t{i}");
            let next = format!("t{}", i + 1);
            base.insert(path(&key), reference_def(TokenCategory::Color, &next));
        }
        base.insert(
            path(&format!("t{chain_len}")),
            value_def(TokenValue::Color(ColorValue::rgb(0x03, 0x03, 0x03))),
        );
        let resolver = TokenResolver::new(base, TokenSet::new());
        let err = resolver
            .resolve(&TokenRef::new(path("t0")))
            .expect_err("上限を超えるチェーンは MaxDepthExceeded になる");
        assert!(matches!(err, TokenError::MaxDepthExceeded { .. }));
    }

    #[test]
    fn max_depth_exceeded_boundary_at_exactly_max_hops() {
        // MAX_ALIAS_DEPTH ちょうどのホップ数（32 回参照を辿った先に値がある）は
        // 許容される境界であることを確認する（`depth > MAX_ALIAS_DEPTH` の意味と
        // 実際の許容ホップ数のずれを検知するための境界値テスト）。
        let mut base = TokenSet::new();
        let hop_count = MAX_ALIAS_DEPTH as usize;
        for i in 0..hop_count {
            let key = format!("t{i}");
            let next = format!("t{}", i + 1);
            base.insert(path(&key), reference_def(TokenCategory::Color, &next));
        }
        base.insert(
            path(&format!("t{hop_count}")),
            value_def(TokenValue::Color(ColorValue::rgb(0x03, 0x03, 0x03))),
        );
        let resolver = TokenResolver::new(base, TokenSet::new());
        let resolved = resolver
            .resolve(&TokenRef::new(path("t0")))
            .expect("ちょうど MAX_ALIAS_DEPTH ホップのチェーンは解決できる");
        assert_eq!(
            resolved.value(),
            &TokenValue::Color(ColorValue::rgb(0x03, 0x03, 0x03))
        );
    }

    #[test]
    fn max_depth_exceeded_boundary_at_one_hop_over_max() {
        // MAX_ALIAS_DEPTH を 1 ホップ超えた時点で確実にエラーになることを確認する
        // （余裕を持たせたチェーン長のテストでは検出できない境界値のズレの検知）。
        let mut base = TokenSet::new();
        let hop_count = MAX_ALIAS_DEPTH as usize + 1;
        for i in 0..hop_count {
            let key = format!("t{i}");
            let next = format!("t{}", i + 1);
            base.insert(path(&key), reference_def(TokenCategory::Color, &next));
        }
        base.insert(
            path(&format!("t{hop_count}")),
            value_def(TokenValue::Color(ColorValue::rgb(0x03, 0x03, 0x03))),
        );
        let resolver = TokenResolver::new(base, TokenSet::new());
        let err = resolver
            .resolve(&TokenRef::new(path("t0")))
            .expect_err("MAX_ALIAS_DEPTH を 1 ホップ超えるチェーンは MaxDepthExceeded になる");
        assert!(matches!(err, TokenError::MaxDepthExceeded { .. }));
    }

    #[test]
    fn invalid_segment_rejects_token_path_construction() {
        let err = TokenPath::new(vec!["..".to_string()]).expect_err("`..` セグメントは拒否される");
        assert!(matches!(err, TokenError::InvalidPath { .. }));

        let err =
            TokenPath::new(vec!["a/b".to_string()]).expect_err("許可文字集合外の文字は拒否される");
        assert!(matches!(err, TokenError::InvalidPath { .. }));
    }

    #[test]
    fn resolve_all_populates_cache_for_subsequent_resolve() {
        let mut base = TokenSet::new();
        base.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x04, 0x04, 0x04))),
        );
        let mut resolver = TokenResolver::new(base, TokenSet::new());
        resolver
            .resolve_all()
            .expect("正常なトークンセットは一括解決できる");
        let resolved = resolver
            .resolve(&TokenRef::new(path("color.primary")))
            .expect("キャッシュ経由で解決できる");
        assert_eq!(
            resolved.value(),
            &TokenValue::Color(ColorValue::rgb(0x04, 0x04, 0x04))
        );
    }

    #[test]
    fn set_theme_overrides_invalidates_cache_and_recomputes() {
        // テーマ切替時にインスタンスを作り直さずキャッシュを
        // 無効化して新テーマの値を反映できることを確認する。
        let mut base = TokenSet::new();
        base.insert(
            path("button.background"),
            reference_def(TokenCategory::Color, "color.primary"),
        );
        base.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))),
        );

        let mut resolver = TokenResolver::new(base, TokenSet::new());
        resolver.resolve_all().expect("初期テーマは一括解決できる");
        let light = resolver
            .resolve(&TokenRef::new(path("button.background")))
            .expect("初期テーマのキャッシュ値を解決できる");
        assert_eq!(
            light.value(),
            &TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))
        );

        let mut dark_overrides = TokenSet::new();
        dark_overrides.insert(
            path("button.background"),
            reference_def(TokenCategory::Color, "color.primary.dark"),
        );
        dark_overrides.insert(
            path("color.primary.dark"),
            value_def(TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))),
        );
        resolver.set_theme_overrides(dark_overrides);

        // set_theme_overrides 直後はキャッシュが空でも resolve はその場で
        // チェーンを辿って新テーマの値を返す。
        let dark_before_resolve_all = resolver
            .resolve(&TokenRef::new(path("button.background")))
            .expect("キャッシュ無効化後もその場解決で新テーマの値が取れる");
        assert_eq!(
            dark_before_resolve_all.value(),
            &TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))
        );

        resolver.resolve_all().expect("新テーマも一括解決できる");
        let dark_after_resolve_all = resolver
            .resolve(&TokenRef::new(path("button.background")))
            .expect("再充填されたキャッシュ経由でも新テーマの値が取れる");
        assert_eq!(
            dark_after_resolve_all.value(),
            &TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))
        );
    }

    #[test]
    fn resolves_typography_reference() {
        // TASK-8.4-S8（#164）: typography カテゴリのトークンもエイリアス経由で
        // 解決できることを確認する（resolve_chain は category() 経由で汎用的に
        // 動作するため、本来は Typography 追加だけで自動的に動く契約）。
        use super::super::typography::{FontStyleValue, FontWeightValue, TypographyValue};

        let mut base = TokenSet::new();
        base.insert(
            path("label.font"),
            reference_def(TokenCategory::Typography, "typography.body"),
        );
        let body = TypographyValue::new(
            "sans-serif",
            16.0,
            24.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect("正当な値");
        base.insert(
            path("typography.body"),
            value_def(TokenValue::Typography(body.clone())),
        );

        let resolver = TokenResolver::new(base, TokenSet::new());
        let resolved = resolver
            .resolve(&TokenRef::new(path("label.font")))
            .expect("typography への参照は解決できる");
        assert_eq!(resolved.value(), &TokenValue::Typography(body));
    }

    #[test]
    fn resolve_typed_delegates_to_typed_token_ref_resolve() {
        // TASK-8.4-S19（#175）: resolve_typed が TypedTokenRef::resolve への薄い
        // 委譲として機能することを確認する。
        use super::super::typed::{ColorKind, TypedTokenRef};

        let mut base = TokenSet::new();
        base.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x05, 0x05, 0x05))),
        );
        let resolver = TokenResolver::new(base, TokenSet::new());

        let token_ref: TypedTokenRef<ColorKind> =
            TypedTokenRef::from_dotted("color.primary").expect("正当なパス");
        let value = resolver
            .resolve_typed(&token_ref)
            .expect("型付き参照は解決できる");
        assert_eq!(value, ColorValue::rgb(0x05, 0x05, 0x05));
    }

    #[test]
    fn typography_reference_rejects_mismatched_category_target() {
        // 参照元が Typography を期待しているのに参照先が Color カテゴリの場合、
        // TypeMismatch として拒否されることを確認する（設計文書 §4.4）。
        let mut base = TokenSet::new();
        base.insert(
            path("label.font"),
            reference_def(TokenCategory::Typography, "color.primary"),
        );
        base.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x00, 0x00, 0x00))),
        );

        let resolver = TokenResolver::new(base, TokenSet::new());
        let err = resolver
            .resolve(&TokenRef::new(path("label.font")))
            .expect_err("カテゴリ不一致は拒否される");
        assert!(matches!(err, TokenError::TypeMismatch { .. }));
    }

    #[test]
    fn validated_constructs_resolver_for_a_valid_set() {
        // TASK-8.4-S18（#174）: 検証済みセットのみで構築する `validated` の
        // 正常系。既存の `new` と同じ解決結果が得られることを確認する。
        let mut base = TokenSet::new();
        base.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x05, 0x05, 0x05))),
        );
        let resolver = TokenResolver::validated(base, TokenSet::new())
            .expect("検証を通過したセットは構築できる");
        let resolved = resolver
            .resolve(&TokenRef::new(path("color.primary")))
            .expect("構築後は通常どおり解決できる");
        assert_eq!(
            resolved.value(),
            &TokenValue::Color(ColorValue::rgb(0x05, 0x05, 0x05))
        );
    }

    // 以下、境界条件・エラーフィールドの内容・`resolve_all` / `set_theme_overrides`
    // の契約を対象とする網羅的テスト（TASK-8.4-S21、#177。上記は設計文書 §6 が
    // 定める最小観点、TASK-8.4-S3、#159）。

    #[test]
    fn resolves_single_step_alias() {
        // 設計文書 §6 観点 1（1 段エイリアス）の明示ケース。既存の
        // `resolves_multi_step_alias_chain` は 2 段以上のみを対象としていた。
        let mut base = TokenSet::new();
        base.insert(path("a"), reference_def(TokenCategory::Color, "b"));
        base.insert(
            path("b"),
            value_def(TokenValue::Color(ColorValue::rgb(0x10, 0x20, 0x30))),
        );
        let resolver = TokenResolver::new(base, TokenSet::new());
        let resolved = resolver
            .resolve(&TokenRef::new(path("a")))
            .expect("1 段のエイリアスは解決できる");
        assert_eq!(
            resolved.value(),
            &TokenValue::Color(ColorValue::rgb(0x10, 0x20, 0x30))
        );
    }

    #[test]
    fn resolves_dimension_and_text_categories() {
        // resolve_chain は category() 経由で汎用的に動作するため、Color 以外の
        // 素朴なカテゴリ（Dimension / Text）でも直接値の解決が動作することを
        // 確認する（typography/spacing 等の専用カテゴリは既存テストで別途被覆済み）。
        let mut base = TokenSet::new();
        base.insert(path("spacing.md"), value_def(TokenValue::Dimension(8.0)));
        base.insert(
            path("font.family"),
            value_def(TokenValue::Text("sans-serif".to_string())),
        );
        let resolver = TokenResolver::new(base, TokenSet::new());

        let dimension = resolver
            .resolve(&TokenRef::new(path("spacing.md")))
            .expect("Dimension カテゴリの直接値は解決できる");
        assert_eq!(dimension.value(), &TokenValue::Dimension(8.0));

        let text = resolver
            .resolve(&TokenRef::new(path("font.family")))
            .expect("Text カテゴリの直接値は解決できる");
        assert_eq!(text.value(), &TokenValue::Text("sans-serif".to_string()));
    }

    #[test]
    fn undefined_reference_mid_chain() {
        // チェーンの起点自体は定義されているが、途中の参照先が未定義の場合の
        // `UndefinedReference` を確認する（既存の `undefined_reference_is_reported`
        // は起点自体が未定義のケースのみを対象としていた）。
        let mut base = TokenSet::new();
        base.insert(path("a"), reference_def(TokenCategory::Color, "b"));
        let resolver = TokenResolver::new(base, TokenSet::new());
        let err = resolver
            .resolve(&TokenRef::new(path("a")))
            .expect_err("チェーン途中の未定義参照はエラーになる");
        match err {
            TokenError::UndefinedReference { reference } => assert_eq!(reference, "b"),
            other => panic!("UndefinedReference を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn type_mismatch_mid_chain_reports_origin_and_target() {
        // a(Color) -> b(Color 参照) -> c(Dimension 値) のチェーンで、
        // origin はチェーンの起点、reference は実際に型不一致が検出された
        // ターゲットを指すことを確認する（設計文書 §4.4 のエラー診断契約）。
        let mut base = TokenSet::new();
        base.insert(path("a"), reference_def(TokenCategory::Color, "b"));
        base.insert(path("b"), reference_def(TokenCategory::Color, "c"));
        base.insert(path("c"), value_def(TokenValue::Dimension(4.0)));
        let resolver = TokenResolver::new(base, TokenSet::new());
        let err = resolver
            .resolve(&TokenRef::new(path("a")))
            .expect_err("チェーン途中の型不一致はエラーになる");
        match err {
            TokenError::TypeMismatch {
                origin,
                reference,
                expected,
                actual,
            } => {
                assert_eq!(origin, "a");
                assert_eq!(reference, "c");
                assert_eq!(expected, "Color");
                assert_eq!(actual, "Dimension");
            }
            other => panic!("TypeMismatch を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn circular_reference_chain_records_visit_order() {
        // `CircularReference::chain` が訪問順を " -> " 区切りで記録することを
        // 契約として固定化する（エラー診断情報の品質、設計文書 §5.2）。
        let mut self_ref_base = TokenSet::new();
        self_ref_base.insert(path("a"), reference_def(TokenCategory::Color, "a"));
        let self_ref_resolver = TokenResolver::new(self_ref_base, TokenSet::new());
        let err = self_ref_resolver
            .resolve(&TokenRef::new(path("a")))
            .expect_err("自己参照は循環になる");
        match err {
            TokenError::CircularReference { chain } => assert_eq!(chain, "a -> a"),
            other => panic!("CircularReference を期待しましたが {other:?} でした"),
        }

        let mut indirect_base = TokenSet::new();
        indirect_base.insert(path("a"), reference_def(TokenCategory::Color, "b"));
        indirect_base.insert(path("b"), reference_def(TokenCategory::Color, "a"));
        let indirect_resolver = TokenResolver::new(indirect_base, TokenSet::new());
        let err = indirect_resolver
            .resolve(&TokenRef::new(path("a")))
            .expect_err("間接循環になる");
        match err {
            TokenError::CircularReference { chain } => assert_eq!(chain, "a -> b -> a"),
            other => panic!("CircularReference を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn cycle_not_containing_start_is_detected() {
        // 起点（a）自体はループの外側にあり、a -> b -> c -> b という「ループへ
        // 合流するチェーン」でも循環として検出されることを確認する
        // （`visited` セットがチェーン全体を対象にしているため）。
        let mut base = TokenSet::new();
        base.insert(path("a"), reference_def(TokenCategory::Color, "b"));
        base.insert(path("b"), reference_def(TokenCategory::Color, "c"));
        base.insert(path("c"), reference_def(TokenCategory::Color, "b"));
        let resolver = TokenResolver::new(base, TokenSet::new());
        let err = resolver
            .resolve(&TokenRef::new(path("a")))
            .expect_err("起点を含まないループへの合流も循環として検出される");
        assert!(matches!(err, TokenError::CircularReference { .. }));
    }

    #[test]
    fn short_cycle_is_circular_not_max_depth() {
        // MAX_ALIAS_DEPTH を大きく下回る短い循環（5 ノード）が、深さ上限超過では
        // なく循環参照として報告されることを確認する（DoS 対策の 2 つの防御
        // 機構 — 循環検出と深さ上限 — が取り違えられないことの契約化）。
        let mut base = TokenSet::new();
        base.insert(path("c1"), reference_def(TokenCategory::Color, "c2"));
        base.insert(path("c2"), reference_def(TokenCategory::Color, "c3"));
        base.insert(path("c3"), reference_def(TokenCategory::Color, "c4"));
        base.insert(path("c4"), reference_def(TokenCategory::Color, "c5"));
        base.insert(path("c5"), reference_def(TokenCategory::Color, "c1"));
        let resolver = TokenResolver::new(base, TokenSet::new());
        let err = resolver
            .resolve(&TokenRef::new(path("c1")))
            .expect_err("短い循環はエラーになる");
        assert!(matches!(err, TokenError::CircularReference { .. }));
    }

    #[test]
    fn max_depth_error_reports_limit_and_origin() {
        // `MaxDepthExceeded` のフィールド内容（limit が MAX_ALIAS_DEPTH と一致し、
        // reference がチェーンの起点を指すこと）を確認する（既存の
        // `max_depth_exceeded_boundary_at_one_hop_over_max` はバリアント一致のみ）。
        let mut base = TokenSet::new();
        let hop_count = MAX_ALIAS_DEPTH as usize + 1;
        for i in 0..hop_count {
            base.insert(
                path(&format!("t{i}")),
                reference_def(TokenCategory::Color, &format!("t{}", i + 1)),
            );
        }
        base.insert(
            path(&format!("t{hop_count}")),
            value_def(TokenValue::Color(ColorValue::rgb(0x03, 0x03, 0x03))),
        );
        let resolver = TokenResolver::new(base, TokenSet::new());
        let err = resolver
            .resolve(&TokenRef::new(path("t0")))
            .expect_err("上限超過はエラーになる");
        match err {
            TokenError::MaxDepthExceeded { reference, limit } => {
                assert_eq!(limit, MAX_ALIAS_DEPTH);
                assert_eq!(reference, "t0");
            }
            other => panic!("MaxDepthExceeded を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn resolve_all_aggregates_errors_and_keeps_partial_results() {
        // 正常キー 1 件・未定義参照キー 1 件・相互循環キー 2 件（cycle.a/cycle.b は
        // 互いに参照し合うため、それぞれの解決が独立に失敗し合計 2 エラーになる）
        // を混在させ、`resolve_all` がエラーを集約しつつ成功キーの値は取得できる
        // ことを確認する（部分的な結果の活用、設計文書 §4.3）。
        let mut base = TokenSet::new();
        base.insert(
            path("ok"),
            value_def(TokenValue::Color(ColorValue::rgb(0x07, 0x07, 0x07))),
        );
        base.insert(
            path("undef"),
            reference_def(TokenCategory::Color, "missing"),
        );
        base.insert(
            path("cycle.a"),
            reference_def(TokenCategory::Color, "cycle.b"),
        );
        base.insert(
            path("cycle.b"),
            reference_def(TokenCategory::Color, "cycle.a"),
        );
        let mut resolver = TokenResolver::new(base, TokenSet::new());

        let errors = resolver
            .resolve_all()
            .expect_err("未定義参照・循環キーがあるため一部失敗する");
        // 失敗するキーは undef・cycle.a・cycle.b の 3 件（ok は成功するため含まない）。
        assert_eq!(errors.len(), 3);
        assert!(errors.iter().any(
            |e| matches!(e, TokenError::UndefinedReference { reference } if reference == "missing")
        ));
        let circular_count = errors
            .iter()
            .filter(|e| matches!(e, TokenError::CircularReference { .. }))
            .count();
        assert_eq!(circular_count, 2);

        // 成功キー（ok）は resolve_all 後も正しい値が取得できる（キャッシュ経由か
        // その場解決かは `resolve` の公開契約上区別できないため、値の正しさのみを
        // 検証する）。
        let resolved = resolver
            .resolve(&TokenRef::new(path("ok")))
            .expect("成功キーの値は resolve_all 後も取得できる");
        assert_eq!(
            resolved.value(),
            &TokenValue::Color(ColorValue::rgb(0x07, 0x07, 0x07))
        );
    }

    #[test]
    fn resolve_all_covers_theme_only_keys() {
        // ベースには存在せずテーマ差分にのみ存在するキーも `resolve_all` の
        // 対象になることを確認する（`keys` がベース・テーマ差分両方の和集合を
        // 取っているという実装契約の固定化）。
        let mut base = TokenSet::new();
        base.insert(
            path("base.only"),
            value_def(TokenValue::Color(ColorValue::rgb(0x11, 0x11, 0x11))),
        );
        let mut theme_overrides = TokenSet::new();
        theme_overrides.insert(
            path("theme.only"),
            value_def(TokenValue::Color(ColorValue::rgb(0x22, 0x22, 0x22))),
        );
        let mut resolver = TokenResolver::new(base, theme_overrides);
        resolver
            .resolve_all()
            .expect("ベース・テーマ両方のキーが解決できる");

        let base_only = resolver
            .resolve(&TokenRef::new(path("base.only")))
            .expect("ベース側のキーが解決できる");
        assert_eq!(
            base_only.value(),
            &TokenValue::Color(ColorValue::rgb(0x11, 0x11, 0x11))
        );
        let theme_only = resolver
            .resolve(&TokenRef::new(path("theme.only")))
            .expect("テーマ差分のみのキーも解決できる");
        assert_eq!(
            theme_only.value(),
            &TokenValue::Color(ColorValue::rgb(0x22, 0x22, 0x22))
        );
    }

    #[test]
    fn theme_override_of_intermediate_chain_link() {
        // 設計文書 §3.3「各ステップのルックアップもテーマ差分優先」契約: 起点
        // （button.background）自体はテーマで上書きされていないが、チェーン
        // 途中のリンク（color.primary）がテーマで上書きされている場合、
        // 上書き後の値が返ることを確認する（既存の `theme_override_changes_alias_target`
        // は起点自体を上書きするケースのみを対象としていた）。
        let mut base = TokenSet::new();
        base.insert(
            path("button.background"),
            reference_def(TokenCategory::Color, "color.primary"),
        );
        base.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))),
        );

        let mut theme_overrides = TokenSet::new();
        theme_overrides.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x09, 0x09, 0x09))),
        );

        let resolver = TokenResolver::new(base, theme_overrides);
        let resolved = resolver
            .resolve(&TokenRef::new(path("button.background")))
            .expect("チェーン途中のテーマ上書きも反映される");
        assert_eq!(
            resolved.value(),
            &TokenValue::Color(ColorValue::rgb(0x09, 0x09, 0x09))
        );
    }

    #[test]
    fn set_theme_overrides_then_resolve_all_drops_stale_keys() {
        // テーマ差し替え後に `resolve_all` を呼ぶと、旧テーマ専用キーはキャッシュ
        // から失われ、以後の `resolve` は `UndefinedReference` になることを確認する
        // （`resolve_all` がキャッシュを全置換する契約、`set_theme_overrides` doc 参照）。
        let mut base = TokenSet::new();
        base.insert(
            path("shared"),
            value_def(TokenValue::Color(ColorValue::rgb(0x05, 0x05, 0x05))),
        );

        let mut theme_a = TokenSet::new();
        theme_a.insert(
            path("theme_a.only"),
            value_def(TokenValue::Color(ColorValue::rgb(0x06, 0x06, 0x06))),
        );

        let mut resolver = TokenResolver::new(base, theme_a);
        resolver.resolve_all().expect("テーマ A は一括解決できる");
        resolver
            .resolve(&TokenRef::new(path("theme_a.only")))
            .expect("テーマ A 専用キーは切替前は解決できる");

        // テーマ B には theme_a.only が存在しない。
        let theme_b = TokenSet::new();
        resolver.set_theme_overrides(theme_b);
        resolver
            .resolve_all()
            .expect("テーマ B も一括解決できる（shared のみが対象）");

        let err = resolver
            .resolve(&TokenRef::new(path("theme_a.only")))
            .expect_err("旧テーマ専用キーは新テーマでは未定義になる");
        assert!(matches!(err, TokenError::UndefinedReference { .. }));

        let shared = resolver
            .resolve(&TokenRef::new(path("shared")))
            .expect("両テーマに共通するキーは引き続き解決できる");
        assert_eq!(
            shared.value(),
            &TokenValue::Color(ColorValue::rgb(0x05, 0x05, 0x05))
        );
    }

    #[test]
    fn validated_rejects_a_set_with_undefined_reference() {
        // `validated` は検証失敗時にレポートを返し構築を拒否する（§5.3 契約）。
        let mut base = TokenSet::new();
        base.insert(
            path("color.primary"),
            reference_def(TokenCategory::Color, "color.missing"),
        );
        let report = TokenResolver::validated(base, TokenSet::new())
            .expect_err("未定義参照を含むセットは構築を拒否される");
        assert!(!report.is_empty());
    }

    // 以下、テーマ切替機構本体（`with_themes` / `active_theme` / `set_active_theme` /
    // `theme_generation`）のテスト（TASK-8.4-S5、#161）。

    use super::super::theme::Theme;

    #[test]
    fn with_themes_constructs_with_initial_theme_and_resolves_active_diff() {
        let base = TokenSet::new();
        let mut light_overrides = TokenSet::new();
        light_overrides.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))),
        );
        let mut dark_overrides = TokenSet::new();
        dark_overrides.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))),
        );

        let resolver = TokenResolver::with_themes(
            base,
            Theme::new(ThemeVariant::Light, light_overrides),
            Theme::new(ThemeVariant::Dark, dark_overrides),
            ThemeVariant::Dark,
        );

        assert_eq!(resolver.active_theme(), ThemeVariant::Dark);
        let resolved = resolver
            .resolve(&TokenRef::new(path("color.primary")))
            .expect("初期テーマ（Dark）の差分で解決できる");
        assert_eq!(
            resolved.value(),
            &TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))
        );
    }

    #[test]
    fn set_active_theme_switches_diff_and_advances_generation() {
        let base = TokenSet::new();
        let mut light_overrides = TokenSet::new();
        light_overrides.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))),
        );
        let mut dark_overrides = TokenSet::new();
        dark_overrides.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))),
        );

        let mut resolver = TokenResolver::with_themes(
            base,
            Theme::new(ThemeVariant::Light, light_overrides),
            Theme::new(ThemeVariant::Dark, dark_overrides),
            ThemeVariant::Light,
        );
        assert_eq!(resolver.theme_generation(), 0);

        resolver
            .set_active_theme(ThemeVariant::Dark)
            .expect("Dark への切替は成功する");
        assert_eq!(resolver.active_theme(), ThemeVariant::Dark);
        assert_eq!(resolver.theme_generation(), 1);
        let dark = resolver
            .resolve(&TokenRef::new(path("color.primary")))
            .expect("切替後は Dark の差分で解決できる");
        assert_eq!(
            dark.value(),
            &TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))
        );

        resolver
            .set_active_theme(ThemeVariant::Light)
            .expect("Light への切替も成功する");
        assert_eq!(resolver.theme_generation(), 2);
        let light = resolver
            .resolve(&TokenRef::new(path("color.primary")))
            .expect("Light へ戻すと Light の差分で解決できる");
        assert_eq!(
            light.value(),
            &TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))
        );
    }

    #[test]
    fn set_active_theme_same_variant_is_idempotent_and_keeps_generation() {
        // 設計文書 §6 観点 4: 同一 ThemeVariant への再切替は no-op（冪等契約）で
        // あり、theme_generation を進めない（DoS 緩和、§4.2・§8）。
        let base = TokenSet::new();
        let resolver = TokenResolver::with_themes(
            base,
            Theme::new(ThemeVariant::Light, TokenSet::new()),
            Theme::new(ThemeVariant::Dark, TokenSet::new()),
            ThemeVariant::Light,
        );
        assert_eq!(resolver.theme_generation(), 0);

        let mut resolver = resolver;
        resolver
            .set_active_theme(ThemeVariant::Light)
            .expect("同一テーマへの切替は成功する（no-op）");
        assert_eq!(
            resolver.theme_generation(),
            0,
            "同一テーマへの再切替は theme_generation を進めない"
        );
        assert_eq!(resolver.active_theme(), ThemeVariant::Light);
    }

    #[test]
    fn set_active_theme_advances_generation_even_when_resolve_all_fails() {
        // テーマ自体はすでに切り替わっているため、resolve_all が Err でも
        // theme_generation は進む契約（`set_active_theme` doc 参照）。
        let mut base = TokenSet::new();
        base.insert(
            path("color.primary"),
            value_def(TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))),
        );
        let mut broken_dark = TokenSet::new();
        broken_dark.insert(
            path("color.primary"),
            reference_def(TokenCategory::Color, "color.missing"),
        );

        let mut resolver = TokenResolver::with_themes(
            base,
            Theme::new(ThemeVariant::Light, TokenSet::new()),
            Theme::new(ThemeVariant::Dark, broken_dark),
            ThemeVariant::Light,
        );

        let errors = resolver
            .set_active_theme(ThemeVariant::Dark)
            .expect_err("壊れたダークテーマへの切替は resolve_all のエラーを返す");
        assert!(!errors.is_empty());
        assert_eq!(
            resolver.active_theme(),
            ThemeVariant::Dark,
            "resolve_all が失敗してもアクティブテーマ自体は切り替わる"
        );
        assert_eq!(
            resolver.theme_generation(),
            1,
            "resolve_all が失敗しても theme_generation は進む"
        );
    }

    #[test]
    fn theme_preference_system_resolves_to_platform_provided_variant_via_set_active_theme() {
        // 設計文書 §6 観点 5: ThemePreference::System をプラットフォームレイヤが
        // 解決した ThemeVariant として set_active_theme へ渡すシナリオ。
        use super::super::theme::ThemePreference;

        let base = TokenSet::new();
        let mut resolver = TokenResolver::with_themes(
            base,
            Theme::new(ThemeVariant::Light, TokenSet::new()),
            Theme::new(ThemeVariant::Dark, {
                let mut set = TokenSet::new();
                set.insert(
                    path("color.primary"),
                    value_def(TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))),
                );
                set
            }),
            ThemeVariant::Light,
        );

        // プラットフォームレイヤが OS 判定した結果（ここでは Dark）を
        // ThemePreference::System 経由で解決し、set_active_theme へ渡す。
        let system_variant = ThemeVariant::Dark;
        let resolved_variant = ThemePreference::System.resolved_with(system_variant);
        resolver
            .set_active_theme(resolved_variant)
            .expect("解決済み ThemeVariant による切替は成功する");

        assert_eq!(resolver.active_theme(), ThemeVariant::Dark);
    }
}
