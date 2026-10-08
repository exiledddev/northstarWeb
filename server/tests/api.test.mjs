// End-to-end tests for the team library API, against `wrangler dev`:
//
//   cp .dev.vars.example .dev.vars        (DEV_LOGIN=1, owner 111111111111111111)
//   npm run migrate:local && npm run dev   (in one terminal)
//   npm test                               (in another)
//
// Each run uses fresh people and fresh scripts, so it can be repeated
// against the same local database.

import { test } from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";

const BASE = process.env.NORTHSTAR_URL ?? "http://127.0.0.1:8787";
const OWNER = "111111111111111111";
const id = () => randomBytes(8).toString("hex");
// a fresh Discord-looking id for every person in every run
const person = () => "9" + String(Date.now()).slice(-9) + String(Math.floor(Math.random() * 1e8)).padStart(8, "0");

/** A browser: keeps its own session cookie, says where it comes from. */
class Browser {
  constructor() {
    this.cookie = "";
  }
  async signIn(discordId, name) {
    const r = await fetch(`${BASE}/auth/dev?id=${discordId}&name=${encodeURIComponent(name)}`, { redirect: "manual" });
    const set = r.headers.getSetCookie().find((c) => c.startsWith("__Host-ns_session=") && !c.includes("Max-Age=0"));
    if (set) this.cookie = set.split(";")[0];
    return r;
  }
  async call(method, path, body, extra = {}) {
    const headers = { Cookie: this.cookie, Origin: BASE, ...extra };
    if (body !== undefined) headers["Content-Type"] = "application/json";
    const r = await fetch(`${BASE}${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      redirect: "manual",
    });
    let json = null;
    const type = r.headers.get("content-type") ?? "";
    if (type.includes("json")) json = await r.json();
    return { status: r.status, json, headers: r.headers };
  }
  get = (p) => this.call("GET", p);
  post = (p, b) => this.call("POST", p, b ?? {});
  put = (p, b) => this.call("PUT", p, b);
  del = (p) => this.call("DELETE", p);
}

const md = (title, line) =>
  `---\ntitle: ${title}\nauthor: Sam\ncontact: \ndraft: First Draft\n---\n\n## INT. ROOM - DAY\n\n${line}\n\n`;

async function team() {
  const a = new Browser();
  const b = new Browser();
  const c = new Browser();
  const bId = person();
  const cId = person();
  await a.signIn(OWNER, "Sam Ito");
  assert.equal((await a.post("/api/members", { id: bId, role: "editor" })).status, 200);
  assert.equal((await a.post("/api/members", { id: cId, role: "viewer" })).status, 200);
  await b.signIn(bId, "Alex Reyes");
  await c.signIn(cId, "June Park");
  return { a, b, c, bId, cId };
}

async function newScript(who, title = "Pilot") {
  const sid = id();
  const r = await who.post("/api/scripts", {
    id: sid,
    title,
    author: "Sam",
    draft: "First Draft",
    preview: "Phones ring.",
    pages: 1,
    scenes: 1,
    words: 10,
    body: md(title, "Phones ring."),
  });
  assert.equal(r.status, 201, JSON.stringify(r.json));
  return sid;
}

test("nobody gets in without signing in, and only listed people sign in", async () => {
  const stranger = new Browser();
  assert.equal((await stranger.get("/api/me")).status, 401);
  assert.equal((await stranger.get("/api/scripts")).status, 401);

  const outsider = person();
  const r = await stranger.signIn(outsider, "Nobody");
  assert.equal(r.status, 302);
  assert.equal(r.headers.get("location"), `/#denied=${outsider}`, "an unlisted person is shown their id, not let in");
  assert.equal((await stranger.get("/api/me")).status, 401);

  const owner = new Browser();
  await owner.signIn(OWNER, "Sam Ito");
  const me = await owner.get("/api/me");
  assert.equal(me.status, 200);
  assert.equal(me.json.role, "owner");
  assert.equal(me.json.me.id, OWNER);
  assert.ok(me.json.team.length > 0);
});

test("a page elsewhere cannot change the library", async () => {
  const { a } = await team();
  const body = { id: id(), title: "X", body: md("X", "x") };
  const evil = await a.call("POST", "/api/scripts", body, { Origin: "https://evil.example" });
  assert.equal(evil.status, 403);
  const noOrigin = await fetch(`${BASE}/api/scripts`, {
    method: "POST",
    headers: { Cookie: a.cookie, "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  assert.equal(noOrigin.status, 403);
});

test("one person edits a script at a time, and nobody overwrites anybody", async () => {
  const { a, b, bId } = await team();
  const sid = await newScript(a);

  // A started it, so A holds it
  const listed = await b.get("/api/scripts");
  const row = listed.json.scripts.find((s) => s.id === sid);
  assert.equal(row.editing, OWNER);
  assert.ok(listed.json.people.some((p) => p.id === OWNER && p.name === "Sam Ito"));

  // B can read it, but not write to it
  const bOpen = await b.post(`/api/scripts/${sid}/open`);
  assert.equal(bOpen.status, 200);
  assert.equal(bOpen.json.lease.mine, false);
  assert.equal(bOpen.json.lease.holder, OWNER);
  assert.match(bOpen.json.body, /Phones ring/);
  const bSave = await b.put(`/api/scripts/${sid}`, { body: md("Pilot", "B was here."), base_version: 1, title: "Pilot", words: 12 });
  assert.equal(bSave.status, 423);
  assert.equal(bSave.json.holder, OWNER);

  // A saves; a save from an old version is refused with the script as it is
  const s1 = await a.put(`/api/scripts/${sid}`, { body: md("Pilot", "Phones ring twice."), base_version: 1, title: "Pilot", words: 13 });
  assert.equal(s1.status, 200);
  assert.equal(s1.json.version, 2);
  const stale = await a.put(`/api/scripts/${sid}`, { body: md("Pilot", "Stale."), base_version: 1, title: "Pilot", words: 9 });
  assert.equal(stale.status, 409);
  assert.equal(stale.json.version, 2);
  assert.match(stale.json.body, /Phones ring twice/);
  assert.equal(stale.json.updated_by, OWNER);

  // A lets go; B takes it and saves
  assert.equal((await a.post(`/api/scripts/${sid}/release`, {})).status, 200);
  const bAgain = await b.post(`/api/scripts/${sid}/open`);
  assert.equal(bAgain.json.lease.mine, true);
  assert.equal(bAgain.json.version, 2);
  const bOk = await b.put(`/api/scripts/${sid}`, { body: md("Pilot", "B's turn."), base_version: 2, title: "Pilot", words: 15 });
  assert.equal(bOk.status, 200);
  assert.equal(bOk.json.version, 3);

  // and now A is the one who has to wait
  const aOpen = await a.post(`/api/scripts/${sid}/open`);
  assert.equal(aOpen.json.lease.mine, false);
  assert.equal(aOpen.json.lease.holder, bId);
  assert.equal(aOpen.json.updated_by, bId);

  // History: A's stretch, then B's
  const h = await a.get(`/api/scripts/${sid}/history`);
  assert.equal(h.status, 200);
  assert.equal(h.json.sessions.length, 2);
  assert.equal(h.json.sessions[0].user_id, bId);
  assert.equal(h.json.sessions[1].user_id, OWNER);
  assert.equal(h.json.sessions[1].saves, 2, "A's create and save are one stretch");
  assert.equal(h.json.sessions[1].words, 13, "words counted from the start");
});

test("a viewer reads, stars and exports, and changes nothing", async () => {
  const { a, c } = await team();
  const sid = await newScript(a);
  assert.equal((await c.post("/api/scripts", { id: id(), title: "Mine", body: md("Mine", "x") })).status, 403);
  const open = await c.post(`/api/scripts/${sid}/open`);
  assert.equal(open.status, 200);
  assert.equal(open.json.lease.mine, false);
  assert.equal((await c.put(`/api/scripts/${sid}`, { body: "x", base_version: 1 })).status, 403);
  assert.equal((await c.del(`/api/scripts/${sid}`)).status, 403);
  assert.equal((await c.post(`/api/scripts/${sid}/snapshots`, { body: "x" })).status, 403);

  // stars are each person's own
  assert.equal((await c.post(`/api/scripts/${sid}/star`, { on: true })).status, 200);
  const mine = (await c.get("/api/scripts")).json.scripts.find((s) => s.id === sid);
  const theirs = (await a.get("/api/scripts")).json.scripts.find((s) => s.id === sid);
  assert.equal(mine.starred, 1);
  assert.equal(theirs.starred, null);
});

test("snapshots round-trip, and automatic ones are capped", async () => {
  const { a } = await team();
  const sid = await newScript(a);
  const t = await a.post(`/api/scripts/${sid}/snapshots`, { label: "Before the rewrite", kind: "manual", pages: 1, body: "eJzLSM3JyVcozy/KSQEAGgQEXQ==" });
  assert.equal(t.status, 201);
  const back = await a.get(`/api/snapshots/${t.json.id}`);
  assert.equal(back.json.body, "eJzLSM3JyVcozy/KSQEAGgQEXQ==");
  assert.equal(back.json.script_id, sid);
  for (let k = 0; k < 32; k++) {
    await a.post(`/api/scripts/${sid}/snapshots`, { label: `auto ${k}`, kind: "auto", pages: 1, body: "x" });
  }
  const list = (await a.get(`/api/scripts/${sid}/snapshots`)).json.snapshots;
  assert.equal(list.filter((s) => s.kind === "auto").length, 30);
  assert.equal(list.filter((s) => s.kind === "manual").length, 1, "a snapshot taken by hand is kept");
  assert.equal(list[0].label, "auto 31", "newest first");
});

test("deleting waits for the editor, and Recently deleted brings it back", async () => {
  const { a, b } = await team();
  const sid = await newScript(b, "Spec");
  assert.equal((await a.del(`/api/scripts/${sid}`)).status, 423, "not while B is in it");
  await b.post(`/api/scripts/${sid}/release`, {});
  assert.equal((await a.del(`/api/scripts/${sid}`)).status, 200);
  assert.ok(!(await a.get("/api/scripts")).json.scripts.some((s) => s.id === sid));
  assert.equal((await a.post(`/api/scripts/${sid}/open`)).status, 404);
  const gone = (await a.get("/api/trash")).json.scripts.find((s) => s.id === sid);
  assert.equal(gone.deleted_by, OWNER);
  assert.equal((await b.post(`/api/scripts/${sid}/restore`)).status, 200);
  assert.ok((await a.get("/api/scripts")).json.scripts.some((s) => s.id === sid));
});

test("a copy has its own title, inside and out", async () => {
  const { a } = await team();
  const sid = await newScript(a, "Northbound");
  const d = await a.post(`/api/scripts/${sid}/duplicate`);
  assert.equal(d.status, 201);
  assert.equal(d.json.title, "Northbound (copy)");
  const open = await a.post(`/api/scripts/${d.json.id}/open`);
  assert.match(open.json.body, /^---\ntitle: Northbound \(copy\)\n/);
});

test("only owners manage the team, and the team keeps an owner", async () => {
  const { a, b, c, bId, cId } = await team();
  assert.equal((await b.post("/api/members", { id: person(), role: "editor" })).status, 403);
  assert.equal((await b.put(`/api/members/${cId}`, { role: "editor" })).status, 403);
  assert.equal((await a.put(`/api/members/${cId}`, { role: "editor" })).status, 200);
  assert.equal((await c.get("/api/me")).json.role, "editor");
  assert.equal((await a.put(`/api/members/${OWNER}`, { role: "viewer" })).status, 400, "configured owners are fixed");
  assert.equal((await a.del(`/api/members/${OWNER}`)).status, 400);
  const list = (await a.get("/api/members")).json.members;
  assert.ok(list.some((m) => m.id === OWNER && m.fixed && m.role === "owner"));
  assert.ok(list.some((m) => m.id === bId && m.name === "Alex Reyes" && !m.pending));

  // taken off the team: signed out everywhere, at once
  assert.equal((await a.del(`/api/members/${bId}`)).status, 200);
  assert.equal((await b.get("/api/me")).status, 401);
  const again = await b.signIn(bId, "Alex Reyes");
  assert.equal(again.headers.get("location"), `/#denied=${bId}`);
});

test("settings are each person's own and size-limited", async () => {
  const { a, b } = await team();
  assert.equal((await a.put("/api/settings", { body: "theme = zen\nlight_mode = yes\n" })).status, 200);
  assert.equal((await a.get("/api/settings")).json.body, "theme = zen\nlight_mode = yes\n");
  assert.equal((await b.get("/api/settings")).json.body, "");
  assert.equal((await a.put("/api/settings", { body: "x".repeat(20000) })).status, 400);
});

test("the library's stamp moves when someone starts editing", async () => {
  const { a, b } = await team();
  const before = (await b.get("/api/stamp")).json;
  const sid = await newScript(a);
  const after = (await b.get("/api/stamp")).json;
  assert.ok(after.stamp > before.stamp);
  assert.ok(after.latest >= before.latest);
  await a.post(`/api/scripts/${sid}/release`, {});
  assert.ok((await b.get("/api/stamp")).json.stamp > after.stamp);
});

test("signing out ends the session", async () => {
  const a = new Browser();
  await a.signIn(OWNER, "Sam Ito");
  assert.equal((await a.get("/api/me")).status, 200);
  const out = await a.post("/auth/signout");
  assert.equal(out.status, 200);
  assert.ok(out.headers.getSetCookie().some((c) => c.startsWith("__Host-ns_session=;") && c.includes("Max-Age=0")));
  assert.equal((await a.get("/api/me")).status, 401);
});
