//! Requests in, responses out: who is asking, whether they may, and which
//! handler answers.

use serde::Serialize;
use worker::wasm_bindgen::JsValue;
use worker::{D1Database, Env, Request, Response, Result};

use crate::logic::{self, Role, Route};
use crate::{api, auth};

/// Everything a handler needs about the request it is answering.
pub struct Cx {
    pub env: Env,
    pub db: D1Database,
    pub now: i64,
    /// "https://northstar.example.workers.dev", where the request was sent.
    pub origin: String,
    pub host: String,
    pub owners: Vec<String>,
}

impl Cx {
    pub fn var(&self, name: &str) -> String {
        self.env.var(name).map(|v| v.to_string()).unwrap_or_default()
    }
    pub fn secret(&self, name: &str) -> String {
        self.env
            .secret(name)
            .map(|v| v.to_string())
            .or_else(|_| self.env.var(name).map(|v| v.to_string()))
            .unwrap_or_default()
    }
    pub fn team_name(&self) -> String {
        let t = self.var("TEAM_NAME");
        if t.trim().is_empty() {
            "The team".to_string()
        } else {
            t
        }
    }
    /// The development sign-in: only with DEV_LOGIN=1 (set in .dev.vars,
    /// which is never deployed), and only on this machine.
    pub fn dev_login(&self) -> bool {
        self.var("DEV_LOGIN") == "1" && logic::is_local(&self.host)
    }
}

/// Someone signed in.
#[derive(Clone)]
pub struct Me {
    pub id: String,
    pub name: String,
    pub role: Role,
}

pub async fn handle(mut req: Request, env: Env) -> Result<Response> {
    let url = req.url()?;
    let route = logic::route(req.method().as_ref(), url.path());
    if route == Route::NotFound {
        // everything else is the app itself
        return match env.assets("ASSETS") {
            Ok(assets) => assets.fetch_request(req).await,
            Err(_) => fail(404, "Not here."),
        };
    }

    let host = url.host_str().unwrap_or("").to_string();
    let origin = match url.port() {
        Some(p) => format!("{}://{host}:{p}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    };
    let cx = Cx {
        db: env.d1("DB")?,
        now: worker::Date::now().as_millis() as i64,
        origin,
        host,
        owners: logic::owners(&env.var("OWNER_DISCORD_IDS").map(|v| v.to_string()).unwrap_or_default()),
        env,
    };

    if route.changes_things() {
        let origin = req.headers().get("Origin")?;
        let site = req.headers().get("Sec-Fetch-Site")?;
        if !logic::same_origin(origin.as_deref(), site.as_deref(), &cx.origin) {
            return fail(403, "That request did not come from Northstar.");
        }
    }

    // signing in needs no session
    match route {
        Route::Discord => return auth::discord(&cx).await,
        Route::DiscordCallback => return auth::discord_callback(&req, &cx).await,
        Route::DevSignIn => return auth::dev_sign_in(&req, &cx).await,
        Route::SignOut => return auth::sign_out(&req, &cx).await,
        _ => {}
    }

    let Some(me) = auth::current(&req, &cx).await? else {
        return fail(401, "Sign in to Northstar first.");
    };
    if route.writes() && !me.role.can_write() {
        return fail(403, "Viewers can read and export, but not change the library.");
    }
    if route.owners_only() && me.role != Role::Owner {
        return fail(403, "Only an owner can change who is on the team.");
    }
    api::answer(route, &mut req, &cx, &me).await
}

// ---------- answers ----------

pub fn json<T: Serialize>(status: u16, value: &T) -> Result<Response> {
    let mut r = Response::from_json(value)?.with_status(status);
    let h = r.headers_mut();
    h.set("Cache-Control", "no-store")?;
    h.set("X-Content-Type-Options", "nosniff")?;
    Ok(r)
}

pub fn fail(status: u16, message: &str) -> Result<Response> {
    json(status, &serde_json::json!({ "error": message }))
}

pub fn done() -> Result<Response> {
    json(200, &serde_json::json!({ "ok": true }))
}

/// A 302 that can still carry cookies (`Response::redirect`'s headers cannot
/// be changed).
pub fn redirect(to: &str, cookies: &[String]) -> Result<Response> {
    let mut r = Response::empty()?.with_status(302);
    let h = r.headers_mut();
    h.set("Location", to)?;
    h.set("Cache-Control", "no-store")?;
    for c in cookies {
        h.append("Set-Cookie", c)?;
    }
    Ok(r)
}

/// A request's JSON body. `Err` is the 400 to answer with: too big, or not
/// what was expected.
pub async fn body<T: serde::de::DeserializeOwned>(req: &mut Request, limit: usize) -> std::result::Result<T, Response> {
    let bad = |m: &str| fail(400, m).unwrap_or_else(|_| Response::empty().expect("an empty response"));
    let text = req.text().await.map_err(|_| bad("The request could not be read."))?;
    if text.len() > limit {
        return Err(bad("That is too big for the team library: the limit is about 1.9 MB a script."));
    }
    serde_json::from_str(&text).map_err(|e| bad(&format!("That request was not understood: {e}")))
}

// ---------- small things ----------

pub fn s(v: &str) -> JsValue {
    JsValue::from_str(v)
}

pub fn n(v: i64) -> JsValue {
    JsValue::from_f64(v as f64)
}

pub fn opt(v: Option<&str>) -> JsValue {
    v.map(JsValue::from_str).unwrap_or(JsValue::NULL)
}

/// `bytes` random bytes, as hex.
pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::getrandom(&mut b).expect("the runtime always has a random source");
    logic::hex(&b)
}

pub fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    logic::hex(&Sha256::digest(text.as_bytes()))
}

/// Everything changes shape now and then; browsers notice by reading this.
pub async fn bump(cx: &Cx) -> Result<()> {
    cx.db.prepare("UPDATE stamp SET v = v + 1 WHERE id = 1").run().await?;
    Ok(())
}
