"use client";

import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { LuChevronDown, LuChevronRight, LuUpload } from "react-icons/lu";
import { toast } from "sonner";
import { LoadingButton } from "@/components/loading-button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { FadingScrollArea } from "@/components/ui/fading-scroll-area";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RippleButton } from "@/components/ui/ripple";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { parseBackendError, translateBackendError } from "@/lib/backend-errors";
import type {
  BrowserProfile,
  CookieReadResult,
  DomainCookies,
  SelectedCookie,
  UnifiedCookie,
} from "@/types";

interface CookieImportResult {
  cookies_imported: number;
  cookies_replaced: number;
  errors: string[];
}

interface CookieManagementDialogProps {
  isOpen: boolean;
  onClose: () => void;
  profile: BrowserProfile | null;
  initialTab?: "import" | "export";
}

type SelectionState = Record<
  string,
  {
    allSelected: boolean;
    cookies: Set<string>;
  }
>;

const ENCRYPTED_EXPORT_HEADER = "COCO_COOKIE_EXPORT_V1";
/** Written by Donut Browser, this fork's upstream. Accepted, never produced. */
const LEGACY_EXPORT_HEADER = "DONUT_COOKIE_EXPORT_V1";

const isEncryptedExport = (content: string): boolean =>
  content.startsWith(ENCRYPTED_EXPORT_HEADER) ||
  content.startsWith(LEGACY_EXPORT_HEADER);

const countCookies = (content: string): number => {
  const trimmed = content.trim();
  if (isEncryptedExport(trimmed)) {
    try {
      const json = trimmed.slice(trimmed.indexOf("\n") + 1);
      const envelope = JSON.parse(json) as { cookieCount?: number };
      return typeof envelope.cookieCount === "number"
        ? envelope.cookieCount
        : 0;
    } catch {
      return 0;
    }
  }
  if (trimmed.startsWith("[")) {
    try {
      const arr = JSON.parse(trimmed);
      if (Array.isArray(arr)) return arr.length;
    } catch {
      // Fall through to Netscape counting
    }
  }
  return content.split("\n").filter((line) => {
    const l = line.trim();
    return l && !l.startsWith("#");
  }).length;
};

function initSelectionFromCookieData(data: CookieReadResult): SelectionState {
  const sel: SelectionState = {};
  for (const d of data.domains) {
    sel[d.domain] = {
      allSelected: true,
      cookies: new Set(d.cookies.map((c) => c.name)),
    };
  }
  return sel;
}

