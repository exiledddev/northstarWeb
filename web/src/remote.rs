//! The team library, behind the same `Store` the desktop's folder sits
//! behind. The app asks; this answers from what it already knows, and goes to
//! the server for the rest, saying so later as events.
//!
//! Three promises it keeps:
//! - Nothing typed is lost. Every change is kept in this browser until the
//!   server has it; a refused save is kept as a snapshot before anything else
//!   happens; a tab reopened after a crash offers back what had not arrived.
//! - Nobody overwrites anybody. Saves go with the version they came from and
//!   the server refuses them if someone saved since.
//! - The free plan lasts. Saves go at most every few seconds, and the
//!   library is checked by reading one small stamp twice a minute.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine;
use northstar::alerts::Tone;
use northstar::backend::{Access, EditSession, Event, Load, Member, Person, Role, Store, Team};
use northstar::eframe::egui;
use northstar::export;
use northstar::model::Document;
use northstar::settings::{AfterExport, Settings};
use northstar::storage::{self, Entry, Snapshot};
use serde::Deserialize;
use serde_json::json;
use web_time::{Duration, SystemTime, UNIX_EPOCH};

use crate::browser::{self, now_ms, Answer, Picker};

/// At most one save every this many milliseconds while someone types.
const SAVE_GAP_MS: f64 = 4000.0;
/// Settings go a second after the last change.
const SETTINGS_GAP_MS: f64 = 1000.0;
/// History and Recently deleted are fetched again after this long.
const STALE_MS: f64 = 30_000.0;

// ---------- what the server sends ----------

#[derive(Deserialize, Clone, Debug)]
pub struct PersonRow {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ScriptRow {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub pages: i64,
    #[serde(default)]
    pub scenes: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub updated_by: String,
    #[serde(default)]
    pub starred: Option<i64>,
    #[serde(default)]
    pub editing: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct MemberRow {
    pub id: String,
    pub name: Option<String>,
    pub avatar: Option<String>,
    pub role: String,
    #[serde(default)]
    pub pending: bool,
    #[serde(default)]
    pub fixed: bool,
}

#[derive(Deserialize, Clone, Debug)]
struct SnapRow {
    id: String,
    taken_at: i64,
    taken_by: String,
    label: Option<String>,
    pages: i64,
}

#[derive(Deserialize, Clone, Debug)]
struct StretchRow {
    user_id: String,
    started_at: i64,
    ended_at: i64,
    words: i64,
    saves: i64,
}

#[derive(Deserialize, Clone, Debug)]
struct GoneRow {
    id: String,
    title: String,
    deleted_at: i64,
    deleted_by: Option<String>,
}

/// What the app was started with: who you are and the library as it stood.
pub struct Boot {
    pub me: PersonRow,
    pub role: Role,
    pub team: String,
    pub settings: String,
    pub scripts: Vec<ScriptRow>,
    pub people: Vec<PersonRow>,
    pub members: Vec<MemberRow>,
}

// ---------- the open script ----------

struct Open {
    id: String,
    /// The version the next save is made from.
    version: i64,
    editable: bool,
    viewer: bool,
    /// Someone else is editing it; they have been seen to let go.
    free_said: bool,
    /// The newest change the server does not have yet.
    pending: Option<Document>,
    saving: bool,
    last_put: f64,
}

#[derive(Default)]
struct Inner {
    ctx: Option<egui::Context>,
    me: Option<PersonRow>,
    role: Option<Role>,
    team_name: String,
    members: Vec<MemberRow>,
    people: HashMap<String, PersonRow>,
    avatars: HashMap<String, Arc<egui::ColorImage>>,
    avatar_asked: HashSet<String>,
    team_dirty: bool,

    scripts: Vec<ScriptRow>,
    list_changed: bool,
    stamp: Option<serde_json::Value>,
    open: Option<Open>,
    /// Scripts made here that the server has not confirmed yet, so opening
    /// one never races its own creation.
    creating: HashMap<String, Document>,
    events: Vec<Event>,
    in_flight: usize,
    offline_said: bool,

    snapshots: HashMap<String, Vec<SnapRow>>,
    snapshots_asked: HashSet<String>,
    history: HashMap<String, (f64, Vec<StretchRow>)>,
    history_asked: HashSet<String>,
    trash: Option<(f64, Vec<GoneRow>)>,
    trash_asked: bool,

