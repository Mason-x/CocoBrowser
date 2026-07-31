#!/usr/bin/env node
/**
 * Compare what our fingerprint-chromium kernel exposes against a real Chrome.
 *
 * The kernel is an ungoogled-chromium build, and ungoogled removes Google
 * components a genuine Chrome ships. Every removal the page can observe is a
 * contradiction with the `Chrome/<major>` identity the persona claims — Widevine
 * being the obvious one, proprietary codecs and the `chrome.*` surface being the
 * others. None of those are things a fingerprint switch can fix, so the point of
 * this script is to find out how large the gap actually is before deciding what
 * to do about it.
 *
 * It launches both browsers against throwaway profiles on loopback CDP, runs the
 * same probe in each, and prints only the fields that disagree.
 *
 *   node scripts/kernel-trace-diff.mjs --kernel "C:\\path\\to\\chrome.exe"
 *   node scripts/kernel-trace-diff.mjs --kernel <path> --chrome <path> --json out.json
 *
 * Chrome is auto-detected from the usual install locations when --chrome is
 * omitted. This drives real browsers, so run it on a desktop session, not CI.
 */

import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer } from "node:net";

const PROBE = `(async () => {
  const out = {};
  const attempt = async (name, fn) => {
    try { out[name] = await fn(); } catch (e) { out[name] = "threw: " + (e && e.name || e); }
  };

  // The headline check. A browser calling itself Chrome that cannot do Widevine
  // is a contradiction any DRM-capable site can compute for free.
  const eme = async (keySystem) => {
    const config = [{
      initDataTypes: ["cenc"],
      videoCapabilities: [{ contentType: 'video/mp4;codecs="avc1.42E01E"' }],
    }];
    const access = await navigator.requestMediaKeySystemAccess(keySystem, config);
    return access ? "supported" : "no access";
  };
  await attempt("emeWidevine", () => eme("com.widevine.alpha"));
  await attempt("emePlayReady", () => eme("com.microsoft.playready"));
  await attempt("emeClearKey", () => eme("org.w3.clearkey"));

  // Proprietary codecs are compiled in or they are not; ungoogled builds have
  // historically differed here.
  await attempt("codecs", () => {
    const types = [
      'video/mp4; codecs="avc1.42E01E"',
      'video/mp4; codecs="hev1.1.6.L93.B0"',
      'audio/mp4; codecs="mp4a.40.2"',
      'audio/mpeg',
      'video/webm; codecs="vp9"',
      'video/mp4; codecs="av01.0.05M.08"',
    ];
    const r = {};
    for (const t of types) r[t] = !!(window.MediaSource && MediaSource.isTypeSupported(t));
    return r;
  });

  await attempt("mediaCapabilities", async () => {
    if (!navigator.mediaCapabilities) return "absent";
    const info = await navigator.mediaCapabilities.decodingInfo({
      type: "media-source",
      video: { contentType: 'video/mp4; codecs="avc1.42E01E"', width: 1920, height: 1080, bitrate: 3000000, framerate: 30 },
    });
    return { supported: info.supported, smooth: info.smooth, powerEfficient: info.powerEfficient };
  });

  await attempt("chromeObject", () => ({
    chrome: typeof window.chrome,
    runtime: !!(window.chrome && window.chrome.runtime),
    loadTimes: !!(window.chrome && window.chrome.loadTimes),
    csi: !!(window.chrome && window.chrome.csi),
    keys: window.chrome ? Object.keys(window.chrome).sort() : [],
  }));

  await attempt("plugins", () => ({
    length: navigator.plugins.length,
    names: Array.from(navigator.plugins).map((p) => p.name).sort(),
    mimeTypes: navigator.mimeTypes.length,
    pdfViewerEnabled: navigator.pdfViewerEnabled,
  }));

  await attempt("identity", () => ({
    userAgent: navigator.userAgent,
    platform: navigator.platform,
    vendor: navigator.vendor,
    languages: Array.from(navigator.languages),
    webdriver: navigator.webdriver,
    hardwareConcurrency: navigator.hardwareConcurrency,
    deviceMemory: navigator.deviceMemory ?? null,
    maxTouchPoints: navigator.maxTouchPoints,
    pdfViewer: navigator.pdfViewerEnabled,
  }));

  await attempt("clientHints", async () => {
    if (!navigator.userAgentData) return "absent";
    const he = await navigator.userAgentData.getHighEntropyValues([
      "architecture", "bitness", "model", "platformVersion", "fullVersionList", "wow64",
    ]);
    return { brands: navigator.userAgentData.brands, mobile: navigator.userAgentData.mobile, ...he };
  });

  await attempt("webgl", () => {
    const gl = document.createElement("canvas").getContext("webgl");
    if (!gl) return "no context";
    const dbg = gl.getExtension("WEBGL_debug_renderer_info");
    return {
      vendor: gl.getParameter(gl.VENDOR),
      renderer: gl.getParameter(gl.RENDERER),
      version: gl.getParameter(gl.VERSION),
      unmaskedVendor: dbg ? gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL) : null,
      unmaskedRenderer: dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : null,
      extensionCount: (gl.getSupportedExtensions() || []).length,
    };
  });

  await attempt("webgpu", async () => {
    if (!navigator.gpu) return "absent";
    const adapter = await navigator.gpu.requestAdapter();
    if (!adapter) return "no adapter";
    const info = adapter.info || {};
    return { vendor: info.vendor ?? null, architecture: info.architecture ?? null };
  });

  await attempt("screen", () => ({
    width: screen.width, height: screen.height,
    availWidth: screen.availWidth, availHeight: screen.availHeight,
    colorDepth: screen.colorDepth, devicePixelRatio: window.devicePixelRatio,
  }));

  await attempt("voices", () => {
    const list = () => speechSynthesis.getVoices().map((v) => v.name + "|" + v.lang);
    const now = list();
    if (now.length) return { count: now.length, sample: now.slice(0, 5) };
    return new Promise((resolve) => {
      // Voices load asynchronously and the remote ones need a network round
      // trip, so a short wait would report an empty list for a browser that
      // simply had not finished.
      const done = () => { const v = list(); resolve({ count: v.length, sample: v.slice(0, 5) }); };
      speechSynthesis.onvoiceschanged = done;
      setTimeout(done, 5000);
    });
  });

  await attempt("mediaDevices", async () => {
    if (!navigator.mediaDevices) return "absent";
    const devices = await navigator.mediaDevices.enumerateDevices();
    const kinds = {};
    for (const d of devices) kinds[d.kind] = (kinds[d.kind] || 0) + 1;
    return { count: devices.length, kinds };
  });

  await attempt("permissions", async () => {
    if (!navigator.permissions) return "absent";
    const r = {};
    for (const name of ["notifications", "geolocation", "camera", "midi"]) {
      try { r[name] = (await navigator.permissions.query({ name })).state; }
      catch (e) { r[name] = "unsupported"; }
    }
    return { states: r, notificationPermission: Notification.permission };
  });

  await attempt("timing", () => ({
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    offset: new Date().getTimezoneOffset(),
    locale: Intl.DateTimeFormat().resolvedOptions().locale,
  }));

  return out;
})()`;

