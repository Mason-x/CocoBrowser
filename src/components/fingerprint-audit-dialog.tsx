"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { isFingerprintKernel } from "@/lib/browser-utils";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import type { BrowserProfile } from "@/types";
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
import { ScrollArea } from "./ui/scroll-area";

export type AuditStatus = "pass" | "warning" | "fail" | "unsupported";

export interface AuditFinding {
  code: string;
  severity: AuditStatus;
  message: string;
  expected?: string | null;
  observed?: string | null;
}

export interface AuditResult {
  profileId: string;
  kernelVersion: string;
  observedAt: number;
  expected: Record<string, unknown>;
  observed?: Record<string, unknown> | null;
  consistencyErrors: AuditFinding[];
  leakFindings: AuditFinding[];
  stabilityHash: string;
  status: AuditStatus;
  collectionMode: string;
}

export interface StabilityReport {
  profileId: string;
  rounds: number;
  hashes: string[];
  stable: boolean;
  status: AuditStatus;
  findings: AuditFinding[];
  lastResult?: AuditResult | null;
}

interface FingerprintAuditDialogProps {
  isOpen: boolean;
  onClose: () => void;
  profile: BrowserProfile | null;
}

function statusClass(status: AuditStatus): string {
  switch (status) {
    case "pass":
      return "text-success";
    case "warning":
      return "text-warning";
    case "fail":
      return "text-destructive";
    default:
      return "text-muted-foreground";
  }
}

export function FingerprintAuditDialog({
  isOpen,
  onClose,
  profile,
}: FingerprintAuditDialogProps) {
  const { t } = useTranslation();
  const [result, setResult] = useState<AuditResult | null>(null);
  const [stability, setStability] = useState<StabilityReport | null>(null);
  const [running, setRunning] = useState(false);
  const [runningStability, setRunningStability] = useState(false);

  const loadLast = useCallback(async () => {
    if (!profile) return;
    try {
      const last = await invoke<AuditResult | null>(
        "get_last_fingerprint_audit",
        { profileId: profile.id },
      );
      setResult(last);
    } catch {
      setResult(null);
    }
  }, [profile]);

  useEffect(() => {
    if (isOpen && profile) {
      void loadLast();
      setStability(null);
    }
  }, [isOpen, profile, loadLast]);

  const runAudit = async (live: boolean) => {
    if (!profile) return;
    setRunning(true);
    try {
      const r = await invoke<AuditResult>("run_fingerprint_audit", {
        profileId: profile.id,
        live,
      });
      setResult(r);
      showSuccessToast(t("audit.completed", { status: r.status }));
    } catch (err) {
      showErrorToast(t("audit.failed"), { description: String(err) });
    } finally {
      setRunning(false);
    }
  };

  const runStability = async () => {
    if (!profile) return;
    setRunningStability(true);
    try {
      const r = await invoke<StabilityReport>(
        "run_fingerprint_stability_audit",
        {
          profileId: profile.id,
          rounds: 10,
          live: true,
        },
      );
      setStability(r);
      if (r.lastResult) setResult(r.lastResult);
      showSuccessToast(
        r.stable ? t("audit.stabilityPass") : t("audit.stabilityFail"),
      );
    } catch (err) {
      showErrorToast(t("audit.stabilityFailed"), {
        description: String(err),
      });
    } finally {
      setRunningStability(false);
    }
  };

  const exportJson = () => {
    if (!result) return;
    const blob = new Blob([JSON.stringify(result, null, 2)], {
      type: "application/json",
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `audit-${profile?.id ?? "profile"}.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  if (!profile) return null;

  const findings = [
    ...(result?.consistencyErrors ?? []),
    ...(result?.leakFindings ?? []),
  ];

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-xl flex flex-col max-h-[90vh]">
        <DialogHeader>
          <DialogTitle>{t("audit.title")}</DialogTitle>
          <DialogDescription>
            {t("audit.description", { name: profile.name })}
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-wrap gap-2">
          <LoadingButton
            type="button"
            size="sm"
            isLoading={running}
            disabled={!isFingerprintKernel(profile.browser) || runningStability}
            onClick={() => void runAudit(true)}
          >
            {t("audit.runLive")}
          </LoadingButton>
          <LoadingButton
            type="button"
            size="sm"
            variant="outline"
            isLoading={running}
            disabled={!isFingerprintKernel(profile.browser) || runningStability}
            onClick={() => void runAudit(false)}
          >
            {t("audit.runStatic")}
          </LoadingButton>
          <LoadingButton
            type="button"
            size="sm"
            variant="outline"
            isLoading={runningStability}
            disabled={!isFingerprintKernel(profile.browser) || running}
            onClick={() => void runStability()}
          >
            {t("audit.runStability")}
          </LoadingButton>
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={!result}
            onClick={exportJson}
          >
            {t("audit.export")}
          </Button>
        </div>

        <p className="text-xs text-muted-foreground">{t("audit.note")}</p>

        <ScrollArea className="flex-1 min-h-[12rem] border border-border rounded-md p-3">
          {!result ? (
            <p className="text-sm text-muted-foreground">{t("audit.empty")}</p>
          ) : (
            <div className="space-y-3 text-sm">
              <div className="flex justify-between gap-2">
                <span className="text-muted-foreground">
                  {t("audit.status")}
                </span>
                <span className={`font-medium ${statusClass(result.status)}`}>
                  {result.status.toUpperCase()}
                </span>
              </div>
              <div className="flex justify-between gap-2">
                <span className="text-muted-foreground">{t("audit.mode")}</span>
                <span>{result.collectionMode}</span>
              </div>
              <div className="flex justify-between gap-2">
                <span className="text-muted-foreground">{t("audit.hash")}</span>
                <span className="font-mono text-xs truncate max-w-[14rem]">
                  {result.stabilityHash}
                </span>
              </div>
              {stability && (
                <div className="rounded-md bg-muted/40 p-2 text-xs">
                  {t("audit.stabilitySummary", {
                    rounds: stability.rounds,
                    stable: stability.stable ? t("audit.yes") : t("audit.no"),
                  })}
                </div>
              )}
              {findings.length === 0 ? (
                <p className="text-success text-sm">{t("audit.noFindings")}</p>
              ) : (
                <ul className="space-y-2">
                  {findings.map((f) => (
                    <li
                      key={`${f.code}-${f.message}`}
                      className="rounded border border-border p-2"
                    >
                      <div
                        className={`text-xs font-semibold ${statusClass(f.severity)}`}
                      >
                        {f.severity} · {f.code}
                      </div>
                      <div>{f.message}</div>
                      {(f.expected || f.observed) && (
                        <div className="mt-1 text-xs text-muted-foreground">
                          {f.expected && (
                            <div>
                              {t("audit.expected")}: {f.expected}
                            </div>
                          )}
                          {f.observed && (
                            <div>
                              {t("audit.observed")}: {f.observed}
                            </div>
                          )}
                        </div>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </ScrollArea>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={onClose}>
            {t("common.buttons.close")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