    settings: Settings,
    settings_due: Option<f64>,
}

type In = Rc<RefCell<Inner>>;

pub struct RemoteStore {
    inner: In,
    team: Team,
    picker: Option<Picker>,
}

impl RemoteStore {
    pub fn new(boot: Boot) -> RemoteStore {
        let mut settings = Settings::parse(&boot.settings);
        // the page's own loading card is the splash in a browser
        settings.splash = false;
        let inner = Rc::new(RefCell::new(Inner {
            me: Some(boot.me.clone()),
            role: Some(boot.role),
            team_name: boot.team,
            members: boot.members,
            people: boot.people.into_iter().map(|p| (p.id.clone(), p)).collect(),
            scripts: boot.scripts,
            settings,
            team_dirty: true,
            ..Default::default()
        }));
        inner.borrow_mut().people.insert(boot.me.id.clone(), boot.me);

        let for_files = inner.clone();
        let picker = Picker::new(Rc::new(move |name, bytes| {
            let mut i = for_files.borrow_mut();
            i.events.push(Event::Imported { name, bytes });
            i.repaint();
        }));

        // twice a minute: keep the script you are in, and look for news
        let tick = inner.clone();
        browser::every(30_000, move || heartbeat(&tick));
        let back = inner.clone();
        browser::on("visibilitychange", true, move || {
            if browser::visible() {
                heartbeat(&back);
            }
        });
        // a closing tab lets go of its script, and leaves anything unsaved
        // in this browser for next time
        let closing = inner.clone();
        browser::on("pagehide", false, move || {
            if let Ok(i) = closing.try_borrow() {
                if let Some(o) = &i.open {
                    if o.editable {
                        browser::beacon(&format!("/api/scripts/{}/release", o.id));
                    }
                }
            }
        });

        let mut store = RemoteStore {
            inner,
            team: Team {
                name: String::new(),
                me: Person::new("", ""),
                role: Role::Viewer,
                members: Vec::new(),
            },
            picker,
        };
        store.rebuild_team();
        store
    }

    /// The app's context, so answers arriving later can ask for a frame.
    pub fn set_context(&self, ctx: egui::Context) {
        self.inner.borrow_mut().ctx = Some(ctx);
    }

    fn rebuild_team(&mut self) {
        let mut i = self.inner.borrow_mut();
        if !i.team_dirty {
            return;
        }
        i.team_dirty = false;
        let me_id = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        let me = i.person(&me_id);
        let members = i
            .members
            .clone()
            .into_iter()
            .map(|m| {
                let mut person = i.person(&m.id);
                if let Some(name) = &m.name {
                    person.name = name.clone();
                } else if m.pending {
                    person.name = m.id.clone();
                }
                Member {
                    person,
                    role: Role::from_slug(&m.role).unwrap_or(Role::Viewer),
                    pending: m.pending,
                    fixed: m.fixed,
                }
            })
            .collect();
        self.team = Team {
            name: i.team_name.clone(),
            me,
            role: i.role.unwrap_or(Role::Viewer),
            members,
        };
    }
}

impl Inner {
    fn repaint(&self) {
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    fn me_id(&self) -> String {
        self.me.as_ref().map(|m| m.id.clone()).unwrap_or_default()
    }

    fn writer(&self) -> bool {
        self.role.map(|r| r.can_write()).unwrap_or(false)
    }

    /// Someone, with their picture once it has arrived. A picture not yet
    /// asked for is asked for now.
    fn person(&mut self, id: &str) -> Person {
        let row = self.people.get(id).cloned();
        let name = row.as_ref().map(|r| r.name.clone()).unwrap_or_else(|| "Someone".to_string());
        let mut p = Person::new(id, name);
        p.avatar = self.avatars.get(id).cloned();
        if p.avatar.is_none() {
            if let Some(version) = row.and_then(|r| r.avatar) {
                if self.avatar_asked.insert(id.to_string()) {
                    fetch_avatar(id.to_string(), version);
                }
            }
        }
        p
    }

    fn say(&mut self, title: &str, detail: &str, tone: Tone) {
        self.events.push(Event::notice(title, detail, tone));
    }

    fn offline(&mut self) {
        if !self.offline_said {
            self.offline_said = true;
            self.say(
                "Can't reach the team library",
                "Your work is kept in this browser and goes as soon as the connection is back.",
                Tone::Warn,
            );
        }
    }
}

// ---------- the store ----------

impl Store for RemoteStore {
    fn team(&self) -> Option<&Team> {
        Some(&self.team)
    }

    fn location(&self) -> String {
        format!("“{}”", self.team.name)
    }

    fn list(&mut self) -> Vec<Entry> {
        let mut i = self.inner.borrow_mut();
        let me = i.me_id();
        let rows = i.scripts.clone();
        rows.iter()
            .map(|r| {
                let edited_by = (!r.updated_by.is_empty()).then(|| i.person(&r.updated_by));
                let editing = r.editing.as_deref().filter(|who| *who != me).map(|who| i.person(who));
                Entry {
                    path: PathBuf::from(&r.id),
                    title: r.title.clone(),
                    modified: at(r.updated_at),
                    preview: r.preview.clone(),
                    starred: r.starred.unwrap_or(0) != 0,
                    pages: r.pages.max(0) as usize,
                    scenes: r.scenes.max(0) as usize,
                    edited_by,
                    editing,
                    author: r.author.clone(),
                }
            })
            .collect()
    }

    fn changed(&mut self) -> bool {
        std::mem::take(&mut self.inner.borrow_mut().list_changed)
    }

    fn load(&mut self, key: &Path) -> Load {
        let id = key_id(key);
        let mut i = self.inner.borrow_mut();
        // one just made here: it is already ours, and the server may not have
        // it yet
        if let Some(doc) = i.creating.get(&id).cloned() {
            i.open = Some(Open::fresh(&id, 1, true, false));
            return Load::Ready(doc, Access::Edit);
        }
        drop(i);
        open_script(self.inner.clone(), id);
        Load::Pending
    }

