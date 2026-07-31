import type { TFunction } from "i18next";
import type { BrowserProfile } from "@/types";

/**
 * Build the local landing page a profile opens on.
 *
 * The page reports what a site actually sees from inside the browser and marks
 * anything that disagrees with the persona we configured. A launch whose
 * timezone or language silently differs from its own configuration is the
 * failure worth catching, and only the browser can report the observed side.
 *
 * Built here rather than in Rust so every label comes from the locale files.
 * Values are injected as a JSON blob and rendered with textContent, never
 * interpolated into markup — a profile name is user input and this page runs in
 * a real browser.
 */
/**
 * Exit-IP lookup endpoint.
 *
 * It has to be called from inside the browser — that is the only way the answer
 * describes the profile's exit rather than the app's own network — which means it
 * must send `Access-Control-Allow-Origin`. The page is loaded from `file://`, so
 * its origin is `null` and anything without CORS is refused before it is sent.
 * ip2location.io sends no CORS headers at all and does not support JSONP, so it
 * cannot be used here however the request is shaped.
 *
 * ipwho.is is keyless, sends `ACAO: *`, and additionally returns the IANA zone id
 * (`America/Los_Angeles`) rather than only a UTC offset, which is what the
 * timezone comparison needs.
 */
const DEFAULT_LOOKUP_URL = "https://ipwho.is/";

export interface WorkbenchOptions {
  /** Override the lookup endpoint. Must be CORS-enabled and ipwho.is-shaped. */
  lookupUrl?: string | null;
  /** Probe a few well-known sites to prove the proxy actually carries traffic. */
  reachability?: boolean;
  /** Display name of the profile's group, resolved by the caller. */
  groupName?: string;
}

/**
 * Reachability targets. Each is fetched `no-cors` for a small static asset, so
 * the check answers "did the connection succeed" without loading a page, running
 * their scripts, or reading a response we are not allowed to read anyway.
 */
const REACHABILITY_TARGETS: { name: string; url: string }[] = [
  { name: "Google", url: "https://www.google.com/generate_204" },
  { name: "YouTube", url: "https://www.youtube.com/favicon.ico" },
  { name: "Facebook", url: "https://www.facebook.com/favicon.ico" },
  { name: "TikTok", url: "https://www.tiktok.com/favicon.ico" },
  { name: "X", url: "https://x.com/favicon.ico" },
];

export interface WorkbenchFiles {
  html: string;
  js: string;
}

