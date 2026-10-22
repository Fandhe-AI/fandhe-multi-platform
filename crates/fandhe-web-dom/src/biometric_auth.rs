//! Phase 11（端末機能の拡充ループ、親 #905）型サンプル「生体認証」の Web 経路（Issue #909）。
//!
//! `fandhe_core::screens::biometric_check`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、WebAuthn
//! （`navigator.credentials`）で platform authenticator の確認を求め結果を書き戻す。
//!
//! Web は `fandhe-device` の `BiometricAuth` トレイトを通らない（`clipboard.rs`/
//! `tts.rs`/`share.rs` と同じ理由。`fandhe-egui-proxy` の feature `device` は wasm32 に
//! 依存を持ち込まない設計）。
//!
//! # サーバー無し・端末ローカル確認に限る（対応表 `docs/device/capabilities/biometric-auth.json`
//! の web 欄と対）
//!
//! 本実装にはサーバー側の資格情報検証が無い。「認証した」は
//! `navigator.credentials.create`/`get` が成功し、返ってきた `authenticatorData` の
//! user verification（UV）フラグ（0x04）が立っていることだけで判定する
//! （platform authenticator が本人確認を行った、という端末ローカルの事実の確認に限る）。
//! challenge への署名・attestation の検証は行わない（そのためのサーバーを持たない）。
//! これはサーバー検証を伴う本人認証ではない。公開鍵・署名・attestationObject・
//! userHandle は保持しない。保持するのは登録時の credential ID のみ（`localStorage`）。
//!
//! # UV フラグは生体認証を意味しない（既知の天井）
//!
//! WebAuthn の user verification は仕様上 PIN・パスワード等も含み（生体方式に限定
//! されない、<https://www.w3.org/TR/webauthn-3/#user-verification>）、Windows Hello
//! を PIN のみで設定した端末では PIN 入力でも UV フラグが立つ。ブラウザ側 API には
//! 「生体で確認したか」を区別する標準的な手段が無いため、本モジュールは UV フラグが
//! 立てば [`fandhe_core::screens::biometric_check::AuthOutcome::Authenticated`] を返す
//! （macOS の `LAPolicyDeviceOwnerAuthenticationWithBiometrics` 固定とは異なり、Web の
//! 「認証した」は「生体認証した」ではなく「OS が本人確認に成功したと判定した」を意味する。
//! `AuthOutcome::Authenticated` のドキュメンテーションコメント・
//! `docs/device/capabilities/biometric-auth.json` の web 欄も同じ区別を明記する。
//! ponytail: ブラウザ側で生体方式を区別する標準 API が無い以上これが技術的な天井。
//! 将来 UA が `credProps` 等で方式を返すようになれば区別を追加する）。
//!
//! # 再認証は端末内の platform authenticator に限定する
//!
//! 登録（`create`）は `authenticatorSelection.authenticatorAttachment: "platform"` で
//! 端末内の authenticator に絞るが、再認証（`get`）の `allowCredentials` は
//! `transports` を指定しないと `hybrid`（QR コード経由でスマートフォン等の別端末を
//! 使う）を含む全 transport を許してしまう。同じ passkey が別端末（スマートフォン等）
//! に同期されている環境では、再認証時に別端末を選んで hybrid 経由で本人確認しても
//! 成功しうる（WebAuthn の `get` は `authenticatorAttachment` に `cross-platform` を
//! 返しうる、<https://www.w3.org/TR/webauthn-3/#dom-publickeycredential-authenticatorattachment>）。
//! これは「この端末で確認した」を意味せず、本モジュールが掲げる「端末ローカル確認」と
//! 一致しない。そのため `allowCredentials[].transports` を `["internal"]` に絞り、
//! さらに `create`/`get` いずれも成功時に返る `authenticatorAttachment` が `"platform"`
//! であることを UV フラグ判定より前に確認する（[`web::attachment_is_platform`]）。
//! `"platform"` でない・未定義（`authenticatorAttachment` 未実装の古いブラウザ）の
//! 場合は `Authenticated` にせず失敗として扱う（[`NOT_PLATFORM`]）。
//!
//! # ブラウザ側の制約
//!
//! - **user activation が必須**: `create`/`get` はボタンの `click` ハンドラから始まる
//!   同期チェーンの中で呼ぶ必要がある（`share.rs` の `navigator.share()` と同じ制約）。
//!   `mount.rs` の `attach_click` → `trigger_rerender` → `render_once` → `effect` は
//!   同一タスク内で同期実行されるため、この呼び出しも要件を満たす。`await` はこの
//!   呼び出しより前に挟まない。
//! - **`NotAllowedError` はキャンセルとして扱う**: ただし可否判定
//!   （`isUserVerifyingPlatformAuthenticatorAvailable`）が `false` を返している場合は
//!   「使えない」に倒す（authenticator が無い環境でも `NotAllowedError` が返ることが
//!   あるため）。credential が authenticator 側で削除済みのときも `NotAllowedError` が
//!   返り、キャンセルと区別できない天井がある（プライバシー仕様。ponytail: 復旧は
//!   `localStorage` のキー削除、自動再登録はしない — 誤ってキャンセルを再登録に倒すと
//!   UX が壊れるため）。
//! - **固定タイムアウトを掛けない**: `timeout` は設定しない（`BiometricCheckState` の
//!   trait doc と同じ方針）。
//!
//! 例外の `message` は `console` 出力・送信を一切行わない（security.md A09。
//! `share.rs::outcome_from_error_name` と同じ方針。`name` のみで固定文言へ畳む）。

