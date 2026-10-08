//! The team library, as JSON.
//!
//! Every handler here runs for someone signed in, on the team, allowed to do
//! what they asked (web.rs checked). Saves carry the version they were made
//! from and are refused if someone else saved since; edits need the script's
//! lease, so two people never type over each other.

use base64::Engine;
use serde::{Deserialize, Serialize};
use worker::{Request, Response, Result};

use crate::logic::{self, Holder, Role, Route, AUTO_SNAPSHOTS, LEASE_MS, MAX_BODY, MAX_SETTINGS, TRASH_MS};
use crate::web::{self, bump, n, opt, s, Cx, Me};

/// Answer a request.
pub async fn answer(route: Route, req: &mut Request, cx: &Cx, me: &Me) -> Result<Response> {
    match route {
        Route::Me => who_am_i(cx, me).await,
        Route::Stamp => stamp(cx).await,
        Route::ListScripts => list(cx, me).await,
        Route::CreateScript => create(req, cx, me).await,
        Route::Open(id) => open(&id, cx, me).await,
        Route::Save(id) => save(&id, req, cx, me).await,
        Route::Renew(id) => renew(&id, cx, me).await,
        Route::Release(id) => release(&id, cx, me).await,
        Route::Star(id) => star(&id, req, cx, me).await,
        Route::Duplicate(id) => duplicate(&id, cx, me).await,
        Route::Delete(id) => delete(&id, cx, me).await,
        Route::Restore(id) => restore(&id, cx).await,
        Route::Trash => trash(cx).await,
        Route::Snapshots(id) => snapshots(&id, cx).await,
        Route::TakeSnapshot(id) => take_snapshot(&id, req, cx, me).await,
        Route::Snapshot(id) => snapshot(&id, cx).await,
        Route::History(id) => history(&id, cx).await,
        Route::Members => members(cx).await,
        Route::AddMember => add_member(req, cx, me).await,
        Route::SetRole(id) => set_role(&id, req, cx).await,
        Route::RemoveMember(id) => remove_member(&id, cx, me).await,
        Route::Settings => settings(cx, me).await,
        Route::PutSettings => put_settings(req, cx, me).await,
        Route::Avatar(id) => avatar(&id, cx).await,
        _ => web::fail(404, "Not here."),
    }
}

/// Someone, as the app draws them.
#[derive(Serialize, Deserialize)]
struct Person {
    id: String,
    name: String,
    /// Which picture they have, for caching; `None` draws initials.
    avatar: Option<String>,
}

async fn people(cx: &Cx) -> Result<Vec<Person>> {
    cx.db
        .prepare("SELECT id, name, avatar_version AS avatar FROM users")
        .all()
        .await?
        .results()
}

async fn who_am_i(cx: &Cx, me: &Me) -> Result<Response> {
    let mut person = Person {
        id: me.id.clone(),
        name: me.name.clone(),
        avatar: None,
    };
    #[derive(Deserialize)]
    struct V {
        avatar_version: Option<String>,
    }
    if let Some(v) = cx
        .db
        .prepare("SELECT avatar_version FROM users WHERE id = ?1")
        .bind(&[s(&me.id)])?
        .first::<V>(None)
        .await?
    {
        person.avatar = v.avatar_version;
    }
    web::json(
        200,
        &serde_json::json!({ "me": person, "role": me.role.slug(), "team": cx.team_name() }),
    )
}

async fn stamp(cx: &Cx) -> Result<Response> {
    #[derive(Serialize, Deserialize)]
    struct Stamp {
        stamp: i64,
        latest: Option<i64>,
    }
    let v: Option<Stamp> = cx
        .db
        .prepare("SELECT (SELECT v FROM stamp WHERE id = 1) AS stamp, (SELECT MAX(updated_at) FROM scripts) AS latest")
        .first(None)
        .await?;
    web::json(200, &v)
}

// ---------- scripts ----------

#[derive(Serialize, Deserialize)]
struct Listed {
    id: String,
    title: String,
    author: String,
    draft: String,
    preview: String,
    pages: i64,
    scenes: i64,
    words: i64,
    version: i64,
    updated_at: i64,
    updated_by: String,
    starred: Option<i64>,
    editing: Option<String>,
}