    fn close(&mut self, key: &Path) {
        let id = key_id(key);
        let mut i = self.inner.borrow_mut();
        let Some(o) = i.open.take() else { return };
        if o.id != id {
            i.open = Some(o);
            return;
        }
        if !o.editable {
            return;
        }
        // what has not gone yet goes now, then the script is let go
        let last = o.pending.map(|doc| (doc, o.version));
        drop(i);
        let inner = self.inner.clone();
        spawn(&self.inner, async move {
            if let Some((doc, version)) = last {
                let body = save_body(&doc, version);
                match browser::call("PUT", &format!("/api/scripts/{id}"), Some(&body)).await {
                    Ok(a) if a.ok() => browser::forget(&draft_key(&id)),
                    // refused or unreachable: the draft stays, and the next
                    // opening of this script offers it back
                    _ => {}
                }
            }
            let _ = browser::call("POST", &format!("/api/scripts/{id}/release"), Some(&json!({}))).await;
            refresh_list(&inner).await;
        });
    }

    fn create(&mut self, doc: &Document) -> Result<PathBuf, String> {
        let id = browser::random_hex(16);
        let mut i = self.inner.borrow_mut();
        if !i.writer() {
            return Err("Viewers can read and export, but not add scripts.".into());
        }
        let me = i.me_id();
        let card = card(doc);
        i.scripts.insert(
            0,
            ScriptRow {
                id: id.clone(),
                title: card["title"].as_str().unwrap_or_default().to_string(),
                author: doc.meta.author.clone(),
                preview: card["preview"].as_str().unwrap_or_default().to_string(),
                pages: card["pages"].as_i64().unwrap_or(0),
                scenes: card["scenes"].as_i64().unwrap_or(0),
                updated_at: now_ms() as i64,
                updated_by: me,
                starred: None,
                editing: None,
            },
        );
        i.list_changed = true;
        i.creating.insert(id.clone(), doc.clone());
        i.open = Some(Open::fresh(&id, 1, true, false));
        browser::store(&draft_key(&id), &draft(1, doc));
        drop(i);
        send_create(self.inner.clone(), id.clone(), doc.clone());
        Ok(PathBuf::from(id))
    }

    fn save(&mut self, key: &Path, doc: &Document) -> Result<(), String> {
        let id = key_id(key);
        let mut i = self.inner.borrow_mut();
        let Some(o) = i.open.as_mut().filter(|o| o.id == id) else {
            return Ok(());
        };
        if !o.editable {
            return Ok(());
        }
        // kept here at once; sent when the gap allows (see pump)
        browser::store(&draft_key(&id), &draft(o.version, doc));
        o.pending = Some(doc.clone());
        Ok(())
    }

    fn duplicate(&mut self, key: &Path) -> Result<Option<String>, String> {
        let id = key_id(key);
        let inner = self.inner.clone();
        spawn(&self.inner, async move {
            match browser::call("POST", &format!("/api/scripts/{id}/duplicate"), Some(&json!({}))).await {
                Ok(a) if a.ok() => {
                    let title = a.json["title"].as_str().unwrap_or("A copy").to_string();
                    refresh_list(&inner).await;
                    inner.borrow_mut().say("Duplicated", &title, Tone::Ok);
                }
                Ok(a) => inner.borrow_mut().say("Could not duplicate", &a.error(), Tone::Danger),
                Err(_) => inner.borrow_mut().offline(),
            }
        });
        Ok(None)
    }

    fn delete(&mut self, key: &Path) -> Result<(), String> {
        let id = key_id(key);
        {
            let mut i = self.inner.borrow_mut();
            i.scripts.retain(|s| s.id != id);
            i.list_changed = true;
            i.trash = None;
            if i.open.as_ref().map(|o| o.id == id).unwrap_or(false) {
                i.open = None;
            }
        }
        browser::forget(&draft_key(&id));
        let inner = self.inner.clone();
        spawn(&self.inner, async move {
            match browser::call("DELETE", &format!("/api/scripts/{id}"), None).await {
                Ok(a) if a.ok() => {}
                Ok(a) => inner.borrow_mut().say("Not deleted", &a.error(), Tone::Warn),
                Err(_) => inner.borrow_mut().offline(),
            }
            refresh_list(&inner).await;
        });
        Ok(())
    }

    fn set_starred(&mut self, key: &Path, on: bool) -> Result<(), String> {
        let id = key_id(key);
        {
            let mut i = self.inner.borrow_mut();
            if let Some(s) = i.scripts.iter_mut().find(|s| s.id == id) {
                s.starred = on.then_some(1);
            }
            i.list_changed = true;
        }
        let inner = self.inner.clone();
        spawn(&self.inner, async move {
            if browser::call("POST", &format!("/api/scripts/{id}/star"), Some(&json!({ "on": on }))).await.is_err() {
                inner.borrow_mut().offline();
            }
        });
        Ok(())
    }

    fn request_edit(&mut self, key: &Path) {
        open_script(self.inner.clone(), key_id(key));
    }

    fn take_snapshot(&mut self, key: &Path, doc: &Document, label: Option<&str>) -> Result<(), String> {
        let id = key_id(key);
        self.inner.borrow_mut().snapshots_asked.remove(&id);
        send_snapshot(&self.inner, id, doc, label.map(str::to_string));
        Ok(())
    }