use fandhe_core::screens::biometric_check::AuthOutcome;

/// `localStorage` に登録済み credential ID を保持するキー。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const STORAGE_KEY: &str = "fandhe.biometric_auth.credential_id";

/// 非対応ブラウザ・secure context 外・`navigator.credentials` 取得失敗時の案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const UNAVAILABLE: &str =
    "このブラウザ・接続では生体認証を使えません（https か localhost が必要）";
/// `localStorage` が使えない（登録済み credential ID を保持できない）ときの案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const STORAGE_UNAVAILABLE: &str = "このブラウザでは認証情報を保持できません";
/// `authenticatorData` を取得できなかった（古い Safari 等）ときの案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const NO_UV_DATA: &str = "認証データを確認できませんでした";
/// `authenticatorAttachment` が `"platform"` ではなかった（別端末で hybrid 経由の
/// 本人確認をした等）ときの案内（モジュール doc「再認証は端末内の platform
/// authenticator に限定する」節参照）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const NOT_PLATFORM: &str =
    "この端末の生体認証・PIN で確認できませんでした（別の端末で確認された可能性があります）";

/// `authenticatorData` の flags バイト（33 バイト目、0-indexed で 32）の UV ビット
/// （0x04）を確認する純関数（ブラウザ API を呼ばないため native からもテストできる、
/// `location.rs::failure_kind_from_code` と同じ理由で `wasm32` gate の外に置く）。
///
/// rpIdHash（32 バイト）+ flags（1 バイト）+ signCount（4 バイト）の最小 37 バイトに
/// 満たない場合は不正なデータとして `false` を返す（外部入力を検証せず解釈しない、
/// security.md A03）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn uv_flag_set(authenticator_data: &[u8]) -> bool {
    authenticator_data.len() >= 37 && authenticator_data[32] & 0x04 != 0
}

/// `PublicKeyCredential.authenticatorAttachment` が端末内の platform authenticator を
/// 意味するかを確認する純関数（`uv_flag_set` と同じ理由で `wasm32` gate の外に置く）。
///
/// `None`（未定義。`authenticatorAttachment` 未実装の古いブラウザ、または値自体が
/// 存在しない）は安全側（非 platform 扱い）に倒す（モジュール doc「再認証は端末内の
/// platform authenticator に限定する」節参照）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn attachment_is_platform(attachment: Option<&str>) -> bool {
    attachment == Some("platform")
}