async fn list(cx: &Cx, me: &Me) -> Result<Response> {
    let scripts: Vec<Listed> = cx
        .db
        .prepare(
            "SELECT s.id, s.title, s.author, s.draft, s.preview, s.pages, s.scenes, s.words, s.version, \
               s.updated_at, s.updated_by, \
               (SELECT 1 FROM stars st WHERE st.user_id = ?1 AND st.script_id = s.id) AS starred, \
               l.user_id AS editing \
             FROM scripts s LEFT JOIN leases l ON l.script_id = s.id AND l.expires_at > ?2 \
             WHERE s.deleted_at IS NULL ORDER BY s.updated_at DESC",
        )
        .bind(&[s(&me.id), n(cx.now)])?
        .all()
        .await?
        .results()?;
    web::json(200, &serde_json::json!({ "scripts": scripts, "people": people(cx).await? }))
}

/// What a save or a new script carries besides its text: the figures the
/// library shows without opening it, worked out by the app that wrote it.
#[derive(Deserialize)]
struct Card {
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    draft: String,
    #[serde(default)]
    preview: String,
    #[serde(default)]
    pages: i64,
    #[serde(default)]
    scenes: i64,
    #[serde(default)]
    words: i64,
}

impl Card {
    fn title(&self) -> String {
        let t = self.title.trim();
        if t.is_empty() {
            "Untitled Script".to_string()
        } else {
            t.chars().take(200).collect()
        }
    }
}

async fn create(req: &mut Request, cx: &Cx, me: &Me) -> Result<Response> {
    #[derive(Deserialize)]
    struct New {
        id: String,
        body: String,
        #[serde(flatten)]
        card: Card,
    }
    let new: New = match web::body(req, MAX_BODY + 4096).await {
        Ok(v) => v,
        Err(r) => return Ok(r),
    };
    if !logic::is_id(&new.id) {
        return web::fail(400, "That is not a script id.");
    }
    let c = &new.card;
    let done = cx
        .db
        .prepare(
            "INSERT INTO scripts (id, title, author, draft, preview, pages, scenes, words, body, version, \
               created_at, created_by, updated_at, updated_by) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?11, ?10, ?11) \
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(&[
            s(&new.id),
            s(&c.title()),
            s(&c.author),
            s(&c.draft),
            s(&c.preview),
            n(c.pages),
            n(c.scenes),
            n(c.words),
            s(&new.body),
            n(cx.now),
            s(&me.id),
        ])?
        .run()
        .await?;
    if changes(&done) == 0 {
        return web::fail(409, "A script with that id already exists.");
    }
    // whoever starts a script is the one editing it
    take_lease(&new.id, cx, me).await?;
    start_stretch(&new.id, c.words, cx, me).await?;
    bump(cx).await?;
    web::json(201, &serde_json::json!({ "id": new.id, "version": 1 }))
}

#[derive(Deserialize)]
struct Head {
    version: i64,
    words: i64,
    updated_by: String,
    updated_at: i64,
}

async fn head(id: &str, cx: &Cx) -> Result<Option<Head>> {
    cx.db
        .prepare("SELECT version, words, updated_by, updated_at FROM scripts WHERE id = ?1 AND deleted_at IS NULL")
        .bind(&[s(id)])?
        .first(None)
        .await
}

async fn open(id: &str, cx: &Cx, me: &Me) -> Result<Response> {
    #[derive(Deserialize)]
    struct Row {
        body: String,
        version: i64,
        updated_at: i64,
        updated_by: String,
    }
    let row: Option<Row> = cx
        .db
        .prepare("SELECT body, version, updated_at, updated_by FROM scripts WHERE id = ?1 AND deleted_at IS NULL")
        .bind(&[s(id)])?
        .first(None)
        .await?;
    let Some(row) = row else {
        return web::fail(404, "That script is not in the library any more.");
    };
    let lease = if me.role.can_write() {
        take_lease(id, cx, me).await?
    } else {
        lease_of(id, cx, me).await?
    };
    web::json(
        200,
        &serde_json::json!({
            "id": id,
            "body": row.body,
            "version": row.version,
            "updated_at": row.updated_at,
            "updated_by": row.updated_by,
            "lease": lease_json(&lease),
        }),
    )
}

