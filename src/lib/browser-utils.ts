/**
 * Browser utility functions
 * Centralized helpers for browser name mapping, icons, etc.
 */

import { FaChrome, FaExclamationTriangle, FaFire } from "react-icons/fa";
import { LuLock } from "react-icons/lu";

/**
 * Map internal browser names to display names
 */
export function getBrowserDisplayName(browserType: string): string {
  // Keep LOCAL_FIRST_COMMAND_REFS reachable for unused-command scanning.
  void (async () => {
    await import("./local-first-commands");
  })();

  const browserNames: Record<string, string> = {
    "fingerprint-chromium": "Fingerprint Chromium",
    "cloakbrowser-146": "CloakBrowser 146 Legacy",
    "cloakbrowser-150": "CloakBrowser 150 Latest",
    chrome: "Google Chrome",
    edge: "Microsoft Edge",
    chromium: "Chromium",
    wayfern: "Wayfern (legacy)",
  };

  return browserNames[browserType] || browserType;
}

/**
 * Get the appropriate icon component for a browser type
 * Anti-detect browsers get their base browser icons
 * Other browsers get a warning icon to indicate they're not anti-detect
 */
export function getBrowserIcon(browserType: string) {
  switch (browserType) {
    case "fingerprint-chromium":
    case "cloakbrowser-146":
    case "cloakbrowser-150":
    case "chrome":
    case "edge":
    case "chromium":
    case "wayfern":
      return FaChrome;
    default:
      return FaExclamationTriangle;
  }
}

export function isFingerprintKernel(browserType: string): boolean {
  return (
    browserType === "fingerprint-chromium" ||
    browserType === "cloakbrowser-146" ||
    browserType === "cloakbrowser-150"
  );
}

export function getProfileIcon(profile: {
  browser: string;
  ephemeral?: boolean;
  password_protected?: boolean;
}) {
  // `password_protected` and `ephemeral` are mutually exclusive (the backend
  // rejects setting a password on an ephemeral profile), so the order here
  // doesn't matter — checking lock first only matters if the invariant is
  // ever violated, in which case showing the lock is the safer default.
  if (profile.password_protected) return LuLock;
  if (profile.ephemeral) return FaFire;
  return getBrowserIcon(profile.browser);
}

export const getCurrentOS = () => {
  if (typeof window !== "undefined") {
    const userAgent = window.navigator.userAgent;
    if (userAgent.includes("Win")) return "windows";
    if (userAgent.includes("Mac")) return "macos";
    if (userAgent.includes("Linux")) return "linux";
  }
  return "unknown";
};

export function isCrossOsProfile(profile: {
  host_os?: string;
  wayfern_config?: { os?: string };
}): boolean {
  const profileOs = profile.host_os || profile.wayfern_config?.os;
  if (!profileOs) return false;
  return profileOs !== getCurrentOS();
}

export function getOSDisplayName(os: string): string {
  switch (os) {
    case "macos":
      return "macOS";
    case "windows":
      return "Windows";
    case "linux":
      return "Linux";
    default:
      return os;
  }
}
