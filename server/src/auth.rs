//! Signing in with Discord, and knowing who is signed in.
//!
//! Discord only speaks OAuth2, not OpenID Connect, so this does the dance
//! itself: Discord says who you are, the team list says whether you may come
//! in, and a session cookie remembers you for a month. Only a hash of the
//! cookie's token is kept, so the database alone signs nobody in.

use base64::Engine;
use serde::Deserialize;
use worker::{Fetch, Headers, Method, Request, RequestInit, Response, Result};

use crate::logic::{self, Role, SESSION_COOKIE, SESSION_MS, STATE_COOKIE};
use crate::web::{self, n, opt, s, Cx, Me};

/// Who is asking: their session, still valid, and still on the team.
pub async fn current(req: &Request, cx: &Cx) -> Result<Option<Me>> {
    let header = req.headers().get("Cookie")?.unwrap_or_default();
    let Some(token) = logic::cookie(&header, SESSION_COOKIE) else {
        return Ok(None);
    };
    let hash = web::sha256_hex(&token);

    #[derive(Deserialize)]
    struct Row {
        id: String,
        name: String,
        expires_at: i64,
        role: Option<String>,
    }
    let row: Option<Row> = cx
        .db
        .prepare(
            "SELECT u.id, u.name, s.expires_at, m.role FROM sessions s \
             JOIN users u ON u.id = s.user_id \
             LEFT JOIN members m ON m.discord_id = u.id \
             WHERE s.hash = ?1",
        )
        .bind(&[s(&hash)])?
        .first(None)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let role = logic::admit(&row.id, &cx.owners, row.role.as_deref().and_then(Role::parse));
    match role {
        Some(role) if row.expires_at > cx.now => Ok(Some(Me {
            id: row.id,
            name: row.name,
            role,
        })),
        // run out, or taken off the team since: the session goes
        _ => {
            cx.db.prepare("DELETE FROM sessions WHERE hash = ?1").bind(&[s(&hash)])?.run().await?;
            Ok(None)
        }
    }
}

/// Off to Discord to ask who this is.
pub async fn discord(cx: &Cx) -> Result<Response> {
    let client = cx.var("DISCORD_CLIENT_ID");
    if client.trim().is_empty() {
        return web::fail(500, "Discord sign-in is not set up yet: DISCORD_CLIENT_ID is missing.");
    }
    let state = web::random_hex(16);
    let url = format!(
        "https://discord.com/oauth2/authorize?response_type=code&client_id={}&scope=identify&prompt=none&state={}&redirect_uri={}",
        enc(client.trim()),
        state,
        enc(&callback_url(cx)),
    );
    web::redirect(&url, &[logic::set_cookie(STATE_COOKIE, &state, 600)])
}

/// Discord sends people back here with a code to trade for who they are.
pub async fn discord_callback(req: &Request, cx: &Cx) -> Result<Response> {
    let url = req.url()?;
    let mut code = None;
    let mut state = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            _ => {}
        }
    }
    let header = req.headers().get("Cookie")?.unwrap_or_default();
    let expected = logic::cookie(&header, STATE_COOKIE);
    let (Some(code), Some(state), Some(expected)) = (code, state, expected) else {
        return web::redirect("/#signin=cancelled", &[]);
    };
    if state != expected {
        return web::redirect("/#signin=expired", &[]);
    }

    // trade the code for a token ...
    let form = format!(
        "client_id={}&client_secret={}&grant_type=authorization_code&code={}&redirect_uri={}",
        enc(cx.var("DISCORD_CLIENT_ID").trim()),
        enc(cx.secret("DISCORD_CLIENT_SECRET").trim()),
        enc(&code),
        enc(&callback_url(cx)),
    );
    let headers = Headers::new();
    headers.set("Content-Type", "application/x-www-form-urlencoded")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(worker::wasm_bindgen::JsValue::from_str(&form)));
    let mut token_resp = Fetch::Request(Request::new_with_init("https://discord.com/api/oauth2/token", &init)?)
        .send()
        .await?;
    #[derive(Deserialize)]
    struct Token {
        access_token: String,
    }
    let Ok(token) = token_resp.json::<Token>().await else {
        return web::redirect("/#signin=failed", &[]);
    };

    // ... and the token for who they are
    #[derive(Deserialize)]
    struct DiscordUser {
        id: String,
        username: String,
        global_name: Option<String>,
        avatar: Option<String>,
    }
    let headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {}", token.access_token))?;
    let mut init = RequestInit::new();
    init.with_method(Method::Get).with_headers(headers);
    let mut who = Fetch::Request(Request::new_with_init("https://discord.com/api/users/@me", &init)?)
        .send()
        .await?;
    let Ok(user) = who.json::<DiscordUser>().await else {
        return web::redirect("/#signin=failed", &[]);
    };

    // their picture, small, kept with them so the team never waits on Discord
    let mut avatar = None;
    if let Some(hash) = &user.avatar {
        let url = format!("https://cdn.discordapp.com/avatars/{}/{hash}.png?size=64", user.id);
        if let Ok(parsed) = worker::Url::parse(&url) {
            if let Ok(mut r) = Fetch::Url(parsed).send().await {
                if r.status_code() == 200 {
                    if let Ok(bytes) = r.bytes().await {
                        if bytes.len() < 200_000 {
                            avatar = Some((base64::engine::general_purpose::STANDARD.encode(bytes), hash.clone()));
                        }
                    }
                }
            }
        }
    }
    let name = logic::display_name(user.global_name.as_deref(), &user.username);
    sign_in(cx, &user.id, &name, avatar).await
}

