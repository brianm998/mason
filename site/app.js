// SPDX-License-Identifier: GPL-3.0-or-later
// Page glue: reads the browser's user agent, loads the latest release, renders pickAssets().
// Test hook: open index.html?release=fixture.json (or fixture.js, which sets
// window.MASON_FIXTURE_RELEASE, for file:// where browsers refuse fetch) to use a local release.
// Add &ua=<user agent> to pretend to be another browser.
(function () {
  "use strict";
  var API = "https://api.github.com/repos/brianm998/mason/releases/latest";
  var params = new URLSearchParams(location.search);
  var $ = function (id) { return document.getElementById(id); };

  function fmtSize(n) {
    if (!n) return "";
    return n >= 1048576 ? (n / 1048576).toFixed(1) + " MB" : Math.max(1, Math.round(n / 1024)) + " KB";
  }
  function fmtDate(s) { return s ? String(s).slice(0, 10) : ""; }
  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  }
  function safeUrl(u) { return /^https:\/\/|^\.?\.?\/|^[\w.-]+$/.test(u) ? u : "#"; }

  function card(title, file, release) {
    var c = el("div", file ? "card" : "card missing");
    c.appendChild(el("h3", "", title));
    if (!file) { c.appendChild(el("p", "muted", "Not available in this release.")); return c; }
    c.appendChild(el("p", "muted", "Version " + release.version + (release.published ? " · " + fmtDate(release.published) : "") +
      (file.size ? " · " + fmtSize(file.size) : "")));
    var a = el("a", "btn", "Download " + file.name);
    a.href = safeUrl(file.url);
    c.appendChild(a);
    return c;
  }

  var names = { mac: "macOS", windows: "Windows", linux: "Linux", ios: "iOS", android: "Android", unknown: "an unknown system" };

  function render(platform, rel) {
    var res = MasonPick.pickAssets(platform, rel);
    $("phone").hidden = res.status !== "unsupported" || (platform.os !== "ios" && platform.os !== "android");
    $("main").hidden = res.status === "none" || !$("phone").hidden;
    if (res.status === "none") { $("status").textContent = "There is no MASON release yet."; return; }
    $("status").textContent = "Latest release: " + res.version;
    if (!$("phone").hidden) return;
    var archNote = platform.os === "linux" && platform.arch !== "unknown" ? " (" + platform.arch + ")" : "";
    $("detected").textContent = res.status === "unsupported"
      ? "We could not tell your system; pick one below."
      : "Detected: " + names[platform.os] + archNote + ".";
    var box = $("downloads"); box.textContent = "";
    if (res.status !== "unsupported") {
      box.appendChild(card("MASON monitor (GUI)", res.gui, res));
      box.appendChild(card("CLI and daemon", res.cli, res));
    }
    var ex = $("extras"); ex.textContent = "";
    res.extras.forEach(function (x) {
      var li = el("li"); var a = el("a", "", x.label + " (" + fmtSize(x.size) + ")"); a.href = safeUrl(x.url);
      li.appendChild(a); ex.appendChild(li);
    });
    $("extras-box").hidden = res.extras.length === 0;
    var key = platform.os === "linux" ? "linux-" + (platform.arch === "arm64" ? "arm64" : "x64") : platform.os;
    if ($("picker").querySelector('option[value="' + key + '"]')) $("picker").value = key;
  }

  function load() {
    var fixture = params.get("release");
    if (!fixture) return fetch(API, { headers: { Accept: "application/vnd.github+json" } })
      .then(function (r) { if (r.status === 404) return null; if (!r.ok) throw new Error(r.status); return r.json(); });
    if (/\.js$/.test(fixture)) return new Promise(function (ok, bad) {
      var s = document.createElement("script"); s.src = fixture;
      s.onload = function () { ok(window.MASON_FIXTURE_RELEASE || null); }; s.onerror = bad;
      document.head.appendChild(s);
    });
    return fetch(fixture).then(function (r) { return r.json(); });
  }

  function start() {
    var uad = navigator.userAgentData;
    var ua = params.get("ua") || navigator.userAgent;
    var base = uad && !params.get("ua") ? { platform: uad.platform } : {};
    var p = uad && uad.getHighEntropyValues && !params.get("ua")
      ? uad.getHighEntropyValues(["architecture"]).then(function (h) { base.architecture = h.architecture; return base; }, function () { return base; })
      : Promise.resolve(base);
    Promise.all([p, load().catch(function () { return undefined; })]).then(function (r) {
      var platform = MasonPick.detectPlatform(ua, r[0]);
      var rel = r[1];
      if (rel === undefined) {
        $("status").innerHTML = "";
        $("status").appendChild(document.createTextNode("Could not reach GitHub. Use the "));
        var a = el("a", "", "releases page"); a.href = "https://github.com/brianm998/mason/releases/latest";
        $("status").appendChild(a); $("status").appendChild(document.createTextNode("."));
        return;
      }
      render(platform, rel);
      $("picker").addEventListener("change", function () {
        var v = this.value;
        render(v.indexOf("linux") === 0 ? { os: "linux", arch: v.slice(6) } : { os: v, arch: "unknown" }, rel);
      });
    });
  }
  start();
})();
