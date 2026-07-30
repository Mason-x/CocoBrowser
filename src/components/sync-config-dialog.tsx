"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { LuEye, LuEyeOff } from "react-icons/lu";
import { LoadingButton } from "@/components/loading-button";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import type { SyncSettings } from "@/types";

interface SyncConfigDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

/**
 * Self-hosted sync configuration. This local-first build has no hosted
 * account tier, so the only supported target is a server URL plus token
 * that the user operates themselves.
 */
export function SyncConfigDialog({ isOpen, onClose }: SyncConfigDialogProps) {
  const { t } = useTranslation();

  const [serverUrl, setServerUrl] = useState("");
  const [token, setToken] = useState("");
  const [isLoading, setIsLoading] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const [isTesting, setIsTesting] = useState(false);
  const [showToken, setShowToken] = useState(false);

  const [connectionStatus, setConnectionStatus] = useState<
    "unknown" | "testing" | "connected" | "error"
  >("unknown");
  const hasConfig = Boolean(serverUrl && token);

  const testConnection = useCallback(async (url: string) => {
    setConnectionStatus("testing");
    try {
      await invoke("check_sync_server_health", { serverUrl: url });
      setConnectionStatus("connected");
    } catch {
      setConnectionStatus("error");
    }
  }, []);

  const loadSettings = useCallback(async () => {
    setIsLoading(true);
    try {
      const settings = await invoke<SyncSettings>("get_sync_settings");
      setServerUrl(settings.sync_server_url ?? "");
      setToken(settings.sync_token ?? "");
      if (settings.sync_server_url && settings.sync_token) {
        void testConnection(settings.sync_server_url);
      }
    } catch (error) {
      console.error("Failed to load sync settings:", error);
    } finally {
      setIsLoading(false);
    }
  }, [testConnection]);

  useEffect(() => {
    if (isOpen) {
      setConnectionStatus("unknown");
      void loadSettings();
    }
  }, [isOpen, loadSettings]);

  const handleTestConnection = useCallback(async () => {
    if (!serverUrl) {
      showErrorToast(t("sync.config.serverUrlRequired"));
      return;
    }

    setIsTesting(true);
    setConnectionStatus("testing");
    try {
      await invoke("check_sync_server_health", { serverUrl });
      setConnectionStatus("connected");
      showSuccessToast(t("sync.config.connectionSuccess"));
    } catch {
      setConnectionStatus("error");
      showErrorToast(t("sync.config.connectFailed"));
    } finally {
      setIsTesting(false);
    }
  }, [serverUrl, t]);

  const handleSave = useCallback(async () => {
    setIsSaving(true);
    try {
      await invoke<SyncSettings>("save_sync_settings", {
        syncServerUrl: serverUrl || null,
        syncToken: token || null,
      });
      try {
        await invoke("restart_sync_service");
      } catch (e) {
        console.error("Failed to restart sync service:", e);
      }
      showSuccessToast(t("sync.config.settingsSaved"));
      onClose();
    } catch (error) {
      console.error("Failed to save sync settings:", error);
      showErrorToast(t("sync.config.saveFailed"));
    } finally {
      setIsSaving(false);
    }
  }, [serverUrl, token, onClose, t]);

  const handleDisconnect = useCallback(async () => {
    setIsSaving(true);
    try {
      await invoke<SyncSettings>("save_sync_settings", {
        syncServerUrl: null,
        syncToken: null,
      });
      try {
        await invoke("restart_sync_service");
      } catch (e) {
        console.error("Failed to restart sync service:", e);
      }
      setServerUrl("");
      setToken("");
      setConnectionStatus("unknown");
      showSuccessToast(t("sync.config.disconnected"));
    } catch (error) {
      console.error("Failed to disconnect:", error);
      showErrorToast(t("sync.config.disconnectFailed"));
    } finally {
      setIsSaving(false);
    }
  }, [t]);

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{t("sync.title")}</DialogTitle>
          <DialogDescription>{t("sync.description")}</DialogDescription>
        </DialogHeader>

        {isLoading ? (
          <div className="flex justify-center py-8">
            <div className="size-6 animate-spin rounded-full border-2 border-current border-t-transparent" />
          </div>
        ) : (
          <div className="grid gap-4 py-4">
            <div className="space-y-2">
              <Label htmlFor="sync-server-url">{t("sync.serverUrl")}</Label>
              <Input
                id="sync-server-url"
                placeholder={t("sync.serverUrlPlaceholder")}
                value={serverUrl}
                onChange={(e) => {
                  setServerUrl(e.target.value);
                }}
              />
            </div>

            <div className="space-y-2">
              <Label htmlFor="sync-token">{t("sync.token")}</Label>
              <div className="relative">
                <Input
                  id="sync-token"
                  type={showToken ? "text" : "password"}
                  placeholder={t("sync.tokenPlaceholder")}
                  value={token}
                  onChange={(e) => {
                    setToken(e.target.value);
                  }}
                  className="pr-10"
                />
                <Tooltip>
                  <TooltipTrigger asChild>
                    <button
                      type="button"
                      onClick={() => {
                        setShowToken(!showToken);
                      }}
                      className="absolute top-1/2 right-3 -translate-y-1/2 transform rounded-sm p-1 transition-colors hover:bg-accent"
                      aria-label={
                        showToken
                          ? t("common.aria.hideToken")
                          : t("common.aria.showToken")
                      }
                    >
                      {showToken ? (
                        <LuEyeOff className="size-4 text-muted-foreground hover:text-foreground" />
                      ) : (
                        <LuEye className="size-4 text-muted-foreground hover:text-foreground" />
                      )}
                    </button>
                  </TooltipTrigger>
                  <TooltipContent>
                    {showToken
                      ? t("common.aria.hideToken")
                      : t("common.aria.showToken")}
                  </TooltipContent>
                </Tooltip>
              </div>
            </div>

            {connectionStatus === "testing" && (
              <div className="flex items-center gap-2 text-sm text-muted-foreground">
                <div className="size-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
                {t("sync.status.syncing")}
              </div>
            )}
            {connectionStatus === "connected" && (
              <div className="flex items-center gap-2 text-sm text-muted-foreground">
                <div className="size-2 rounded-full bg-success" />
                {t("sync.status.connected")}
              </div>
            )}
            {connectionStatus === "error" && (
              <div className="flex items-center gap-2 text-sm text-muted-foreground">
                <div className="size-2 rounded-full bg-destructive" />
                {t("sync.status.disconnected")}
              </div>
            )}
          </div>
        )}

        <DialogFooter className="flex gap-2">
          {hasConfig && (
            <Button
              variant="outline"
              onClick={() => void handleDisconnect()}
              disabled={isSaving}
            >
              {t("common.buttons.disconnect")}
            </Button>
          )}
          <Button
            variant="outline"
            onClick={() => void handleTestConnection()}
            disabled={isTesting || !serverUrl}
          >
            {isTesting
              ? t("sync.config.testing")
              : t("sync.config.testConnection")}
          </Button>
          <LoadingButton
            onClick={() => void handleSave()}
            isLoading={isSaving}
            disabled={!serverUrl || !token}
          >
            {t("common.buttons.save")}
          </LoadingButton>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
