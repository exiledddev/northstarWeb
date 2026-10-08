-- Northstar Web: the team library. One team per deployment.
-- Times are milliseconds since the Unix epoch. People are Discord user ids.

-- Everyone who has signed in. Their name and picture come from Discord and
-- are refreshed at every sign-in.
CREATE TABLE users (
  id             TEXT PRIMARY KEY,         -- Discord user id
  name           TEXT NOT NULL,
  avatar         TEXT,                     -- 64px PNG, base64
  avatar_version TEXT,                     -- Discord's avatar hash, for caching
  created_at     INTEGER NOT NULL,
  last_seen      INTEGER NOT NULL
);

-- The team list: who may sign in, and as what. Owners named in the
-- OWNER_DISCORD_IDS setting are owners whether or not they are listed here.
CREATE TABLE members (
  discord_id TEXT PRIMARY KEY,
  role       TEXT NOT NULL CHECK (role IN ('owner', 'editor', 'viewer')),
  added_by   TEXT,
  added_at   INTEGER NOT NULL
);

-- Signed-in browsers. Only a SHA-256 of the cookie's token is stored, so the
-- table alone cannot sign anyone in.
CREATE TABLE sessions (
  hash       TEXT PRIMARY KEY,
  user_id    TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE INDEX sessions_by_user ON sessions (user_id);

-- The scripts. `body` is exactly what the desktop app writes to a .md file
-- (storage::to_markdown), so any script can be downloaded and opened there.
CREATE TABLE scripts (
  id         TEXT PRIMARY KEY,
  title      TEXT NOT NULL,
  author     TEXT NOT NULL DEFAULT '',
  draft      TEXT NOT NULL DEFAULT '',
  preview    TEXT NOT NULL DEFAULT '',
  pages      INTEGER NOT NULL DEFAULT 0,
  scenes     INTEGER NOT NULL DEFAULT 0,
  words      INTEGER NOT NULL DEFAULT 0,
  body       TEXT NOT NULL,
  version    INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  created_by TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  updated_by TEXT NOT NULL,
  deleted_at INTEGER,
  deleted_by TEXT
);
CREATE INDEX scripts_by_update ON scripts (updated_at);

-- Stars are each person's own.
CREATE TABLE stars (
  user_id   TEXT NOT NULL,
  script_id TEXT NOT NULL,
  PRIMARY KEY (user_id, script_id)
);

-- Who is editing what. One editor per script; a lease runs out 90 seconds
-- after it was last renewed.
CREATE TABLE leases (
  script_id  TEXT PRIMARY KEY,
  user_id    TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);

-- Dated copies. `body` is the markdown, deflated and base64'd by the browser.
-- Automatic ones are capped per script; ones taken by hand are kept.
CREATE TABLE snapshots (
  id        TEXT PRIMARY KEY,
  script_id TEXT NOT NULL,
  taken_at  INTEGER NOT NULL,
  taken_by  TEXT NOT NULL,
  label     TEXT,
  kind      TEXT NOT NULL CHECK (kind IN ('manual', 'auto')),
  pages     INTEGER NOT NULL DEFAULT 0,
  body      TEXT NOT NULL
);
CREATE INDEX snapshots_by_script ON snapshots (script_id, taken_at);

-- Who worked on what, when: one row per stretch of saves by one person,
-- closed by ten minutes of quiet or by someone else saving.
CREATE TABLE edits (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  script_id   TEXT NOT NULL,
  user_id     TEXT NOT NULL,
  started_at  INTEGER NOT NULL,
  ended_at    INTEGER NOT NULL,
  words_delta INTEGER NOT NULL DEFAULT 0,
  saves       INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX edits_by_script ON edits (script_id, ended_at);

-- Each person's settings, in the desktop app's settings.conf format.
CREATE TABLE settings (
  user_id TEXT PRIMARY KEY,
  body    TEXT NOT NULL
);

-- Bumped whenever the library's shape changes (a script added, deleted or
-- restored, someone starts or stops editing), so browsers can ask "anything
-- new?" by reading one row.
CREATE TABLE stamp (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  v  INTEGER NOT NULL
);
INSERT INTO stamp (id, v) VALUES (1, 0);
