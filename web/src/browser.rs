//! The browser around the app: fetch, downloads, the file picker, this
//! browser's own storage, timers. Everything here is a thin wrapper over the
//! web platform, so the store reads like Rust.

use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

pub fn window() -> web_sys::Window {
    web_sys::window().expect("running in a browser window")
}

pub fn document() -> web_sys::Document {
    window().document().expect("a page")
}

/// Milliseconds since the epoch, by the browser's clock.
pub fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// What the server said: its status and, when it sent JSON, the JSON.
pub struct Answer {
    pub status: u16,
    pub json: serde_json::Value,
}

impl Answer {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn error(&self) -> String {
        self.json
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("The team library did not answer.")
            .to_string()
    }
}

/// Ask our own server something. `Err` only when it could not be reached.
pub async fn call(method: &str, path: &str, body: Option<&serde_json::Value>) -> Result<Answer, String> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    let headers = web_sys::Headers::new().map_err(js_err)?;
    headers.set("Accept", "application/json").map_err(js_err)?;
    if let Some(b) = body {
        headers.set("Content-Type", "application/json").map_err(js_err)?;
        init.set_body(&JsValue::from_str(&b.to_string()));
    }
    init.set_headers(&headers);
    let request = web_sys::Request::new_with_str_and_init(path, &init).map_err(js_err)?;
    let resp: web_sys::Response = JsFuture::from(window().fetch_with_request(&request))
        .await
        .map_err(js_err)?
        .dyn_into()
        .map_err(js_err)?;
    let status = resp.status();
    let is_json = resp
        .headers()
        .get("content-type")
        .ok()
        .flatten()
        .map(|t| t.contains("json"))
        .unwrap_or(false);
    let json = if is_json {
        let text = JsFuture::from(resp.text().map_err(js_err)?).await.map_err(js_err)?;
        serde_json::from_str(&text.as_string().unwrap_or_default()).unwrap_or(serde_json::Value::Null)
    } else {
        serde_json::Value::Null
    };
    Ok(Answer { status, json })
}

/// The bytes at a path on our server (a picture).
pub async fn bytes(path: &str) -> Result<Vec<u8>, String> {
    let resp: web_sys::Response = JsFuture::from(window().fetch_with_str(path))
        .await
        .map_err(js_err)?
        .dyn_into()
        .map_err(js_err)?;
    if !resp.ok() {
        return Err(format!("{}", resp.status()));
    }
    let buf = JsFuture::from(resp.array_buffer().map_err(js_err)?).await.map_err(js_err)?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

/// Let go of a script as the tab closes: a beacon is the one request a
/// closing page is allowed to finish.
pub fn beacon(path: &str) {
    let _ = window().navigator().send_beacon(path);
}

/// Hand a file to the browser as a download.
pub fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    let mime = match name.rsplit('.').next().unwrap_or("") {
        "pdf" => "application/pdf",
        "fdx" => "application/xml",
        "md" => "text/markdown",
        _ => "text/plain",
    };
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes));
    let props = web_sys::BlobPropertyBag::new();
    props.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &props).map_err(js_err)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js_err)?;
    let a: web_sys::HtmlAnchorElement = document()
        .create_element("a")
        .map_err(js_err)?
        .dyn_into()
        .map_err(|_| "not an anchor".to_string())?;
    a.set_href(&url);
    a.set_download(name);
    a.style().set_property("display", "none").map_err(js_err)?;
    document().body().ok_or("no page body")?.append_child(&a).map_err(js_err)?;
    a.click();
    a.remove();
    // the download has its own copy once it has started
    let revoke = Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 60_000);
    Ok(())
}

pub fn open_tab(url: &str) {
    let _ = window().open_with_url_and_target_and_features(url, "_blank", "noopener");
}

pub fn go(path: &str) {
    let _ = window().location().assign(path);
}

pub fn reload() {
    let _ = window().location().reload();
}

// ---------- this browser's own storage ----------

pub fn stored(key: &str) -> Option<String> {
    window().local_storage().ok()??.get_item(key).ok()?
}

