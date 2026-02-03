// HTTP client for web chat API.

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, Response};

pub async fn send_chat_message(scope: &str, text: &str) -> Result<(), String> {
    let window = web_sys::window().ok_or("No window")?;
    let location = window.location();
    let origin = location.origin().map_err(|_| "No origin")?;
    let url = format!("{}/api/chat", origin);

    let body = serde_json::json!({
        "scope": scope,
        "text": text
    });
    let body_str = serde_json::to_string(&body).map_err(|e| e.to_string())?;

    let opts = RequestInit::new();
    opts.set_method("POST");
    opts.set_body(&wasm_bindgen::JsValue::from_str(&body_str));

    let request = Request::new_with_str_and_init(&url, &opts).map_err(|e| format!("{:?}", e))?;
    request
        .headers()
        .set("Content-Type", "application/json")
        .map_err(|e| format!("{:?}", e))?;

    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("{:?}", e))?;

    let resp: Response = resp_value
        .dyn_into()
        .map_err(|_| "Response cast failed")?;

    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }

    Ok(())
}