function parseArgs(argv) {
  const args = {};
  for (let i = 2; i < argv.length; i += 1) {
    const key = argv[i];
    if (!key.startsWith("--")) continue;
    args[key.slice(2)] = argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[++i] : "true";
  }
  return args;
}

const CHROME_CANDIDATES = [
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
  join(process.env.LOCALAPPDATA || "", "Google\\Chrome\\Application\\chrome.exe"),
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/usr/bin/google-chrome",
  "/usr/bin/google-chrome-stable",
];

function findChrome() {
  return CHROME_CANDIDATES.find((p) => p && existsSync(p)) ?? null;
}

function freePort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

async function waitForCdp(port, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (res.ok) return await res.json();
    } catch {
      // Not up yet.
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`CDP never came up on 127.0.0.1:${port}`);
}

async function evaluateInPage(port, expression) {
  const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  const page = targets.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
  if (!page) throw new Error("no page target to evaluate in");

  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("error", () => reject(new Error("CDP socket failed")), { once: true });
  });

  try {
    // about:blank is an opaque origin, which hides the secure-context-only
    // surfaces this probe is about. A real https page is the only way to see
    // them, and it has to finish loading before the probe runs.
    ws.send(JSON.stringify({ id: 1, method: "Page.navigate", params: { url: "https://example.com/" } }));
    await new Promise((r) => setTimeout(r, 3000));

    const result = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("probe timed out")), 30000);
      ws.addEventListener("message", (event) => {
        const msg = JSON.parse(event.data);
        if (msg.id !== 2) return;
        clearTimeout(timer);
        if (msg.error) reject(new Error(JSON.stringify(msg.error)));
        else if (msg.result?.exceptionDetails) reject(new Error(msg.result.exceptionDetails.text));
        else resolve(msg.result.result.value);
      });
      ws.send(JSON.stringify({
        id: 2,
        method: "Runtime.evaluate",
        params: { expression, awaitPromise: true, returnByValue: true },
      }));
    });
    return result;
  } finally {
    ws.close();
  }
}

