//! Permissions API の問い合わせ（端末機能の Web 経路で共用）。

use wasm_bindgen::{JsCast, JsValue};

/// `navigator.permissions.query({name})` の `PermissionStatus` を返す。
///
/// API が無い・名前を受け付けない（reject）・結果が object でない環境は `None`。
/// 呼び出し側は `None` を「許可状態は不明」として扱い、拒否とは見なさない。
pub async fn query_permission(name: &str) -> Option<JsValue> {
    let get =
        |t: &JsValue, k: &str| js_sys::Reflect::get(t, &k.into()).unwrap_or(JsValue::UNDEFINED);
    let navigator: JsValue = web_sys::window()?.navigator().into();
    let permissions = get(&navigator, "permissions");
    let query = get(&permissions, "query");
    if !query.is_function() {
        return None;
    }
    let desc = js_sys::Object::new();
    js_sys::Reflect::set(&desc, &"name".into(), &name.into()).ok()?;
    let promise = query
        .unchecked_into::<js_sys::Function>()
        .call1(&permissions, &desc)
        .ok()?
        .dyn_into::<js_sys::Promise>()
        .ok()?;
    let status = wasm_bindgen_futures::JsFuture::from(promise).await.ok()?;
    status.is_object().then_some(status)
}