async fn save(id: &str, req: &mut Request, cx: &Cx, me: &Me) -> Result<Response> {
    #[derive(Deserialize)]
    struct Save {
        body: String,
        base_version: i64,
        #[serde(flatten)]
        card: Card,
    }
    let save: Save = match web::body(req, MAX_BODY + 4096).await {
        Ok(v) => v,
        Err(r) => return Ok(r),
    };
    let Some(before) = head(id, cx).await? else {
        return web::fail(404, "That script is not in the library any more.");
    };

    // only the person holding the script writes to it
    match lease_of(id, cx, me).await? {
        Holder::Other(who) => {
            return web::json(
                423,
                &serde_json::json!({ "error": "Someone else is editing this script.", "holder": who }),
            )
        }
        // a lease that ran out (a sleeping laptop) is simply taken back,
        // unless the version check below says someone saved meanwhile
        Holder::Free | Holder::Me => {}
    }

    if before.version != save.base_version {
        return conflict(id, cx).await;
    }
    let c = &save.card;
    let done = cx
        .db
        .prepare(
            "UPDATE scripts SET body = ?1, title = ?2, author = ?3, draft = ?4, preview = ?5, pages = ?6, \
               scenes = ?7, words = ?8, version = version + 1, updated_at = ?9, updated_by = ?10 \
             WHERE id = ?11 AND version = ?12 AND deleted_at IS NULL",
        )
        .bind(&[
            s(&save.body),
            s(&c.title()),
            s(&c.author),
            s(&c.draft),
            s(&c.preview),
            n(c.pages),
            n(c.scenes),
            n(c.words),
            n(cx.now),
            s(&me.id),
            s(id),
            n(save.base_version),
        ])?
        .run()
        .await?;
    if changes(&done) == 0 {
        // someone got in between the check and the write
        return conflict(id, cx).await;
    }
    take_lease(id, cx, me).await?;
    record_stretch(id, c.words - before.words, cx, me).await?;
    // someone else's script becoming yours changes the library's shape
    if before.updated_by != me.id {
        bump(cx).await?;
    }
    let _ = before.updated_at;
    web::json(200, &serde_json::json!({ "version": save.base_version + 1 }))
}

/// 409: here is the script as it is now, and who saved it.
async fn conflict(id: &str, cx: &Cx) -> Result<Response> {
    #[derive(Deserialize)]
    struct Now {
        body: String,
        version: i64,
        updated_by: String,
    }
    let now: Option<Now> = cx
        .db
        .prepare("SELECT body, version, updated_by FROM scripts WHERE id = ?1")
        .bind(&[s(id)])?
        .first(None)
        .await?;
    match now {
        Some(now) => web::json(
            409,
            &serde_json::json!({
                "error": "Someone saved this script after you opened it.",
                "body": now.body,
                "version": now.version,
                "updated_by": now.updated_by,
            }),
        ),
        None => web::fail(404, "That script is not in the library any more."),
    }
}

// ---------- leases ----------

async fn lease_of(id: &str, cx: &Cx, me: &Me) -> Result<Holder> {
    #[derive(Deserialize)]
    struct L {
        user_id: String,
        expires_at: i64,
    }
    let l: Option<L> = cx
        .db
        .prepare("SELECT user_id, expires_at FROM leases WHERE script_id = ?1")
        .bind(&[s(id)])?
        .first(None)
        .await?;
    Ok(logic::holder(l.as_ref().map(|l| (l.user_id.as_str(), l.expires_at)), &me.id, cx.now))
}

/// Take (or keep) a script's lease, unless someone else holds it.
async fn take_lease(id: &str, cx: &Cx, me: &Me) -> Result<Holder> {
    let before = lease_of(id, cx, me).await?;
    if let Holder::Other(_) = before {
        return Ok(before);
    }
    // the condition makes this safe against a race: a lease held by someone
    // else, still running, is never overwritten
    cx.db
        .prepare(
            "INSERT INTO leases (script_id, user_id, expires_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT (script_id) DO UPDATE SET user_id = excluded.user_id, expires_at = excluded.expires_at \
             WHERE leases.user_id = excluded.user_id OR leases.expires_at <= ?4",
        )
        .bind(&[s(id), s(&me.id), n(cx.now + LEASE_MS), n(cx.now)])?
        .run()
        .await?;
    let after = lease_of(id, cx, me).await?;
    if before == Holder::Free && after == Holder::Me {
        bump(cx).await?;
    }
    Ok(after)
}

