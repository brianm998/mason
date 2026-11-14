// SPDX-License-Identifier: GPL-3.0-or-later
// Pure functions that choose which MASON release assets to offer a visitor. No DOM, no network.
// Asset names are the contract in docs/RELEASING.md. Works in browsers, node and JavaScriptCore.
(function (root) {
  "use strict";

  // userAgentData: optional object with `platform` and `architecture`, e.g. the result of
  // navigator.userAgentData.getHighEntropyValues(['architecture']) merged with its `platform`.
  function detectPlatform(userAgent, userAgentData) {
    var ua = String(userAgent || "");
    var uad = userAgentData || {};
    var uaPlatform = String(uad.platform || "").toLowerCase();
    var os;
    if (/iPhone|iPad|iPod/i.test(ua) || uaPlatform === "ios") os = "ios";
    else if (/Android/i.test(ua) || uaPlatform === "android") os = "android";
    else if (/Windows/i.test(ua) || uaPlatform === "windows") os = "windows";
    else if (/Macintosh|Mac OS X/i.test(ua) || uaPlatform === "macos") os = "mac";
    else if (/Linux|X11|CrOS/i.test(ua) || uaPlatform === "linux") os = "linux";
    else os = "unknown";
    var arch = "unknown";
    var hint = String(uad.architecture || "").toLowerCase();
    if (hint === "arm" || hint === "arm64" || hint === "aarch64") arch = "arm64";
    else if (hint === "x86" || hint === "x64" || hint === "x86_64") arch = "x64";
    else if (/aarch64|arm64|armv8/i.test(ua)) arch = "arm64";
    else if (/x86_64|x64|amd64|Win64|WOW64/i.test(ua)) arch = "x64";
    return { os: os, arch: arch };
  }

  function versionOf(release) {
    return String((release && release.tag_name) || "").replace(/^v/, "");
  }

  function esc(s) { return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"); }

  // Exact names from docs/RELEASING.md for one version.
  function names(v) {
    return {
      tarMacArm: "mason-" + v + "-aarch64-apple-darwin.tar.gz",
      tarMacX64: "mason-" + v + "-x86_64-apple-darwin.tar.gz",
      tarLinuxX64: "mason-" + v + "-x86_64-unknown-linux-gnu.tar.gz",
      tarLinuxArm: "mason-" + v + "-aarch64-unknown-linux-gnu.tar.gz",
      winZip: "mason-" + v + "-x86_64-pc-windows-msvc.zip",
      winCliMsi: "mason-" + v + "-x86_64-windows-setup.msi",
      debX64: "mason_" + v + "_amd64.deb",
      debArm: "mason_" + v + "_arm64.deb",
      macPkg: "mason-" + v + "-macos.pkg",
      macDmg: "MASON-" + v + "-macos.dmg",
      monDeb: "mason-monitor_" + v + "_amd64.deb",
      monWinMsi: "MASON-Monitor-" + v + "-windows.msi",
      sums: "SHA256SUMS"
    };
  }

  // -> {status, version, published, platform, gui, cli, extras}
  // status: "ok" (gui and cli both found), "partial" (one found), "unavailable" (this release has
  // none of the files for this system), "unsupported" (phones, unknown systems), "none" (no release).
  // A missing file is null; the function never substitutes a file for another system.
  function pickAssets(platform, release) {
    var p = platform || { os: "unknown", arch: "unknown" };
    var out = { status: "none", version: "", published: "", platform: p, gui: null, cli: null, extras: [] };
    if (!release || !release.tag_name) return out;
    var v = versionOf(release);
    out.version = v;
    out.published = release.published_at || "";
    var byName = {};
    (release.assets || []).forEach(function (a) { byName[a.name] = a; });
    function find(name, label) {
      var a = byName[name];
      if (!a) return null;
      return { name: a.name, label: label, url: a.browser_download_url, size: a.size || 0 };
    }
    function pushAll(list) {
      list.forEach(function (x) { if (x) out.extras.push(x); });
    }
    var n = names(v);
    if (p.os === "ios" || p.os === "android" || p.os === "unknown") {
      out.status = "unsupported";
      return out;
    }
    var known = p.arch === "x64" || p.arch === "arm64";
    if (p.os === "mac") {
      out.gui = find(n.macDmg, "MASON monitor (macOS .dmg)");
      out.cli = find(n.macPkg, "CLI and daemon (macOS .pkg)");
      if (p.arch !== "x64") pushAll([find(n.tarMacArm, "Apple silicon .tar.gz")]);
      if (p.arch !== "arm64") pushAll([find(n.tarMacX64, "Intel .tar.gz")]);
    } else if (p.os === "windows") {
      out.gui = find(n.monWinMsi, "MASON monitor (Windows .msi)");
      out.cli = find(n.winCliMsi, "CLI and daemon (Windows .msi)");
      pushAll([find(n.winZip, "CLI and daemon (.zip)")]);
    } else if (p.os === "linux") {
      // The Compose monitor is published for amd64 only; on arm64 there is no GUI file.
      if (p.arch === "x64") {
        out.gui = find(n.monDeb, "MASON monitor (Linux amd64 .deb)");
        out.cli = find(n.debX64, "CLI and daemon (Linux amd64 .deb)");
        pushAll([find(n.tarLinuxX64, "Linux x86_64 .tar.gz")]);
      } else if (p.arch === "arm64") {
        out.cli = find(n.debArm, "CLI and daemon (Linux arm64 .deb)");
        pushAll([find(n.tarLinuxArm, "Linux aarch64 .tar.gz")]);
      } else {
        // Architecture unknown: offer no guess as the main download, list every Linux file.
        pushAll([find(n.debX64, "CLI and daemon (Linux amd64 .deb)"),
          find(n.debArm, "CLI and daemon (Linux arm64 .deb)"),
          find(n.monDeb, "MASON monitor (Linux amd64 .deb)"),
          find(n.tarLinuxX64, "Linux x86_64 .tar.gz"),
          find(n.tarLinuxArm, "Linux aarch64 .tar.gz")]);
      }
    }
    pushAll([find(n.sums, "SHA256SUMS")]);
    var found = (out.gui ? 1 : 0) + (out.cli ? 1 : 0);
    var mainExtras = out.extras.filter(function (x) { return x.name !== n.sums; }).length;
    if (found === 2) out.status = "ok";
    else if (found === 1) out.status = "partial";
    else if (mainExtras > 0 && !known && p.os === "linux") out.status = "partial";
    else out.status = "unavailable";
    return out;
  }

  var api = { detectPlatform: detectPlatform, pickAssets: pickAssets, assetNames: names };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.MasonPick = api;
})(typeof globalThis !== "undefined" ? globalThis : this);
