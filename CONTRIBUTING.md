# Contributing to Blackline Web

Blackline Web is the team edition of
[Blackline](https://github.com/exiledddev/northstar): the app itself lives
there, and this repository adds the browser shell and the Cloudflare Worker.
A change to how the app looks or behaves belongs in the main repository; its
[CONTRIBUTING.md](https://github.com/exiledddev/northstar/blob/main/CONTRIBUTING.md)
has the rules, and they apply here too: a writer's scripts are never lost or
changed behind their back.

To work on this repository, follow *Working on it locally* in the
[README](README.md): the server's tests (`cd server && cargo test`), the API
tests and the browser tests all run against `npm run dev`.

By contributing you agree that your contribution is licensed under the
project's [MIT licence](LICENSE).