fn lease_json(h: &Holder) -> serde_json::Value {
    match h {
        Holder::Me => serde_json::json!({ "mine": true, "holder": null }),
        Holder::Other(who) => serde_json::json!({ "mine": false, "holder": who }),
        Holder::Free => serde_json::json!({ "mine": false, "holder": null }),
    }
}

async fn renew(id: &str, cx: &Cx, me: &Me) -> Result<Response> {
    if head(id, cx).await?.is_none() {
        return web::fail(404, "That script is not in the library any more.");
    }
    let h = if me.role.can_write() {
        take_lease(id, cx, me).await?
    } else {
        lease_of(id, cx, me).await?
    };
    web::json(200, &lease_json(&h))
}

async fn release(id: &str, cx: &Cx, me: &Me) -> Result<Response> {
    let gone = cx
        .db
        .prepare("DELETE FROM leases WHERE script_id = ?1 AND user_id = ?2")
        .bind(&[s(id), s(&me.id)])?
        .run()
        .await?;
    if changes(&gone) > 0 {
        bump(cx).await?;
    }
    web::done()
}

// ---------- the rest of a script's life ----------

async fn star(id: &str, req: &mut Request, cx: &Cx, me: &Me) -> Result<Response> {
    #[derive(Deserialize)]
    struct On {
        on: bool,
    }
    let on: On = match web::body(req, 256).await {
        Ok(v) => v,
        Err(r) => return Ok(r),
    };
    let q = if on.on {
        "INSERT OR IGNORE INTO stars (user_id, script_id) VALUES (?1, ?2)"
    } else {
        "DELETE FROM stars WHERE user_id = ?1 AND script_id = ?2"
    };
    cx.db.prepare(q).bind(&[s(&me.id), s(id)])?.run().await?;
    web::done()
}

async fn duplicate(id: &str, cx: &Cx, me: &Me) -> Result<Response> {
    #[derive(Deserialize)]
    struct Row {
        title: String,
        author: String,
        draft: String,
        preview: String,
        pages: i64,
        scenes: i64,
        words: i64,
        body: String,
    }
    let row: Option<Row> = cx
        .db
        .prepare(
            "SELECT title, author, draft, preview, pages, scenes, words, body FROM scripts \
             WHERE id = ?1 AND deleted_at IS NULL",
        )
        .bind(&[s(id)])?
        .first(None)
        .await?;
    let Some(row) = row else {
        return web::fail(404, "That script is not in the library any more.");
    };
    let new_id = web::random_hex(12);
    let title = format!("{} (copy)", row.title);
    cx.db
        .prepare(
            "INSERT INTO scripts (id, title, author, draft, preview, pages, scenes, words, body, version, \
               created_at, created_by, updated_at, updated_by) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?11, ?10, ?11)",
        )
        .bind(&[
            s(&new_id),
            s(&title),
            s(&row.author),
            s(&row.draft),
            s(&row.preview),
            n(row.pages),
            n(row.scenes),
            n(row.words),
            s(&logic::retitle(&row.body, &title)),
            n(cx.now),
            s(&me.id),
        ])?
        .run()
        .await?;
    bump(cx).await?;
    web::json(201, &serde_json::json!({ "id": new_id, "title": title }))
}

async fn delete(id: &str, cx: &Cx, me: &Me) -> Result<Response> {
    if let Holder::Other(who) = lease_of(id, cx, me).await? {
        return web::json(
            423,
            &serde_json::json!({ "error": "Someone is editing this script: it can be deleted once they finish.", "holder": who }),
        );
    }
    cx.db
        .prepare("UPDATE scripts SET deleted_at = ?1, deleted_by = ?2 WHERE id = ?3 AND deleted_at IS NULL")
        .bind(&[n(cx.now), s(&me.id), s(id)])?
        .run()
        .await?;
    cx.db.prepare("DELETE FROM leases WHERE script_id = ?1").bind(&[s(id)])?.run().await?;
    bump(cx).await?;
    web::done()
}

async fn restore(id: &str, cx: &Cx) -> Result<Response> {
    cx.db
        .prepare("UPDATE scripts SET deleted_at = NULL, deleted_by = NULL WHERE id = ?1")
        .bind(&[s(id)])?
        .run()
        .await?;
    bump(cx).await?;
    web::done()
}

