// Before the first paint: the saved theme (src/lib/theme.ts, same key) on <html>, so a dark window does not flash light.
try {
  var v = JSON.parse(localStorage.getItem("codeck.theme") || "{}").value;
  var dark = v === "dark" || (v !== "light" && matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
} catch (e) {
  // Left to src/lib/theme.ts.
}
