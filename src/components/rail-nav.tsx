"use client";

import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { FaDownload } from "react-icons/fa";
import { FiWifi } from "react-icons/fi";
import { GoGear, GoKebabHorizontal } from "react-icons/go";
import {
  LuCpu,
  LuKeyboard,
  LuPanelLeftClose,
  LuPanelLeftOpen,
  LuPlug,
  LuPuzzle,
  LuUser,
  LuUsers,
} from "react-icons/lu";
import { cn } from "@/lib/utils";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";

const RAIL_EXPANDED_KEY = "rail-nav-expanded";

export type AppPage =
  | "profiles"
  | "proxies"
  | "extensions"
  | "groups"
  | "vpns"
  | "settings"
  | "integrations"
  | "import"
  | "shortcuts"
  | "kernels";

interface RailNavProps {
  currentPage: AppPage;
  onNavigate: (page: AppPage) => void;
}

interface RailItem {
  page: AppPage;
  Icon: React.ComponentType<{ className?: string }>;
  labelKey: string;
}

const TOP_ITEMS: RailItem[] = [
  { page: "profiles", Icon: LuUser, labelKey: "rail.profiles" },
  { page: "proxies", Icon: FiWifi, labelKey: "rail.network" },
  { page: "extensions", Icon: LuPuzzle, labelKey: "rail.extensions" },
  { page: "groups", Icon: LuUsers, labelKey: "rail.groups" },
  { page: "kernels", Icon: LuCpu, labelKey: "rail.kernels" },
  { page: "integrations", Icon: LuPlug, labelKey: "rail.integrations" },
];

interface MoreMenuItem {
  page: AppPage;
  Icon: React.ComponentType<{ className?: string }>;
  labelKey: string;
  hintKey: string;
}

const MORE_ITEMS: MoreMenuItem[] = [
  {
    page: "import",
    Icon: FaDownload,
    labelKey: "rail.more.importProfile",
    hintKey: "rail.more.importProfileHint",
  },
  {
    page: "shortcuts",
    Icon: LuKeyboard,
    labelKey: "rail.more.keyboardShortcuts",
    hintKey: "rail.more.keyboardShortcutsHint",
  },
];

