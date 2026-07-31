"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import type { SyncSettings } from "@/types";

interface KernelUpdateStatus {
  installedVersions: string[];
  auditedNotInstalled: string | null;
  latestAudited: string | null;
}

interface GeoIpStatus {
  available: boolean;
  stale: boolean;
  downloading: boolean;
}

export interface HeaderStatus {
  /// A newer audited kernel exists that is not installed here.
  kernelUpdate: string | null;
  kernelInstalled: string | null;
  geoipStale: boolean;
  geoipMissing: boolean;
  geoipDownloading: boolean;
  syncConfigured: boolean;
  syncBusy: boolean;
  refresh: () => void;
}

// The kernel check is served from a six-hour cache unless forced, so polling it
// is cheap and stays inside GitHub's unauthenticated rate limit. Half that
// interval keeps the header no more than one cache period behind.
const POLL_MS = 3 * 60 * 60 * 1000;

export function useHeaderStatus(): HeaderStatus {
  const [kernel, setKernel] = useState<KernelUpdateStatus | null>(null);
  const [geoip, setGeoip] = useState<GeoIpStatus | null>(null);
  const [syncConfigured, setSyncConfigured] = useState(false);
  const [syncingProfiles, setSyncingProfiles] = useState<Set<string>>(
    () => new Set(),
  );

  const refresh = useCallback(() => {
    void (async () => {
      try {
        setKernel(
          await invoke<KernelUpdateStatus>("check_kernel_updates_command", {
            kernelId: "fingerprint-chromium",
            force: false,
          }),
        );
      } catch (error) {
        console.error("Failed to check kernel updates:", error);
      }
      try {
        setGeoip(await invoke<GeoIpStatus>("get_geoip_status"));
      } catch (error) {
        console.error("Failed to read GeoIP status:", error);
      }
      try {
        // Same two-field test page.tsx uses: a URL without a token cannot
        // actually reach the server, so it does not count as configured.
        const settings = await invoke<SyncSettings>("get_sync_settings");
        setSyncConfigured(
          Boolean(settings.sync_server_url && settings.sync_token),
        );
      } catch (error) {
        console.error("Failed to read sync configuration:", error);
        setSyncConfigured(false);
      }
    })();
  }, []);

  useEffect(() => {
    refresh();
    const timer = window.setInterval(refresh, POLL_MS);
    return () => {
      window.clearInterval(timer);
    };
  }, [refresh]);

  // Sync settings can change while the app is open; re-read on the same event
  // the settings dialog emits after a successful save.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void (async () => {
      unlisten = await listen("sync-settings-changed", () => {
        refresh();
      });
    })();
    return () => {
      unlisten?.();
    };
  }, [refresh]);

  // Track which profiles are mid-sync so the header can show one aggregate
  // spinner. Terminal statuses (synced/error/disabled) clear the entry.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void (async () => {
      unlisten = await listen<{ profile_id: string; status: string }>(
        "profile-sync-status",
        (event) => {
          const { profile_id, status } = event.payload;
          setSyncingProfiles((prev) => {
            const isBusy = status === "syncing";
            if (isBusy === prev.has(profile_id)) return prev;
            const next = new Set(prev);
            if (isBusy) {
              next.add(profile_id);
            } else {
              next.delete(profile_id);
            }
            return next;
          });
        },
      );
    })();
    return () => {
      unlisten?.();
    };
  }, []);

  return {
    kernelUpdate: kernel?.auditedNotInstalled ?? null,
    kernelInstalled: kernel?.installedVersions?.[0] ?? null,
    geoipStale: geoip ? geoip.stale : false,
    geoipMissing: geoip ? !geoip.available : false,
    geoipDownloading: geoip?.downloading ?? false,
    syncConfigured,
    syncBusy: syncingProfiles.size > 0,
    refresh,
  };
}