async function probe(label, executable, extraArgs) {
  const port = await freePort();
  const userDataDir = await mkdtemp(join(tmpdir(), "coco-trace-"));
  const args = [
    `--user-data-dir=${userDataDir}`,
    `--remote-debugging-port=${port}`,
    "--remote-debugging-address=127.0.0.1",
    "--no-first-run",
    "--no-default-browser-check",
    "--hide-crash-restore-bubble",
    ...extraArgs,
    "about:blank",
  ];
  process.stderr.write(`[${label}] launching ${executable}\n`);
  const child = spawn(executable, args, { stdio: "ignore", detached: false });
  try {
    const version = await waitForCdp(port);
    const observed = await evaluateInPage(port, PROBE);
    return { label, executable, cdpVersion: version, observed };
  } finally {
    child.kill();
    await rm(userDataDir, { recursive: true, force: true }).catch(() => {});
  }
}

/** Flatten to `a.b.c` leaves so two runs can be compared key by key. */
function flatten(value, prefix = "", into = {}) {
  if (value === null || typeof value !== "object") {
    into[prefix] = value;
    return into;
  }
  if (Array.isArray(value)) {
    into[prefix] = JSON.stringify(value);
    return into;
  }
  for (const [key, child] of Object.entries(value)) {
    flatten(child, prefix ? `${prefix}.${key}` : key, into);
  }
  return into;
}

/**
 * Fields that differ for reasons unrelated to the build. The persona
 * deliberately moves these, so reporting them would bury the findings that
 * matter under noise the kernel is supposed to produce.
 */
const EXPECTED_TO_DIFFER = [
  /^identity\.userAgent$/,
  /^identity\.(hardwareConcurrency|deviceMemory)$/,
  /^identity\.languages$/,
  /^clientHints\./,
  /^timing\./,
  /^webgl\.(vendor|renderer|unmaskedVendor|unmaskedRenderer)$/,
  /^webgpu\./,
  // Geometry only. Both runs are on one machine, so anything else under screen.
  // differing is the build talking, not the display — colorDepth in particular,
  // which Chromium pins to 24 and which no persona field moves.
  /^screen\.(width|height|availWidth|availHeight|devicePixelRatio)$/,
];

function diff(chrome, kernel) {
  const a = flatten(chrome);
  const b = flatten(kernel);
  const keys = [...new Set([...Object.keys(a), ...Object.keys(b)])].sort();
  const rows = [];
  for (const key of keys) {
    if (JSON.stringify(a[key]) === JSON.stringify(b[key])) continue;
    rows.push({
      key,
      chrome: a[key] === undefined ? "(absent)" : a[key],
      kernel: b[key] === undefined ? "(absent)" : b[key],
      expected: EXPECTED_TO_DIFFER.some((re) => re.test(key)),
    });
  }
  return rows;
}

function report(rows) {
  const real = rows.filter((r) => !r.expected);
  const noise = rows.filter((r) => r.expected);

  const render = (list) => {
    for (const row of list) {
      process.stdout.write(`\n  ${row.key}\n`);
      process.stdout.write(`      chrome: ${JSON.stringify(row.chrome)}\n`);
      process.stdout.write(`      kernel: ${JSON.stringify(row.kernel)}\n`);
    }
  };

  process.stdout.write(`\n=== Contradictions with the claimed Chrome identity (${real.length}) ===\n`);
  if (real.length === 0) process.stdout.write("\n  none — the kernel matches Chrome on every non-persona field.\n");
  else render(real);

  process.stdout.write(`\n=== Differences the persona is supposed to cause (${noise.length}) ===\n`);
  render(noise);
  process.stdout.write("\n");
}

async function main() {
  const args = parseArgs(process.argv);
  const kernel = args.kernel;
  if (!kernel || !existsSync(kernel)) {
    process.stderr.write(
      "Pass the kernel binary: --kernel \"<...>\\fingerprint-chromium\\148.0.7778.215\\chrome.exe\"\n",
    );
    process.exit(2);
  }
  const chrome = args.chrome ?? findChrome();
  if (!chrome || !existsSync(chrome)) {
    process.stderr.write("Could not find Chrome; pass --chrome <path>\n");
    process.exit(2);
  }

  // The kernel is given the same switches a real launch uses, so the comparison
  // describes the browser our users actually get rather than a bare binary.
  const kernelArgs = [
    "--fingerprint=424242",
    "--fingerprint-platform=windows",
    "--fingerprint-brand=Chrome",
    `--fingerprint-brand-version=${args.major ?? "148"}`,
  ];

  const chromeRun = await probe("chrome", chrome, []);
  const kernelRun = await probe("kernel", kernel, kernelArgs);

  const rows = diff(chromeRun.observed, kernelRun.observed);
  report(rows);

  if (args.json) {
    await writeFile(
      args.json,
      JSON.stringify({ chrome: chromeRun, kernel: kernelRun, diff: rows }, null, 2),
    );
    process.stderr.write(`Full observations written to ${args.json}\n`);
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack ?? error}\n`);
  process.exit(1);
});