    fn snapshots(&mut self, key: &Path) -> Option<Vec<Snapshot>> {
        let id = key_id(key);
        let mut i = self.inner.borrow_mut();
        if i.snapshots_asked.insert(id.clone()) {
            drop(i);
            let inner = self.inner.clone();
            spawn(&self.inner, async move {
                if let Ok(a) = browser::call("GET", &format!("/api/scripts/{id}/snapshots"), None).await {
                    if let Ok(rows) = serde_json::from_value::<Vec<SnapRow>>(a.json["snapshots"].clone()) {
                        inner.borrow_mut().snapshots.insert(id, rows);
                    }
                }
            });
            i = self.inner.borrow_mut();
        }
        let rows = i.snapshots.get(&key_id(key)).cloned()?;
        Some(
            rows.iter()
                .map(|r| Snapshot {
                    path: PathBuf::from(&r.id),
                    when: when(r.taken_at),
                    pages: r.pages.max(0) as usize,
                    by: Some(i.person(&r.taken_by)),
                    label: r.label.clone(),
                })
                .collect(),
        )
    }

    fn load_snapshot(&mut self, snap: &Path) -> Load {
        let sid = key_id(snap);
        let inner = self.inner.clone();
        spawn(&self.inner, async move {
            match browser::call("GET", &format!("/api/snapshots/{sid}"), None).await {
                Ok(a) if a.ok() => match inflate(a.json["body"].as_str().unwrap_or_default()) {
                    Some(text) => {
                        let doc = storage::from_markdown(&text);
                        inner.borrow_mut().events.push(Event::SnapshotDoc { snap: PathBuf::from(sid), doc });
                    }
                    None => inner.borrow_mut().say("Could not restore", "That snapshot could not be read.", Tone::Danger),
                },
                Ok(a) => inner.borrow_mut().say("Could not restore", &a.error(), Tone::Danger),
                Err(_) => inner.borrow_mut().offline(),
            }
        });
        Load::Pending
    }

    fn history(&mut self, key: &Path) -> Option<Vec<EditSession>> {
        let id = key_id(key);
        let mut i = self.inner.borrow_mut();
        let stale = i.history.get(&id).map(|(t, _)| now_ms() - t > STALE_MS).unwrap_or(true);
        if stale && i.history_asked.insert(id.clone()) {
            drop(i);
            let inner = self.inner.clone();
            let id2 = id.clone();
            spawn(&self.inner, async move {
                if let Ok(a) = browser::call("GET", &format!("/api/scripts/{id2}/history"), None).await {
                    let mut i = inner.borrow_mut();
                    learn_people(&mut i, &a.json);
                    if let Ok(rows) = serde_json::from_value::<Vec<StretchRow>>(a.json["sessions"].clone()) {
                        i.history.insert(id2.clone(), (now_ms(), rows));
                    }
                    i.history_asked.remove(&id2);
                }
            });
            i = self.inner.borrow_mut();
        }
        let rows = i.history.get(&id).map(|(_, r)| r.clone())?;
        Some(
            rows.iter()
                .map(|r| EditSession {
                    by: i.person(&r.user_id),
                    from: at(r.started_at),
                    to: at(r.ended_at),
                    words: r.words,
                    saves: r.saves.max(0) as u32,
                })
                .collect(),
        )
    }

    fn trash(&mut self) -> Option<Vec<Entry>> {
        let mut i = self.inner.borrow_mut();
        let stale = i.trash.as_ref().map(|(t, _)| now_ms() - t > STALE_MS).unwrap_or(true);
        if stale && !i.trash_asked {
            i.trash_asked = true;
            drop(i);
            let inner = self.inner.clone();
            spawn(&self.inner, async move {
                if let Ok(a) = browser::call("GET", "/api/trash", None).await {
                    let mut i = inner.borrow_mut();
                    learn_people(&mut i, &a.json);
                    if let Ok(rows) = serde_json::from_value::<Vec<GoneRow>>(a.json["scripts"].clone()) {
                        i.trash = Some((now_ms(), rows));
                    }
                }
                inner.borrow_mut().trash_asked = false;
            });
            i = self.inner.borrow_mut();
        }
        let rows = i.trash.as_ref().map(|(_, r)| r.clone())?;
        Some(
            rows.iter()
                .map(|r| Entry {
                    path: PathBuf::from(&r.id),
                    title: r.title.clone(),
                    modified: at(r.deleted_at),
                    preview: String::new(),
                    starred: false,
                    pages: 0,
                    scenes: 0,
                    edited_by: r.deleted_by.as_deref().map(|who| i.person(who)),
                    editing: None,
                    author: String::new(),
                })
                .collect(),
        )
    }

    fn restore(&mut self, key: &Path) {
        let id = key_id(key);
        self.inner.borrow_mut().trash = None;
        let inner = self.inner.clone();
        spawn(&self.inner, async move {
            match browser::call("POST", &format!("/api/scripts/{id}/restore"), Some(&json!({}))).await {
                Ok(a) if a.ok() => {}
                Ok(a) => inner.borrow_mut().say("Could not restore", &a.error(), Tone::Danger),
                Err(_) => inner.borrow_mut().offline(),
            }
            refresh_list(&inner).await;
        });
    }

    fn add_member(&mut self, discord_id: &str, role: Role) {
        let body = json!({ "id": discord_id, "role": role.slug() });
        team_change(&self.inner, "POST", "/api/members".to_string(), Some(body));
    }

