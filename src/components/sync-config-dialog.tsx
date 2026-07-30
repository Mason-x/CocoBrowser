"use client";

import { useTranslation } from "react-i18next";
import { SyncServerSettings } from "@/components/sync-server-settings";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

interface SyncConfigDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

/**
 * Self-hosted sync configuration as a dialog, reached from a profile's sync
 * dialog when no server is configured yet. The same form also lives inline on the
 * settings page, which is where people look for it; both render
 * `SyncServerSettings` so the two cannot drift.
 */
export function SyncConfigDialog({ isOpen, onClose }: SyncConfigDialogProps) {
  const { t } = useTranslation();

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{t("sync.title")}</DialogTitle>
          <DialogDescription>{t("sync.description")}</DialogDescription>
        </DialogHeader>

        <div className="py-2">
          <SyncServerSettings key={String(isOpen)} onSaved={onClose} />
        </div>
      </DialogContent>
    </Dialog>
  );
}
