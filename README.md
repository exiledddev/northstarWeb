# Northstar Web

The team edition of [Northstar](https://github.com/exiledddev/northstar), the
screenwriting studio by MarkedExiled Software. Your team uses it in a
browser instead of on one person's machine.

- **The same app.** The browser runs Northstar's own Rust code, compiled to
  WebAssembly. The editor, Cards, Reading mode, every theme and the PDF export
  are exactly the desktop app's.
- **One shared library.** It lives in a Cloudflare D1 database, served by a
  small Rust Worker. Both fit in Cloudflare's free plan.
- **Discord accounts.** You sign in with Discord. Only accounts on the team's
  list get in. The script picker shows who edited what, and each script has a
  history of who worked on it and when.
- **Nobody overwrites anybody.** One person edits a script at a time; everyone
  else reads along live and sees who is editing.

Your local Northstar library (`~/.local/share/northstar`) is never touched by
any of this. The web app has no access to it, and moving scripts to the team
uploads copies.

See [PLAN.md](PLAN.md) for the design, the database choice and the free-tier
numbers.

## Layout

```
web/       the browser app: Northstar compiled to wasm, plus the team store
server/    the Cloudflare Worker: Discord sign-in, the API, D1 migrations
```

Build and deploy steps live in `server/README.md` once the code lands.

MarkedExiled Software.