    fn set_role(&mut self, discord_id: &str, role: Role) {
        let body = json!({ "role": role.slug() });
        team_change(&self.inner, "PUT", format!("/api/members/{discord_id}"), Some(body));
    }

    fn remove_member(&mut self, discord_id: &str) {
        team_change(&self.inner, "DELETE", format!("/api/members/{discord_id}"), None);
    }

    fn sign_out(&mut self) {
        spawn(&self.inner, async move {
            let _ = browser::call("POST", "/auth/signout", Some(&json!({}))).await;
            browser::reload();
        });
    }

    fn read_settings(&mut self) -> Settings {
        self.inner.borrow().settings.clone()
    }

    fn write_settings(&mut self, s: &Settings) -> Result<(), String> {
        let mut i = self.inner.borrow_mut();
        i.settings = s.clone();
        i.settings_due = Some(now_ms() + SETTINGS_GAP_MS);
        Ok(())
    }

    fn deliver(&mut self, name: &str, bytes: Vec<u8>, _after: Option<AfterExport>) -> Result<String, String> {
        browser::download(name, &bytes)?;
        Ok(format!("{name} — in your downloads"))
    }

    fn start_import(&mut self) {
        match &self.picker {
            Some(p) => p.open(),
            None => self.inner.borrow_mut().say("No file picker", "Drop a file onto the page instead.", Tone::Warn),
        }
    }

    fn open_url(&mut self, url: &str) {
        browser::open_tab(url);
    }

    fn events(&mut self) -> Vec<Event> {
        pump(&self.inner);
        collect_avatars(self);
        self.rebuild_team();
        std::mem::take(&mut self.inner.borrow_mut().events)
    }

