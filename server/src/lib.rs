//! Northstar Web's server: a Cloudflare Worker in front of a D1 database.
//!
//! It signs people in with Discord, keeps the team list, and serves the team
//! library to the Northstar app running in their browsers. The app itself —
//! the wasm and its page — is served by Cloudflare straight from the static
//! assets; only `/api/*` and `/auth/*` reach this code.

pub mod logic;

#[cfg(target_arch = "wasm32")]
mod api;
#[cfg(target_arch = "wasm32")]
mod auth;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_arch = "wasm32")]
use worker::{event, Context, Env, Request, Response, Result};

#[cfg(target_arch = "wasm32")]
#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    match web::handle(req, env).await {
        Ok(r) => Ok(r),
        Err(e) => {
            // the details go to the Worker's log; the browser gets a plain answer
            worker::console_error!("northstar: {e}");
            web::fail(500, "Something went wrong on the server. Your work is kept in your browser; try again in a moment.")
        }
    }
}
