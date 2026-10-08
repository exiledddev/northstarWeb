// Starts Northstar. Kept out of index.html so the page's Content Security
// Policy can forbid inline scripts.
import init from "./northstar_web.js";

// The app's shortcuts that a browser would otherwise act on itself (find,
// bookmarks, save page...). The app still gets every one of these keys: this
// only stops the browser doing its own thing as well. Copy, paste, cut,
// reload and the developer tools are left alone.
const APP_KEYS = new Set([
  "s", "f", "h", "b", "i", "e", "g", "z", "y", ",", ".", "=", "+", "-", "enter",
  "1", "2", "3", "4", "5", "6", "7",
]);
window.addEventListener(
  "keydown",
  (e) => {
    if ((e.ctrlKey || e.metaKey) && APP_KEYS.has(e.key.toLowerCase())) e.preventDefault();
    // Alt+1–7 pick an element in the web build (Ctrl+1–7 switch tabs)
    if (e.altKey && !e.ctrlKey && /^[1-7]$/.test(e.key)) e.preventDefault();
  },
  { capture: true },
);

init().catch((err) => {
  const line = document.getElementById("ns-loading-line");
  if (line) line.textContent = "Northstar could not start in this browser: " + err;
});