pub fn store(key: &str, value: &str) {
    if let Ok(Some(s)) = window().local_storage() {
        let _ = s.set_item(key, value);
    }
}

pub fn forget(key: &str) {
    if let Ok(Some(s)) = window().local_storage() {
        let _ = s.remove_item(key);
    }
}

// ---------- ids ----------

/// `bytes` random bytes as hex, from the browser's cryptographic source.
pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    if let Ok(c) = window().crypto() {
        let _ = c.get_random_values_with_u8_array(&mut b);
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---------- the file picker ----------

/// A hidden file input that hands each picked file's name and contents to
/// `then`. Clicked from the app's own Import button: the browser still counts
/// that as the person's click, so the picker is allowed to open.
pub struct Picker {
    input: web_sys::HtmlInputElement,
    _on_change: Closure<dyn FnMut()>,
}

impl Picker {
    pub fn new(then: Rc<dyn Fn(String, Vec<u8>)>) -> Option<Picker> {
        let input: web_sys::HtmlInputElement = document().create_element("input").ok()?.dyn_into().ok()?;
        input.set_type("file");
        input.set_multiple(true);
        input.set_accept(".md,.markdown,.fountain,.spmd,.fdx,.txt");
        input.style().set_property("display", "none").ok()?;
        document().body()?.append_child(&input).ok()?;
        let me = input.clone();
        let on_change = Closure::<dyn FnMut()>::new(move || {
            let Some(files) = me.files() else { return };
            for k in 0..files.length() {
                let Some(file) = files.get(k) else { continue };
                let then = then.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let name = file.name();
                    if let Ok(buf) = JsFuture::from(file.array_buffer()).await {
                        then(name, js_sys::Uint8Array::new(&buf).to_vec());
                    }
                });
            }
            // the same file can be picked twice in a row
            me.set_value("");
        });
        input
            .add_event_listener_with_callback("change", on_change.as_ref().unchecked_ref())
            .ok()?;
        Some(Picker {
            input,
            _on_change: on_change,
        })
    }

    pub fn open(&self) {
        self.input.click();
    }
}

// ---------- timers and page events ----------

/// Run `tick` every `ms` milliseconds, for as long as the page is open. A
/// browser keeps these going in a background tab (if slower), unlike the
/// app's own frames.
pub fn every(ms: i32, tick: impl FnMut() + 'static) {
    let f = Closure::<dyn FnMut()>::new(tick);
    let _ = window().set_interval_with_callback_and_timeout_and_arguments_0(f.as_ref().unchecked_ref(), ms);
    f.forget();
}

/// Run `then` whenever the page event `name` fires on the window or document.
pub fn on(name: &str, on_document: bool, then: impl FnMut() + 'static) {
    let f = Closure::<dyn FnMut()>::new(then);
    let target: web_sys::EventTarget = if on_document { document().into() } else { window().into() };
    let _ = target.add_event_listener_with_callback(name, f.as_ref().unchecked_ref());
    f.forget();
}

pub fn visible() -> bool {
    document().visibility_state() == web_sys::VisibilityState::Visible
}

/// The page's address after `#`, without it.
pub fn hash() -> String {
    window()
        .location()
        .hash()
        .unwrap_or_default()
        .trim_start_matches('#')
        .to_string()
}

pub fn clear_hash() {
    if let Ok(h) = window().history() {
        let _ = h.replace_state_with_url(&JsValue::NULL, "", Some("/"));
    }
}

/// The loading card in index.html fades out once the app has a frame.
pub fn hide_loading_card() {
    if let Some(card) = document().get_element_by_id("ns-loading") {
        let _ = card.class_list().add_1("gone");
    }
}

/// Say something on the loading card, when the app cannot start at all.
pub fn loading_card_says(text: &str) {
    if let Some(line) = document().get_element_by_id("ns-loading-line") {
        line.set_text_content(Some(text));
    }
}

pub fn js_err(e: JsValue) -> String {
    e.as_string()
        .or_else(|| js_sys::JSON::stringify(&e).ok().and_then(|s| s.as_string()))
        .unwrap_or_else(|| "a browser error".to_string())
}
