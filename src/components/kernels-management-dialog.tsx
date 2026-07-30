"use client";

import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { matchProfilePersonaToExit } from "@/lib/geo-persona";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";

// Keep Tauri command wired for the unused-command regression test; full UI
// for match-to-exit lives on the profile identity form (Phase 6).
void matchProfilePersonaToExit;

import { Button } from "./ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";

interface KernelAsset {
  id: string;
  version: string;
  platform: string;
  url: string;
  sha256: string;
  size: number;
  executableCandidates: string[];
  sourceStatus: string;
}

interface KernelManifest {
  schemaVersion: number;
  kernels: KernelAsset[];
}

interface InstalledKernel {
  id: string;
  version: string;
  platform: string;
  installPath: string;
  executable: string;
  sha256: string;
  sourceStatus: string;
  installedAt: number;
}

interface KernelUpdateStatus {
  kernelId: string;
  platform: string;
  installedVersions: string[];
  latestAudited: string | null;
  auditedNotInstalled: string | null;
  latestUpstream: string | null;
  upstreamAheadOfAudited: boolean;
  upstreamUrl: string;
  checkedAt: number;
  error: string | null;
}

interface GeoIpStatus {
  available: boolean;
  stale: boolean;
  lastDownload: number | null;
  sizeBytes: number | null;
  downloading: boolean;
}

interface KernelsManagementDialogProps {
  isOpen: boolean;
  onClose: () => void;
  subPage?: boolean;
}

function formatTime(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}