export function RailNav({ currentPage, onNavigate }: RailNavProps) {
  const { t } = useTranslation();
  const [moreOpen, setMoreOpen] = useState(false);
  const [expanded, setExpanded] = useState(() => {
    try {
      const stored = localStorage.getItem(RAIL_EXPANDED_KEY);
      return stored === null ? true : stored === "1";
    } catch {
      return true;
    }
  });
  const toggleExpanded = useCallback(() => {
    setExpanded((prev) => {
      const next = !prev;
      try {
        localStorage.setItem(RAIL_EXPANDED_KEY, next ? "1" : "0");
      } catch {
        // ignore
      }
      return next;
    });
  }, []);

  return (
    <nav
      className={cn(
        // pt-3.5 centres the first nav item on the table header row beside it:
        // that header is 36px tall under the main area's 10px top padding, so
        // its centre sits at 28px — matching a 28px-tall item starting at 14px.
        "relative flex shrink-0 flex-col gap-1 border-r border-border bg-background pt-3.5 pb-2 transition-[width] duration-200 ease-in-out select-none",
        expanded ? "w-max min-w-24 px-2 items-stretch" : "w-10 items-center",
      )}
    >
      <div
        className={cn(
          "flex min-h-0 w-full scrollbar-none flex-col gap-1 overflow-y-auto [-ms-overflow-style:none] [&::-webkit-scrollbar]:hidden",
          expanded ? "items-stretch" : "items-center",
        )}
      >
        {TOP_ITEMS.map(({ page, Icon, labelKey }) => {
          const active = currentPage === page;
          const button = (
            <button
              type="button"
              onClick={() => {
                onNavigate(page);
              }}
              aria-label={t(labelKey)}
              aria-current={active ? "page" : undefined}
              className={cn(
                "relative flex shrink-0 cursor-pointer items-center rounded-md transition-colors duration-100",
                expanded
                  ? "h-7 justify-center gap-2 px-2"
                  : "size-7 justify-center",
                active
                  ? "bg-muted/50 text-foreground"
                  : "text-muted-foreground hover:bg-muted/30 hover:text-card-foreground",
              )}
            >
              {active && (
                <span
                  aria-hidden="true"
                  className={cn(
                    "absolute inset-y-1.5 w-[2px] rounded-full bg-foreground",
                    expanded ? "left-[-9px]" : "left-[-7px]",
                  )}
                />
              )}
              <Icon className="size-3.5 shrink-0" />
              {expanded && (
                <span className="whitespace-nowrap text-xs font-medium">
                  {t(labelKey)}
                </span>
              )}
            </button>
          );
          if (expanded) {
            return <div key={page}>{button}</div>;
          }
          return (
            <Tooltip key={page} delayDuration={300}>
              <TooltipTrigger asChild>{button}</TooltipTrigger>
              <TooltipContent side="right">{t(labelKey)}</TooltipContent>
            </Tooltip>
          );
        })}
      </div>

      <div className="flex-1" />

      {/* Expand/collapse toggle */}
      {(() => {
        const toggleButton = (
          <button
            type="button"
            onClick={toggleExpanded}
            aria-label={expanded ? t("rail.collapse") : t("rail.expand")}
            className={cn(
              "flex shrink-0 cursor-pointer items-center rounded-md text-muted-foreground transition-colors duration-100 hover:bg-muted/30 hover:text-card-foreground",
              expanded
                ? "h-7 justify-center gap-2 px-2"
                : "size-7 justify-center",
            )}
          >
            {expanded ? (
              <LuPanelLeftClose className="size-3.5 shrink-0" />
            ) : (
              <LuPanelLeftOpen className="size-3.5 shrink-0" />
            )}
            {expanded && (
              <span className="whitespace-nowrap text-xs font-medium">
                {t("rail.collapse")}
              </span>
            )}
          </button>
        );
        if (expanded) return toggleButton;
        return (
          <Tooltip delayDuration={300}>
            <TooltipTrigger asChild>{toggleButton}</TooltipTrigger>
            <TooltipContent side="right">{t("rail.expand")}</TooltipContent>
          </Tooltip>
        );
      })()}

      {/* More menu */}
      {(() => {
        const moreButton = (
          <button
            type="button"
            onClick={() => {
              setMoreOpen((v) => !v);
            }}
            aria-label={t("rail.more.label")}
            aria-expanded={moreOpen}
            className={cn(
              "flex shrink-0 cursor-pointer items-center rounded-md transition-colors duration-100",
              expanded
                ? "h-7 justify-center gap-2 px-2"
                : "size-7 justify-center",
              moreOpen
                ? "bg-muted/50 text-foreground"
                : "text-muted-foreground hover:bg-muted/30 hover:text-card-foreground",
            )}
          >
            <GoKebabHorizontal className="size-3.5 shrink-0" />
            {expanded && (
              <span className="whitespace-nowrap text-xs font-medium">
                {t("rail.more.label")}
              </span>
            )}
          </button>
        );
        if (expanded) return moreButton;
        return (
          <Tooltip delayDuration={300}>
            <TooltipTrigger asChild>{moreButton}</TooltipTrigger>
            <TooltipContent side="right">{t("rail.more.label")}</TooltipContent>
          </Tooltip>
        );
      })()}

      {/* Settings */}
      {(() => {
        const settingsActive = currentPage === "settings";
        const settingsButton = (
          <button
            type="button"
            onClick={() => {
              onNavigate("settings");
            }}
            aria-label={t("rail.settings")}
            aria-current={settingsActive ? "page" : undefined}
            className={cn(
              "relative flex shrink-0 cursor-pointer items-center rounded-md transition-colors duration-100",
              expanded
                ? "h-7 justify-center gap-2 px-2"
                : "size-7 justify-center",
              settingsActive
                ? "bg-muted/50 text-foreground"
                : "text-muted-foreground hover:bg-muted/30 hover:text-card-foreground",
            )}
          >
            {settingsActive && (
              <span
                aria-hidden="true"
                className={cn(
                  "absolute inset-y-1.5 w-[2px] rounded-full bg-foreground",
                  expanded ? "left-[-9px]" : "left-[-7px]",
                )}
              />
            )}
            <GoGear className="size-3.5 shrink-0" />
            {expanded && (
              <span className="whitespace-nowrap text-xs font-medium">
                {t("rail.settings")}
              </span>
            )}
          </button>
        );
        if (expanded) return settingsButton;
        return (
          <Tooltip delayDuration={300}>
            <TooltipTrigger asChild>{settingsButton}</TooltipTrigger>
            <TooltipContent side="right">{t("rail.settings")}</TooltipContent>
          </Tooltip>
        );
      })()}

      {moreOpen && (
        <>
          <button
            type="button"
            aria-label={t("rail.more.closeAriaLabel")}
            className="fixed inset-0 z-30 cursor-default bg-transparent"
            onClick={() => {
              setMoreOpen(false);
            }}
          />
          <div
            className={cn(
              "absolute bottom-14 z-40 w-56 animate-in rounded-lg border border-border bg-card p-1 shadow-2xl duration-100 fade-in-0 slide-in-from-bottom-1",
              expanded ? "left-2" : "left-11",
            )}
          >
            {MORE_ITEMS.map(({ page, Icon, labelKey, hintKey }) => (
              <button
                key={page}
                type="button"
                onClick={() => {
                  setMoreOpen(false);
                  onNavigate(page);
                }}
                className="flex w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors duration-100 hover:bg-accent"
              >
                <span className="grid size-5 shrink-0 place-items-center rounded bg-muted text-muted-foreground">
                  <Icon className="size-3" />
                </span>
                <span className="flex min-w-0 flex-col">
                  <span className="truncate text-xs font-medium text-foreground">
                    {t(labelKey)}
                  </span>
                  <span className="truncate text-[10px] text-muted-foreground">
                    {t(hintKey)}
                  </span>
                </span>
              </button>
            ))}
          </div>
        </>
      )}
    </nav>
  );
}