/// The development sign-in: `/auth/dev?id=<discord id>&name=Sam`, on this
/// machine only, with DEV_LOGIN=1. The team list still decides.
pub async fn dev_sign_in(req: &Request, cx: &Cx) -> Result<Response> {
    if !cx.dev_login() {
        return web::fail(404, "Not here.");
    }
    let url = req.url()?;
    let mut id = String::new();
    let mut name = String::new();
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "id" => id = v.into_owned(),
            "name" => name = v.into_owned(),
            _ => {}
        }
    }
    if !logic::is_discord_id(&id) {
        return web::fail(400, "id must be a Discord user id");
    }
    if name.trim().is_empty() {
        name = format!("Dev {}", &id[id.len() - 4..]);
    }
    sign_in(cx, &id, &name, None).await
}

pub async fn sign_out(req: &Request, cx: &Cx) -> Result<Response> {
    let header = req.headers().get("Cookie")?.unwrap_or_default();
    if let Some(token) = logic::cookie(&header, SESSION_COOKIE) {
        cx.db
            .prepare("DELETE FROM sessions WHERE hash = ?1")
            .bind(&[s(&web::sha256_hex(&token))])?
            .run()
            .await?;
    }
    let mut r = web::done()?;
    r.headers_mut().append("Set-Cookie", &logic::set_cookie(SESSION_COOKIE, "", 0))?;
    Ok(r)
}

/// Let someone in — if they are on the list.
async fn sign_in(cx: &Cx, id: &str, name: &str, avatar: Option<(String, String)>) -> Result<Response> {
    #[derive(Deserialize)]
    struct Listed {
        role: String,
    }
    let listed: Option<Listed> = cx
        .db
        .prepare("SELECT role FROM members WHERE discord_id = ?1")
        .bind(&[s(id)])?
        .first(None)
        .await?;
    let role = logic::admit(id, &cx.owners, listed.and_then(|l| Role::parse(&l.role)));
    let clear_state = logic::set_cookie(STATE_COOKIE, "", 0);
    if role.is_none() {
        // not on the team: the app shows them their id to send to an owner
        return web::redirect(&format!("/#denied={id}"), &[clear_state]);
    }

    let (pic, version) = match &avatar {
        Some((b64, v)) => (Some(b64.as_str()), Some(v.as_str())),
        None => (None, None),
    };
    cx.db
        .prepare(
            "INSERT INTO users (id, name, avatar, avatar_version, created_at, last_seen) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?5) \
             ON CONFLICT (id) DO UPDATE SET name = excluded.name, \
               avatar = COALESCE(excluded.avatar, users.avatar), \
               avatar_version = COALESCE(excluded.avatar_version, users.avatar_version), \
               last_seen = excluded.last_seen",
        )
        .bind(&[s(id), s(name), opt(pic), opt(version), n(cx.now)])?
        .run()
        .await?;

    let token = web::random_hex(32);
    cx.db
        .prepare("INSERT INTO sessions (hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)")
        .bind(&[s(&web::sha256_hex(&token)), s(id), n(cx.now), n(cx.now + SESSION_MS)])?
        .run()
        .await?;
    // and a little housekeeping while we are here
    cx.db
        .prepare("DELETE FROM sessions WHERE expires_at < ?1")
        .bind(&[n(cx.now)])?
        .run()
        .await?;

    web::redirect(
        "/",
        &[logic::set_cookie(SESSION_COOKIE, &token, SESSION_MS / 1000), clear_state],
    )
}

fn callback_url(cx: &Cx) -> String {
    format!("{}/auth/discord/callback", cx.origin)
}

/// Percent-encoding for a query string value.
fn enc(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for b in v.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