export function KernelsManagementDialog({
  isOpen,
  onClose,
  subPage = false,
}: KernelsManagementDialogProps) {
  const { t } = useTranslation();
  const [manifest, setManifest] = useState<KernelManifest | null>(null);
  const [installed, setInstalled] = useState<InstalledKernel[]>([]);
  const [updateStatus, setUpdateStatus] = useState<KernelUpdateStatus | null>(
    null,
  );
  const [geoip, setGeoip] = useState<GeoIpStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const [checking, setChecking] = useState(false);
  const [updatingGeoip, setUpdatingGeoip] = useState(false);
  const [installingKey, setInstallingKey] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [m, list] = await Promise.all([
        invoke<KernelManifest>("list_kernel_manifest"),
        invoke<InstalledKernel[]>("list_installed_kernels"),
      ]);
      setManifest(m);
      setInstalled(list);
    } catch (err) {
      showErrorToast(t("kernels.loadFailed"), {
        description: String(err),
      });
    } finally {
      setLoading(false);
    }
  }, [t]);

  const refreshGeoip = useCallback(async () => {
    try {
      setGeoip(await invoke<GeoIpStatus>("get_geoip_status"));
    } catch (err) {
      console.error("Failed to load GeoIP status:", err);
    }
  }, []);

  // `force` skips the six-hour upstream cache; the cached path keeps opening
  // this page cheap and stays within GitHub's unauthenticated rate limit.
  const checkUpdates = useCallback(async (force: boolean) => {
    setChecking(true);
    try {
      setUpdateStatus(
        await invoke<KernelUpdateStatus>("check_kernel_updates_command", {
          kernelId: "fingerprint-chromium",
          force,
        }),
      );
    } catch (err) {
      console.error("Failed to check kernel updates:", err);
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    if (isOpen) {
      void refresh();
      void refreshGeoip();
      void checkUpdates(false);
    }
  }, [isOpen, refresh, refreshGeoip, checkUpdates]);

  const isInstalled = (id: string, version: string) =>
    installed.some((k) => k.id === id && k.version === version);

  const handleInstall = async (asset: KernelAsset) => {
    const key = `${asset.id}@${asset.version}`;
    setInstallingKey(key);
    try {
      await invoke<InstalledKernel>("install_kernel", {
        id: asset.id,
        version: asset.version,
      });
      showSuccessToast(t("kernels.installSuccess", { version: asset.version }));
      await refresh();
      await checkUpdates(false);
    } catch (err) {
      showErrorToast(t("kernels.installFailed"), {
        description: String(err),
      });
    } finally {
      setInstallingKey(null);
    }
  };

  const handleGeoipUpdate = async () => {
    setUpdatingGeoip(true);
    try {
      await invoke("download_geoip_database");
      showSuccessToast(t("kernels.geoip.updateSuccess"));
      await refreshGeoip();
    } catch (err) {
      showErrorToast(t("kernels.geoip.updateFailed"), {
        description: String(err),
      });
    } finally {
      setUpdatingGeoip(false);
    }
  };

  const geoipBusy = updatingGeoip || geoip?.downloading === true;
  // Only call the refresh "update" when something is actually pending. A fresh
  // database can still be re-downloaded on demand, so the button stays enabled
  // — it just stops implying that action is required.
  const geoipNeedsUpdate = geoip ? !geoip.available || geoip.stale : false;

  return (
    <Dialog
      open={isOpen}
      onOpenChange={(open) => !open && onClose()}
      subPage={subPage}
    >
      <DialogContent className="max-w-2xl flex flex-col">
        <DialogHeader>
          <DialogTitle>{t("kernels.title")}</DialogTitle>
          <DialogDescription>{t("kernels.description")}</DialogDescription>
        </DialogHeader>

        <div className="mt-4 space-y-4">
          <p className="text-sm text-muted-foreground">
            {t("kernels.securityNote")}
          </p>

          {/* Kernel update status and GeoIP sit side by side: they are separate
              concerns with different trust models, so they never share a column. */}
          <div className="grid gap-4 sm:grid-cols-2 items-start">
            {/* Kernel updates */}
            <div className="flex h-full flex-col rounded-md border border-border p-3 space-y-2">
              <div className="flex items-center justify-between gap-3">
                <h3 className="text-sm font-medium">
                  {t("kernels.updates.title")}
                </h3>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={checking}
                  onClick={() => void checkUpdates(true)}
                >
                  {checking
                    ? t("kernels.updates.checking")
                    : t("kernels.updates.checkNow")}
                </Button>
              </div>

              {updateStatus && (
                <div className="space-y-1.5 text-xs">
                  {updateStatus.auditedNotInstalled ? (
                    <p className="text-success">
                      {t("kernels.updates.auditedAvailable", {
                        version: updateStatus.auditedNotInstalled,
                      })}
                    </p>
                  ) : (
                    !updateStatus.upstreamAheadOfAudited && (
                      <p className="text-muted-foreground">
                        {t("kernels.updates.upToDate")}
                      </p>
                    )
                  )}

                  {updateStatus.upstreamAheadOfAudited &&
                    updateStatus.latestUpstream && (
                      <div className="space-y-1">
                        <p className="text-warning">
                          {t("kernels.updates.upstreamAhead", {
                            version: updateStatus.latestUpstream,
                          })}
                        </p>
                        <p className="text-muted-foreground">
                          {t("kernels.updates.upstreamAheadHint")}
                        </p>
                        <button
                          type="button"
                          className="text-muted-foreground underline hover:text-foreground"
                          onClick={() => void openUrl(updateStatus.upstreamUrl)}
                        >
                          {t("kernels.updates.viewUpstream")}
                        </button>
                      </div>
                    )}

                  {updateStatus.error ? (
                    <p className="text-muted-foreground">
                      {t("kernels.updates.checkFailed")}
                    </p>
                  ) : (
                    <p className="text-muted-foreground">
                      {t("kernels.updates.lastChecked", {
                        time: formatTime(updateStatus.checkedAt),
                      })}
                    </p>
                  )}
                </div>
              )}
            </div>

            {/* GeoIP */}
            <div className="flex h-full flex-col rounded-md border border-border p-3 space-y-2">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <h3 className="text-sm font-medium">
                    {t("kernels.geoip.title")}
                  </h3>
                  <p className="text-xs text-muted-foreground mt-1">
                    {t("kernels.geoip.description")}
                  </p>
                </div>
                <Button
                  size="sm"
                  variant={geoipNeedsUpdate ? "default" : "outline"}
                  disabled={geoipBusy}
                  onClick={() => void handleGeoipUpdate()}
                >
                  {geoipBusy
                    ? t("kernels.geoip.updating")
                    : geoipNeedsUpdate
                      ? t("kernels.geoip.updateNow")
                      : t("kernels.geoip.redownload")}
                </Button>
              </div>

              {geoip && (
                <div className="text-xs space-y-1">
                  {!geoip.available ? (
                    <p className="text-warning">{t("kernels.geoip.missing")}</p>
                  ) : geoip.stale ? (
                    <p className="text-warning">{t("kernels.geoip.stale")}</p>
                  ) : (
                    <p className="text-success">{t("kernels.geoip.current")}</p>
                  )}
                  {geoip.lastDownload !== null && (
                    <p className="text-muted-foreground">
                      {t("kernels.geoip.updatedAt", {
                        time: formatTime(geoip.lastDownload),
                      })}
                      {geoip.sizeBytes
                        ? ` · ${(geoip.sizeBytes / (1024 * 1024)).toFixed(1)} MB`
                        : ""}
                    </p>
                  )}
                  {!geoipNeedsUpdate && (
                    <p className="text-muted-foreground">
                      {t("kernels.geoip.freshnessNote")}
                    </p>
                  )}
                </div>
              )}
            </div>
          </div>

          {loading && !manifest ? (
            <p className="text-sm text-muted-foreground">
              {t("common.buttons.loading")}
            </p>
          ) : (
            <ul className="space-y-3">
              {(manifest?.kernels ?? []).map((asset) => {
                const key = `${asset.id}@${asset.version}`;
                const installedAlready = isInstalled(asset.id, asset.version);
                const busy = installingKey === key;
                return (
                  <li
                    key={key}
                    className="rounded-md border border-border p-3 flex flex-col gap-2"
                  >
                    <div className="flex items-start justify-between gap-3">
                      <div>
                        <p className="font-medium text-sm">
                          {asset.id} · {asset.version}
                        </p>
                        <p className="text-xs text-muted-foreground mt-1">
                          {asset.platform} ·{" "}
                          {(asset.size / (1024 * 1024)).toFixed(1)} MB
                        </p>
                        <p className="text-xs text-muted-foreground break-all mt-1">
                          SHA-256: {asset.sha256}
                        </p>
                        {asset.sourceStatus === "binary-source-delayed" && (
                          <p className="text-xs text-warning mt-1">
                            {t("kernels.sourceDelayed")}
                          </p>
                        )}
                      </div>
                      <Button
                        size="sm"
                        disabled={busy || installedAlready}
                        onClick={() => void handleInstall(asset)}
                      >
                        {installedAlready
                          ? t("kernels.installed")
                          : busy
                            ? t("kernels.installing")
                            : t("kernels.install")}
                      </Button>
                    </div>
                  </li>
                );
              })}
            </ul>
          )}

          {installed.length > 0 && (
            <div>
              <h3 className="text-sm font-medium mb-2">
                {t("kernels.installedList")}
              </h3>
              <ul className="space-y-1 text-xs text-muted-foreground">
                {installed.map((k) => (
                  <li key={`${k.id}-${k.version}`} className="break-all">
                    {k.id} {k.version} → {k.executable}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