/// 例外オブジェクトの `name` と可否判定の結果を画面結果へ写す純関数
/// （`share.rs::outcome_from_error_name` と同じ理由で `wasm32` gate の外に置く）。
///
/// `platform_available`: `isUserVerifyingPlatformAuthenticatorAvailable()` の結果
/// （取得できなかった場合は `None`）。`NotAllowedError` はキャンセルと「使えない」の
/// 両方の原因になりうるため、この値で区別する（モジュール doc 参照）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_error_name(
    name: Option<&str>,
    platform_available: Option<bool>,
) -> Result<AuthOutcome, String> {
    match name {
        Some("NotAllowedError") if platform_available == Some(false) => {
            Ok(AuthOutcome::Unavailable)
        }
        Some("NotAllowedError") => Ok(AuthOutcome::Cancelled),
        Some("NotSupportedError") => Ok(AuthOutcome::Unavailable),
        _ if platform_available == Some(false) => Ok(AuthOutcome::Unavailable),
        Some("SecurityError") => Err(UNAVAILABLE.to_string()),
        _ => Ok(AuthOutcome::Failed),
    }
}

/// バイト列を小文字 hex 文字列へ符号化する（credential ID を `localStorage` の
/// 文字列値として保持するだけの最小実装。base64 ライブラリを新規に足さない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 1 桁の hex 文字を 4 ビット値へ写す（バイト単位で判定するため、非 ASCII
/// （マルチバイト UTF-8）入力でも文字境界を跨がない）。
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// [`encode_hex`] の逆変換。不正な文字列（奇数長・非 hex 文字・非 ASCII）は `None`
/// （`localStorage` は他スクリプトからも書き込める外部入力のため検証する、
/// security.md A03）。バイト単位（`as_bytes`）で処理し文字列スライスを使わないため、
/// 非 ASCII 入力（マルチバイト UTF-8）でも文字境界を分断しない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| Some((hex_digit(chunk[0])? << 4) | hex_digit(chunk[1])?))
        .collect()
}

