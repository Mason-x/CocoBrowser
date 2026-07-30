"use client";

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { matchProfilePersonaToExit } from "@/lib/geo-persona";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import type { BrowserProfile, FingerprintPersona } from "@/types";
import { LoadingButton } from "./loading-button";
import { Button } from "./ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
import { Input } from "./ui/input";
import { Label } from "./ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";

const CORE_OPTIONS = [2, 4, 6, 8, 12, 16, 24, 32] as const;

interface FingerprintIdentityDialogProps {
  isOpen: boolean;
  onClose: () => void;
  profile: BrowserProfile | null;
  isRunning?: boolean;
  onSaved?: (profile: BrowserProfile) => void;
}

function normalizePersona(p: FingerprintPersona): FingerprintPersona {
  return {
    ...p,
    acceptLanguages: p.acceptLanguages?.length
      ? p.acceptLanguages
      : [p.language],
    spoofingDisabled: p.spoofingDisabled ?? [],
  };
}

export function FingerprintIdentityDialog({
  isOpen,
  onClose,
  profile,
  isRunning = false,
  onSaved,
}: FingerprintIdentityDialogProps) {
  const { t } = useTranslation();
  const [persona, setPersona] = useState<FingerprintPersona | null>(null);
  const [mode, setMode] = useState<"auto" | "advanced">("advanced");
  const [saving, setSaving] = useState(false);
  const [matching, setMatching] = useState(false);
  const [regenerating, setRegenerating] = useState(false);

  useEffect(() => {
    if (profile?.persona) {
      setPersona(normalizePersona(profile.persona));
    } else {
      setPersona(null);
    }
  }, [profile]);

  if (profile?.browser !== "fingerprint-chromium") {
    return null;
  }

  const update = <K extends keyof FingerprintPersona>(
    key: K,
    value: FingerprintPersona[K],
  ) => {
    setPersona((prev) => (prev ? { ...prev, [key]: value } : prev));
  };

  const handleSave = async () => {
    if (!persona || isRunning) return;
    setSaving(true);
    try {
      const updated = await invoke<BrowserProfile>("update_profile_persona", {
        profileId: profile.id,
        persona,
      });
      showSuccessToast(t("identity.saved"));
      onSaved?.(updated);
      onClose();
    } catch (err) {
      showErrorToast(t("identity.saveFailed"), {
        description: String(err),
      });
    } finally {
      setSaving(false);
    }
  };

  const handleRegenerate = async () => {
    if (isRunning) return;
    const ok = window.confirm(t("identity.regenerateConfirm"));
    if (!ok) return;
    setRegenerating(true);
    try {
      const updated = await invoke<BrowserProfile>(
        "regenerate_profile_persona",
        { profileId: profile.id },
      );
      if (updated.persona) {
        setPersona(normalizePersona(updated.persona));
      }
      showSuccessToast(t("identity.regenerated"));
      onSaved?.(updated);
    } catch (err) {
      showErrorToast(t("identity.regenerateFailed"), {
        description: String(err),
      });
    } finally {
      setRegenerating(false);
    }
  };

  const handleMatchExit = async () => {
    if (isRunning) return;
    setMatching(true);
    try {
      const updated = await matchProfilePersonaToExit(profile.id);
      if (updated.persona) {
        setPersona(normalizePersona(updated.persona));
      }
      showSuccessToast(t("identity.matchedExit"));
      onSaved?.(updated);
    } catch (err) {
      showErrorToast(t("identity.matchExitFailed"), {
        description: String(err),
      });
    } finally {
      setMatching(false);
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="flex max-h-[85vh] max-w-lg flex-col overflow-hidden">
        <DialogHeader>
          <DialogTitle>{t("identity.title")}</DialogTitle>
          <DialogDescription>
            {t("identity.description", { name: profile.name })}
          </DialogDescription>
        </DialogHeader>

        {!persona ? (
          <p className="text-sm text-muted-foreground py-4">
            {t("identity.missingPersona")}
          </p>
        ) : (
          <div className="space-y-4 overflow-y-auto py-2 pr-1">
            {isRunning && (
              <p className="text-sm text-warning">
                {t("identity.runningWarning")}
              </p>
            )}

            <div className="flex gap-2">
              <Button
                type="button"
                size="sm"
                variant={mode === "auto" ? "default" : "outline"}
                onClick={() => setMode("auto")}
              >
                {t("identity.modeAuto")}
              </Button>
              <Button
                type="button"
                size="sm"
                variant={mode === "advanced" ? "default" : "outline"}
                onClick={() => setMode("advanced")}
              >
                {t("identity.modeAdvanced")}
              </Button>
            </div>

            <div className="rounded-md border border-border p-3 space-y-2 text-sm">
              <div className="flex justify-between gap-2">
                <span className="text-muted-foreground">
                  {t("identity.seed")}
                </span>
                <span className="font-mono">{persona.seed}</span>
              </div>
              <div className="flex justify-between gap-2">
                <span className="text-muted-foreground">
                  {t("identity.kernel")}
                </span>
                <span>
                  {profile.browser} · {profile.version}
                </span>
              </div>
              <div className="flex justify-between gap-2">
                <span className="text-muted-foreground">
                  {t("identity.platform")}
                </span>
                <span>
                  {persona.platform} {persona.platformVersion ?? ""} / Chrome{" "}
                  {persona.brandVersion}
                </span>
              </div>
              <div className="flex justify-between gap-2">
                <span className="text-muted-foreground">
                  {t("identity.capabilityRevision")}
                </span>
                <span className="max-w-[16rem] truncate font-mono text-xs">
                  {persona.capabilityRevision}
                </span>
              </div>
              {persona.proxyGeoSignature && (
                <div className="flex justify-between gap-2">
                  <span className="text-muted-foreground">
                    {t("identity.geoStamp")}
                  </span>
                  <span className="font-mono text-xs truncate max-w-[14rem]">
                    {persona.proxyGeoSignature}
                  </span>
                </div>
              )}
            </div>

            <p className="text-xs text-muted-foreground">
              {t("identity.seedDrivenNote")}
            </p>

            {(mode === "advanced" || mode === "auto") && (
              <div className="grid gap-3">
                <div className="grid gap-1.5">
                  <Label htmlFor="persona-lang">{t("identity.language")}</Label>
                  <Input
                    id="persona-lang"
                    value={persona.language}
                    disabled={isRunning || mode === "auto"}
                    onChange={(e) => {
                      const language = e.target.value;
                      update("language", language);
                      update("acceptLanguages", [
                        language,
                        language.split("-")[0] || language,
                      ]);
                    }}
                  />
                </div>
                <div className="grid gap-1.5">
                  <Label htmlFor="persona-tz">{t("identity.timezone")}</Label>
                  <Input
                    id="persona-tz"
                    value={persona.timezone}
                    disabled={isRunning || mode === "auto"}
                    onChange={(e) => update("timezone", e.target.value)}
                  />
                </div>
                {mode === "advanced" && (
                  <>
                    <div className="grid gap-1.5">
                      <Label htmlFor="persona-accept-languages">
                        {t("identity.acceptLanguages")}
                      </Label>
                      <Input
                        id="persona-accept-languages"
                        value={persona.acceptLanguages.join(", ")}
                        disabled={isRunning}
                        onChange={(e) =>
                          update(
                            "acceptLanguages",
                            e.target.value
                              .split(",")
                              .map((value) => value.trim())
                              .filter(Boolean),
                          )
                        }
                      />
                    </div>
                    <div className="grid gap-1.5">
                      <Label>{t("identity.cores")}</Label>
                      <Select
                        value={String(persona.hardwareConcurrency ?? 8)}
                        disabled={isRunning}
                        onValueChange={(v) =>
                          update("hardwareConcurrency", Number(v))
                        }
                      >
                        <SelectTrigger>
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {CORE_OPTIONS.map((c) => (
                            <SelectItem key={c} value={String(c)}>
                              {c}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                    <div className="grid grid-cols-2 gap-2">
                      <div className="grid gap-1.5">
                        <Label htmlFor="w">{t("identity.windowWidth")}</Label>
                        <Input
                          id="w"
                          type="number"
                          value={persona.windowWidth}
                          disabled={isRunning}
                          onChange={(e) =>
                            update(
                              "windowWidth",
                              Number(e.target.value) || 1920,
                            )
                          }
                        />
                      </div>
                      <div className="grid gap-1.5">
                        <Label htmlFor="h">{t("identity.windowHeight")}</Label>
                        <Input
                          id="h"
                          type="number"
                          value={persona.windowHeight}
                          disabled={isRunning}
                          onChange={(e) =>
                            update(
                              "windowHeight",
                              Number(e.target.value) || 1080,
                            )
                          }
                        />
                      </div>
                    </div>
                  </>
                )}
              </div>
            )}

            {mode === "auto" && (
              <p className="text-xs text-muted-foreground">
                {t("identity.autoModeHint")}
              </p>
            )}

            <div className="flex flex-wrap gap-2 pt-1">
              <LoadingButton
                type="button"
                variant="outline"
                size="sm"
                isLoading={matching}
                disabled={isRunning}
                onClick={() => void handleMatchExit()}
              >
                {t("identity.matchExit")}
              </LoadingButton>
              <LoadingButton
                type="button"
                variant="outline"
                size="sm"
                isLoading={regenerating}
                disabled={isRunning}
                onClick={() => void handleRegenerate()}
              >
                {t("identity.regenerate")}
              </LoadingButton>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => {
                  // Dispatch custom event so page can open audit without prop drilling.
                  window.dispatchEvent(
                    new CustomEvent("open-fingerprint-audit", {
                      detail: { profileId: profile.id },
                    }),
                  );
                }}
              >
                {t("audit.action")}
              </Button>
            </div>
            <p className="text-xs text-warning">
              {t("identity.regenerateWarning")}
            </p>
          </div>
        )}

        <DialogFooter>
          <Button type="button" variant="outline" onClick={onClose}>
            {t("common.buttons.cancel")}
          </Button>
          <LoadingButton
            type="button"
            isLoading={saving}
            disabled={!persona || isRunning || mode === "auto"}
            onClick={() => void handleSave()}
          >
            {t("common.buttons.save")}
          </LoadingButton>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