export function CookieManagementDialog({
  isOpen,
  onClose,
  profile,
  initialTab = "import",
}: CookieManagementDialogProps) {
  const { t } = useTranslation();
  // Import state
  const [fileContent, setFileContent] = useState<string | null>(null);
  const [fileName, setFileName] = useState<string | null>(null);
  const [cookieCount, setCookieCount] = useState(0);
  const [importPassword, setImportPassword] = useState("");
  const [isImporting, setIsImporting] = useState(false);
  const [importResult, setImportResult] = useState<CookieImportResult | null>(
    null,
  );

  // Export state
  const [format, setFormat] = useState<"netscape" | "json">("json");
  const [exportPassword, setExportPassword] = useState("");
  const [exportPasswordConfirm, setExportPasswordConfirm] = useState("");
  const [isExporting, setIsExporting] = useState(false);
  const [exportCookieData, setExportCookieData] =
    useState<CookieReadResult | null>(null);
  const [isLoadingExportCookies, setIsLoadingExportCookies] = useState(false);
  const [exportSelection, setExportSelection] = useState<SelectionState>({});
  const [expandedDomains, setExpandedDomains] = useState<Set<string>>(
    new Set(),
  );
  const [activeTab, setActiveTab] = useState<string>(initialTab);

  const selectedExportCount = useMemo(() => {
    let count = 0;
    for (const domain of Object.keys(exportSelection)) {
      const ds = exportSelection[domain];
      if (ds.allSelected) {
        const domainData = exportCookieData?.domains.find(
          (d) => d.domain === domain,
        );
        count += domainData?.cookie_count ?? 0;
      } else {
        count += ds.cookies.size;
      }
    }
    return count;
  }, [exportSelection, exportCookieData]);

  const loadExportCookies = useCallback(
    async (profileId: string) => {
      if (exportCookieData) return;
      setIsLoadingExportCookies(true);
      try {
        const result = await invoke<CookieReadResult>("read_profile_cookies", {
          profileId,
        });
        setExportCookieData(result);
        setExportSelection(initSelectionFromCookieData(result));
      } catch (err) {
        toast.error(
          t("cookies.management.loadFailed", {
            error: err instanceof Error ? err.message : String(err),
          }),
        );
      } finally {
        setIsLoadingExportCookies(false);
      }
    },
    [exportCookieData, t],
  );

  useEffect(() => {
    if (activeTab === "export" && profile && !exportCookieData) {
      void loadExportCookies(profile.id);
    }
  }, [activeTab, profile, exportCookieData, loadExportCookies]);

  const resetImportState = useCallback(() => {
    setFileContent(null);
    setFileName(null);
    setCookieCount(0);
    setImportPassword("");
    setIsImporting(false);
    setImportResult(null);
  }, []);

  const resetExportState = useCallback(() => {
    setFormat("json");
    setExportPassword("");
    setExportPasswordConfirm("");
    setIsExporting(false);
    setExportCookieData(null);
    setExportSelection({});
    setExpandedDomains(new Set());
  }, []);

  const handleClose = useCallback(() => {
    resetImportState();
    resetExportState();
    setActiveTab(initialTab);
    onClose();
  }, [resetImportState, resetExportState, onClose, initialTab]);

  const handleTabChange = useCallback(
    (tab: string) => {
      setActiveTab(tab);
      resetImportState();
      if (tab !== "export") {
        resetExportState();
      }
    },
    [resetImportState, resetExportState],
  );

  const handleFileRead = useCallback(
    (file: File) => {
      const reader = new FileReader();
      reader.onload = (e) => {
        const content = e.target?.result as string;
        setFileContent(content);
        setFileName(file.name);
        setCookieCount(countCookies(content));
        setImportPassword("");
      };
      reader.onerror = () => {
        toast.error(t("cookies.management.fileReadError"));
      };
      reader.readAsText(file);
    },
    [t],
  );

  const handleImport = useCallback(async () => {
    if (!fileContent || !profile) return;
    setIsImporting(true);
    try {
      const result = await invoke<CookieImportResult>(
        "import_cookies_from_file",
        {
          profileId: profile.id,
          content: fileContent,
          password: isEncryptedExport(fileContent) ? importPassword : null,
        },
      );
      setImportResult(result);
    } catch (error) {
      toast.error(
        parseBackendError(error)
          ? translateBackendError(t, error)
          : error instanceof Error
            ? error.message
            : String(error),
      );
    } finally {
      setIsImporting(false);
    }
  }, [fileContent, importPassword, profile, t]);

  const getSelectedCookies = useCallback((): SelectedCookie[] => {
    if (!exportCookieData) return [];
    const result: SelectedCookie[] = [];
    for (const domain of exportCookieData.domains) {
      const ds = exportSelection[domain.domain];
      if (!ds) continue;
      if (ds.allSelected) {
        result.push(
          ...domain.cookies.map((cookie) => ({
            domain: cookie.domain,
            name: cookie.name,
          })),
        );
      } else {
        result.push(
          ...domain.cookies
            .filter((cookie) => ds.cookies.has(cookie.name))
            .map((cookie) => ({
              domain: cookie.domain,
              name: cookie.name,
            })),
        );
      }
    }
    return result;
  }, [exportCookieData, exportSelection]);

  const handleExport = useCallback(async () => {
    if (!profile) return;
    if (exportPassword.length < 12) {
      toast.error(t("cookies.export.passwordTooShort", { min: 12 }));
      return;
    }
    if (exportPassword !== exportPasswordConfirm) {
      toast.error(t("cookies.export.passwordMismatch"));
      return;
    }
    setIsExporting(true);
    try {
      const selectedCookies = getSelectedCookies();
      const content = await invoke<string>("export_profile_cookies", {
        profileId: profile.id,
        format,
        selectedCookies,
        password: exportPassword,
      });

      const defaultName = `${profile.name}_cookies.cocookies`;

      const filePath = await save({
        defaultPath: defaultName,
        filters: [
          {
            name: t("cookies.export.encryptedFile"),
            extensions: ["cocookies"],
          },
        ],
      });

      if (!filePath) {
        setIsExporting(false);
        return;
      }

      await writeTextFile(filePath, content);
      toast.success(t("cookies.export.success"));
      handleClose();
    } catch (error) {
      toast.error(
        parseBackendError(error)
          ? translateBackendError(t, error)
          : error instanceof Error
            ? error.message
            : String(error),
      );
    } finally {
      setIsExporting(false);
    }
  }, [
    profile,
    exportPassword,
    exportPasswordConfirm,
    getSelectedCookies,
    format,
    t,
    handleClose,
  ]);

  const toggleDomain = useCallback(
    (domain: string, cookies: UnifiedCookie[]) => {
      setExportSelection((prev) => {
        // `prev[domain]` is `undefined` when the domain was previously fully
        // deselected (entries are deleted on empty — see toggleCookie). Treat
        // missing as "not selected" so re-enabling falls through to the add
        // branch instead of crashing on `.allSelected`.
        if (prev[domain]?.allSelected) {
          const next = { ...prev };
          delete next[domain];
          return next;
        }
        return {
          ...prev,
          [domain]: {
            allSelected: true,
            cookies: new Set(cookies.map((c) => c.name)),
          },
        };
      });
    },
    [],
  );

  const toggleCookie = useCallback(
    (domain: string, cookieName: string, totalCookies: number) => {
      setExportSelection((prev) => {
        const current = prev[domain] ?? {
          allSelected: false,
          cookies: new Set<string>(),
        };
        const newCookies = new Set(current.cookies);
        if (newCookies.has(cookieName)) {
          newCookies.delete(cookieName);
        } else {
          newCookies.add(cookieName);
        }
        if (newCookies.size === 0) {
          const next = { ...prev };
          delete next[domain];
          return next;
        }
        return {
          ...prev,
          [domain]: {
            allSelected: newCookies.size === totalCookies,
            cookies: newCookies,
          },
        };
      });
    },
    [],
  );

  const toggleExpand = useCallback((domain: string) => {
    setExpandedDomains((prev) => {
      const next = new Set(prev);
      if (next.has(domain)) {
        next.delete(domain);
      } else {
        next.add(domain);
      }
      return next;
    });
  }, []);

  const toggleSelectAll = useCallback(() => {
    if (!exportCookieData) return;
    if (selectedExportCount === exportCookieData.total_count) {
      setExportSelection({});
    } else {
      setExportSelection(initSelectionFromCookieData(exportCookieData));
    }
  }, [exportCookieData, selectedExportCount]);

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className="max-w-[min(44rem,calc(100%-4rem))]">
        <DialogHeader>
          <DialogTitle>{t("cookies.management.title")}</DialogTitle>
        </DialogHeader>

        <Tabs
          defaultValue={initialTab}
          onValueChange={handleTabChange}
          className="w-full"
        >
          <TabsList className="grid w-full grid-cols-2">
            <TabsTrigger value="import">
              {t("cookies.management.tabImport")}
            </TabsTrigger>
            <TabsTrigger value="export">
              {t("cookies.management.tabExport")}
            </TabsTrigger>
          </TabsList>

          <TabsContent value="import" className="mt-4 space-y-4">
            {!fileContent && (
              <div className="space-y-4">
                <p className="text-sm text-muted-foreground">
                  {t("cookies.management.importDescription")}
                </p>
                <div
                  role="button"
                  tabIndex={0}
                  className="flex cursor-pointer flex-col items-center justify-center rounded-lg border-2 border-dashed border-muted-foreground/25 p-8 transition-colors hover:border-muted-foreground/50"
                  onClick={() =>
                    document.getElementById("cookie-file-input")?.click()
                  }
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      document.getElementById("cookie-file-input")?.click();
                    }
                  }}
                >
                  <LuUpload className="mb-4 size-10 text-muted-foreground" />
                  <p className="text-center text-sm text-muted-foreground">
                    {t("cookies.management.dropPrompt")}
                    <br />
                    <span className="text-xs">
                      {t("cookies.management.fileFormats")}
                    </span>
                  </p>
                  <input
                    id="cookie-file-input"
                    type="file"
                    accept=".txt,.cookies,.json,.cocookies"
                    className="hidden"
                    onChange={(e) => {
                      const file = e.target.files?.[0];
                      if (file) handleFileRead(file);
                      e.target.value = "";
                    }}
                  />
                </div>
              </div>
            )}

            {fileContent && !importResult && (
              <div className="space-y-4">
                <div className="flex items-center gap-3 rounded-lg bg-muted/30 p-4">
                  <div>
                    <div className="font-medium">{fileName}</div>
                    <div className="text-sm text-muted-foreground">
                      {t("cookies.management.cookiesFound", {
                        count: cookieCount,
                      })}
                    </div>
                  </div>
                </div>
                {isEncryptedExport(fileContent) && (
                  <div className="space-y-2">
                    <Label htmlFor="cookie-import-password">
                      {t("cookies.export.passwordLabel")}
                    </Label>
                    <Input
                      id="cookie-import-password"
                      type="password"
                      value={importPassword}
                      onChange={(event) => {
                        setImportPassword(event.target.value);
                      }}
                      autoComplete="off"
                    />
                  </div>
                )}
                <div className="flex justify-end gap-2">
                  <RippleButton variant="outline" onClick={resetImportState}>
                    {t("cookies.management.backButton")}
                  </RippleButton>
                  <LoadingButton
                    isLoading={isImporting}
                    onClick={() => void handleImport()}
                    disabled={
                      cookieCount === 0 ||
                      (isEncryptedExport(fileContent) && !importPassword)
                    }
                  >
                    {t("cookies.management.importButton")}
                  </LoadingButton>
                </div>
              </div>
            )}

            {importResult && (
              <div className="space-y-4">
                <div className="rounded-lg bg-success/10 p-4">
                  <div className="font-medium text-success">
                    {t("cookies.management.importedSuccess", {
                      imported: importResult.cookies_imported,
                      replaced: importResult.cookies_replaced,
                    })}
                  </div>
                  {importResult.errors.length > 0 && (
                    <div className="mt-2 text-sm text-muted-foreground">
                      {t("cookies.management.linesSkipped", {
                        count: importResult.errors.length,
                      })}
                    </div>
                  )}
                </div>
                <div className="flex justify-end">
                  <RippleButton onClick={handleClose}>
                    {t("cookies.management.doneButton")}
                  </RippleButton>
                </div>
              </div>
            )}
          </TabsContent>

          <TabsContent value="export" className="mt-4 space-y-3">
            <div className="space-y-2">
              <Label>{t("cookies.export.formatLabel")}</Label>
              <Select
                value={format}
                onValueChange={(v) => {
                  setFormat(v as "netscape" | "json");
                }}
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="json">
                    {t("cookies.export.json")}
                  </SelectItem>
                  <SelectItem value="netscape">
                    {t("cookies.export.netscape")}
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="cookie-export-password">
                  {t("cookies.export.passwordLabel")}
                </Label>
                <Input
                  id="cookie-export-password"
                  type="password"
                  value={exportPassword}
                  onChange={(event) => {
                    setExportPassword(event.target.value);
                  }}
                  autoComplete="new-password"
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="cookie-export-password-confirm">
                  {t("cookies.export.passwordConfirm")}
                </Label>
                <Input
                  id="cookie-export-password-confirm"
                  type="password"
                  value={exportPasswordConfirm}
                  onChange={(event) => {
                    setExportPasswordConfirm(event.target.value);
                  }}
                  autoComplete="new-password"
                />
              </div>
              <p className="text-xs text-muted-foreground sm:col-span-2">
                {t("cookies.export.encryptionNotice", { min: 12 })}
              </p>
            </div>

            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <Label>
                  {t("cookies.management.cookiesLabel")}{" "}
                  {exportCookieData && (
                    <span className="font-normal text-muted-foreground">
                      {t("cookies.management.selectionStatus", {
                        selected: selectedExportCount,
                        total: exportCookieData.total_count,
                      })}
                    </span>
                  )}
                </Label>
                {exportCookieData && exportCookieData.total_count > 0 && (
                  <button
                    type="button"
                    className="text-xs text-muted-foreground transition-colors hover:text-foreground"
                    onClick={toggleSelectAll}
                  >
                    {selectedExportCount === exportCookieData.total_count
                      ? t("cookies.management.deselectAll")
                      : t("cookies.management.selectAll")}
                  </button>
                )}
              </div>

              {isLoadingExportCookies ? (
                <div className="flex h-24 items-center justify-center">
                  <div className="size-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
                </div>
              ) : !exportCookieData || exportCookieData.domains.length === 0 ? (
                <div className="rounded-md border p-4 text-center text-sm text-muted-foreground">
                  {t("cookies.management.noCookies")}
                </div>
              ) : (
                <FadingScrollArea className="h-[clamp(140px,30vh,420px)]">
                  <div className="space-y-1 p-2">
                    {exportCookieData.domains.map((domain) => (
                      <ExportDomainRow
                        key={domain.domain}
                        domain={domain}
                        selection={exportSelection}
                        isExpanded={expandedDomains.has(domain.domain)}
                        onToggleDomain={toggleDomain}
                        onToggleCookie={toggleCookie}
                        onToggleExpand={toggleExpand}
                      />
                    ))}
                  </div>
                </FadingScrollArea>
              )}
            </div>

            <div className="flex justify-end gap-2">
              <RippleButton variant="outline" onClick={handleClose}>
                {t("common.buttons.cancel")}
              </RippleButton>
              <LoadingButton
                isLoading={isExporting}
                onClick={() => void handleExport()}
                disabled={
                  selectedExportCount === 0 ||
                  exportPassword.length < 12 ||
                  exportPassword !== exportPasswordConfirm
                }
              >
                {t("cookies.management.exportButton")}
              </LoadingButton>
            </div>
          </TabsContent>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}

