// SPDX-License-Identifier: GPL-3.0-or-later
// Runs after pick.js and a `var FIXTURE = {...};` prelude, concatenated by run-pick-test.sh, so it
// works under node and JavaScriptCore alike. `MasonPick` is the global pick.js sets.
var failures = 0;
function eq(got, want, what) {
  if (got !== want) { failures++; print("FAIL " + what + ": got " + got + ", want " + want); }
}
var UA = {
  macSafari: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15",
  macChrome: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
  winEdge: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36 Edg/126.0.0.0",
  linuxX64: "Mozilla/5.0 (X11; Linux x86_64; rv:127.0) Gecko/20100101 Firefox/127.0",
  linuxArm: "Mozilla/5.0 (X11; Linux aarch64; rv:127.0) Gecko/20100101 Firefox/127.0",
  linuxBare: "Mozilla/5.0 (X11; Linux; rv:127.0) Gecko/20100101 Firefox/127.0",
  iphone: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
  android: "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36"
};
var d = MasonPick.detectPlatform;
function os(ua, uad) { return d(ua, uad).os; }
eq(os(UA.macSafari), "mac", "mac safari");
eq(os(UA.macChrome), "mac", "mac chrome");
eq(os(UA.winEdge), "windows", "windows edge");
eq(os(UA.linuxX64), "linux", "linux x64");
eq(os(UA.iphone), "ios", "iphone");
eq(os(UA.android), "android", "android is not linux");
eq(d(UA.linuxX64).arch, "x64", "linux x64 arch");
eq(d(UA.linuxArm).arch, "arm64", "linux arm arch");
eq(d(UA.linuxBare).arch, "unknown", "linux bare arch");
eq(d(UA.linuxBare, { platform: "Linux", architecture: "arm" }).arch, "arm64", "uad arm");
eq(d(UA.linuxBare, { architecture: "x86" }).arch, "x64", "uad x86");
eq(d("", null).os, "unknown", "empty ua");

function names(list) { return list.map(function (x) { return x.name; }).join(","); }
var P = MasonPick.pickAssets;
var v = "1.2.3";
var r = P(d(UA.macSafari), FIXTURE);
eq(r.status, "ok", "mac status"); eq(r.gui.name, "MASON-" + v + "-macos.dmg", "mac gui"); eq(r.cli.name, "mason-" + v + "-macos.pkg", "mac cli");
eq(r.version, v, "version");
r = P(d(UA.winEdge), FIXTURE);
eq(r.gui.name, "MASON-Monitor-" + v + "-windows.msi", "win gui"); eq(r.cli.name, "mason-" + v + "-x86_64-windows-setup.msi", "win cli");
eq(names(r.extras), "mason-" + v + "-x86_64-pc-windows-msvc.zip,SHA256SUMS", "win extras");
r = P(d(UA.linuxX64), FIXTURE);
eq(r.gui.name, "mason-monitor_" + v + "_amd64.deb", "linux gui"); eq(r.cli.name, "mason_" + v + "_amd64.deb", "linux cli");
r = P(d(UA.linuxArm), FIXTURE);
eq(r.gui, null, "linux arm has no gui"); eq(r.cli.name, "mason_" + v + "_arm64.deb", "linux arm cli"); eq(r.status, "partial", "linux arm status");
r = P(d(UA.linuxBare), FIXTURE);
eq(r.gui, null, "unknown arch gui"); eq(r.cli, null, "unknown arch cli");
eq(r.extras.length, 6, "unknown arch lists every linux file");
eq(P(d(UA.iphone), FIXTURE).status, "unsupported", "iphone");
eq(P(d(UA.android), FIXTURE).status, "unsupported", "android");
eq(P(d(UA.macSafari), null).status, "none", "no release");

// A release missing a platform's assets is "unavailable", never another system's file.
function without(re) {
  return { tag_name: FIXTURE.tag_name, assets: FIXTURE.assets.filter(function (a) { return !re.test(a.name); }) };
}
r = P(d(UA.macSafari), without(/macos|apple-darwin/));
eq(r.status, "unavailable", "mac missing"); eq(r.gui, null, "mac missing gui"); eq(r.cli, null, "mac missing cli"); eq(r.extras.length, 1, "mac missing keeps only sums");
r = P(d(UA.winEdge), without(/windows/));
eq(r.status, "unavailable", "windows missing"); eq(r.gui, null, "windows missing gui"); eq(r.cli, null, "windows missing cli");
r = P(d(UA.linuxX64), without(/\.deb$|linux/));
eq(r.status, "unavailable", "linux missing"); eq(r.gui, null, "linux missing gui");
r = P(d(UA.linuxX64), without(/^mason-monitor/));
eq(r.status, "partial", "linux without monitor"); eq(r.gui, null, "linux without monitor gui");
r = P(d(UA.linuxArm), without(/arm64\.deb|aarch64-unknown/));
eq(r.status, "unavailable", "linux arm missing");
r = P(d(UA.winEdge), { tag_name: "v1.2.3", assets: [] });
eq(r.status, "unavailable", "empty assets");

if (failures) { print(failures + " failure(s)"); throw new Error("pick tests failed"); }
print("pick tests passed");