    fn busy(&self) -> bool {
        let i = self.inner.borrow();
        i.in_flight > 0
            || i.settings_due.is_some()
            || i.open.as_ref().map(|o| o.pending.is_some()).unwrap_or(false)
    }
}

impl Open {
    fn fresh(id: &str, version: i64, editable: bool, viewer: bool) -> Open {
        Open {
            id: id.to_string(),
            version,
            editable,
            viewer,
            free_said: false,
            pending: None,
            saving: false,
            last_put: 0.0,
        }
    }
}

// ---------- talking to the server ----------

/// Run a request without blocking a frame, counting it while it runs.
fn spawn(inner: &In, f: impl std::future::Future<Output = ()> + 'static) {
    inner.borrow_mut().in_flight += 1;
    let inner = inner.clone();
    wasm_bindgen_futures::spawn_local(async move {
        f.await;
        let mut i = inner.borrow_mut();
        i.in_flight = i.in_flight.saturating_sub(1);
        i.repaint();
    });
}

/// Send what is due: a save, at most every few seconds; settings, a moment
/// after the last change.
fn pump(inner: &In) {
    let now = now_ms();
    let mut i = inner.borrow_mut();

    if let Some(due) = i.settings_due {
        if now >= due {
            i.settings_due = None;
            let body = json!({ "body": i.settings.serialize() });
            drop(i);
            spawn(inner, async move {
                let _ = browser::call("PUT", "/api/settings", Some(&body)).await;
            });
            i = inner.borrow_mut();
        }
    }

    let creating: HashSet<String> = i.creating.keys().cloned().collect();
    let Some(o) = i.open.as_mut() else { return };
    if o.pending.is_none() || o.saving || !o.editable || now - o.last_put < SAVE_GAP_MS || creating.contains(&o.id) {
        return;
    }
    let Some(doc) = o.pending.take() else { return };
    o.saving = true;
    o.last_put = now;
    let (id, version) = (o.id.clone(), o.version);
    drop(i);
    let inner2 = inner.clone();
    spawn(inner, async move {
        let body = save_body(&doc, version);
        let answer = browser::call("PUT", &format!("/api/scripts/{id}"), Some(&body)).await;
        saved(&inner2, &id, doc, answer);
    });
}

/// What became of a save.
fn saved(inner: &In, id: &str, sent: Document, answer: Result<Answer, String>) {
    let mut i = inner.borrow_mut();
    let me = i.me_id();
    let still_open = i.open.as_ref().map(|o| o.id == id).unwrap_or(false);
    match answer {
        Ok(a) if a.ok() => {
            i.offline_said = false;
            let version = a.json["version"].as_i64().unwrap_or(0);
            let card = card(&sent);
            if let Some(row) = i.scripts.iter_mut().find(|s| s.id == id) {
                row.title = card["title"].as_str().unwrap_or_default().to_string();
                row.author = sent.meta.author.clone();
                row.preview = card["preview"].as_str().unwrap_or_default().to_string();
                row.pages = card["pages"].as_i64().unwrap_or(0);
                row.scenes = card["scenes"].as_i64().unwrap_or(0);
                row.updated_at = now_ms() as i64;
                row.updated_by = me;
            }
            i.list_changed = true;
            i.history_asked.remove(id);
            if let Some(o) = i.open.as_mut().filter(|o| o.id == id) {
                o.saving = false;
                o.version = version;
                if o.pending.is_none() {
                    browser::forget(&draft_key(id));
                } else if let Some(p) = &o.pending {
                    // the newer change is kept against the new version
                    browser::store(&draft_key(id), &draft(version, p));
                }
            } else {
                browser::forget(&draft_key(id));
            }
        }
        Ok(a) if a.status == 409 => {
            // someone saved first: yours is kept as a snapshot, theirs shown
            let theirs = storage::from_markdown(a.json["body"].as_str().unwrap_or_default());
            let by = i.person(a.json["updated_by"].as_str().unwrap_or_default());
            let version = a.json["version"].as_i64().unwrap_or(0);
            if let Some(o) = i.open.as_mut().filter(|o| o.id == id) {
                o.saving = false;
                o.version = version;
                o.pending = None;
            }
            drop(i);
            // the draft in this browser goes once the snapshot is safe
            send_snapshot(inner, id.to_string(), &sent, Some("Your version (kept)".into()));
            let mut i = inner.borrow_mut();
            if still_open {
                i.events.push(Event::Conflict { key: PathBuf::from(id), theirs, by });
            }
        }
        Ok(a) if a.status == 423 => {
            // someone else has it now: yours is kept, the page goes read only
            let by = i.person(a.json["holder"].as_str().unwrap_or_default());
            let mut unsent = None;
            if let Some(o) = i.open.as_mut().filter(|o| o.id == id) {
                o.saving = false;
                o.editable = false;
                unsent = o.pending.take();
            }
            drop(i);
            // what was refused, and anything typed since, are both kept
            send_snapshot(inner, id.to_string(), &sent, Some("Your version (kept)".into()));
            if let Some(newer) = unsent {
                send_snapshot(inner, id.to_string(), &newer, Some("Your version (kept)".into()));
            }
            let mut i = inner.borrow_mut();
            if still_open {
                i.events.push(Event::Locked { key: PathBuf::from(id), by });
            }
        }
        Ok(a) => {
            // refused for another reason: say so, keep it for a retry
            let msg = a.error();
            if let Some(o) = i.open.as_mut().filter(|o| o.id == id) {
                o.saving = false;
                if o.pending.is_none() {
                    o.pending = Some(sent);
                }
                o.last_put = now_ms() + 20_000.0;
            }
            i.say("Not saved to the team library yet", &msg, Tone::Warn);
        }
        Err(_) => {
            // unreachable: try again shortly; the draft keeps it meanwhile
            if let Some(o) = i.open.as_mut().filter(|o| o.id == id) {
                o.saving = false;
                if o.pending.is_none() {
                    o.pending = Some(sent);
                }
                o.last_put = now_ms() + 6000.0;
            }
            i.offline();
        }
    }
}

/// Ask for a script, and for the right to edit it.
fn open_script(inner: In, id: String) {
    let i2 = inner.clone();
    spawn(&inner, async move {
        let answer = browser::call("POST", &format!("/api/scripts/{id}/open"), Some(&json!({}))).await;
        let mut i = i2.borrow_mut();
        let a = match answer {
            Ok(a) if a.ok() => a,
            Ok(a) => {
                let msg = a.error();
                i.say("Could not open", &msg, Tone::Danger);
                return;
            }
            Err(_) => {
                i.offline();
                return;
            }
        };
        let body = a.json["body"].as_str().unwrap_or_default().to_string();
        let version = a.json["version"].as_i64().unwrap_or(0);
        let updated_by = a.json["updated_by"].as_str().unwrap_or_default().to_string();
        let mine = a.json["lease"]["mine"].as_bool().unwrap_or(false);
        let holder = a.json["lease"]["holder"].as_str().map(str::to_string);
        let viewer = !i.writer();
        let me = i.me_id();
        let first_name = i.me.as_ref().map(|m| m.name.split_whitespace().next().unwrap_or("").to_string()).unwrap_or_default();
        let mut doc = storage::from_markdown(&body);
        let access = if mine {
            Access::Edit
        } else {
            Access::ReadOnly { by: holder.as_deref().map(|h| i.person(h)) }
        };
        let mut open = Open::fresh(&id, version, mine, viewer);

        // anything this browser had not delivered last time?
        let mut keep_aside = None;
        if let Some(d) = browser::stored(&draft_key(&id)).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
            let dv = d["version"].as_i64().unwrap_or(-1);
            let text = d["body"].as_str().unwrap_or_default().to_string();
            if text != body {
                if dv == version && mine {
                    // nobody saved since: carry on from it
                    doc = storage::from_markdown(&text);
                    open.pending = Some(doc.clone());
                    i.say(
                        "Picked up where you left off",
                        "Changes that had not reached the team library were still in this browser. They are going now.",
                        Tone::Info,
                    );
                } else if !viewer {
                    keep_aside = Some(storage::from_markdown(&text));
                }
            }
            if open.pending.is_none() && keep_aside.is_none() {
                browser::forget(&draft_key(&id));
            }
        }

        i.open = Some(open);
        i.events.push(Event::Opened { key: PathBuf::from(&id), doc: doc.clone(), access });
        drop(i);

        if let Some(old) = keep_aside {
            send_snapshot(&i2, id.clone(), &old, Some("Unsaved changes from this browser".into()));
            i2.borrow_mut().say(
                "Kept your unsaved changes",
                "Someone saved this script after you left, so what this browser still had is under More → Snapshots.",
                Tone::Info,
            );
        }
        // someone else's script becoming yours: the version before is kept
        if mine && !updated_by.is_empty() && updated_by != me {
            let label = if first_name.is_empty() {
                "Before a new editor".to_string()
            } else {
                format!("Before {first_name} edited")
            };
            send_snapshot(&i2, id, &storage::from_markdown(&body), Some(label));
        }
    });
}

fn send_create(inner: In, id: String, doc: Document) {
    let i2 = inner.clone();
    spawn(&inner, async move {
        let mut body = card(&doc);
        body["id"] = json!(id);
        body["body"] = json!(storage::to_markdown(&doc));
        let answer = browser::call("POST", "/api/scripts", Some(&body)).await;
        let mut i = i2.borrow_mut();
        match answer {
            Ok(a) if a.ok() || a.status == 409 => {
                i.creating.remove(&id);
            }
            Ok(a) => {
                let msg = a.error();
                i.creating.remove(&id);
                i.scripts.retain(|s| s.id != id);
                i.list_changed = true;
                i.say("Could not add the script", &msg, Tone::Danger);
            }
            Err(_) => {
                // try again in a moment; it stays in this browser meanwhile
                i.offline();
                drop(i);
                let again = i2.clone();
                let cb = wasm_bindgen::closure::Closure::once_into_js(move || send_create(again, id, doc));
                let _ = browser::window().set_timeout_with_callback_and_timeout_and_arguments_0(
                    wasm_bindgen::JsCast::unchecked_ref(&cb),
                    8000,
                );
            }
        }
    });
}

fn send_snapshot(inner: &In, id: String, doc: &Document, label: Option<String>) {
    let kind = if label.is_some() { "auto" } else { "manual" };
    let body = json!({
        "label": label,
        "kind": kind,
        "pages": export::page_count(doc),
        "body": deflate(&storage::to_markdown(doc)),
    });
    let markdown = storage::to_markdown(doc);
    let i2 = inner.clone();
    spawn(inner, async move {
        let answer = browser::call("POST", &format!("/api/scripts/{id}/snapshots"), Some(&body)).await;
        let mut i = i2.borrow_mut();
        i.snapshots_asked.remove(&id);
        i.snapshots.remove(&id);
        if matches!(answer, Ok(ref a) if a.ok()) {
            // a draft in this browser that says the same is safe now
            let same = browser::stored(&draft_key(&id))
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .map(|d| d["body"].as_str() == Some(markdown.as_str()))
                .unwrap_or(false);
            if same {
                browser::forget(&draft_key(&id));
            }
        } else {
            i.say(
                "Could not keep a snapshot",
                "The team library did not take it. What you wrote is still in this browser.",
                Tone::Warn,
            );
        }
    });
}

fn team_change(inner: &In, method: &'static str, path: String, body: Option<serde_json::Value>) {
    let i2 = inner.clone();
    spawn(inner, async move {
        match browser::call(method, &path, body.as_ref()).await {
            Ok(a) if a.ok() => {}
            Ok(a) => {
                let msg = a.error();
                i2.borrow_mut().say("The team was not changed", &msg, Tone::Warn);
            }
            Err(_) => i2.borrow_mut().offline(),
        }
        refresh_members(&i2).await;
    });
}

async fn refresh_list(inner: &In) {
    if let Ok(a) = browser::call("GET", "/api/scripts", None).await {
        if a.ok() {
            let mut i = inner.borrow_mut();
            learn_people(&mut i, &a.json);
            if let Ok(rows) = serde_json::from_value::<Vec<ScriptRow>>(a.json["scripts"].clone()) {
                // a script made here that the server has not confirmed stays
                let creating: Vec<ScriptRow> = i
                    .scripts
                    .iter()
                    .filter(|s| i.creating.contains_key(&s.id) && !rows.iter().any(|r| r.id == s.id))
                    .cloned()
                    .collect();
                i.scripts = creating.into_iter().chain(rows).collect();
                i.list_changed = true;
            }
        }
    }
}

async fn refresh_members(inner: &In) {
    if let Ok(a) = browser::call("GET", "/api/members", None).await {
        if a.ok() {
            if let Ok(rows) = serde_json::from_value::<Vec<MemberRow>>(a.json["members"].clone()) {
                let mut i = inner.borrow_mut();
                for m in &rows {
                    if let Some(name) = &m.name {
                        let row = PersonRow { id: m.id.clone(), name: name.clone(), avatar: m.avatar.clone() };
                        i.people.insert(m.id.clone(), row);
                    }
                }
                i.members = rows;
                i.team_dirty = true;
            }
        }
    }
}

fn learn_people(i: &mut Inner, json: &serde_json::Value) {
    if let Ok(people) = serde_json::from_value::<Vec<PersonRow>>(json["people"].clone()) {
        for p in people {
            i.people.insert(p.id.clone(), p);
        }
        i.team_dirty = true;
    }
}

/// Twice a minute, and whenever the tab comes back: keep the script you are
/// editing, follow the one you are reading, and look for news.
fn heartbeat(inner: &In) {
    let (open, stamp) = {
        let i = inner.borrow();
        (i.open.as_ref().map(|o| (o.id.clone(), o.editable, o.version, o.viewer, o.free_said)), i.stamp.clone())
    };
    let i2 = inner.clone();
    spawn(inner, async move {
        if let Some((id, editable, version, viewer, free_said)) = open {
            if editable {
                if let Ok(a) = browser::call("POST", &format!("/api/scripts/{id}/lease"), Some(&json!({}))).await {
                    if a.ok() && !a.json["mine"].as_bool().unwrap_or(true) {
                        // someone else has it now (this tab slept past its lease)
                        let holder = a.json["holder"].as_str().unwrap_or_default().to_string();
                        let mut i = i2.borrow_mut();
                        let by = i.person(&holder);
                        let mut unsent = None;
                        if let Some(o) = i.open.as_mut().filter(|o| o.id == id) {
                            o.editable = false;
                            unsent = o.pending.take();
                        }
                        i.events.push(Event::Locked { key: PathBuf::from(&id), by });
                        drop(i);
                        if let Some(doc) = unsent {
                            send_snapshot(&i2, id.clone(), &doc, Some("Your version (kept)".into()));
                        }
                    }
                }
            } else if let Ok(a) = browser::call("GET", &format!("/api/scripts/{id}"), None).await {
                if a.ok() {
                    let mut i = i2.borrow_mut();
                    let v = a.json["version"].as_i64().unwrap_or(version);
                    if v != version {
                        let doc = storage::from_markdown(a.json["body"].as_str().unwrap_or_default());
                        if let Some(o) = i.open.as_mut().filter(|o| o.id == id) {
                            o.version = v;
                        }
                        i.events.push(Event::Refreshed { key: PathBuf::from(&id), doc });
                    }
                    let free = a.json["lease"]["holder"].is_null();
                    if free && !viewer && !free_said {
                        if let Some(o) = i.open.as_mut().filter(|o| o.id == id) {
                            o.free_said = true;
                        }
                        i.events.push(Event::LockFree { key: PathBuf::from(&id) });
                    }
                }
            }
        }
        // news: one row tells whether anything changed
        if let Ok(a) = browser::call("GET", "/api/stamp", None).await {
            if a.ok() && Some(&a.json) != stamp.as_ref() {
                let membership = stamp.as_ref().map(|s| s["stamp"] != a.json["stamp"]).unwrap_or(true);
                i2.borrow_mut().stamp = Some(a.json.clone());
                refresh_list(&i2).await;
                if membership {
                    refresh_members(&i2).await;
                }
            }
        }
    });
}

fn fetch_avatar(id: String, version: String) {
    wasm_bindgen_futures::spawn_local(async move {
        let Ok(bytes) = browser::bytes(&format!("/api/avatars/{id}?v={version}")).await else {
            return;
        };
        let Ok(img) = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png) else {
            return;
        };
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        AVATARS.with(|a| a.borrow_mut().push((id, Arc::new(image))));
    });
}

thread_local! {
    /// Pictures that have arrived, waiting for the store to collect them.
    static AVATARS: RefCell<Vec<(String, Arc<egui::ColorImage>)>> = const { RefCell::new(Vec::new()) };
}

/// Collect pictures that have arrived. Called by the app's frame through
/// `events`.
pub fn collect_avatars(store: &RemoteStore) {
    let arrived: Vec<_> = AVATARS.with(|a| std::mem::take(&mut *a.borrow_mut()));
    if arrived.is_empty() {
        return;
    }
    let mut i = store.inner.borrow_mut();
    for (id, img) in arrived {
        i.avatars.insert(id, img);
    }
    i.team_dirty = true;
    i.list_changed = true;
    i.repaint();
}

// ---------- small things ----------

fn key_id(key: &Path) -> String {
    key.to_string_lossy().into_owned()
}

fn draft_key(id: &str) -> String {
    format!("northstar-draft:{id}")
}

fn draft(version: i64, doc: &Document) -> String {
    json!({ "version": version, "body": storage::to_markdown(doc) }).to_string()
}

/// What the library shows about a script without opening it.
fn card(doc: &Document) -> serde_json::Value {
    let title = if doc.meta.title.trim().is_empty() { "Untitled Script" } else { doc.meta.title.trim() };
    let preview: String = doc
        .blocks
        .iter()
        .map(|b| b.text.trim())
        .find(|t| !t.is_empty())
        .unwrap_or("")
        .chars()
        .take(70)
        .collect();
    json!({
        "title": title,
        "author": doc.meta.author,
        "draft": doc.meta.draft,
        "preview": preview,
        "pages": export::page_count(doc),
        "scenes": doc.scene_count(),
        "words": doc.word_count(),
    })
}

fn save_body(doc: &Document, version: i64) -> serde_json::Value {
    let mut body = card(doc);
    body["body"] = json!(storage::to_markdown(doc));
    body["base_version"] = json!(version);
    body
}

fn deflate(text: &str) -> String {
    let packed = miniz_oxide::deflate::compress_to_vec(text.as_bytes(), 6);
    base64::engine::general_purpose::STANDARD.encode(packed)
}

fn inflate(b64: &str) -> Option<String> {
    let packed = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let bytes = miniz_oxide::inflate::decompress_to_vec(&packed).ok()?;
    String::from_utf8(bytes).ok()
}

fn at(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64)
}

/// "2026-09-24 14:05:09", as a snapshot reads on the desktop.
fn when(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}
