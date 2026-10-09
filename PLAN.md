# Northstar for the web — the plan

## Status (October 2026)

Built and tested: everything in this plan up to and including phase 2, plus
Discord accounts, the Home script picker and History. Since then, two writing
features made only in the browser for now:

- **Acts:** a new page and a centred, underlined title per act, and END OF
  ACT ONE at its close; `# ACT ONE` in the file.
- **Chosen character colours:** Random or Custom; a chosen colour is a hue
  kept in the script's front matter, for the whole team.
- **Your own shortcuts:** Settings → Keyboard, kept in each person's own
  settings on the server, with a reset to the defaults.

The desktop app shows acts and chosen colours in a script that has them, but
does not make them, and keeps its own shortcuts.

- **Desktop app:** restructured as a library behind a `Store` trait, with its
  file format byte-for-byte unchanged (golden test) and a pre-install backup in
  `install.sh`. 100 tests.
- **Server:** `server/`. 11 logic tests and 11 end-to-end API tests. Discord
  settings are Worker secrets, and the first deploy creates the database.
- **Browser app:** `web/`. 6 Playwright tests in Chromium.

Not built yet: phase 3 (real-time co-writing) and phase 4 (comments, revision
mode, desktop "Connect to team"). Setup and deploy steps are in
[README.md](README.md).

## TL;DR

- **Same app in the browser.** The Rust/egui Northstar code compiles to WebAssembly, so the web app looks and works exactly like the desktop app (and stays consistent with Tesseract) because it is the same code.
- **Free backend.** One Cloudflare Worker, written in Rust, serves the app files and a small API. The free D1 database (SQLite) holds the team library: 5 GB, plenty for a small team. If we ever outgrow the free daily limits, the $5/month paid plan lifts them with no code changes.
- **Discord accounts.** You sign in with Discord. Only accounts on the team list can get in. You and any other owners are listed in the config. Owners add everyone else by Discord ID from a **Team** tab in Settings, and set them as owner, editor or viewer. No Cloudflare Access is needed: Discord login is built into the Worker.
- **Script picker on entry (web only).** After signing in you land on a Home screen in the app's design: a greeting, "Continue writing", your starred and team scripts as cards showing who edited them last (with Discord avatar) and who is editing right now, plus New script and Import. The desktop app is unchanged.
- **Who made the edits.**
  - Every save is credited to the person who made it, shown as "Edited by Alex · 5 min ago" across the app.
  - A per-script **History** panel lists each editing session: who, when, and words added or removed.
  - A Contributors row in Details.
  - A snapshot is taken automatically whenever a different person starts editing, so anyone's work can be restored.
- **Collaboration, for now:** one person edits a script at a time, and everyone else gets a live read-only view saying "Alex is editing". Saves are refused, never silently overwritten, if someone else saved in between. Real-time co-writing is the later phase 3.
- **Your scripts are never touched.** The web app can't reach your local files. The desktop app keeps its file format byte-for-byte, a test enforces that, and `install.sh` makes a backup copy before installing.

---

## Context

Northstar is a single-user desktop app (Rust, eframe/egui 0.29, files under `~/.local/share/northstar/`). The team wants to use it from a browser. The team edition adds two things the desktop app doesn't need:
- a script picker when you open the website
- Discord accounts that show who made each edit

Decisions:
- Sign-in is limited to listed accounts.
- Roles are set inside Northstar by owners.
- The Home screen is for the web only; the desktop app is unchanged.

The code is split across two public repos: `exiledddev/northstar` (desktop and shared code) and `exiledddev/northstarWeb` (the web client, server and deploy setup).

## Rule zero: your existing scripts are never touched

- **The browser can't reach local files.** The web app only reads a file the user picks in Import, and never writes it.
- **The desktop format and behaviour are unchanged.** The storage code moves behind a `Store` interface, as a move, not a rewrite. The golden test `files_from_the_first_northstar_still_open_unchanged` (`src/tests.rs`) is extended: real old-format scripts must load and save back byte-for-byte identical. All 69 existing tests must still pass.
- **`install.sh` makes a backup.** Before installing a new build, it copies `~/.local/share/northstar` to `~/.local/share/northstar-backup-<date>` with `cp -a`. It only copies, never deletes.
- **Tests never use the real folder.** They run in a temporary `XDG_DATA_HOME` (the `VAULT` sandbox in `src/tests.rs`).

