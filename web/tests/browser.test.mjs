// Northstar Web in a real browser (Chromium, through Playwright), against
// `wrangler dev` serving web/dist:
//
//   (cd ../server && npm run dev)      with DEV_LOGIN=1 in server/.dev.vars
//   ./build.sh && npm test
//
// The app draws onto a canvas, so these tests act the way a person does —
// click where things are, type — and check what reached the server. Screens
// are saved to tests/shots/ to look at.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, writeFileSync, readFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright";

const BASE = process.env.NORTHSTAR_URL ?? "http://127.0.0.1:8787";
const OWNER = "111111111111111111";
const SHOTS = new URL("./shots/", import.meta.url).pathname;
mkdirSync(SHOTS, { recursive: true });
const person = () => "8" + String(Date.now()).slice(-9) + String(Math.floor(Math.random() * 1e8)).padStart(8, "0");
const VIEW = { width: 1320, height: 880 };

let browser;
before(async () => {
  browser = await chromium.launch({
    args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"],
  });
});
after(async () => browser?.close());

/** A person at a browser: their own cookies, their own page — and a
 * second, quiet page of the same browser to ask the server things with, the
 * way the app itself does. */
async function someone(id, name, settings = "theme = bloodmoon\nlight_mode = no\nsplash = no\n") {
  const context = await browser.newContext({ viewport: VIEW, acceptDownloads: true });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => m.type() === "error" && !/401/.test(m.text()) && errors.push(m.text()));
  const api = await context.newPage();
  await api.goto(`${BASE}/northstar.svg`);
  await api.evaluate(
    async ([id, name]) => {
      await fetch(`/auth/dev?id=${id}&name=${encodeURIComponent(name)}`, { redirect: "manual" });
    },
    [id, name],
  );
  const admitted = (await call(api, "GET", "/api/me")).status === 200;
  if (admitted) {
    // a known look, whatever an earlier run left
    await call(api, "PUT", "/api/settings", { body: settings });
  }
  return { context, page, api, errors, admitted };
}

async function call(api, method, path, data) {
  return api.evaluate(
    async ([method, path, data]) => {
      const r = await fetch(path, {
        method,
        headers: data === null ? {} : { "Content-Type": "application/json" },
        body: data === null ? undefined : JSON.stringify(data),
      });
      let json = null;
      try {
        json = await r.json();
      } catch {}
      return { status: r.status, json };
    },
    [method, path, data ?? null],
  );
}

/** Open the app and wait until it has drawn itself. The app's page is
 * brought to the front: a browser only opens a file picker for the tab in
 * front, and the helper page `someone` opens would otherwise be. */
async function openApp(page) {
  await page.bringToFront();
  await page.goto(BASE);
  await page.waitForSelector("#ns-loading.gone", { state: "attached", timeout: 30_000 });
  await page.waitForTimeout(1200);
}

async function newScript(api, title, line) {
  const id = randomBytes(8).toString("hex");
  const body = `---\ntitle: ${title}\nauthor: Sam Ito\ncontact: \ndraft: First Draft\n---\n\n## INT. ROOM - DAY\n\n${line}\n\n`;
  const r = await call(api, "POST", "/api/scripts", { id, title, author: "Sam Ito", preview: line, pages: 1, scenes: 1, words: 6, body });
  assert.equal(r.status, 201);
  // let go, so the browser opening it takes it the way it would any script
  await call(api, "POST", `/api/scripts/${id}/release`, {});
  return id;
}

test("a stranger is met by the door, in the app's own design", async () => {
  const context = await browser.newContext({ viewport: VIEW });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await openApp(page);
  await page.screenshot({ path: join(SHOTS, "1-sign-in.png") });
  assert.deepEqual(errors, []);

  // someone Discord knows but the team does not: shown their id to pass on
  const stranger = person();
  await page.goto(`${BASE}/auth/dev?id=${stranger}&name=Nobody`);
  await page.waitForSelector("#ns-loading.gone", { state: "attached", timeout: 30_000 });
  await page.waitForTimeout(800);
  await page.screenshot({ path: join(SHOTS, "2-not-on-the-list.png") });
  assert.equal(new URL(page.url()).hash, "", "the id is taken off the address once read");
  assert.deepEqual(errors, []);
  await context.close();
});