#[cfg(target_arch = "wasm32")]
mod web {
    use fandhe_core::screens::biometric_check::{
        finish_auth, mark_authenticating, take_request, AuthOutcome, BiometricCheckState,
    };
    use js_sys::{Array, Object, Reflect, Uint8Array};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        AuthenticatorAssertionResponse, AuthenticatorAttachment, AuthenticatorAttestationResponse,
        AuthenticatorSelectionCriteria, CredentialCreationOptions, CredentialRequestOptions,
        PublicKeyCredential, PublicKeyCredentialCreationOptions, PublicKeyCredentialDescriptor,
        PublicKeyCredentialParameters, PublicKeyCredentialRequestOptions,
        PublicKeyCredentialRpEntity, PublicKeyCredentialType, PublicKeyCredentialUserEntity,
        UserVerificationRequirement,
    };

    use super::{
        attachment_is_platform, decode_hex, encode_hex, outcome_from_error_name, uv_flag_set,
        NOT_PLATFORM, NO_UV_DATA, STORAGE_KEY, STORAGE_UNAVAILABLE, UNAVAILABLE,
    };
    use crate::mount::Applier;

    /// `target[name]` が `undefined`/`null` ではない値として実在するかを確認する
    /// （`share.rs::has_function` と同じ理由。型に現れない JS 側の実態を境界で検証する、
    /// security.md A03）。`navigator.credentials` はメソッドではなくオブジェクトを返す
    /// getter のため、関数判定の [`has_function`]（`create`/`canShare` 等のメソッド用）
    /// ではなく本関数で存在確認する。
    fn has_property(target: &JsValue, name: &str) -> bool {
        Reflect::get(target, &JsValue::from_str(name))
            .map(|value| !value.is_undefined() && !value.is_null())
            .unwrap_or(false)
    }

    /// `window.PublicKeyCredential` と、その静的メソッド
    /// `isUserVerifyingPlatformAuthenticatorAvailable` の実在を確認する。
    /// `navigator.credentials` はあっても `PublicKeyCredential` グローバルが
    /// 未定義の環境があり、未定義のまま静的メソッドを呼ぶと同期例外になって
    /// `finish_auth` に到達せず `Authenticating` のまま固まる（`has_property` と
    /// 同じ理由、security.md A03）。
    fn has_platform_authenticator_check(window: &web_sys::Window) -> bool {
        let window_value: JsValue = window.clone().into();
        match Reflect::get(&window_value, &JsValue::from_str("PublicKeyCredential")) {
            Ok(ctor) if !ctor.is_undefined() && !ctor.is_null() => Reflect::get(
                &ctor,
                &JsValue::from_str("isUserVerifyingPlatformAuthenticatorAvailable"),
            )
            .map(|f| f.is_function())
            .unwrap_or(false),
            _ => false,
        }
    }

    /// 32 バイトの challenge を毎回新規生成する（固定値・`Math.random` 不使用、
    /// security.md A02）。
    fn random_challenge(window: &web_sys::Window) -> Result<Vec<u8>, String> {
        let crypto = window.crypto().map_err(|_| UNAVAILABLE.to_string())?;
        let mut challenge = [0u8; 32];
        crypto
            .get_random_values_with_u8_array(&mut challenge)
            .map_err(|_| UNAVAILABLE.to_string())?;
        Ok(challenge.to_vec())
    }

    /// 登録用オプション（`navigator.credentials.create`）を組み立てる。
    ///
    /// `user_id` は呼び出し元（[`biometric_check_effect`]）が所有し、
    /// `credentials.create_with_options` の呼び出しが終わるまで生存させる必要がある
    /// （wasm-bindgen の `&mut [u8]` 引数は wasm 線形メモリを指すビューとして
    /// JS 側オブジェクトへそのまま渡り、コピーされない。この関数内のローカル変数に
    /// していると関数から戻った時点でスタックが破棄され、後続の呼び出しで
    /// メモリが上書きされて不正な user id が送られうる）。
    fn creation_options(
        challenge: &mut [u8],
        user_id: &mut [u8],
    ) -> PublicKeyCredentialCreationOptions {
        let rp = PublicKeyCredentialRpEntity::new("fandhe 型サンプル");
        // ユーザー識別子は「型サンプル」用の固定値（サーバーが無く実利用者を区別しない、
        // モジュール doc「サーバー無し・端末ローカル確認に限る」節参照）。
        let user = PublicKeyCredentialUserEntity::new_with_u8_slice(
            "fandhe-sample",
            "fandhe 型サンプル",
            user_id,
        );

        let es256 = PublicKeyCredentialParameters::new(-7, PublicKeyCredentialType::PublicKey);
        let rs256 = PublicKeyCredentialParameters::new(-257, PublicKeyCredentialType::PublicKey);
        let pub_key_cred_params = Array::new();
        pub_key_cred_params.push(&es256);
        pub_key_cred_params.push(&rs256);

        let selection = AuthenticatorSelectionCriteria::new();
        selection.set_authenticator_attachment(AuthenticatorAttachment::Platform);
        selection.set_user_verification(UserVerificationRequirement::Required);
        selection.set_resident_key("preferred");

        let options = PublicKeyCredentialCreationOptions::new_with_u8_slice(
            challenge,
            &pub_key_cred_params.into(),
            &rp,
            &user,
        );
        options.set_authenticator_selection(&selection);
        options
    }

    /// 認証用オプション（`navigator.credentials.get`）を組み立てる。
    ///
    /// `transports` を `["internal"]` に絞り、`hybrid`（別端末を QR コード経由で使う
    /// 経路）を候補から外す（モジュール doc「再認証は端末内の platform authenticator
    /// に限定する」節参照。この指定はブラウザへの要求であり必須の防御線ではないため、
    /// 成功後の `authenticatorAttachment` 確認と併せて二重化する）。
    fn request_options(
        challenge: &mut [u8],
        credential_id: &mut [u8],
    ) -> PublicKeyCredentialRequestOptions {
        let descriptor = PublicKeyCredentialDescriptor::new_with_u8_slice(
            credential_id,
            PublicKeyCredentialType::PublicKey,
        );
        let transports = Array::new();
        transports.push(&JsValue::from_str("internal"));
        descriptor.set_transports(&transports.into());
        let allow_credentials = Array::new();
        allow_credentials.push(&descriptor);

        let options = PublicKeyCredentialRequestOptions::new_with_u8_slice(challenge);
        options.set_allow_credentials(&allow_credentials.into());
        options.set_user_verification(UserVerificationRequirement::Required);
        options
    }

    /// `authenticatorData` を UV フラグへ写す（登録・認証どちらのレスポンスも
    /// `ArrayBuffer` を返す点は共通、取得手段だけ異なる）。
    fn extract_uv(response: &JsValue, is_create: bool) -> Result<bool, String> {
        let buffer = if is_create {
            let resp: &AuthenticatorAttestationResponse = response.unchecked_ref();
            resp.get_authenticator_data()
                .map_err(|_| NO_UV_DATA.to_string())?
        } else {
            let resp: &AuthenticatorAssertionResponse = response.unchecked_ref();
            resp.authenticator_data()
        };
        let bytes = Uint8Array::new(&buffer).to_vec();
        Ok(uv_flag_set(&bytes))
    }

    /// `mount_with_effect` へ渡す effect。`biometric_check` 画面の認証要求を処理する。
    ///
    /// 要求が無ければ何もしない（`mount` モジュール doc の「effect フック」節の契約）。
    pub fn biometric_check_effect(
        state: &BiometricCheckState,
        applier: &Applier<BiometricCheckState>,
    ) {
        let Some(request) = take_request(state) else {
            return;
        };

        // 次の再描画から `take_request` が `None` を返すようにする
        // （`share.rs` の「二重実行防止は不要」節と同じ判断）。
        applier.apply(mark_authenticating);
        let generation = request.generation;

        let finish = |applier: &Applier<BiometricCheckState>,
                      result: Result<AuthOutcome, String>| {
            let applier = applier.clone();
            applier.apply(move |s| finish_auth(s, generation, result));
        };

        let Some(window) = web_sys::window() else {
            finish(applier, Err(UNAVAILABLE.to_string()));
            return;
        };
        if !window.is_secure_context() {
            finish(applier, Err(UNAVAILABLE.to_string()));
            return;
        }
        let navigator = window.navigator();
        let navigator_value: JsValue = navigator.clone().into();
        if !has_property(&navigator_value, "credentials") {
            finish(applier, Err(UNAVAILABLE.to_string()));
            return;
        }

        let storage = match window.local_storage() {
            Ok(Some(storage)) => storage,
            _ => {
                finish(applier, Err(STORAGE_UNAVAILABLE.to_string()));
                return;
            }
        };

        let mut challenge = match random_challenge(&window) {
            Ok(c) => c,
            Err(message) => {
                finish(applier, Err(message));
                return;
            }
        };

        // `get_item` の失敗（アクセス拒否等）は「未登録」と区別する。未登録
        // （`Ok(None)`）や壊れた値（`decode_hex` が `None`）は create 経路へ倒してよいが、
        // 失敗はここで打ち切らないと create 経路が走り登録を繰り返しうる。
        let existing_id = match storage.get_item(STORAGE_KEY) {
            Ok(value) => value.and_then(|hex| decode_hex(&hex)),
            Err(_) => {
                finish(applier, Err(STORAGE_UNAVAILABLE.to_string()));
                return;
            }
        };

        let credentials = navigator.credentials();
        let is_create = existing_id.is_none();
        // `user_id` は create 経路でのみ使うが、`creation_options` に渡した後も
        // `credentials.create_with_options` の呼び出しが終わるまで生存させる必要があるため
        // ここ（`biometric_check_effect` のスコープ）で保持する（`creation_options` の
        // ドキュメンテーションコメント参照）。
        let mut user_id = [1u8];
        // `create`/`get` は user activation を要する同期チェーン内で呼ぶ（モジュール doc
        // 参照）。ここより前に `await` を挟まない。
        let promise = if let Some(mut credential_id) = existing_id {
            let options = request_options(&mut challenge, &mut credential_id);
            let request = CredentialRequestOptions::new();
            request.set_public_key(&options);
            credentials.get_with_options(&request)
        } else {
            let options = creation_options(&mut challenge, &mut user_id);
            let request = CredentialCreationOptions::new();
            request.set_public_key(&options);
            credentials.create_with_options(&request)
        };

        // 可否判定は認証 Promise の発火後でよい（既にブラウザ側で走っている）。
        // `PublicKeyCredential` グローバルが無い環境では呼ばない（`has_platform_authenticator_check`
        // 参照）。呼べない場合は「判定不能」として扱い、後段は `Err(_)` と同じ `None` に倒す。
        let availability = has_platform_authenticator_check(&window)
            .then(PublicKeyCredential::is_user_verifying_platform_authenticator_available);

        let promise = match promise {
            Ok(p) => p,
            Err(err) => {
                let name = Reflect::get(&err, &JsValue::from_str("name"))
                    .ok()
                    .and_then(|v| v.as_string());
                finish(applier, outcome_from_error_name(name.as_deref(), None));
                return;
            }
        };

        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = match JsFuture::from(promise).await {
                Ok(cred) => {
                    let cred: PublicKeyCredential = cred.unchecked_into();
                    let cred_value: JsValue = cred.clone().into();
                    // `authenticatorAttachment` は成功判定より前に確認する（UV フラグを
                    // 見る前にここで弾く。モジュール doc「再認証は端末内の platform
                    // authenticator に限定する」節参照。`web_sys_unstable_apis` cfg を
                    // プロジェクト全体へ広げないよう、型付き getter ではなく既存の
                    // `Reflect` 経由（`has_property` と同じ手段）で読む）。
                    let attachment =
                        Reflect::get(&cred_value, &JsValue::from_str("authenticatorAttachment"))
                            .ok()
                            .and_then(|v| v.as_string());
                    if !attachment_is_platform(attachment.as_deref()) {
                        Err(NOT_PLATFORM.to_string())
                    } else {
                        let response = cred.response();
                        let response_value: JsValue = response.into();
                        match extract_uv(&response_value, is_create) {
                            Ok(true) => {
                                // UV フラグが立った（=「OS が本人確認に成功したと判定した」。
                                // PIN 等の生体以外の方式を含みうる、モジュール doc「UV フラグは
                                // 生体認証を意味しない」節参照）。
                                //
                                // 登録直後のみ credential ID を保存する（既に登録済みの
                                // `get` 経路では上書きの必要が無い）。保存に失敗すると
                                // 次回も ID が無く create が走り登録を繰り返すため、
                                // 成功扱いにせず利用者へ伝える。
                                if is_create {
                                    let raw_id =
                                        Uint8Array::new(&Object::from(cred.raw_id())).to_vec();
                                    match storage.set_item(STORAGE_KEY, &encode_hex(&raw_id)) {
                                        Ok(()) => Ok(AuthOutcome::Authenticated),
                                        Err(_) => Err(STORAGE_UNAVAILABLE.to_string()),
                                    }
                                } else {
                                    Ok(AuthOutcome::Authenticated)
                                }
                            }
                            Ok(false) => Ok(AuthOutcome::Failed),
                            Err(message) => Err(message),
                        }
                    }
                }
                Err(err) => {
                    // `message` は転送しない（モジュール doc 参照）。`name` のみで判定する。
                    let name = Reflect::get(&err, &JsValue::from_str("name"))
                        .ok()
                        .and_then(|v| v.as_string());
                    let platform_available = match availability {
                        Some(promise) => match JsFuture::from(promise).await {
                            Ok(v) => v.as_bool(),
                            Err(_) => None,
                        },
                        None => None,
                    };
                    outcome_from_error_name(name.as_deref(), platform_available)
                }
            };
            applier.apply(move |s| finish_auth(s, generation, result));
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::biometric_check_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uv_flag_detects_bit() {
        let mut data = vec![0u8; 37];
        data[32] = 0x04;
        assert!(uv_flag_set(&data));
    }

    #[test]
    fn uv_flag_absent_without_bit() {
        let mut data = vec![0u8; 37];
        data[32] = 0x01;
        assert!(!uv_flag_set(&data));
    }

    #[test]
    fn uv_flag_false_when_too_short() {
        let data = vec![0u8; 36];
        assert!(!uv_flag_set(&data));
    }

    #[test]
    fn not_allowed_without_platform_is_unavailable() {
        assert_eq!(
            outcome_from_error_name(Some("NotAllowedError"), Some(false)),
            Ok(AuthOutcome::Unavailable)
        );
    }

    #[test]
    fn not_allowed_with_platform_is_cancelled() {
        assert_eq!(
            outcome_from_error_name(Some("NotAllowedError"), Some(true)),
            Ok(AuthOutcome::Cancelled)
        );
        assert_eq!(
            outcome_from_error_name(Some("NotAllowedError"), None),
            Ok(AuthOutcome::Cancelled)
        );
    }

    #[test]
    fn not_supported_is_unavailable() {
        assert_eq!(
            outcome_from_error_name(Some("NotSupportedError"), None),
            Ok(AuthOutcome::Unavailable)
        );
    }

    #[test]
    fn security_error_is_failure_message() {
        assert_eq!(
            outcome_from_error_name(Some("SecurityError"), None),
            Err(UNAVAILABLE.to_string())
        );
    }

    #[test]
    fn unknown_or_missing_name_is_failed_outcome() {
        assert_eq!(outcome_from_error_name(None, None), Ok(AuthOutcome::Failed));
        assert_eq!(
            outcome_from_error_name(Some("InvalidStateError"), None),
            Ok(AuthOutcome::Failed)
        );
        assert_eq!(
            outcome_from_error_name(Some("AbortError"), None),
            Ok(AuthOutcome::Failed)
        );
    }

    #[test]
    fn no_branch_produces_authenticated() {
        // `Authenticated` は UV フラグを実際に確認したときのみ（`mod web` の
        // `extract_uv` 経路）成立する。例外系からは絶対に出さない
        // （security.md A07、モジュール doc 参照）。
        let names = [
            None,
            Some("NotAllowedError"),
            Some("NotSupportedError"),
            Some("SecurityError"),
            Some("AbortError"),
            Some("InvalidStateError"),
            Some("UnknownError"),
        ];
        for name in names {
            for platform_available in [None, Some(true), Some(false)] {
                assert_ne!(
                    outcome_from_error_name(name, platform_available),
                    Ok(AuthOutcome::Authenticated)
                );
            }
        }
    }

    #[test]
    fn attachment_platform_is_accepted() {
        assert!(attachment_is_platform(Some("platform")));
    }

    #[test]
    fn attachment_cross_platform_is_rejected() {
        // hybrid 経由（別端末での本人確認）は `cross-platform` として返る
        // （P1 指摘の再発防止、モジュール doc「再認証は端末内の platform
        // authenticator に限定する」節参照）。
        assert!(!attachment_is_platform(Some("cross-platform")));
    }

    #[test]
    fn attachment_missing_is_rejected() {
        // `authenticatorAttachment` 未実装の古いブラウザは安全側（非 platform）に倒す。
        assert!(!attachment_is_platform(None));
    }

    #[test]
    fn hex_round_trips() {
        let bytes = vec![0u8, 1, 255, 16, 128];
        let hex = encode_hex(&bytes);
        assert_eq!(decode_hex(&hex), Some(bytes));
    }

    #[test]
    fn decode_hex_rejects_odd_length() {
        assert_eq!(decode_hex("abc"), None);
    }

    #[test]
    fn decode_hex_rejects_non_hex() {
        assert_eq!(decode_hex("zz"), None);
    }

    #[test]
    fn decode_hex_rejects_non_ascii_without_panicking() {
        // UTF-8 で 4 バイトの絵文字は偶数長チェックを通過しうるが、バイト単位で
        // 判定するため文字境界を分断せず `None` を返す（P1 指摘の再発防止）。
        assert_eq!(decode_hex("😀"), None);
    }
}