async fn trash(cx: &Cx) -> Result<Response> {
    // what has waited 30 days goes for good, with everything that hung off it
    let cutoff = cx.now - TRASH_MS;
    let old = "SELECT id FROM scripts WHERE deleted_at IS NOT NULL AND deleted_at < ?1";
    for q in [
        format!("DELETE FROM snapshots WHERE script_id IN ({old})"),
        format!("DELETE FROM edits WHERE script_id IN ({old})"),
        format!("DELETE FROM stars WHERE script_id IN ({old})"),
        "DELETE FROM scripts WHERE deleted_at IS NOT NULL AND deleted_at < ?1".to_string(),
    ] {
        cx.db.prepare(q).bind(&[n(cutoff)])?.run().await?;
    }
    #[derive(Serialize, Deserialize)]
    struct Gone {
        id: String,
        title: String,
        deleted_at: i64,
        deleted_by: Option<String>,
    }
    let gone: Vec<Gone> = cx
        .db
        .prepare("SELECT id, title, deleted_at, deleted_by FROM scripts WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC")
        .all()
        .await?
        .results()?;
    web::json(200, &serde_json::json!({ "scripts": gone, "people": people(cx).await? }))
}

// ---------- snapshots and history ----------

async fn snapshots(id: &str, cx: &Cx) -> Result<Response> {
    #[derive(Serialize, Deserialize)]
    struct Snap {
        id: String,
        taken_at: i64,
        taken_by: String,
        label: Option<String>,
        kind: String,
        pages: i64,
    }
    let list: Vec<Snap> = cx
        .db
        .prepare(
            "SELECT id, taken_at, taken_by, label, kind, pages FROM snapshots WHERE script_id = ?1 \
             ORDER BY taken_at DESC LIMIT 200",
        )
        .bind(&[s(id)])?
        .all()
        .await?
        .results()?;
    web::json(200, &serde_json::json!({ "snapshots": list }))
}

async fn take_snapshot(id: &str, req: &mut Request, cx: &Cx, me: &Me) -> Result<Response> {
    #[derive(Deserialize)]
    struct Take {
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        pages: i64,
        body: String,
    }
    let take: Take = match web::body(req, MAX_BODY + 4096).await {
        Ok(v) => v,
        Err(r) => return Ok(r),
    };
    if head(id, cx).await?.is_none() {
        return web::fail(404, "That script is not in the library any more.");
    }
    let kind = if take.kind.as_deref() == Some("auto") { "auto" } else { "manual" };
    let label = take.label.map(|l| l.chars().take(120).collect::<String>());
    let snap_id = web::random_hex(12);
    cx.db
        .prepare(
            "INSERT INTO snapshots (id, script_id, taken_at, taken_by, label, kind, pages, body) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&[
            s(&snap_id),
            s(id),
            n(cx.now),
            s(&me.id),
            opt(label.as_deref()),
            s(kind),
            n(take.pages),
            s(&take.body),
        ])?
        .run()
        .await?;
    if kind == "auto" {
        cx.db
            .prepare(
                "DELETE FROM snapshots WHERE script_id = ?1 AND kind = 'auto' AND id NOT IN \
                 (SELECT id FROM snapshots WHERE script_id = ?1 AND kind = 'auto' ORDER BY taken_at DESC LIMIT ?2)",
            )
            .bind(&[s(id), n(AUTO_SNAPSHOTS as i64)])?
            .run()
            .await?;
    }
    web::json(201, &serde_json::json!({ "id": snap_id }))
}

async fn snapshot(id: &str, cx: &Cx) -> Result<Response> {
    #[derive(Serialize, Deserialize)]
    struct Snap {
        id: String,
        script_id: String,
        body: String,
    }
    let snap: Option<Snap> = cx
        .db
        .prepare("SELECT id, script_id, body FROM snapshots WHERE id = ?1")
        .bind(&[s(id)])?
        .first(None)
        .await?;
    match snap {
        Some(snap) => web::json(200, &snap),
        None => web::fail(404, "That snapshot is not there any more."),
    }
}

async fn history(id: &str, cx: &Cx) -> Result<Response> {
    #[derive(Serialize, Deserialize)]
    struct Stretch {
        user_id: String,
        started_at: i64,
        ended_at: i64,
        words: i64,
        saves: i64,
    }
    let list: Vec<Stretch> = cx
        .db
        .prepare(
            "SELECT user_id, started_at, ended_at, words_delta AS words, saves FROM edits WHERE script_id = ?1 \
             ORDER BY ended_at DESC LIMIT 200",
        )
        .bind(&[s(id)])?
        .all()
        .await?
        .results()?;
    web::json(200, &serde_json::json!({ "sessions": list, "people": people(cx).await? }))
}

