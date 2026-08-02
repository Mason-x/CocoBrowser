"use client";

import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { translateBackendError } from "@/lib/backend-errors";
import { matchProfilePersonaToExit } from "@/lib/geo-persona";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { Button } from "./ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
import { Input } from "./ui/input";

void matchProfilePersonaToExit;

interface KernelAsset {
  id: string;
  version: string;
  platform: string;
  sha256: string;
  size: number;
  sourceStatus: string;
}

interface KernelManifest {
  kernels: KernelAsset[];
}

interface InstalledKernel {
  id: string;
  version: string;
  platform: string;
  executable: string;
}

interface CloakLicenseStatus {
  configured: boolean;
  valid: boolean | null;
  plan: string | null;
  expires: string | null;
  activeSessions: number | null;
  sessionLimit: number | null;
}

interface CloakLatestRelease {
  id: string;
  version: string;
  platform: string;
  sourceStatus: string;
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
  const [latest, setLatest] = useState<CloakLatestRelease | null>(null);
  const [license, setLicense] = useState<CloakLicenseStatus | null>(null);
  const [licenseKey, setLicenseKey] = useState("");
  const [geoip, setGeoip] = useState<GeoIpStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const [installing, setInstalling] = useState<string | null>(null);
  const [savingLicense, setSavingLicense] = useState(false);
  const [updatingGeoip, setUpdatingGeoip] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [kernelManifest, installedKernels, licenseStatus] =
        await Promise.all([
          invoke<KernelManifest>("list_kernel_manifest"),
          invoke<InstalledKernel[]>("list_installed_kernels"),
          invoke<CloakLicenseStatus>("get_cloak_license_status", {
            refresh: false,
          }),
        ]);
      setManifest(kernelManifest);
      setInstalled(installedKernels);
      setLicense(licenseStatus);
      try {
        setLatest(await invoke<CloakLatestRelease>("get_cloak_latest_release"));
      } catch (error) {
        console.error("Failed to resolve CloakBrowser latest release:", error);
        setLatest(null);
      }
    } catch (error) {
      showErrorToast(t("kernels.loadFailed"), {
        description: translateBackendError(t, error),
      });
    } finally {
      setLoading(false);
    }
  }, [t]);

  const refreshGeoip = useCallback(async () => {
    try {
      setGeoip(await invoke<GeoIpStatus>("get_geoip_status"));
    } catch (error) {
      console.error("Failed to load GeoIP status:", error);
    }
  }, []);

  useEffect(() => {
    if (isOpen) {
      void refresh();
      void refreshGeoip();
    }
  }, [isOpen, refresh, refreshGeoip]);

  const isInstalled = (id: string, version: string) =>
    installed.some((kernel) => kernel.id === id && kernel.version === version);

  const installLegacy = async (asset: KernelAsset) => {
    setInstalling(asset.id);
    try {
      await invoke<InstalledKernel>("install_kernel", {
        id: asset.id,
        version: asset.version,
      });
      showSuccessToast(t("kernels.installSuccess", { version: asset.version }));
      await refresh();
    } catch (error) {
      showErrorToast(t("kernels.installFailed"), {
        description: translateBackendError(t, error),
      });
    } finally {
      setInstalling(null);
    }
  };

  const installLatest = async () => {
    setInstalling("cloakbrowser-150");
    try {
      const kernel = await invoke<InstalledKernel>("install_cloak_latest");
      showSuccessToast(
        t("kernels.installSuccess", { version: kernel.version }),
      );
      await refresh();
    } catch (error) {
      showErrorToast(t("kernels.installFailed"), {
        description: translateBackendError(t, error),
      });
    } finally {
      setInstalling(null);
    }
  };

  const saveLicense = async () => {
    setSavingLicense(true);
    try {
      setLicense(
        await invoke<CloakLicenseStatus>("set_cloak_license_key", {
          key: licenseKey,
        }),
      );
      setLicenseKey("");
      showSuccessToast(t("kernels.cloak.licenseSaved"));
    } catch (error) {
      showErrorToast(t("kernels.cloak.licenseSaveFailed"), {
        description: translateBackendError(t, error),
      });
    } finally {
      setSavingLicense(false);
    }
  };

  const clearLicense = async () => {
    setSavingLicense(true);
    try {
      await invoke("clear_cloak_license_key");
      setLicense(null);
      setLicenseKey("");
      await refresh();
      showSuccessToast(t("kernels.cloak.licenseCleared"));
    } catch (error) {
      showErrorToast(t("kernels.cloak.licenseClearFailed"), {
        description: translateBackendError(t, error),
      });
    } finally {
      setSavingLicense(false);
    }
  };

  const validateLicense = async () => {
    setSavingLicense(true);
    try {
      setLicense(
        await invoke<CloakLicenseStatus>("get_cloak_license_status", {
          refresh: true,
        }),
      );
    } catch (error) {
      showErrorToast(t("kernels.cloak.licenseCheckFailed"), {
        description: translateBackendError(t, error),
      });
    } finally {
      setSavingLicense(false);
    }
  };

  const updateGeoip = async () => {
    setUpdatingGeoip(true);
    try {
      await invoke("download_geoip_database");
      showSuccessToast(t("kernels.geoip.updateSuccess"));
      await refreshGeoip();
    } catch (error) {
      showErrorToast(t("kernels.geoip.updateFailed"), {
        description: String(error),
      });
    } finally {
      setUpdatingGeoip(false);
    }
  };

  const legacy = manifest?.kernels.find(
    (asset) => asset.id === "cloakbrowser-146",
  );
  const latestInstalled = latest
    ? isInstalled(latest.id, latest.version)
    : false;
  const geoipBusy = updatingGeoip || geoip?.downloading === true;
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

        <div className="mt-4 space-y-4 overflow-y-auto">
          <div className="rounded-md border border-border p-3 space-y-3">
            <div className="flex items-start justify-between gap-3">
              <div>
                <h3 className="text-sm font-medium">
                  {t("kernels.cloak.licenseTitle")}
                </h3>
                <p className="mt-1 text-xs text-muted-foreground">
                  {t("kernels.cloak.licenseDescription")}
                </p>
              </div>
              <Button
                size="sm"
                variant="outline"
                onClick={() => void openUrl("https://cloakbrowser.dev/free")}
              >
                {t("kernels.cloak.getFreeKey")}
              </Button>
            </div>
            <div className="flex gap-2">
              <Input
                type="password"
                value={licenseKey}
                onChange={(event) => setLicenseKey(event.target.value)}
                placeholder={t("kernels.cloak.licensePlaceholder")}
                autoComplete="off"
              />
              <Button
                disabled={savingLicense || !licenseKey.trim()}
                onClick={() => void saveLicense()}
              >
                {t("common.buttons.save")}
              </Button>
            </div>
            <div className="flex flex-wrap items-center gap-2 text-xs">
              <span
                className={
                  license?.configured ? "text-success" : "text-muted-foreground"
                }
              >
                {license?.configured
                  ? t("kernels.cloak.licenseConfigured")
                  : t("kernels.cloak.licenseMissing")}
              </span>
              {license?.valid !== null && license?.valid !== undefined && (
                <span
                  className={
                    license.valid ? "text-success" : "text-destructive"
                  }
                >
                  {license.valid
                    ? t("kernels.cloak.licenseValid", {
                        plan: license.plan ?? "",
                      })
                    : t("kernels.cloak.licenseInvalid")}
                </span>
              )}
              {license?.activeSessions !== null &&
                license?.activeSessions !== undefined && (
                  <span className="text-muted-foreground">
                    {t("kernels.cloak.sessions", {
                      active: license.activeSessions,
                      limit: license.sessionLimit ?? t("kernels.cloak.unknown"),
                    })}
                  </span>
                )}
              {license?.configured && (
                <>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={savingLicense}
                    onClick={() => void validateLicense()}
                  >
                    {t("kernels.cloak.validate")}
                  </Button>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={savingLicense}
                    onClick={() => void clearLicense()}
                  >
                    {t("common.buttons.clear")}
                  </Button>
                </>
              )}
            </div>
            <p className="text-xs text-warning">
              {t("kernels.cloak.privacyNotice")}
            </p>
          </div>

          <div className="grid gap-3 sm:grid-cols-2">
            <div className="rounded-md border border-border p-3 space-y-2">
              <div>
                <p className="text-sm font-medium">
                  {t("kernels.cloak.latestTitle")}
                </p>
                <p className="mt-1 text-xs text-muted-foreground">
                  {latest
                    ? t("kernels.cloak.versionPlatform", {
                        version: latest.version,
                        platform: latest.platform,
                      })
                    : t("kernels.cloak.latestUnavailable")}
                </p>
                <p className="mt-1 text-xs text-warning">
                  {t("kernels.cloak.oneSession")}
                </p>
                {!license?.configured && (
                  <p className="mt-1 text-xs text-warning">
                    {t("kernels.cloak.keyRequiredBeforeInstall")}
                  </p>
                )}
              </div>
              <Button
                size="sm"
                disabled={
                  !latest ||
                  !license?.configured ||
                  latestInstalled ||
                  installing === "cloakbrowser-150"
                }
                onClick={() => void installLatest()}
              >
                {latestInstalled
                  ? t("kernels.installed")
                  : installing === "cloakbrowser-150"
                    ? t("kernels.installing")
                    : t("kernels.install")}
              </Button>
            </div>

            {legacy && (
              <div className="rounded-md border border-border p-3 space-y-2">
                <div>
                  <p className="text-sm font-medium">
                    {t("kernels.cloak.legacyTitle")}
                  </p>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {t("kernels.cloak.versionPlatform", {
                      version: legacy.version,
                      platform: legacy.platform,
                    })}
                  </p>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {t("kernels.sizeMegabytes", {
                      size: (legacy.size / (1024 * 1024)).toFixed(1),
                    })}
                  </p>
                  <p className="mt-1 break-all text-xs text-muted-foreground">
                    {t("kernels.sha256", { value: legacy.sha256 })}
                  </p>
                </div>
                <Button
                  size="sm"
                  disabled={
                    isInstalled(legacy.id, legacy.version) ||
                    installing === legacy.id
                  }
                  onClick={() => void installLegacy(legacy)}
                >
                  {isInstalled(legacy.id, legacy.version)
                    ? t("kernels.installed")
                    : installing === legacy.id
                      ? t("kernels.installing")
                      : t("kernels.install")}
                </Button>
              </div>
            )}
          </div>

          <div className="rounded-md border border-border p-3 space-y-2">
            <div className="flex items-start justify-between gap-3">
              <div>
                <h3 className="text-sm font-medium">
                  {t("kernels.geoip.title")}
                </h3>
                <p className="mt-1 text-xs text-muted-foreground">
                  {t("kernels.geoip.description")}
                </p>
              </div>
              <Button
                size="sm"
                variant={geoipNeedsUpdate ? "default" : "outline"}
                disabled={geoipBusy}
                onClick={() => void updateGeoip()}
              >
                {geoipBusy
                  ? t("kernels.geoip.updating")
                  : geoipNeedsUpdate
                    ? t("kernels.geoip.updateNow")
                    : t("kernels.geoip.redownload")}
              </Button>
            </div>
            {geoip && (
              <p className="text-xs text-muted-foreground">
                {!geoip.available
                  ? t("kernels.geoip.missing")
                  : geoip.stale
                    ? t("kernels.geoip.stale")
                    : t("kernels.geoip.current")}
                {geoip.lastDownload !== null
                  ? ` · ${t("kernels.geoip.updatedAt", {
                      time: formatTime(geoip.lastDownload),
                    })}`
                  : ""}
              </p>
            )}
          </div>

          {loading && installed.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              {t("common.buttons.loading")}
            </p>
          ) : (
            installed.length > 0 && (
              <div>
                <h3 className="mb-2 text-sm font-medium">
                  {t("kernels.installedList")}
                </h3>
                <ul className="space-y-1 text-xs text-muted-foreground">
                  {installed.map((kernel) => (
                    <li
                      key={`${kernel.id}-${kernel.version}`}
                      className="break-all"
                    >
                      {kernel.id} {kernel.version} — {kernel.executable}
                      {kernel.id === "fingerprint-chromium" && (
                        <> · {t("kernels.cloak.legacyInstalledOnly")}</>
                      )}
                    </li>
                  ))}
                </ul>
              </div>
            )
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