---

## 1. Architecture

```
Browser ── northstar_web.wasm (the same egui app) ──fetch──► Worker (Rust, workers-rs 0.8.7)
                                                              ├─ static assets: web/dist (free, unmetered)
                                                              ├─ /auth/*  Discord OAuth2, sessions
                                                              └─ /api/*   library, leases, history, team ──► D1
```

**Where the code lives:**
- **`northstar`** (branch `claude/inspiring-galileo-58npkg`) becomes both a library and a binary. The library is the whole app plus a `Store` trait (the interface every storage backend implements). The binary is the desktop app.
- **`northstarWeb`:**
  - `main` starts with `PLAN.md` and `README.md`.
  - The code goes on branch `claude/inspiring-galileo-58npkg`, in three folders:
    - `web/`: the browser crate, depending on `northstar` through a git dependency pinned to a commit.
    - `server/`: the Worker.
    - `server/migrations/`: the database schema files.
  - `web` and `server` are separate cargo projects, so their wasm-bindgen versions never have to agree.

## 2. Changes in `northstar` (shared and desktop)

| File | Change |
|---|---|
| `Cargo.toml` | Add `[lib]` + `[[bin]]`, both named `northstar`. Add `web-time = "1"`. Browser-only extra features: `chrono` with `wasmbind`, `printpdf` with `js-sys`, so dates work in the browser |
| `src/lib.rs` (new) | The module list moves here from `main.rs`, plus `pub use eframe;` so the web crate uses exactly the same egui |
| `src/main.rs` | Desktop entry only. Behaviour unchanged |
| `src/backend.rs` (new) | The `Store` trait, its types, and `LocalBackend` (the desktop backend; see §3) |
| `src/storage.rs` | File-system functions are compiled for desktop only. `Entry`/`Snapshot` use `web_time::SystemTime` (the same type on desktop) and gain optional `edited_by`/`editing`/`by` fields, which are always `None` on desktop. `import_file` is split into a pure `import_text(name, text)` that both platforms use |
| `src/export.rs` | New `render(doc, format, opts) -> (file_name, bytes)`. `export_opts` calls it and writes the same files as today (PDF goes to bytes through printpdf's `save` into a `Vec<u8>`) |
| `src/app.rs` | ~30 `storage::` calls become `self.store.*`. Async results arrive as events (§3). Adds: read-only state, the Home screen route, a Team tab in Settings, History and Recently-deleted panels, avatars, and "Edited by" lines. The desktop code path stays the same |
| `src/home.rs` (new) | The Home screen (§5). Shown only when the store is a team store |
| `src/chrome.rs` | In the browser: no window buttons, window drag or resize grips, and the window counts as maximised (square corners). The title bar's right side gets a Home button and an avatar menu with Sign out |
| `src/alerts.rs`, `pages.rs`, `splash.rs` | `std::time::Instant` → `web_time::Instant`, because `std`'s version panics in the browser |
| `src/theme.rs` | Monospace font discovery is desktop only. Outfit and Courier Prime are already bundled |
| `src/icons.rs` | Add Home, Discord, History, Trash and SignOut glyphs, drawn as vectors like the existing ones |
| `install.sh` | Pre-install backup copy (Rule zero) |
| `src/tests.rs`, `src/uitests.rs` | Golden test. A `MemoryBackend` test double (in-memory store) to drive: pending open, read-only, conflict, home → open, history panel, team tab visible only on team stores |

## 3. The `Store` trait (`src/backend.rs`)

The calls are synchronous and answer from a cache. Slow answers arrive later through `events()`, which the app drains every frame. This is the same pattern as today's import-picker channel (`app.rs` ~719/3725).

```rust
pub struct Person { pub id: String, pub name: String, pub avatar: Option<Arc<egui::ColorImage>> }
pub enum Access { Edit, ReadOnly { by: Option<Person> } }   // by = None → you are a viewer
pub enum Load { Ready(Document, Access), Pending, Failed(String) }
pub enum Event {
    Opened { key: PathBuf, doc: Document, access: Access },
    LibraryChanged,
    Conflict { key: PathBuf, theirs: Document, by: Person },   // yours was already kept as a snapshot
    Locked { key: PathBuf, by: Person },  LockFree { key: PathBuf },
    SnapshotDoc { snap: PathBuf, doc: Document },
    Imported { name: String, bytes: Vec<u8> },
    Notice { title: String, detail: String, tone: Tone },
    SignedOut,
}
pub trait Store {
    fn team(&self) -> Option<&Team>;                     // None on desktop → no team UI at all
    fn list(&mut self) -> Vec<Entry>;  fn changed(&mut self) -> bool;
    fn load(&mut self, key: &Path) -> Load;  fn close(&mut self, key: &Path);
    fn create(&mut self, doc: &Document) -> Result<PathBuf, String>;
    fn save(&mut self, key: &Path, doc: &Document) -> Result<(), String>;
    fn follow_title(&mut self, key: &Path, title: &str) -> PathBuf;   // desktop: today's file rename logic, moved verbatim
    fn duplicate / delete / set_starred / take_snapshot / snapshots / load_snapshot …
    fn history(&mut self, key: &Path) -> Option<Vec<EditSession>>;    // team only
    fn trash(&mut self) -> Option<Vec<Entry>>;  fn restore(&mut self, key: &Path);
    fn read_settings / write_settings / tesseract_look (desktop only)
    fn deliver(&mut self, name: &str, bytes: Vec<u8>, after: AfterExport) -> Result<String, String>;
    fn start_import(&mut self);  fn open_url(&mut self, url: &str);
    fn events(&mut self) -> Vec<Event>;
}
```

`LocalBackend` wraps today's `storage::` functions unchanged: the rename logic from `App::save` (`app.rs` 399–421), the folder watch, the file-picker thread, and xdg-open/reveal.

## 4. Server (`northstarWeb/server`, workers-rs 0.8.7 with `d1`)

### D1 schema (`migrations/0001_init.sql`), one team per deployment

```sql
users     (id TEXT PK /*Discord id*/, name, avatar TEXT /*64px PNG, base64*/, created_at, last_seen)
members   (discord_id TEXT PK, role TEXT CHECK(role IN('owner','editor','viewer')), added_by, added_at)
sessions  (hash TEXT PK /*SHA-256 of cookie token*/, user_id, expires_at)
scripts   (id TEXT PK, title, author, draft, preview, pages, scenes, words,
           body TEXT /*storage::to_markdown*/, version INT, created_at, created_by,
           updated_at, updated_by, deleted_at, deleted_by)       + INDEX(updated_at)
stars     (user_id, script_id, PK(user_id, script_id))
leases    (script_id PK, user_id, expires_at)
snapshots (id PK, script_id, taken_at, taken_by, label, kind /*manual|auto*/, pages,
           body TEXT /*deflate+base64, done in the browser*/)  + INDEX(script_id, taken_at)
edits     (id PK, script_id, user_id, started_at, ended_at, words_delta, saves) + INDEX(script_id, ended_at)
settings  (user_id PK, body TEXT /*Settings::serialize*/)
stamp     (id=1, v)      -- bumped on create/delete/restore/lease change; clients poll it cheaply
```

### Discord sign-in (no Cloudflare Access)

**Discord** only offers plain OAuth2, so the Worker runs the sign-in flow itself:
1. `/auth/discord` redirects to Discord with the `identify` scope and a random `state` value, stored in an `__Host-` cookie that lasts 10 minutes.
2. `/auth/discord/callback` checks `state`, exchanges the code for a token (using the `DISCORD_CLIENT_SECRET` secret), then fetches the account from `/users/@me`.
3. It decides whether the person may come in:
   - Their Discord ID is in `OWNER_DISCORD_IDS` (config): they're an owner.
   - Their ID is in `members`: they get that role.
   - Otherwise they're sent to `/#denied=<id>`. The app then shows "You're not on this team's list yet. Send this Discord ID to an owner", with a Copy button.
4. On success it stores the name (Discord display name) and the 64-pixel PNG avatar, creates a 30-day session (a 256-bit random token; only its SHA-256 is stored), and sets the cookie `__Host-ns_session` (HttpOnly, Secure, SameSite=Lax). Then it redirects to `/`.

**Sign out:** `POST /auth/signout` deletes the session.

**Protection on every API write:**
- the session must be valid
- the role must allow the action
- the `Origin` header must match the site
- the body must be JSON and at most 2 MB

**Local dev login** (`/auth/dev?id=&name=`) is only enabled when three things hold: `DEV_LOGIN=1` (set only in `.dev.vars`, which is never deployed), the request host is `localhost`/`127.0.0.1`, and no Discord client ID is configured. The end-to-end tests use it to act as two different people.

### API (all JSON, all need a session)

| Route | Notes |
|---|---|
| `GET /api/me` | You, your role, the team name |
| `GET /api/stamp` | `{stamp, latest_update}` (reads one row). Clients poll it every 30 s, and when the tab regains focus |
| `GET/POST /api/scripts` | List (with starred-by-me, edited-by and editing-now); create |
| `POST /api/scripts/:id/open` | Body, version, last editor, and tries to take the edit lease in one round trip |
| `PUT /api/scripts/:id` | `{body, base_version, title, author, draft, preview, pages, scenes, words}`. Answers: 409 if someone saved since (sends back the current body and who saved it), 423 if you don't hold the lease, 403 for viewers. It also adds to or extends that person's editing session in `edits` (a gap of 10+ minutes starts a new session) |
| `POST/DELETE /api/scripts/:id/lease`, `POST /api/scripts/:id/release` | Renew every 30 s (90 s expiry) / free. `release` accepts `sendBeacon` when a tab closes |
| `POST /api/scripts/:id/star`, `/duplicate`; `DELETE /api/scripts/:id`; `POST /api/scripts/:id/restore`; `GET /api/trash` | Deleting is soft: a script goes to **Recently deleted** for 30 days |
| `GET/POST /api/scripts/:id/snapshots`, `GET /api/snapshots/:sid` | At most 30 automatic snapshots per script (oldest dropped); manual ones are kept forever |
| `GET /api/scripts/:id/history` | Editing sessions, newest first |
| `GET /api/members`, `POST /api/members`, `PUT/DELETE /api/members/:discord_id` | Owners only for changes. You can't remove the last owner |
| `GET/PUT /api/settings` | Your settings, stored in the `settings.conf` format |

Pure logic is unit-tested natively in `server/src/logic.rs`, which has no worker dependency:
- who may do what
- lease decisions
- merging saves into editing sessions
- cookie and state parsing
- input limits

## 5. Web client (`northstarWeb/web`)

### Boot

`index.html` shows a CSS card that copies `splash.rs` (the Northstar mark, NORTHSTAR, the tagline, MARKEDEXILED SOFTWARE) while the wasm downloads. Then `start()` calls `/api/me`:
- **401:** a small egui `SignIn` app, built from `northstar::{theme, ui, logo, icons}`, shows "Continue with Discord", or the not-on-the-list message.
- **200:** it fetches settings, the script list and the team members, fills the `RemoteStore` cache, and starts `northstar::app::App::with_store(cc, store)`. The app opens on the **Home screen**.

### `RemoteStore`

Implements `Store` using `ehttp` (browser fetch) with callbacks that push events and request a repaint.
- **Saves.** Autosave after 4 seconds of quiet, or every 20 seconds while typing continuously. Each save sends the `base_version`.
- **Local draft.** Every edit is also kept in browser storage (`localStorage`), keyed by script and version. If the tab crashes or the network drops, reopening offers to recover the unsaved text.
- **Timers.** A JavaScript `setInterval` (it keeps ticking in background tabs) renews the lease and polls `/api/stamp`.
- **Conflicts.** Your version is uploaded first as an automatic snapshot labelled "Your version (kept)". Then theirs is loaded and a notice says who saved it.
- **Handover snapshot.** When you start editing a script someone else edited last, the store first saves an automatic snapshot "Before {you} edited".

### Browser glue

- **Export:** downloads a file; `render()` produces the same bytes as desktop.
- **Import:** a hidden `<input type=file multiple>`. Clicking it from the egui click handler works because the browser still counts it as part of your click. Dropped files arrive as bytes; both feed `import_text`.
- **Links:** YouTrack opens through `ctx.open_url`.
- **Shortcuts:** a capture-phase listener calls `preventDefault` on the app's shortcuts (Ctrl+S/F/B/I/E/G/H/,/./1–7…) so the browser doesn't act on them. Browsers never hand Ctrl+N/T/W to a page, so web gets Ctrl+Alt+N (new script) and Alt+1–7 as alternatives, shown in tooltips.

### What you see

- **Home screen** (`src/home.rs`, shared code, web only):
  - Greeting, plus the team name and a member count.
  - New script (the existing gradient button) and Import.
  - A **Continue writing** card for the script you edited last.
  - A search field.
  - **Starred** and **Team scripts** card grids. Each card: title, author, pages and scenes, the editor's avatar with "Edited by Alex · 2h ago", and a quiet dot with "Alex is editing" when the script is locked.
  - Keyboard: arrows and Enter; Ctrl+F to search.
  - Cards lift on hover (`theme::lift_shadow`, no glow, about 300 ms).
  - Footer: the YouTrack button and the studio name.
- **Read-only.** When someone else holds the lease:
  - The script opens in Reading mode, with a banner chip: avatar, "Alex is editing", "read only".
  - Write and Cards are disabled, with a tooltip explaining why.
  - `save()` refuses to run.
  - When the lease frees up, the chip changes to "Alex finished · Edit now".
  - Viewers always get the chip "View only".
- **Who edited what:**
  - "Edited by" on library rows and Home cards.
  - The Details panel gets a Contributors row (avatars) and "Created by".
  - A **History** panel (More → History): each session's avatar, name, time span and word change.
  - Snapshots show who took them and their labels.
  - New scripts on the web use your Discord name as the author.
- **Settings:**
  - New **Team** tab: your account and Sign out, plus members with role controls. Owners also get "Add by Discord ID".
  - Hidden on the web, because they're desktop-only: blur, follow Tesseract, library folder, after-export.
- **Avatars.** The server stores PNG bytes; the web crate decodes them (the `image` crate, PNG only) into `ColorImage`. Avatars are drawn as circles. Anyone without one gets their initials on a colour from `theme::character_colors`.

`web/build.sh` builds the wasm, runs `wasm-bindgen --target web` (it reads the matching version from `Cargo.lock` and prints the install command if that version is missing), runs `wasm-opt -Oz` if it is installed, and copies `index.html` and the icon into `dist/`.

## 6. Deploy (all free; written up in the `README.md`)

1. Build and deploy: `web/build.sh`, then `cd server && npm run deploy`. The first deploy creates the D1 database (wrangler 4.45+ provisions a binding that has no `database_id`, and writes the id back into `wrangler.toml`).
2. Create the tables: `npm run migrate:remote`.
3. Create a Discord application. Add the redirects `https://<name>.<you>.workers.dev/auth/discord/callback` and `http://localhost:8787/auth/discord/callback`.
4. Store three secrets with `wrangler secret put`: `OWNER_DISCORD_IDS`, `DISCORD_CLIENT_ID`, `DISCORD_CLIENT_SECRET`. Being secrets, they stay out of the repository and no deploy can reset them.
5. Add teammates in Settings → Team.

## 7. Order of work (each step committed and pushed)

1. **northstar:** library + binary split, `web-time`, `cargo build --target wasm32-unknown-unknown --lib` compiles. No behaviour change.
2. **northstar:** `Store` trait + `LocalBackend` + app refactor + `export::render` + `import_text` + golden test + `install.sh` backup. The 69 tests and the new ones pass.
3. **northstar:** Home screen, read-only mode, History, Recently deleted, Team tab, avatars, browser chrome, plus `MemoryBackend` UI tests.
4. **northstarWeb `main`:** `PLAN.md` + `README.md`.
5. **northstarWeb branch:** server, schema, Discord authentication, API, logic unit tests, Node end-to-end tests.
6. **northstarWeb branch:** web crate, `RemoteStore`, SignIn, `index.html`, `build.sh`, Playwright end-to-end tests. Pin the `northstar` dependency to the pushed commit.
7. Deploy guide, final checks, and screenshots compared with desktop.

Later and not in this build: real-time co-writing (phase 3: `yrs`, a Durable Object per script, line-level authorship from yrs client IDs); desktop "Connect to team".

## 8. Verification

- **northstar:**
  - `cargo test` (69 existing + golden + `MemoryBackend` UI tests), `cargo clippy`, `cargo build --target wasm32-unknown-unknown --lib`.
  - Desktop screenshots under Xvfb compared with before: should be identical.
  - **Scripts-untouched check:** take `sha256sum` of a copied library, run the desktop app (open, browse, quit), and compare.
- **server:**
  - `cargo test` for the logic module; `worker-build --release` stays under 3 MB.
  - Against `wrangler dev --local` plus migrations, `node --test server/tests/api.test.mjs` covers:
    - signed-out requests get 401
    - dev login as A (owner) and B
    - B is refused until A adds B
    - create, list, open, save
    - saving from an out-of-date version gets 409
    - B saving while A holds the lease gets 423
    - a viewer saving gets 403
    - stars are per person
    - delete, Recently deleted, restore
    - snapshots round-trip, with the automatic-snapshot cap
    - editing sessions merge, and split after a 10-minute gap
    - non-owners can't change roles
    - signing out ends the session
- **web:**
  - `web/build.sh`, then Playwright (Chromium is installed; WebGL through SwiftShader) against `wrangler dev` serving `dist/`:
    - the sign-in screen renders
    - A dev-logs in and the Home screen renders (screenshot in a few themes)
    - open a script, type, and autosave: the API has the new body
    - a second browser as B sees the "A is editing" chip and read-only mode
    - export produces a download starting with `%PDF`
    - importing a `.fountain` creates a script
    - History shows A's session
  - Compare screenshots side by side with the desktop ones.

## 9. Free-tier figures (checked Oct 2026)

- **D1:** 5 GB per account (500 MB per database); 5M rows read and 100k rows written per day; 2 MB per row; 7-day point-in-time restore.
- **Workers:** 100k requests per day, 10 ms CPU per request, 3 MB script size; static assets are free.
- **Budget for 5 writers working 6 hours a day:**
  - about 9k saves × about 4 rows written ≈ 36k rows written per day (cap: 100k)
  - about 25k Worker requests per day (cap: 100k)
- **Over a cap,** saves fail until 00:00 UTC. The local draft keeps the work, and a notice says so. Workers Paid ($5/month) removes the caps.

Sources:
- Cloudflare: [D1 limits](https://developers.cloudflare.com/d1/platform/limits), [Workers limits](https://developers.cloudflare.com/workers/platform/limits), [Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/), [Access generic OIDC](https://developers.cloudflare.com/cloudflare-one/identity/idp-integration/generic-oidc/)
- Discord: [OIDC bridge (shows Access needs a bridge for Discord)](https://github.com/Erisa/discord-oidc-worker), [OAuth2 scopes](https://discord.js.org/docs/packages/core/main/OAuth2Scopes:Enum)
- [Neon](https://neon.com/blog/neon-free-plan-1-gb-per-project), [Supabase](https://automationatlas.io/answers/supabase-free-tier-limits-2026/), [Turso](https://turso.tech/pricing.md), [eframe web limits](https://docs.rs/crate/eframe/latest)