/// Extend the last stretch of work, or start a new one.
async fn record_stretch(id: &str, words_delta: i64, cx: &Cx, me: &Me) -> Result<()> {
    #[derive(Deserialize)]
    struct Last {
        id: i64,
        user_id: String,
        ended_at: i64,
    }
    let last: Option<Last> = cx
        .db
        .prepare("SELECT id, user_id, ended_at FROM edits WHERE script_id = ?1 ORDER BY ended_at DESC LIMIT 1")
        .bind(&[s(id)])?
        .first(None)
        .await?;
    match last {
        Some(l) if logic::extends_stretch(Some((&l.user_id, l.ended_at)), &me.id, cx.now) => {
            cx.db
                .prepare("UPDATE edits SET ended_at = ?1, words_delta = words_delta + ?2, saves = saves + 1 WHERE id = ?3")
                .bind(&[n(cx.now), n(words_delta), n(l.id)])?
                .run()
                .await?;
            Ok(())
        }
        _ => start_stretch(id, words_delta, cx, me).await,
    }
}

async fn start_stretch(id: &str, words_delta: i64, cx: &Cx, me: &Me) -> Result<()> {
    cx.db
        .prepare(
            "INSERT INTO edits (script_id, user_id, started_at, ended_at, words_delta, saves) VALUES (?1, ?2, ?3, ?3, ?4, 1)",
        )
        .bind(&[s(id), s(&me.id), n(cx.now), n(words_delta)])?
        .run()
        .await?;
    Ok(())
}

// ---------- the team ----------

async fn members(cx: &Cx) -> Result<Response> {
    #[derive(Deserialize)]
    struct Row {
        discord_id: String,
        role: String,
        name: Option<String>,
        avatar: Option<String>,
    }
    let rows: Vec<Row> = cx
        .db
        .prepare(
            "SELECT m.discord_id, m.role, u.name, u.avatar_version AS avatar FROM members m \
             LEFT JOIN users u ON u.id = m.discord_id ORDER BY m.added_at",
        )
        .all()
        .await?
        .results()?;
    #[derive(Deserialize)]
    struct U {
        name: String,
        avatar: Option<String>,
    }
    let mut out = Vec::new();
    // owners from configuration first: they are always on the team
    for o in &cx.owners {
        let u: Option<U> = cx
            .db
            .prepare("SELECT name, avatar_version AS avatar FROM users WHERE id = ?1")
            .bind(&[s(o)])?
            .first(None)
            .await?;
        out.push(serde_json::json!({
            "id": o,
            "name": u.as_ref().map(|u| u.name.clone()),
            "avatar": u.as_ref().and_then(|u| u.avatar.clone()),
            "role": "owner",
            "pending": u.is_none(),
            "fixed": true,
        }));
    }
    for r in rows.into_iter().filter(|r| !cx.owners.contains(&r.discord_id)) {
        out.push(serde_json::json!({
            "id": r.discord_id,
            "pending": r.name.is_none(),
            "name": r.name,
            "avatar": r.avatar,
            "role": r.role,
            "fixed": false,
        }));
    }
    web::json(200, &serde_json::json!({ "members": out }))
}

async fn add_member(req: &mut Request, cx: &Cx, me: &Me) -> Result<Response> {
    #[derive(Deserialize)]
    struct Add {
        id: String,
        role: String,
    }
    let add: Add = match web::body(req, 512).await {
        Ok(v) => v,
        Err(r) => return Ok(r),
    };
    let Some(role) = Role::parse(&add.role) else {
        return web::fail(400, "The role must be owner, editor or viewer.");
    };
    if !logic::is_discord_id(&add.id) {
        return web::fail(400, "That is not a Discord user id.");
    }
    cx.db
        .prepare(
            "INSERT INTO members (discord_id, role, added_by, added_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (discord_id) DO UPDATE SET role = excluded.role",
        )
        .bind(&[s(&add.id), s(role.slug()), s(&me.id), n(cx.now)])?
        .run()
        .await?;
    web::done()
}

