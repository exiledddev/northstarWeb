# Northstar Web

The team edition of [Northstar](https://github.com/exiledddev/northstar), the
screenwriting studio by MarkedExiled Software. Your whole team uses it in a
browser, on one shared library.

![Home: the script picker](docs/home.png)

- **The same app.** The browser runs Northstar's own Rust code, compiled to
  WebAssembly. The editor, Cards, Reading mode, every theme and the PDF export
  are exactly the desktop app's.
- **Discord accounts.** Everyone signs in with Discord. Only people on the
  team's list get in. Owners add teammates by Discord ID and set them as owner,
  editor or viewer.
- **A script picker when you arrive.** Every script appears as a card with who
  saved it last and who is in it right now. Starred scripts come first, and
  your last script is one click away.
- **Who wrote what.**
  - Every save is credited to its writer.
  - History shows each stretch of work: who, when, and how many words.
  - When someone new picks up a script, the version before them is kept as a
    snapshot.
- **Nobody overwrites anybody.** One person edits a script at a time; everyone
  else reads along live, with a chip saying who is editing. When they finish,
  the next person can take over.
- **Nothing typed is lost.** Changes stay in your browser until the server has
  them, and a crashed tab offers them back. A save the server refuses is kept
  as a snapshot before anything else happens. Deleted scripts wait 30 days in
  Recently deleted.
- **Acts.** *New act* in the Write ribbon (or Ctrl+Shift+Enter) starts an act
  at the scene you are in, named ACT ONE, ACT TWO… in order; rename it to
  TEASER or COLD OPEN by typing over it. In the script an act is a wide
  divider with a constellation of its own. In print every act starts a new
  page with its title centred, bold and underlined, and ends with a centred
  END OF ACT ONE. The navigator and Cards group scenes by act.
- **Character colours you choose.** In Settings → Colour, character colours
  are *Random* (dealt from the wheel, as before; Shuffle deals again) or
  *Custom*. Click a character's colour, in the Cast panel or the Colour tab,
  to choose theirs; they keep it everywhere, in every theme and in the PDF.
  Chosen colours are kept in the script, so the whole team sees the same
  ones. *Use random* gives a colour back to chance.
- **Free to run.** One Cloudflare Worker and a D1 database, both on
  Cloudflare's free plan (numbers in [PLAN.md](PLAN.md)).

Your own Northstar library on your computer (`~/.local/share/northstar`) is
never touched by any of this. The web app cannot see it. Moving scripts to the
team uploads copies (see below).

| Signing in | Someone else is editing |
|---|---|
| ![The door](docs/sign-in.png) | ![Reading along](docs/read-along.png) |

![A new act, ready to be named](docs/act.png)

## How it fits together

```
browser ── Northstar (wasm) ──► Cloudflare Worker (Rust) ──► D1 (SQLite)
                                 ├─ the app's files (served free, no Worker run)
                                 ├─ /auth/*  Discord sign-in, sessions
                                 └─ /api/*   scripts, leases, history, team
```

| Folder | What it is |
|---|---|
| `web/` | The browser app: the `northstar` crate (pinned to a commit) plus a `Store` that talks to the server, the sign-in screen, and the page around it |
| `server/` | The Worker: Discord OAuth2, the API, the D1 schema (`migrations/`) |
| `PLAN.md` | The design: why Cloudflare, the free-tier numbers, what comes next |

## Set it up (once, about 20 minutes)

You need a free Cloudflare account and a Discord account. These steps are for
Debian, Ubuntu and Pop!_OS, and are done from your own computer: Cloudflare's
"Import a repository" builder cannot build the Rust app.

### 1. Tools

```sh
# Rust, if you don't have it
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add wasm32-unknown-unknown

# Node.js 20 or newer, for Cloudflare's wrangler tool
sudo apt install nodejs npm        # or from nodejs.org if your release has an older one

# the wasm-bindgen CLI at the exact version web/Cargo.lock names
# (web/build.sh tells you the version if it is wrong)
cargo install wasm-bindgen-cli --version 0.2.129

# optional: makes the download smaller
sudo apt install binaryen
```

### 2. Build and deploy

```sh
web/build.sh                  # builds the app into web/dist
cd server
npm install
npx wrangler login            # opens the browser once
npm run deploy                # builds the Worker and publishes both
npm run migrate:remote        # creates the tables
```

The first `npm run deploy` also creates the D1 database and writes its
`database_id` into `server/wrangler.toml`. Commit that line, so later deploys
use the same database. It prints the address of your app:
`https://northstar.<your-subdomain>.workers.dev`. The app answers there now,
but nobody can sign in until steps 3 and 4 are done.

If the deploy stops because there is no database (an older wrangler can't
create one), run `npx wrangler d1 create northstar`, add the `database_id` it
prints to the `[[d1_databases]]` section of `server/wrangler.toml`, and run
`npm run deploy` again.

### 3. A Discord application, for signing in

1. Go to <https://discord.com/developers/applications> and choose **New
   Application**. Call it "Northstar".
2. Open **OAuth2**. Copy the **Client ID**. Choose **Reset Secret** and copy
   the **Client Secret**; keep it private.
3. Under **Redirects**, add:
   - `https://northstar.<your-subdomain>.workers.dev/auth/discord/callback`,
     the address step 2 printed. If you use a custom domain, use that
     instead.
   - `http://localhost:8787/auth/discord/callback`, for trying it locally.
4. Your own Discord user ID: in Discord, open **Settings → Advanced**, turn on
   **Developer Mode**, then right-click your name and choose **Copy User ID**.

### 4. Three secrets

Still in `server/`:

```sh
npx wrangler secret put OWNER_DISCORD_IDS       # your Discord user ID (several: comma separated)
npx wrangler secret put DISCORD_CLIENT_ID       # the Client ID from step 3
npx wrangler secret put DISCORD_CLIENT_SECRET   # the Client Secret from step 3
```

Each one asks for its value and takes effect at once; there is nothing to
redeploy. They live in Cloudflare, not in the repository, and no later deploy
changes them. Owners listed in `OWNER_DISCORD_IDS` can always sign in.
`TEAM_NAME`, what the app calls your team, is in `server/wrangler.toml`.

Open the address from step 2 and sign in with Discord. You are the owner.

### 5. Add your team

In the app, open your picture (top right) → **The team** → **Add someone**.
Paste their Discord user ID, pick **Editor** or **Viewer**, and send them the
address. Anyone not on the list who tries to sign in sees their own Discord ID
and is asked to send it to you.

![The Team tab](docs/team.png)

## Moving scripts from the desktop app

On the web, use **Import** (on Home, or in the library) and select the `.md`
files in `~/.local/share/northstar/scripts`. You can pick several at once.
The browser reads copies: your local files stay exactly as they were. Fountain
(`.fountain`) and Final Draft (`.fdx`) files import the same way.

## Updating

- **The app.** When Northstar itself changes, point `rev` in `web/Cargo.toml`
  at the new commit of `exiledddev/northstar`, then `web/build.sh` and
  `npm run deploy` again.
- **The server.** If a change adds files to `server/migrations/`, run
  `npm run migrate:remote` before deploying.
- Deploying never touches the secrets from step 4, or the scripts in the
  database.

## Working on it locally

```sh
cd server
cp .dev.vars.example .dev.vars   # DEV_LOGIN=1; owner 111111111111111111
npm install
npm run migrate:local
npm run dev                      # http://127.0.0.1:8787

# in another terminal
NORTHSTAR=../northstar web/build.sh    # or plain web/build.sh for the pinned commit
```

Sign in without Discord at `http://127.0.0.1:8787/auth/dev?id=111111111111111111&name=Sam`.
This development sign-in only works on your own machine with `DEV_LOGIN=1`,
which is never deployed. The team list still decides who gets in.

### Tests

```sh
cd server && cargo test            # the server's decisions, natively
cd server && npm test              # the whole API, against `npm run dev`
cd web && npm install && npm test  # the real app in Chromium (Playwright), against `npm run dev`
```

## Keeping it safe

- Everything except the app's own files needs a signed-in session, and every
  change must come from the app's own pages.
- Sessions last a month. Only a hash of each session token is stored.
  Removing someone from the team signs them out everywhere at once.
- The Content Security Policy (`web/static/_headers`) allows no inline
  scripts and no third-party connections.
- Backups: D1 can restore any moment from the last 7 days (Time Travel, free).
  For a copy of your own, run
  `npx wrangler d1 export northstar --remote --output northstar-backup.sql`.

## Limits worth knowing

- **Browsers.** Built for desktop browsers with WebGL 2 (Chrome, Edge,
  Firefox, Safari); the automated tests run in Chromium. Phones and tablets
  are not supported yet.
- **Keyboard shortcuts.** Browsers keep Ctrl+N and Ctrl+1–7 for themselves.
  Use Ctrl+Alt+N for a new script and Alt+1–7 for elements. Ctrl+Shift+Enter
  starts an act.
- **One editor per script, for now.** Real-time co-writing is the next phase
  (see [PLAN.md](PLAN.md)).

MarkedExiled Software.
