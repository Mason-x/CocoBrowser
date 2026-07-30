import type { CloudUser, Entitlements } from "@/types";

/**
 * Local-first build: all desktop capabilities are unlocked without login,
 * subscription, or remote entitlement delivery. Cloud user data is ignored.
 */
export const LOCAL_FULL_ENTITLEMENTS: Entitlements = {
  active: true,
  browserAutomation: true,
  // Cross-OS spoofing remains disabled at the kernel (v0.1) — this flag only
  // removes the paid UI lock so local same-OS identity editing works.
  crossOsFingerprints: true,
  cloudBackup: false,
  teamCollaboration: false,
  profileLimit: 0,
  requestsPerHour: 1000,
};

/**
 * Effective entitlements for the local fingerprint browser product.
 * Always returns full local automation capabilities; no account required.
 */
export function getEntitlements(
  _user?: CloudUser | null | undefined,
): Entitlements {
  return LOCAL_FULL_ENTITLEMENTS;
}