interface ExportDomainRowProps {
  domain: DomainCookies;
  selection: SelectionState;
  isExpanded: boolean;
  onToggleDomain: (domain: string, cookies: UnifiedCookie[]) => void;
  onToggleCookie: (
    domain: string,
    cookieName: string,
    totalCookies: number,
  ) => void;
  onToggleExpand: (domain: string) => void;
}

function ExportDomainRow({
  domain,
  selection,
  isExpanded,
  onToggleDomain,
  onToggleCookie,
  onToggleExpand,
}: ExportDomainRowProps) {
  const domainSelection = selection[domain.domain];
  const isAllSelected = domainSelection?.allSelected ?? false;
  const selectedCount = domainSelection?.cookies.size ?? 0;
  const isPartial =
    selectedCount > 0 && selectedCount < domain.cookie_count && !isAllSelected;

  return (
    <div>
      <div className="flex items-center gap-2 rounded p-1.5 hover:bg-accent/50">
        <Checkbox
          checked={isAllSelected || isPartial}
          onCheckedChange={() => {
            onToggleDomain(domain.domain, domain.cookies);
          }}
          className={isPartial ? "opacity-70" : ""}
        />
        <button
          type="button"
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-1 border-none bg-transparent text-left text-sm"
          onClick={() => {
            onToggleExpand(domain.domain);
          }}
        >
          {isExpanded ? (
            <LuChevronDown className="size-3.5" />
          ) : (
            <LuChevronRight className="size-3.5" />
          )}
          <span className="truncate font-medium">{domain.domain}</span>
          <span className="shrink-0 text-xs text-muted-foreground">
            ({domain.cookie_count})
          </span>
        </button>
      </div>
      {isExpanded && (
        <div className="ml-7 space-y-0.5 border-l pl-2">
          {domain.cookies.map((cookie) => {
            const isSelected =
              domainSelection?.cookies.has(cookie.name) ?? false;
            return (
              <div
                key={`${domain.domain}-${cookie.name}`}
                className="flex items-center gap-2 rounded p-1 text-sm hover:bg-accent/30"
              >
                <Checkbox
                  checked={isSelected || isAllSelected}
                  onCheckedChange={() => {
                    onToggleCookie(
                      domain.domain,
                      cookie.name,
                      domain.cookie_count,
                    );
                  }}
                />
                <span className="truncate">{cookie.name}</span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