async fn set_role(id: &str, req: &mut Request, cx: &Cx) -> Result<Response> {
    #[derive(Deserialize)]
    struct Set {
        role: String,
    }
    let set: Set = match web::body(req, 256).await {
        Ok(v) => v,
        Err(r) => return Ok(r),
    };
    let Some(role) = Role::parse(&set.role) else {
        return web::fail(400, "The role must be owner, editor or viewer.");
    };
    if cx.owners.iter().any(|o| o == id) {
        return web::fail(400, "That owner is set in the server's configuration.");
    }
    if role != Role::Owner && !owner_remains_without(id, cx).await? {
        return web::fail(400, "The team needs an owner: make someone else one first.");
    }
    cx.db
        .prepare("UPDATE members SET role = ?1 WHERE discord_id = ?2")
        .bind(&[s(role.slug()), s(id)])?
        .run()
        .await?;
    web::done()
}

async fn remove_member(id: &str, cx: &Cx, me: &Me) -> Result<Response> {
    if cx.owners.iter().any(|o| o == id) {
        return web::fail(400, "That owner is set in the server's configuration.");
    }
    if id == me.id {
        return web::fail(400, "Ask another owner to take you off the team.");
    }
    if !owner_remains_without(id, cx).await? {
        return web::fail(400, "The team needs an owner: make someone else one first.");
    }
    for q in [
        "DELETE FROM members WHERE discord_id = ?1",
        // signed out everywhere, at once
        "DELETE FROM sessions WHERE user_id = ?1",
        "DELETE FROM leases WHERE user_id = ?1",
    ] {
        cx.db.prepare(q).bind(&[s(id)])?.run().await?;
    }
    bump(cx).await?;
    web::done()
}

/// Would the team still have an owner if `id` were not one?
async fn owner_remains_without(id: &str, cx: &Cx) -> Result<bool> {
    if !cx.owners.is_empty() {
        return Ok(true);
    }
    #[derive(Deserialize)]
    struct C {
        c: i64,
    }
    let c: Option<C> = cx
        .db
        .prepare("SELECT COUNT(*) AS c FROM members WHERE role = 'owner' AND discord_id != ?1")
        .bind(&[s(id)])?
        .first(None)
        .await?;
    Ok(c.map(|c| c.c > 0).unwrap_or(false))
}

// ---------- you ----------

async fn settings(cx: &Cx, me: &Me) -> Result<Response> {
    #[derive(Deserialize)]
    struct B {
        body: String,
    }
    let b: Option<B> = cx
        .db
        .prepare("SELECT body FROM settings WHERE user_id = ?1")
        .bind(&[s(&me.id)])?
        .first(None)
        .await?;
    web::json(200, &serde_json::json!({ "body": b.map(|b| b.body).unwrap_or_default() }))
}

async fn put_settings(req: &mut Request, cx: &Cx, me: &Me) -> Result<Response> {
    #[derive(Deserialize)]
    struct B {
        body: String,
    }
    let b: B = match web::body(req, MAX_SETTINGS).await {
        Ok(v) => v,
        Err(r) => return Ok(r),
    };
    cx.db
        .prepare(
            "INSERT INTO settings (user_id, body) VALUES (?1, ?2) \
             ON CONFLICT (user_id) DO UPDATE SET body = excluded.body",
        )
        .bind(&[s(&me.id), s(&b.body)])?
        .run()
        .await?;
    web::done()
}

async fn avatar(id: &str, cx: &Cx) -> Result<Response> {
    #[derive(Deserialize)]
    struct A {
        avatar: Option<String>,
    }
    let a: Option<A> = cx
        .db
        .prepare("SELECT avatar FROM users WHERE id = ?1")
        .bind(&[s(id)])?
        .first(None)
        .await?;
    let Some(b64) = a.and_then(|a| a.avatar) else {
        return web::fail(404, "No picture.");
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
        return web::fail(404, "No picture.");
    };
    let mut r = Response::from_bytes(bytes)?;
    let h = r.headers_mut();
    h.set("Content-Type", "image/png")?;
    // the app asks with ?v=<version>, so a new picture is a new address
    h.set("Cache-Control", "private, max-age=604800, immutable")?;
    h.set("X-Content-Type-Options", "nosniff")?;
    Ok(r)
}

fn changes(r: &worker::D1Result) -> usize {
    r.meta().ok().flatten().and_then(|m| m.changes).unwrap_or(0)
}