export function buildWorkbenchPage(
  profile: BrowserProfile,
  t: TFunction,
  options: WorkbenchOptions = {},
): { html: string; js: string } {
  const groupName = options.groupName;
  const persona = profile.persona;
  const lookupUrl = options.lookupUrl?.trim() || DEFAULT_LOOKUP_URL;

  const data = {
    // Regenerated per launch, so the page can tell a fresh launch from another
    // tab opened during the same one.
    launchId: `${Date.now()}-${Math.random().toString(36).slice(2)}`,
    profileName: profile.name,
    profile: {
      group: profile.group_id ? (groupName ?? "") : "",
      note: profile.note ?? "",
      kernel: [profile.browser, profile.version].filter(Boolean).join(" · "),
    },
    lookupUrl,
    targets: options.reachability === false ? [] : REACHABILITY_TARGETS,
    // Fallback only. These are the persona as it stands before the launch runs,
    // and the geo gate rewrites the timezone and language of a persona that
    // follows its exit — so the launch overwrites this with `expected.json`, and
    // the page prefers that. Comparison is unconditional either way: a field
    // that follows the exit still has one correct value once the gate has picked
    // it, and "did `--timezone` actually take effect" is the question this page
    // exists to answer. Gating the comparison on the follow flags turned it off
    // for the timezone in every default profile.
    expected: {
      timezone: persona?.timezone ?? "",
      language: persona?.language ?? "",
    },
    labels: {
      title: t("workbench.title"),
      subtitle: t("workbench.subtitle"),
      exitIp: t("workbench.exitIp"),
      location: t("workbench.location"),
      isp: t("workbench.isp"),
      postcode: t("workbench.postcode"),
      exitTimezone: t("workbench.exitTimezone"),
      exitMismatch: t("workbench.exitMismatch"),
      environment: t("workbench.environment"),
      timezone: t("workbench.timezone"),
      language: t("workbench.language"),
      userAgent: t("workbench.userAgent"),
      platform: t("workbench.platform"),
      screen: t("workbench.screen"),
      cores: t("workbench.cores"),
      memory: t("workbench.memory"),
      webgl: t("workbench.webgl"),
      loading: t("workbench.loading"),
      lookupFailed: t("workbench.lookupFailed"),
      mismatch: t("workbench.mismatch"),
      expectedValue: t("workbench.expectedValue"),
      unknown: t("workbench.unknown"),
      notCheckedYet: t("workbench.notCheckedYet"),
      refresh: t("workbench.refresh"),
      profile: t("workbench.profile"),
      profileName: t("workbench.profileName"),
      profileGroup: t("workbench.profileGroup"),
      profileNote: t("workbench.profileNote"),
      profileKernel: t("workbench.profileKernel"),
      reachable: t("workbench.reachable"),
      unreachable: t("workbench.unreachable"),
      checking: t("workbench.checking"),
    },
  };

  const html = `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeHtml(data.labels.title)}</title>
<style>
  :root { color-scheme: light dark; }
  body {
    margin: 0; padding: 32px 20px;
    font: 14px/1.6 system-ui, -apple-system, "Segoe UI", sans-serif;
    background: #f6f7f9; color: #16181d;
  }
  @media (prefers-color-scheme: dark) {
    body { background: #16181d; color: #e6e8ec; }
    .card { background: #1e2128 !important; border-color: #2b2f38 !important; }
    dt { color: #9aa2b1 !important; }
    .ip { background: #14301f !important; color: #7ee2a8 !important; }
    .geo b { color: #e6e8ec !important; }
    .chip { border-color: #2b2f38 !important; }
  }
  .wrap { max-width: 1080px; margin: 0 auto; }
  .cols { display: grid; grid-template-columns: minmax(0, 2fr) minmax(0, 1fr); gap: 16px; align-items: start; }
  @media (max-width: 860px) { .cols { grid-template-columns: minmax(0, 1fr); } }
  h1 { font-size: 18px; margin: 0 0 4px; font-weight: 600; }
  .sub { color: #6b7280; margin: 0 0 20px; font-size: 13px; }
  .card {
    background: #fff; border: 1px solid #e3e6ea; border-radius: 12px;
    padding: 20px; margin-bottom: 16px;
  }
  .ip-row { display: flex; align-items: center; justify-content: center; gap: 10px; }
  .ip {
    background: #e8f7ee; color: #10783f;
    border-radius: 8px; padding: 6px 14px;
    font: 600 22px/1.3 ui-monospace, SFMono-Regular, Menlo, monospace;
    word-break: break-all;
  }
  .geo { display: flex; flex-wrap: wrap; justify-content: center; gap: 6px 24px; margin-top: 14px; color: #6b7280; font-size: 13px; }
  .geo b { color: #16181d; font-weight: 500; }
  dl { display: grid; grid-template-columns: minmax(88px, auto) 1fr; gap: 8px 16px; margin: 16px 0 0; }
  dt { color: #6b7280; font-size: 13px; }
  dd { margin: 0; word-break: break-word; }
  .mono { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 12.5px; }
  .bad { color: #c0392b; font-weight: 600; }
  .reach { display: flex; flex-wrap: wrap; justify-content: center; gap: 8px; margin-top: 16px; }
  .chip {
    display: inline-flex; align-items: center; gap: 6px;
    border: 1px solid #e3e6ea; border-radius: 999px; padding: 4px 12px; font-size: 13px;
  }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: #c9ced6; flex: none; }
  .dot.ok { background: #16a34a; }
  .dot.no { background: #c0392b; }
  .note { color: #c0392b; font-size: 12px; display: block; }
  button {
    font: inherit; cursor: pointer; border: 1px solid #d0d5dd;
    background: transparent; color: inherit; border-radius: 8px; padding: 4px 12px;
  }
  .head { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; }
</style>
</head>
<body>
<div class="wrap">
  <div class="head">
    <div>
      <h1 id="l-title"></h1>
      <p class="sub" id="l-sub"></p>
    </div>
    <button id="refresh" type="button"></button>
  </div>

  <div class="cols">
    <div>
      <div class="card">
        <div class="ip-row"><span class="ip" id="ip"></span></div>
        <div class="reach" id="reach"></div>
        <div class="geo">
          <span><span id="l-loc"></span>: <b id="loc"></b></span>
          <span><span id="l-post"></span>: <b id="post"></b></span>
          <span><span id="l-isp"></span>: <b id="isp"></b></span>
          <span><span id="l-proxy"></span>: <b id="proxy"></b></span>
        </div>
      </div>

      <div class="card">
        <strong id="l-env"></strong>
        <dl>
          <dt id="l-tz"></dt><dd id="tz"></dd>
          <dt id="l-lang"></dt><dd id="lang"></dd>
          <dt id="l-plat"></dt><dd id="plat"></dd>
          <dt id="l-screen"></dt><dd id="screen"></dd>
          <dt id="l-cores"></dt><dd id="cores"></dd>
          <dt id="l-mem"></dt><dd id="mem"></dd>
          <dt id="l-webgl"></dt><dd class="mono" id="webgl"></dd>
          <dt id="l-ua"></dt><dd class="mono" id="ua"></dd>
        </dl>
      </div>
    </div>

    <div class="card">
      <strong id="l-profile"></strong>
      <dl>
        <dt id="l-pname"></dt><dd id="pname"></dd>
        <dt id="l-pgroup"></dt><dd id="pgroup"></dd>
        <dt id="l-pnote"></dt><dd id="pnote"></dd>
        <dt id="l-pkernel"></dt><dd id="pkernel"></dd>
      </dl>
    </div>
  </div>
</div>
<script id="coco-data" type="application/json">${escapeJsonForScript(
    JSON.stringify(data),
  )}</script>
<script src="workbench.js"></script>
</body>
</html>`;

  const js = `(() => {
  const D = JSON.parse(document.getElementById("coco-data").textContent);
  const L = D.labels;
  const set = (id, text) => { document.getElementById(id).textContent = text; };

  set("l-title", L.title);
  set("l-sub", D.profileName ? D.profileName + " · " + L.subtitle : L.subtitle);
  set("refresh", L.refresh);
  set("l-loc", L.location); set("l-post", L.postcode);
  set("l-isp", L.isp); set("l-proxy", L.exitTimezone);
  set("l-env", L.environment);
  set("l-profile", L.profile);
  set("l-pname", L.profileName); set("pname", D.profileName || L.unknown);
  set("l-pgroup", L.profileGroup); set("pgroup", D.profile.group || "—");
  set("l-pnote", L.profileNote); set("pnote", D.profile.note || "—");
  set("l-pkernel", L.profileKernel); set("pkernel", D.profile.kernel || L.unknown);
  set("l-tz", L.timezone); set("l-lang", L.language);
  set("l-plat", L.platform); set("l-screen", L.screen);
  set("l-cores", L.cores); set("l-mem", L.memory);
  set("l-webgl", L.webgl); set("l-ua", L.userAgent);

  /**
   * Replace a field's value and drop everything the last render put on it.
   * Setting textContent alone left the "bad" class and any note elements in
   * place, so re-checking stacked a second copy of every mismatch onto the first
   * and a field that had recovered stayed red.
   */
  function reset(id) {
    const dd = document.getElementById(id);
    dd.classList.remove("bad");
    dd.textContent = "";
    return dd;
  }

  function annotate(dd, text, extraClass) {
    dd.classList.add("bad");
    const note = document.createElement("span");
    note.className = extraClass ? "note " + extraClass : "note";
    note.textContent = text;
    dd.appendChild(note);
  }

  /** Show a value and, when it contradicts the persona, say so. */
  function setChecked(id, observed, expected) {
    const dd = reset(id);
    dd.textContent = observed || L.unknown;
    if (expected && observed && observed !== expected) {
      annotate(dd, L.mismatch + " — " + L.expectedValue + ": " + expected);
    }
  }

  /**
   * What the launch settled on, written by the backend after the geo gate. The
   * values compiled into this page predate that step, so they are the fallback
   * rather than the answer.
   */
  async function loadExpected() {
    try {
      const res = await fetch("expected.json", { cache: "no-store" });
      const j = await res.json();
      if (j && typeof j.timezone === "string" && j.timezone) D.expected.timezone = j.timezone;
      if (j && typeof j.language === "string" && j.language) D.expected.language = j.language;
    } catch (_) {}
  }

  function readEnvironment() {
    let tz = "";
    try { tz = Intl.DateTimeFormat().resolvedOptions().timeZone || ""; } catch (_) {}
    setChecked("tz", tz, D.expected.timezone);
    setChecked("lang", navigator.language || "", D.expected.language);

    set("plat", navigator.platform || L.unknown);
    set("screen", screen.width + " x " + screen.height + " @" + (window.devicePixelRatio || 1) + "x");
    set("cores", String(navigator.hardwareConcurrency || L.unknown));
    set("mem", navigator.deviceMemory ? navigator.deviceMemory + " GB" : L.unknown);
    set("ua", navigator.userAgent || L.unknown);

    let webgl = L.unknown;
    try {
      const gl = document.createElement("canvas").getContext("webgl");
      const ext = gl && gl.getExtension("WEBGL_debug_renderer_info");
      if (gl && ext) {
        webgl = gl.getParameter(ext.UNMASKED_VENDOR_WEBGL) + " / " +
                gl.getParameter(ext.UNMASKED_RENDERER_WEBGL);
      }
    } catch (_) {}
    set("webgl", webgl);
  }

  const CACHE_KEY = "coco.workbench.last";
  const LAUNCH_KEY = "coco.workbench.launch";

  function renderExit(j) {
    set("ip", j.ip || L.unknown);
    set("loc", [j.country, j.region, j.city].filter(Boolean).join(" · ") || L.unknown);
    set("post", j.postal || L.unknown);
    const conn = j.connection || {};
    set("isp", [conn.isp || conn.org, conn.asn ? "AS" + conn.asn : ""]
      .filter(Boolean).join(" · ") || L.unknown);
    const exitTz = (j.timezone && j.timezone.id) || "";
    set("proxy", exitTz || L.unknown);
    // This runs after readEnvironment has already rendered the timezone, so it
    // has to clear its own previous note rather than rely on that reset — a
    // cached result rendered by restore() arrives without one.
    const dd = document.getElementById("tz");
    for (const stale of Array.from(dd.querySelectorAll(".exit-note"))) stale.remove();
    if (exitTz) {
      let browserTz = "";
      try { browserTz = Intl.DateTimeFormat().resolvedOptions().timeZone || ""; } catch (_) {}
      if (browserTz && browserTz !== exitTz) {
        annotate(dd, L.exitMismatch + " — " + exitTz, "exit-note");
      }
    }
  }

  function renderReach(results) {
    const box = document.getElementById("reach");
    box.textContent = "";
    for (const r of results) {
      const chip = document.createElement("span");
      chip.className = "chip";
      const dot = document.createElement("span");
      dot.className = "dot " + (r.ok ? "ok" : "no");
      const label = document.createElement("span");
      label.textContent = r.name + " · " + (r.ok ? L.reachable : L.unreachable);
      chip.append(dot, label);
      box.appendChild(chip);
    }
  }

  /** Last result, so a new tab can show something without probing again. */
  function restore() {
    let cached = null;
    try { cached = JSON.parse(localStorage.getItem(CACHE_KEY) || "null"); } catch (_) {}
    if (!cached) {
      set("ip", L.notCheckedYet);
      return;
    }
    if (cached.exit) renderExit(cached.exit);
    if (cached.reach) renderReach(cached.reach);
  }

  function remember(patch) {
    try {
      let cached = {};
      try { cached = JSON.parse(localStorage.getItem(CACHE_KEY) || "{}") || {}; } catch (_) {}
      localStorage.setItem(CACHE_KEY, JSON.stringify(Object.assign(cached, patch)));
    } catch (_) {}
  }

  async function readExit() {
    set("ip", L.loading);
    set("loc", ""); set("post", ""); set("isp", ""); set("proxy", "");
    try {
      // No ip parameter: the service reports the caller's address, which through
      // the profile's proxy is the exit a site would see.
      const res = await fetch(D.lookupUrl, { cache: "no-store" });
      const j = await res.json();
      renderExit(j);
      remember({ exit: j });
    } catch (_) {
      set("ip", L.lookupFailed);
    }
  }

  /**
   * Reachability, not identity: an opaque no-cors fetch tells us whether the
   * connection succeeded, which is exactly the question ("does the proxy carry
   * traffic to this host"). Nothing about the response is or can be read.
   */
  async function readReachability() {
    if (!D.targets.length) return;
    const box = document.getElementById("reach");
    box.textContent = "";
    const chips = D.targets.map((target) => {
      const chip = document.createElement("span");
      chip.className = "chip";
      const dot = document.createElement("span");
      dot.className = "dot";
      const label = document.createElement("span");
      label.textContent = target.name + " · " + L.checking;
      chip.append(dot, label);
      box.appendChild(chip);
      return { target, dot, label };
    });

    const results = [];
    await Promise.all(
      chips.map(async ({ target, dot, label }) => {
        const ctl = new AbortController();
        const timer = setTimeout(() => ctl.abort(), 8000);
        try {
          await fetch(target.url, {
            mode: "no-cors",
            cache: "no-store",
            signal: ctl.signal,
            redirect: "follow",
          });
          dot.classList.add("ok");
          label.textContent = target.name + " · " + L.reachable;
          results.push({ name: target.name, ok: true });
        } catch (_) {
          dot.classList.add("no");
          label.textContent = target.name + " · " + L.unreachable;
          results.push({ name: target.name, ok: false });
        } finally {
          clearTimeout(timer);
        }
      }),
    );
    remember({ reach: results });
  }

  document.getElementById("refresh").addEventListener("click", () => {
    void (async () => {
      await loadExpected();
      readEnvironment();
      void readExit();
      void readReachability();
    })();
  });

  void (async () => {
    await loadExpected();
    readEnvironment();
    // Probe once per launch. Every other new tab would otherwise repeat one IP
    // lookup and five platform requests, which is traffic the user never asked
    // for and a pattern worth avoiding.
    let seen = null;
    try { seen = localStorage.getItem(LAUNCH_KEY); } catch (_) {}
    if (seen !== D.launchId) {
      try { localStorage.setItem(LAUNCH_KEY, D.launchId); } catch (_) {}
      void readExit();
      void readReachability();
    } else {
      restore();
    }
  })();
})();
`;

  return { html, js };
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * `</script>` inside JSON would close the block early, and `<!--` starts an HTML
 * comment inside it. Both are reachable through a profile name.
 */
function escapeJsonForScript(json: string): string {
  return json.replace(/</g, "\\u003c").replace(/>/g, "\\u003e");
}
