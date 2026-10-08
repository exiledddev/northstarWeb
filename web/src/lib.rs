//! Northstar in a browser tab.
//!
//! This crate is small on purpose: the app — editor, Cards, Reading mode,
//! every theme, the PDF — is the desktop app's own code (the `northstar`
//! crate). What lives here is only what a browser needs instead of a desktop:
//! the team library behind the app's `Store` (remote.rs), the door you sign
//! in through (signin.rs), and the glue to the page (browser.rs).

mod browser;
mod remote;
mod signin;

use northstar::app::App;
use northstar::backend::Role;
use northstar::eframe;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use remote::{Boot, MemberRow, PersonRow, RemoteStore, ScriptRow};
use signin::{SignIn, Why};

#[wasm_bindgen(start)]
pub fn start() {
    wasm_bindgen_futures::spawn_local(boot());
}

async fn boot() {
    let Some(canvas) = browser::document()
        .get_element_by_id("ns-canvas")
        .and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok())
    else {
        browser::loading_card_says("This page is missing its canvas.");
        return;
    };

    let me = match browser::call("GET", "/api/me", None).await {
        Ok(a) => a,
        Err(_) => {
            browser::loading_card_says("Can't reach the team library. Check your connection and reload.");
            return;
        }
    };
    if me.status == 401 {
        // not signed in, or back from Discord with something to say
        let hash = browser::hash();
        let why = if let Some(id) = hash.strip_prefix("denied=") {
            Why::Denied(id.to_string())
        } else if hash.starts_with("signin=expired") {
            Why::Trouble("That sign-in took too long. Try again.")
        } else if hash.starts_with("signin=failed") {
            Why::Trouble("Discord did not say who you are. Try again.")
        } else {
            Why::SignIn
        };
        browser::clear_hash();
        run(canvas, move |cc| Box::new(SignIn::new(cc, why))).await;
        return;
    }
    if !me.ok() {
        browser::loading_card_says(&me.error());
        return;
    }

    // who you are, and the library as it stands
    let settings = browser::call("GET", "/api/settings", None).await.ok();
    let scripts = browser::call("GET", "/api/scripts", None).await.ok();
    let members = browser::call("GET", "/api/members", None).await.ok();
    let take = |a: &Option<browser::Answer>, k: &str| a.as_ref().map(|a| a.json[k].clone()).unwrap_or_default();
    let Ok(me_row) = serde_json::from_value::<PersonRow>(me.json["me"].clone()) else {
        browser::loading_card_says("The team library sent something this page does not understand. Reload to try again.");
        return;
    };
    let boot = Boot {
        me: me_row,
        role: Role::from_slug(me.json["role"].as_str().unwrap_or("viewer")).unwrap_or(Role::Viewer),
        team: me.json["team"].as_str().unwrap_or("The team").to_string(),
        settings: take(&settings, "body").as_str().unwrap_or_default().to_string(),
        scripts: serde_json::from_value::<Vec<ScriptRow>>(take(&scripts, "scripts")).unwrap_or_default(),
        people: serde_json::from_value::<Vec<PersonRow>>(take(&scripts, "people")).unwrap_or_default(),
        members: serde_json::from_value::<Vec<MemberRow>>(take(&members, "members")).unwrap_or_default(),
    };
    browser::clear_hash();
    let store = RemoteStore::new(boot);
    run(canvas, move |cc| {
        store.set_context(cc.egui_ctx.clone());
        Box::new(App::with_store(&cc.egui_ctx, Box::new(store)))
    })
    .await;
}

/// Start an app on the page's canvas, and let the loading card go.
async fn run(
    canvas: web_sys::HtmlCanvasElement,
    make: impl FnOnce(&eframe::CreationContext<'_>) -> Box<dyn eframe::App> + 'static,
) {
    let options = eframe::WebOptions::default();
    let started = eframe::WebRunner::new()
        .start(canvas, options, Box::new(move |cc| Ok(make(cc))))
        .await;
    match started {
        Ok(()) => browser::hide_loading_card(),
        Err(e) => browser::loading_card_says(&format!(
            "This browser could not start Northstar ({}). It needs WebGL 2.",
            browser::js_err(e)
        )),
    }
}