test("writing in a browser reaches the team library, and a PDF comes back", async () => {
  const sam = await someone(OWNER, "Sam Ito");
  const title = `Browser ${randomBytes(3).toString("hex")}`;
  const id = await newScript(sam.api, title, "Phones ring.");

  await openApp(sam.page);
  await sam.page.screenshot({ path: join(SHOTS, "3-home.png") });

  // the script just made is the one to continue writing
  const opened = sam.page.waitForResponse((r) => r.url().endsWith(`/api/scripts/${id}/open`));
  await sam.page.mouse.click(650, 230);
  assert.equal((await opened).status(), 200);
  await sam.page.waitForTimeout(1500);

  // the caret starts at the end of the first line
  const saved = sam.page.waitForResponse((r) => r.url().endsWith(`/api/scripts/${id}`) && r.request().method() === "PUT", { timeout: 20_000 });
  await sam.page.keyboard.type(" AT DUSK");
  assert.equal((await saved).status(), 200);
  const back = await call(sam.api, "GET", `/api/scripts/${id}`);
  assert.match(back.json.body, /## INT\. ROOM - DAY AT DUSK/);
  await sam.page.screenshot({ path: join(SHOTS, "4-writing.png") });

  // Quick Export: the same PDF the desktop makes, as a download
  const download = sam.page.waitForEvent("download");
  await sam.page.keyboard.press("Control+E");
  const file = await download;
  assert.match(file.suggestedFilename(), /\.pdf$/);
  const bytes = readFileSync(await file.path());
  assert.equal(bytes.subarray(0, 5).toString(), "%PDF-");

  assert.deepEqual(sam.errors, []);
  await sam.context.close();
});

test("while one person writes, another reads along and cannot type over them", async () => {
  const sam = await someone(OWNER, "Sam Ito");
  const alexId = person();
  assert.equal((await call(sam.api, "POST", "/api/members", { id: alexId, role: "editor" })).status, 200);
  const alex = await someone(alexId, "Alex Reyes");
  assert.ok(alex.admitted);

  const id = await newScript(sam.api, `Shared ${randomBytes(3).toString("hex")}`, "Snow falls.");
  // Sam has it open
  const samOpen = await call(sam.api, "POST", `/api/scripts/${id}/open`, {});
  assert.equal(samOpen.json.lease.mine, true);

  await openApp(alex.page);
  const opened = alex.page.waitForResponse((r) => r.url().endsWith(`/api/scripts/${id}/open`));
  // the newest script is the one Home offers first
  await alex.page.mouse.click(650, 230);
  const answer = await (await opened).json();
  assert.equal(answer.lease.mine, false);
  assert.equal(answer.lease.holder, OWNER);
  await alex.page.waitForTimeout(1500);
  await alex.page.screenshot({ path: join(SHOTS, "5-read-along.png") });

  // typing goes nowhere: no save leaves Alex's browser
  let tried = false;
  alex.page.on("request", (r) => r.method() === "PUT" && r.url().includes(`/api/scripts/${id}`) && (tried = true));
  await alex.page.keyboard.type("Alex was here");
  await alex.page.waitForTimeout(6000);
  assert.equal(tried, false);
  const still = await call(sam.api, "GET", `/api/scripts/${id}`);
  assert.doesNotMatch(still.json.body, /Alex was here/);

  assert.deepEqual(alex.errors, []);
  await alex.context.close();
  await sam.context.close();
});

test("a Fountain file picked in the browser becomes a team script", async () => {
  const sam = await someone(OWNER, "Sam Ito");
  await openApp(sam.page);

  const title = `Imported ${randomBytes(3).toString("hex")}`;
  const path = join(tmpdir(), `${title.replace(" ", "-")}.fountain`);
  writeFileSync(path, `Title: ${title}\nAuthor: June Park\n\nEXT. HARBOUR - NIGHT\n\nFog rolls in.\n\nJUNE\nWe're late.\n`);
  const chooser = sam.page.waitForEvent("filechooser", { timeout: 10_000 });
  await sam.page.mouse.click(992, 89); // Home's Import button
  const created = sam.page.waitForResponse((r) => r.url().endsWith("/api/scripts") && r.request().method() === "POST");
  await (await chooser).setFiles(path);
  assert.equal((await created).status(), 201);
  const list = await call(sam.api, "GET", "/api/scripts");
  const row = list.json.scripts.find((s) => s.title === title);
  assert.ok(row, "the imported script is in the team library");
  assert.equal(row.author, "June Park");
  assert.equal(row.scenes, 1);
  // and it opens, ready to write
  await sam.page.waitForTimeout(1500);
  await sam.page.screenshot({ path: join(SHOTS, "6-imported.png") });
  assert.deepEqual(sam.errors, []);
  await sam.context.close();
});
