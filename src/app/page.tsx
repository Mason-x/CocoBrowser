"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrent } from "@tauri-apps/plugin-deep-link";
import { useOnborda } from "onborda";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CloneProfileDialog } from "@/components/clone-profile-dialog";
import { CloseConfirmDialog } from "@/components/close-confirm-dialog";
import { CommandPalette } from "@/components/command-palette";
import { CookieCopyDialog } from "@/components/cookie-copy-dialog";
import { CookieManagementDialog } from "@/components/cookie-management-dialog";
import { CreateProfileDialog } from "@/components/create-profile-dialog";
import { DeleteConfirmationDialog } from "@/components/delete-confirmation-dialog";
import { ExtensionGroupAssignmentDialog } from "@/components/extension-group-assignment-dialog";
import { ExtensionManagementDialog } from "@/components/extension-management-dialog";
import { FingerprintAuditDialog } from "@/components/fingerprint-audit-dialog";
import { FingerprintIdentityDialog } from "@/components/fingerprint-identity-dialog";
import { GroupAssignmentDialog } from "@/components/group-assignment-dialog";
import { GroupManagementDialog } from "@/components/group-management-dialog";
import HomeHeader from "@/components/home-header";
import { ImportProfileDialog } from "@/components/import-profile-dialog";
import { IntegrationsDialog } from "@/components/integrations-dialog";
import { KernelsManagementDialog } from "@/components/kernels-management-dialog";
import { ONBOARDING_TOUR } from "@/components/onboarding-provider";
import { PermissionDialog } from "@/components/permission-dialog";
import { ProfilesDataTable } from "@/components/profile-data-table";
import {
  type PasswordDialogMode,
  ProfilePasswordDialog,
} from "@/components/profile-password-dialog";
import { ProfileSelectorDialog } from "@/components/profile-selector-dialog";
import { ProfileSyncDialog } from "@/components/profile-sync-dialog";
import { ProxyAssignmentDialog } from "@/components/proxy-assignment-dialog";
import { ProxyManagementDialog } from "@/components/proxy-management-dialog";
import { type AppPage, RailNav } from "@/components/rail-nav";
import { SettingsDialog } from "@/components/settings-dialog";
import { ShortcutsPage } from "@/components/shortcuts-page";
import { SyncAllDialog } from "@/components/sync-all-dialog";
import { SyncConfigDialog } from "@/components/sync-config-dialog";
import { SyncFollowerDialog } from "@/components/sync-follower-dialog";
import { ThankYouDialog } from "@/components/thank-you-dialog";
import { WelcomeDialog } from "@/components/welcome-dialog";
import { WindowResizeWarningDialog } from "@/components/window-resize-warning-dialog";
import { useGroupEvents } from "@/hooks/use-group-events";
import type { PermissionType } from "@/hooks/use-permissions";
import { usePermissions } from "@/hooks/use-permissions";
import { useProfileEvents } from "@/hooks/use-profile-events";
import { useProxyEvents } from "@/hooks/use-proxy-events";
import { useSyncSessions } from "@/hooks/use-sync-session";
import { useVpnEvents } from "@/hooks/use-vpn-events";
import { translateBackendError } from "@/lib/backend-errors";
import {
  ONBOARDING_TOUR_FINISHED_EVENT,
  setOnboardingActive,
} from "@/lib/onboarding-signal";
import {
  matchesGroupDigit,
  matchesShortcut,
  SHORTCUTS,
  type ShortcutId,
} from "@/lib/shortcuts";
import {
  dismissToast,
  showErrorToast,
  showSuccessToast,
  showSyncProgressToast,
  showToast,
} from "@/lib/toast-utils";
import { buildWorkbenchPage } from "@/lib/workbench-page";
import type { BrowserProfile, SyncSettings } from "@/types";

type BrowserTypeString = "fingerprint-chromium";

interface PendingUrl {
  id: string;
  url: string;
}

export default function Home() {
  const { t } = useTranslation();
  // Use the new profile events hook for centralized profile management
  const {
    profiles,
    runningProfiles,
    isLoading: profilesLoading,
    error: profilesError,
  } = useProfileEvents();

  // First-run onboarding tour (Onborda).
  const { startOnborda, setCurrentStep, isOnbordaVisible, currentStep } =
    useOnborda();
  const onboardingHandledRef = useRef(false);
  const [welcomeOpen, setWelcomeOpen] = useState(false);
  const [thankYouOpen, setThankYouOpen] = useState(false);
  // null = onboarding decision pending; false = not a first-run onboarding (run
  // the normal permission checks); true = first-run onboarding, so the welcome
  // flow drives permissions and the standalone permission dialog is suppressed.
  const [firstRunOnboarding, setFirstRunOnboarding] = useState<boolean | null>(
    null,
  );

  // Welcome flow finished. Existing-profile users are done after the welcome;
  // users with no profile yet continue into the in-app
  // product tour that walks them through creating their first profile.
  const handleWelcomeComplete = useCallback(() => {
    setWelcomeOpen(false);
    setFirstRunOnboarding(false);
    if (profiles.length === 0) {
      startOnborda(ONBOARDING_TOUR);
    }
  }, [startOnborda, profiles.length]);

  // The product tour finished (user clicked "Finish", not "Skip") → celebrate.
  useEffect(() => {
    const handler = () => setThankYouOpen(true);
    window.addEventListener(ONBOARDING_TOUR_FINISHED_EVENT, handler);
    return () =>
      window.removeEventListener(ONBOARDING_TOUR_FINISHED_EVENT, handler);
  }, []);

  // Suppress the global browser-download toasts while onboarding (welcome or
  // tour) is active — the welcome dialog shows setup progress itself.
  useEffect(() => {
    setOnboardingActive(welcomeOpen || isOnbordaVisible);
  }, [welcomeOpen, isOnbordaVisible]);

  // While the tour is visible, keep the body pinned to the left. Onborda calls
  // scrollIntoView({ inline: "center" }) on the highlighted element; because the
  // body is overflow-hidden it can still be scrolled programmatically, which
  // would shove the whole app (rail and all) sideways with no way to scroll
  // back. The profile table keeps its own scroll container, untouched here.
  useEffect(() => {
    if (!isOnbordaVisible) return;
    const pin = () => {
      if (document.body.scrollLeft !== 0) document.body.scrollLeft = 0;
      if (document.documentElement.scrollLeft !== 0)
        document.documentElement.scrollLeft = 0;
    };
    pin();
    window.addEventListener("scroll", pin, true);
    return () => window.removeEventListener("scroll", pin, true);
  }, [isOnbordaVisible]);

  // On the very first launch, show the local-first welcome flow
  // (one-shot: the backend flag is set immediately so it can't trigger again).
  // The welcome dialog itself decides whether to continue into the browser
  // download + profile-creation flow — only when the user has no profile yet.
  useEffect(() => {
    if (profilesLoading || onboardingHandledRef.current) return;
    onboardingHandledRef.current = true;
    void (async () => {
      try {
        const completed = await invoke<boolean>("get_onboarding_completed");
        if (completed) {
          setFirstRunOnboarding(false);
          return;
        }
        await invoke("complete_onboarding");
        setFirstRunOnboarding(true);
        setWelcomeOpen(true);
      } catch (err) {
        console.error("Onboarding init failed:", err);
        setFirstRunOnboarding(false);
      }
    })();
  }, [profilesLoading]);

  // Advance from the "create a profile" step to the "DNS blocking" step as soon
  // as the user's first profile exists (its DNS dropdown is now in the DOM).
  useEffect(() => {
    if (isOnbordaVisible && currentStep === 0 && profiles.length > 0) {
      // Small delay so the new profile row (and its DNS dropdown target) has
      // mounted before Onborda re-points at it.
      setCurrentStep(1, 300);
    }
  }, [isOnbordaVisible, currentStep, profiles.length, setCurrentStep]);

  const {
    groups: groupsData,
    isLoading: groupsLoading,
    error: groupsError,
  } = useGroupEvents();

  const {
    storedProxies,
    isLoading: proxiesLoading,
    error: proxiesError,
  } = useProxyEvents();

  const { vpnConfigs } = useVpnEvents();

  // Synchronizer sessions
  const { getProfileSyncInfo } = useSyncSessions();
  const [syncLeaderProfile, setSyncLeaderProfile] =
    useState<BrowserProfile | null>(null);

  // Local automation is not a cloud entitlement. Cross-OS spoofing stays
  // disabled because fingerprint-chromium cannot safely honor it.
  const crossOsUnlocked = false;
  const automationUnlocked = true;

  const [selfHostedSyncConfigured, setSelfHostedSyncConfigured] =
    useState(false);

  const checkSelfHostedSync = useCallback(async () => {
    try {
      const settings = await invoke<SyncSettings>("get_sync_settings");
      const hasConfig = Boolean(
        settings.sync_server_url && settings.sync_token,
      );
      setSelfHostedSyncConfigured(hasConfig);
    } catch {
      setSelfHostedSyncConfigured(false);
    }
  }, []);

  const syncUnlocked = crossOsUnlocked || selfHostedSyncConfigured;

  const [currentPage, setCurrentPage] = useState<AppPage>("profiles");
  // Tracks which tab inside the shared proxy-management page should be active.
  // The VPN rail item routes to the same page but pre-selects the VPN tab.
  const [proxyManagementInitialTab, setProxyManagementInitialTab] = useState<
    "proxies" | "vpns"
  >("proxies");
  const [extensionManagementInitialTab, setExtensionManagementInitialTab] =
    useState<"extensions" | "groups">("extensions");
  const [integrationsInitialTab, setIntegrationsInitialTab] = useState<
    "api" | "mcp"
  >("api");
  const [createProfileDialogOpen, setCreateProfileDialogOpen] = useState(false);
  const [settingsDialogOpen, setSettingsDialogOpen] = useState(false);
  const [integrationsDialogOpen, setIntegrationsDialogOpen] = useState(false);
  const [importProfileDialogOpen, setImportProfileDialogOpen] = useState(false);
  const [proxyManagementDialogOpen, setProxyManagementDialogOpen] =
    useState(false);
  const [identityDialogOpen, setIdentityDialogOpen] = useState(false);
  const [currentProfileForIdentity, setCurrentProfileForIdentity] =
    useState<BrowserProfile | null>(null);
  const [auditDialogOpen, setAuditDialogOpen] = useState(false);
  const [currentProfileForAudit, setCurrentProfileForAudit] =
    useState<BrowserProfile | null>(null);
  const [groupManagementDialogOpen, setGroupManagementDialogOpen] =
    useState(false);
  const [extensionManagementDialogOpen, setExtensionManagementDialogOpen] =
    useState(false);
  const [kernelsManagementDialogOpen, setKernelsManagementDialogOpen] =
    useState(false);
  const [groupAssignmentDialogOpen, setGroupAssignmentDialogOpen] =
    useState(false);
  const [
    extensionGroupAssignmentDialogOpen,
    setExtensionGroupAssignmentDialogOpen,
  ] = useState(false);
  const [
    selectedProfilesForExtensionGroup,
    setSelectedProfilesForExtensionGroup,
  ] = useState<string[]>([]);
  const [proxyAssignmentDialogOpen, setProxyAssignmentDialogOpen] =
    useState(false);
  const [cookieCopyDialogOpen, setCookieCopyDialogOpen] = useState(false);
  const [cookieManagementDialogOpen, setCookieManagementDialogOpen] =
    useState(false);
  const [
    currentProfileForCookieManagement,
    setCurrentProfileForCookieManagement,
  ] = useState<BrowserProfile | null>(null);
  const [selectedProfilesForCookies, setSelectedProfilesForCookies] = useState<
    string[]
  >([]);
  const [selectedGroupId, setSelectedGroupId] = useState<string>("__all__");
  const [selectedProfilesForGroup, setSelectedProfilesForGroup] = useState<
    string[]
  >([]);
  const [selectedProfilesForProxy, setSelectedProfilesForProxy] = useState<
    string[]
  >([]);
  const [selectedProfiles, setSelectedProfiles] = useState<string[]>([]);
  const [searchQuery, setSearchQuery] = useState<string>("");
  const [pendingUrls, setPendingUrls] = useState<PendingUrl[]>([]);
  const [cloneProfile, setCloneProfile] = useState<BrowserProfile | null>(null);
  const [passwordDialogProfile, setPasswordDialogProfile] =
    useState<BrowserProfile | null>(null);
  const [passwordDialogMode, setPasswordDialogMode] =
    useState<PasswordDialogMode>("set");
  const pendingLaunchAfterUnlockRef = useRef<BrowserProfile | null>(null);
  const [windowResizeWarningOpen, setWindowResizeWarningOpen] = useState(false);
  const windowResizeWarningResolver = useRef<
    ((proceed: boolean) => void) | null
  >(null);
  const [permissionDialogOpen, setPermissionDialogOpen] = useState(false);
  const [currentPermissionType, setCurrentPermissionType] =
    useState<PermissionType>("microphone");
  const [showBulkDeleteConfirmation, setShowBulkDeleteConfirmation] =
    useState(false);
  const [isBulkDeleting, setIsBulkDeleting] = useState(false);
  const [syncConfigDialogOpen, setSyncConfigDialogOpen] = useState(false);
  const [syncAllDialogOpen, setSyncAllDialogOpen] = useState(false);
  const [profileSyncDialogOpen, setProfileSyncDialogOpen] = useState(false);
  const [currentProfileForSync, setCurrentProfileForSync] =
    useState<BrowserProfile | null>(null);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  // Owned by page.tsx so the command palette can request opening the profile
  // info dialog. ProfilesDataTable consumes it through controlled props.
  const [profileInfoDialog, setProfileInfoDialog] =
    useState<BrowserProfile | null>(null);
  const { isMicrophoneAccessGranted, isCameraAccessGranted, isInitialized } =
    usePermissions();

  const handleSelectGroup = useCallback((groupId: string) => {
    setSelectedGroupId(groupId);
    setSelectedProfiles([]);
  }, []);

  const handleRailNavigate = useCallback((page: AppPage) => {
    // Always reset every sub-page-able dialog before opening the next one,
    // so navigating from one rail item to another doesn't stack two
    // sub-pages on top of each other.
    setSettingsDialogOpen(false);
    setProxyManagementDialogOpen(false);
    setExtensionManagementDialogOpen(false);
    setGroupManagementDialogOpen(false);
    setIntegrationsDialogOpen(false);
    setImportProfileDialogOpen(false);
    setKernelsManagementDialogOpen(false);

    setCurrentPage(page);
    switch (page) {
      case "profiles":
        break;
      case "settings":
        setSettingsDialogOpen(true);
        break;
      case "proxies":
        setProxyManagementInitialTab("proxies");
        setProxyManagementDialogOpen(true);
        break;
      case "extensions":
        setExtensionManagementDialogOpen(true);
        break;
      case "groups":
        setGroupManagementDialogOpen(true);
        break;
      case "kernels":
        setKernelsManagementDialogOpen(true);
        break;
      case "integrations":
        setIntegrationsDialogOpen(true);
        break;
      case "import":
        setImportProfileDialogOpen(true);
        break;
      case "vpns":
        // VPNs share the proxy management page; pre-select the VPN tab so
        // the user lands directly on the right list.
        setProxyManagementInitialTab("vpns");
        setProxyManagementDialogOpen(true);
        break;
      case "shortcuts":
        // Plain page render — nothing else to open.
        break;
    }
  }, []);

  const runShortcut = useCallback(
    (id: ShortcutId) => {
      switch (id) {
        case "openPalette":
          setCommandPaletteOpen(true);
          break;
        case "openShortcuts":
          handleRailNavigate("shortcuts");
          break;
        case "importProfile":
          handleRailNavigate("import");
          break;
        case "goProfiles":
          handleRailNavigate("profiles");
          break;
        case "goProxies": {
          // Mod+N: navigate first time; flip proxies↔vpns on subsequent presses.
          // handleRailNavigate("proxies"|"vpns") already updates the dialog's
          // initialTab, so we just pick the right destination.
          if (currentPage === "proxies") {
            handleRailNavigate("vpns");
          } else if (currentPage === "vpns") {
            handleRailNavigate("proxies");
          } else {
            handleRailNavigate(
              proxyManagementInitialTab === "vpns" ? "vpns" : "proxies",
            );
          }
          break;
        }
        case "goExtensions": {
          // Mod+E: flip extensions↔groups tab inside the dialog when already there.
          if (currentPage === "extensions") {
            setExtensionManagementInitialTab((cur) =>
              cur === "extensions" ? "groups" : "extensions",
            );
          } else {
            handleRailNavigate("extensions");
          }
          break;
        }
        case "goGroups":
          handleRailNavigate("groups");
          break;
        case "goIntegrations": {
          // Mod+I: flip api↔mcp tab when already on integrations.
          if (currentPage === "integrations") {
            setIntegrationsInitialTab((cur) => (cur === "api" ? "mcp" : "api"));
          } else {
            handleRailNavigate("integrations");
          }
          break;
        }
        case "goAccount":
          // Account/subscription UI removed in local-first build.
          break;
        case "goSettings":
          handleRailNavigate("settings");
          break;
      }
    },
    [handleRailNavigate, currentPage, proxyManagementInitialTab],
  );

  // Ordered list the digit shortcuts and palette consume. "__all__" is index 1
  // so Mod+1 always lands on the unfiltered view; the user's groups follow.
  const orderedGroupTargets = useMemo(
    () => [
      { id: "__all__", name: t("rail.profiles") },
      ...groupsData.map((g) => ({ id: g.id, name: g.name })),
    ],
    [groupsData, t],
  );

  const selectGroupByDigit = useCallback(
    (digit: number) => {
      const target = orderedGroupTargets[digit - 1];
      if (!target) return;
      handleRailNavigate("profiles");
      handleSelectGroup(target.id);
    },
    [orderedGroupTargets, handleRailNavigate, handleSelectGroup],
  );

  useEffect(() => {
    // Global keydown — handles Mod+1..9 group jumps first, then falls back to
    // the static SHORTCUTS table. Skipped while typing in an input, EXCEPT
    // ⌘K and ⌘/ which are meta-level shortcuts and should always be reachable.
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const tag = target?.tagName;
      const isTyping =
        tag === "INPUT" ||
        tag === "TEXTAREA" ||
        tag === "SELECT" ||
        target?.isContentEditable === true;

      const digit = matchesGroupDigit(e);
      if (digit !== null) {
        if (isTyping) return;
        if (digit - 1 >= orderedGroupTargets.length) return;
        e.preventDefault();
        selectGroupByDigit(digit);
        return;
      }

      for (const s of SHORTCUTS) {
        if (!matchesShortcut(s, e)) continue;
        if (isTyping && s.id !== "openPalette" && s.id !== "openShortcuts") {
          return;
        }
        e.preventDefault();
        runShortcut(s.id);
        return;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [runShortcut, selectGroupByDigit, orderedGroupTargets.length]);

  const [processingUrls, setProcessingUrls] = useState<Set<string>>(new Set());

  const handleUrlOpen = useCallback(
    (url: string) => {
      // Prevent duplicate processing of the same URL
      if (processingUrls.has(url)) {
        console.log("URL already being processed:", url);
        return;
      }

      setProcessingUrls((prev) => new Set(prev).add(url));

      try {
        console.log("URL received for opening:", url);

        // Always show profile selector for manual selection - never auto-open
        // Replace any existing pending URL with the new one
        setPendingUrls([{ id: Date.now().toString(), url }]);
      } finally {
        // Remove URL from processing set after a short delay to prevent rapid duplicates
        setTimeout(() => {
          setProcessingUrls((prev) => {
            const next = new Set(prev);
            next.delete(url);
            return next;
          });
        }, 1000);
      }
    },
    [processingUrls],
  );

  // Check for startup URLs but only process them once
  const [hasCheckedStartupUrl, setHasCheckedStartupUrl] = useState(false);
  const checkCurrentUrl = useCallback(async () => {
    if (hasCheckedStartupUrl) return;

    try {
      const currentUrl = await getCurrent();
      if (currentUrl && currentUrl.length > 0) {
        console.log("Startup URL detected:", currentUrl[0]);
        handleUrlOpen(currentUrl[0]);
      }
    } catch (error) {
      console.error("Failed to check current URL:", error);
    } finally {
      setHasCheckedStartupUrl(true);
    }
  }, [handleUrlOpen, hasCheckedStartupUrl]);

  // Handle profile errors from useProfileEvents hook
  useEffect(() => {
    if (profilesError) {
      showErrorToast(profilesError);
    }
  }, [profilesError]);

  // Handle group errors from useGroupEvents hook
  useEffect(() => {
    if (groupsError) {
      showErrorToast(groupsError);
    }
  }, [groupsError]);

  // Handle proxy errors from useProxyEvents hook
  useEffect(() => {
    if (proxiesError) {
      showErrorToast(proxiesError);
    }
  }, [proxiesError]);

  const checkAllPermissions = useCallback(() => {
    try {
      // Wait for permissions to be initialized before checking
      if (!isInitialized) {
        return;
      }

      // Check if any permissions are not granted - prioritize missing permissions
      if (!isMicrophoneAccessGranted) {
        setCurrentPermissionType("microphone");
        setPermissionDialogOpen(true);
      } else if (!isCameraAccessGranted) {
        setCurrentPermissionType("camera");
        setPermissionDialogOpen(true);
      }
    } catch (error) {
      console.error("Failed to check permissions:", error);
    }
  }, [isMicrophoneAccessGranted, isCameraAccessGranted, isInitialized]);

  const checkNextPermission = useCallback(
    (justGranted?: PermissionType) => {
      try {
        // Treat the just-granted permission as already granted even if our
        // own usePermissions instance hasn't observed it yet — it polls on a
        // 5 s cadence and would otherwise leave the dialog stuck on the
        // permission the user just successfully granted.
        const micGranted =
          isMicrophoneAccessGranted || justGranted === "microphone";
        const camGranted = isCameraAccessGranted || justGranted === "camera";

        if (!micGranted) {
          setCurrentPermissionType("microphone");
          setPermissionDialogOpen(true);
        } else if (!camGranted) {
          setCurrentPermissionType("camera");
          setPermissionDialogOpen(true);
        } else {
          setPermissionDialogOpen(false);
        }
      } catch (error) {
        console.error("Failed to check next permission:", error);
      }
    },
    [isMicrophoneAccessGranted, isCameraAccessGranted],
  );

  const listenForUrlEvents = useCallback(async () => {
    // Collect every listener we register so that — whether setup completes or
    // throws partway through — we tear down exactly what was registered.
    // Previously the Tauri unlisten handles were discarded (so re-runs stacked
    // duplicate handlers and a single URL was handled N times), and a failing
    // listen() call would leak the listeners that had already succeeded.
    const unlisteners: Array<() => void> = [];
    let handleLogoUrlEvent: ((event: CustomEvent) => void) | undefined;
    const teardown = () => {
      for (const unlisten of unlisteners) unlisten();
      if (handleLogoUrlEvent) {
        window.removeEventListener(
          "url-open-request",
          handleLogoUrlEvent as EventListener,
        );
      }
    };

    try {
      // Listen for URL open events from the deep link handler (when app is already running)
      unlisteners.push(
        await listen<string>("url-open-request", (event) => {
          console.log("Received URL open request:", event.payload);
          handleUrlOpen(event.payload);
        }),
      );

      // Listen for show profile selector events
      unlisteners.push(
        await listen<string>("show-profile-selector", (event) => {
          console.log("Received show profile selector request:", event.payload);
          handleUrlOpen(event.payload);
        }),
      );

      // Listen for show create profile dialog events
      unlisteners.push(
        await listen<string>("show-create-profile-dialog", (event) => {
          console.log(
            "Received show create profile dialog request:",
            event.payload,
          );
          showErrorToast(t("errors.noProfilesForUrl"));
          setCreateProfileDialogOpen(true);
        }),
      );

      // Listen for custom logo click events
      handleLogoUrlEvent = (event: CustomEvent) => {
        console.log("Received logo URL event:", event.detail);
        handleUrlOpen(event.detail);
      };
      window.addEventListener(
        "url-open-request",
        handleLogoUrlEvent as EventListener,
      );

      return teardown;
    } catch (error) {
      console.error("Failed to setup URL listener:", error);
      // Tear down whatever did register before the failure so nothing leaks.
      teardown();
    }
  }, [handleUrlOpen, t]);

  const handleConfigureIdentity = useCallback((profile: BrowserProfile) => {
    setCurrentProfileForIdentity(profile);
    setIdentityDialogOpen(true);
  }, []);

  const handleOpenAudit = useCallback((profile: BrowserProfile) => {
    setCurrentProfileForAudit(profile);
    setAuditDialogOpen(true);
  }, []);

  useEffect(() => {
    const onOpenAudit = (ev: Event) => {
      const detail = (ev as CustomEvent<{ profileId?: string }>).detail;
      const id = detail?.profileId;
      if (!id) return;
      const p = profiles.find((x) => x.id === id);
      if (p) handleOpenAudit(p);
    };
    window.addEventListener("open-fingerprint-audit", onOpenAudit);
    return () => {
      window.removeEventListener("open-fingerprint-audit", onOpenAudit);
    };
  }, [profiles, handleOpenAudit]);

  const handleCreateProfile = useCallback(
    async (profileData: {
      name: string;
      browserStr: BrowserTypeString;
      version: string;
      releaseType: string;
      proxyId?: string;
      vpnId?: string;
      groupId?: string;
      extensionGroupId?: string;
      ephemeral?: boolean;
      dnsBlocklist?: string;
      launchHook?: string;
      password?: string;
    }) => {
      try {
        const profile = await invoke<BrowserProfile>(
          "create_browser_profile_new",
          {
            name: profileData.name,
            browserStr: profileData.browserStr,
            version: profileData.version,
            releaseType: profileData.releaseType,
            proxyId: profileData.proxyId,
            vpnId: profileData.vpnId,
            groupId:
              profileData.groupId ??
              (selectedGroupId && selectedGroupId !== "__all__"
                ? selectedGroupId
                : undefined),
            ephemeral: profileData.ephemeral,
            dnsBlocklist: profileData.dnsBlocklist,
            launchHook: profileData.launchHook,
          },
        );

        if (profileData.extensionGroupId) {
          try {
            await invoke("assign_extension_group_to_profile", {
              profileId: profile.id,
              extensionGroupId: profileData.extensionGroupId,
            });
          } catch (err) {
            console.error("Failed to assign extension group:", err);
          }
        }

        if (profileData.password && !profileData.ephemeral) {
          try {
            await invoke("set_profile_password", {
              profileId: profile.id,
              password: profileData.password,
            });
          } catch (err) {
            showErrorToast(
              t("errors.setProfilePasswordFailed", {
                error: translateBackendError(t, err),
              }),
            );
          }
        }

        // No need to manually reload - useProfileEvents will handle the update
      } catch (error) {
        showErrorToast(
          t("errors.createProfileFailed", {
            error: translateBackendError(t, error),
          }),
        );
        // Rethrow so the create dialog keeps itself open (its own handler
        // skips closing on error), letting the user fix the proxy/VPN and retry.
        throw error;
      }
    },
    [selectedGroupId, t],
  );

  const launchProfile = useCallback(
    async (profile: BrowserProfile) => {
      console.log("Starting launch for profile:", profile.name);

      // Password-protected: must be unlocked before launch
      if (profile.password_protected) {
        try {
          const isLocked = await invoke<boolean>("is_profile_locked", {
            profileId: profile.id,
          });
          if (isLocked) {
            pendingLaunchAfterUnlockRef.current = profile;
            setPasswordDialogMode("unlock");
            setPasswordDialogProfile(profile);
            return;
          }
        } catch (err) {
          console.error("Failed to check profile lock state:", err);
        }
      }

      // Show one-time warning about window resizing for fingerprinted browsers.
      // Resizing changes the reported viewport and breaks Persona consistency,
      // so this matters for the current kernel too — it previously only fired
      // for the legacy engine.
      if (profile.browser === "fingerprint-chromium") {
        try {
          const dismissed = await invoke<boolean>(
            "get_window_resize_warning_dismissed",
          );
          if (!dismissed) {
            const proceed = await new Promise<boolean>((resolve) => {
              windowResizeWarningResolver.current = resolve;
              setWindowResizeWarningOpen(true);
            });
            if (!proceed) {
              return;
            }
          }
        } catch (error) {
          console.error("Failed to check window resize warning:", error);
        }
      }

      // Local landing page showing what the browser actually presents. Written
      // per launch so it reflects the persona as configured right now. Any
      // failure here is cosmetic — fall through and open the browser anyway.
      try {
        const settings = await invoke<{
          show_workbench_page?: boolean;
          ip_lookup_url?: string | null;
          workbench_reachability_checks?: boolean;
        }>("get_app_settings");
        if (settings.show_workbench_page !== false) {
          // Written as a new-tab-override extension; the backend loads it at
          // launch. Nothing is passed as a URL, so the address bar stays empty.
          const files = buildWorkbenchPage(profile, t, {
            lookupUrl: settings.ip_lookup_url,
            reachability: settings.workbench_reachability_checks !== false,
            groupName: profile.group_id
              ? groupsData.find((g) => g.id === profile.group_id)?.name
              : undefined,
          });
          await invoke("write_workbench_page", {
            profileId: profile.id,
            html: files.html,
            js: files.js,
          });
        }
      } catch (error) {
        console.error("Failed to prepare the workbench page:", error);
      }

      try {
        const result = await invoke<BrowserProfile>("launch_browser_profile", {
          profile,
        });
        console.log("Successfully launched profile:", result.name);
      } catch (err: unknown) {
        console.error("Failed to launch browser:", err);
        // Backend errors arrive as a JSON code object; String(err) would put the
        // raw `{"code":...}` in front of the user, untranslated.
        showErrorToast(
          t("errors.launchBrowserFailed", {
            error: translateBackendError(t, err),
          }),
        );
        // Callers use this to clear their own launching state. Every call site
        // must handle it — a bare `void launchProfile(...)` becomes an unhandled
        // rejection, because the toast above does not "handle" the promise.
        throw err;
      }
    },
    [t, groupsData],
  );

  const handleCloneProfile = useCallback((profile: BrowserProfile) => {
    setCloneProfile(profile);
  }, []);

  const handleSetPassword = useCallback((profile: BrowserProfile) => {
    pendingLaunchAfterUnlockRef.current = null;
    setPasswordDialogMode("set");
    setPasswordDialogProfile(profile);
  }, []);

  const handleChangePassword = useCallback((profile: BrowserProfile) => {
    pendingLaunchAfterUnlockRef.current = null;
    setPasswordDialogMode("change");
    setPasswordDialogProfile(profile);
  }, []);

  const handleRemovePassword = useCallback((profile: BrowserProfile) => {
    pendingLaunchAfterUnlockRef.current = null;
    setPasswordDialogMode("remove");
    setPasswordDialogProfile(profile);
  }, []);

  const handleDeleteProfile = useCallback(
    async (profile: BrowserProfile) => {
      console.log("Attempting to delete profile:", profile.name);

      try {
        // First check if the browser is running for this profile
        const isRunning = await invoke<boolean>("check_browser_status", {
          profile,
        });

        if (isRunning) {
          showErrorToast(t("errors.cannotDeleteRunningProfile"));
          return;
        }

        // Attempt to delete the profile
        await invoke("delete_profile", { profileId: profile.id });
        console.log("Profile deletion command completed successfully");

        // No need to manually reload - useProfileEvents will handle the update
        console.log("Profile deleted successfully");
      } catch (err: unknown) {
        console.error("Failed to delete profile:", err);
        const errorMessage = err instanceof Error ? err.message : String(err);
        showErrorToast(
          t("errors.deleteProfileFailed", { error: errorMessage }),
        );
      }
    },
    [t],
  );

  const handleRenameProfile = useCallback(
    async (profileId: string, newName: string) => {
      try {
        await invoke("rename_profile", { profileId, newName });
        // No need to manually reload - useProfileEvents will handle the update
      } catch (err: unknown) {
        console.error("Failed to rename profile:", err);
        showErrorToast(
          t("errors.renameProfileFailed", { error: JSON.stringify(err) }),
        );
        throw err;
      }
    },
    [t],
  );

  const handleKillProfile = useCallback(
    async (profile: BrowserProfile) => {
      console.log("Starting kill for profile:", profile.name);

      try {
        await invoke("kill_browser_profile", { profile });
        console.log("Successfully killed profile:", profile.name);
        // No need to manually reload - useProfileEvents will handle the update
      } catch (err: unknown) {
        console.error("Failed to kill browser:", err);
        showErrorToast(
          t("errors.killBrowserFailed", {
            error: translateBackendError(t, err),
          }),
        );
        // Re-throw the error so the table component can handle loading state cleanup
        throw err;
      }
    },
    [t],
  );

  const handleDeleteSelectedProfiles = useCallback(
    async (profileIds: string[]) => {
      try {
        await invoke("delete_selected_profiles", { profileIds });
        // No need to manually reload - useProfileEvents will handle the update
      } catch (err: unknown) {
        console.error("Failed to delete selected profiles:", err);
        showErrorToast(
          t("errors.deleteSelectedProfilesFailed", {
            error: JSON.stringify(err),
          }),
        );
      }
    },
    [t],
  );

  const handleAssignProfilesToGroup = useCallback((profileIds: string[]) => {
    setSelectedProfilesForGroup(profileIds);
    setGroupAssignmentDialogOpen(true);
  }, []);

  const handleBulkDelete = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    setShowBulkDeleteConfirmation(true);
  }, [selectedProfiles]);

  const confirmBulkDelete = useCallback(async () => {
    if (selectedProfiles.length === 0) return;

    setIsBulkDeleting(true);
    try {
      await invoke("delete_selected_profiles", {
        profileIds: selectedProfiles,
      });
      // No need to manually reload - useProfileEvents will handle the update
      setSelectedProfiles([]);
      setShowBulkDeleteConfirmation(false);
    } catch (error) {
      console.error("Failed to delete selected profiles:", error);
      showErrorToast(
        t("errors.deleteSelectedProfilesFailed", {
          error: JSON.stringify(error),
        }),
      );
    } finally {
      setIsBulkDeleting(false);
    }
  }, [selectedProfiles, t]);

  const handleBulkGroupAssignment = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    handleAssignProfilesToGroup(selectedProfiles);
    setSelectedProfiles([]);
  }, [selectedProfiles, handleAssignProfilesToGroup]);

  const handleAssignExtensionGroup = useCallback((profileIds: string[]) => {
    setSelectedProfilesForExtensionGroup(profileIds);
    setExtensionGroupAssignmentDialogOpen(true);
  }, []);

  const handleBulkExtensionGroupAssignment = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    handleAssignExtensionGroup(selectedProfiles);
    setSelectedProfiles([]);
  }, [selectedProfiles, handleAssignExtensionGroup]);

  const handleExtensionGroupAssignmentComplete = useCallback(() => {
    setExtensionGroupAssignmentDialogOpen(false);
    setSelectedProfilesForExtensionGroup([]);
  }, []);

  const handleAssignProfilesToProxy = useCallback((profileIds: string[]) => {
    setSelectedProfilesForProxy(profileIds);
    setProxyAssignmentDialogOpen(true);
  }, []);

  const handleBulkProxyAssignment = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    handleAssignProfilesToProxy(selectedProfiles);
    setSelectedProfiles([]);
  }, [selectedProfiles, handleAssignProfilesToProxy]);

  const handleBulkCopyCookies = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    const eligibleProfiles = profiles.filter(
      (p) =>
        selectedProfiles.includes(p.id) && p.browser === "fingerprint-chromium",
    );
    if (eligibleProfiles.length === 0) {
      showErrorToast(t("errors.cookieCopyUnsupportedBrowser"));
      return;
    }
    setSelectedProfilesForCookies(eligibleProfiles.map((p) => p.id));
    setCookieCopyDialogOpen(true);
  }, [selectedProfiles, profiles, t]);

  const [pendingBulkAction, setPendingBulkAction] = useState<{
    action: "run" | "stop";
    profiles: BrowserProfile[];
  } | null>(null);
  const [isBulkActing, setIsBulkActing] = useState(false);

  const executeBulkRun = useCallback(
    async (targets: BrowserProfile[]) => {
      setIsBulkActing(true);
      try {
        await Promise.allSettled(targets.map((p) => launchProfile(p)));
        setSelectedProfiles([]);
      } finally {
        setIsBulkActing(false);
        setPendingBulkAction(null);
      }
    },
    [launchProfile],
  );

  const executeBulkStop = useCallback(
    async (targets: BrowserProfile[]) => {
      setIsBulkActing(true);
      try {
        await Promise.allSettled(targets.map((p) => handleKillProfile(p)));
        setSelectedProfiles([]);
      } finally {
        setIsBulkActing(false);
        setPendingBulkAction(null);
      }
    },
    [handleKillProfile],
  );

  // Bulk run/stop only touch eligible profiles (run: not already running;
  // stop: currently running). An empty result shows a toast instead of a silent
  // no-op (guard), and 10+ targets require confirmation before launching/stopping.
  const handleBulkRun = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    const targets = profiles.filter(
      (p) => selectedProfiles.includes(p.id) && !runningProfiles.has(p.id),
    );
    if (targets.length === 0) {
      showErrorToast(t("profiles.bulkRun.noneToRun"));
      return;
    }
    if (targets.length >= 10) {
      setPendingBulkAction({ action: "run", profiles: targets });
      return;
    }
    void executeBulkRun(targets);
  }, [selectedProfiles, profiles, runningProfiles, executeBulkRun, t]);

  const handleBulkStop = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    const targets = profiles.filter(
      (p) => selectedProfiles.includes(p.id) && runningProfiles.has(p.id),
    );
    if (targets.length === 0) {
      showErrorToast(t("profiles.bulkStop.noneToStop"));
      return;
    }
    if (targets.length >= 10) {
      setPendingBulkAction({ action: "stop", profiles: targets });
      return;
    }
    void executeBulkStop(targets);
  }, [selectedProfiles, profiles, runningProfiles, executeBulkStop, t]);

  const handleCopyCookiesToProfile = useCallback((profile: BrowserProfile) => {
    setSelectedProfilesForCookies([profile.id]);
    setCookieCopyDialogOpen(true);
  }, []);

  const handleOpenCookieManagement = useCallback((profile: BrowserProfile) => {
    setCurrentProfileForCookieManagement(profile);
    setCookieManagementDialogOpen(true);
  }, []);

  const handleGroupAssignmentComplete = useCallback(() => {
    // No need to manually reload - useProfileEvents will handle the update
    setGroupAssignmentDialogOpen(false);
    setSelectedProfilesForGroup([]);
  }, []);

  const handleProxyAssignmentComplete = useCallback(() => {
    // No need to manually reload - useProfileEvents will handle the update
    setProxyAssignmentDialogOpen(false);
    setSelectedProfilesForProxy([]);
  }, []);

  const handleGroupManagementComplete = useCallback(async () => {
    // No need to manually reload - useProfileEvents will handle the update
  }, []);

  const handleOpenProfileSyncDialog = useCallback((profile: BrowserProfile) => {
    setCurrentProfileForSync(profile);
    setProfileSyncDialogOpen(true);
  }, []);

  const handleToggleProfileSync = useCallback(
    async (profile: BrowserProfile) => {
      try {
        const enabling = !profile.sync_mode || profile.sync_mode === "Disabled";
        await invoke("set_profile_sync_mode", {
          profileId: profile.id,
          syncMode: enabling ? "Regular" : "Disabled",
        });
        showSuccessToast(
          t(enabling ? "sync.enabledToast" : "sync.disabledToast"),
          {
            description: t(
              enabling ? "sync.enabledDescription" : "sync.disabledDescription",
            ),
          },
        );
      } catch (error) {
        console.error("Failed to toggle sync:", error);
        showErrorToast(t("errors.updateSyncSettingsFailed"));
      }
    },
    [t],
  );

  useEffect(() => {
    let disposed = false;
    let unlistenStatus: (() => void) | undefined;
    let unlistenProgress: (() => void) | undefined;
    const profilesWithTransfer = new Set<string>();
    void (async () => {
      try {
        unlistenStatus = await listen<{
          profile_id: string;
          status: string;
          error?: string;
          profile_name?: string;
        }>("profile-sync-status", (event) => {
          const { profile_id, status, error, profile_name } = event.payload;
          const toastId = `sync-${profile_id}`;
          const profile = profiles.find((p) => p.id === profile_id);
          const name =
            profile_name || profile?.name || t("common.labels.unknownProfile");

          if (status === "synced") {
            dismissToast(toastId);
            if (profilesWithTransfer.has(profile_id)) {
              profilesWithTransfer.delete(profile_id);
              showSuccessToast(t("sync.toast.profileSynced", { name }));
            }
          } else if (status === "error") {
            dismissToast(toastId);
            profilesWithTransfer.delete(profile_id);
            showErrorToast(
              error
                ? t("sync.toast.profileSyncFailedWithError", { name, error })
                : t("sync.toast.profileSyncFailed", { name }),
            );
          }
        });

        unlistenProgress = await listen<{
          profile_id: string;
          phase: string;
          total_files?: number;
          total_bytes?: number;
          completed_files?: number;
          completed_bytes?: number;
          speed_bytes_per_sec?: number;
          eta_seconds?: number;
          failed_count?: number;
          profile_name?: string;
        }>("profile-sync-progress", (event) => {
          const payload = event.payload;
          const toastId = `sync-${payload.profile_id}`;
          const profile = profiles.find((p) => p.id === payload.profile_id);
          const name =
            payload.profile_name ||
            profile?.name ||
            t("common.labels.unknownProfile");

          if (
            payload.phase === "started" ||
            payload.phase === "uploading" ||
            payload.phase === "downloading"
          ) {
            profilesWithTransfer.add(payload.profile_id);
            showSyncProgressToast(
              name,
              {
                completed_files: payload.completed_files ?? 0,
                total_files: payload.total_files ?? 0,
                completed_bytes: payload.completed_bytes ?? 0,
                total_bytes: payload.total_bytes ?? 0,
                speed_bytes_per_sec: payload.speed_bytes_per_sec ?? 0,
                eta_seconds: payload.eta_seconds ?? 0,
                failed_count: payload.failed_count ?? 0,
                phase: payload.phase,
              },
              { id: toastId, profileId: payload.profile_id },
            );
          }
        });
        // If the effect was torn down while we were awaiting the listeners,
        // unlisten immediately — the cleanup below already ran and would have
        // missed these handles. (Tauri unlisten is safe to call more than once.)
        if (disposed) {
          unlistenStatus?.();
          unlistenProgress?.();
        }
      } catch (error) {
        console.error("Failed to listen for sync events:", error);
      }
    })();
    return () => {
      disposed = true;
      if (unlistenStatus) unlistenStatus();
      if (unlistenProgress) unlistenProgress();
    };
  }, [profiles, t]);

  useEffect(() => {
    // Listen for URL open events. Guard against the effect tearing down (or
    // re-running) before the async listener setup resolves: if that happens,
    // run the cleanup as soon as it's available so the listeners never leak.
    let cleanup: (() => void) | undefined;
    let disposed = false;
    void listenForUrlEvents().then((cleanupFn) => {
      if (disposed) {
        cleanupFn?.();
        return;
      }
      cleanup = cleanupFn;
    });

    // Check for startup URLs (when app was launched as default browser)
    void checkCurrentUrl();

    return () => {
      disposed = true;
      cleanup?.();
    };
  }, [listenForUrlEvents, checkCurrentUrl]);

  // E2E encryption listeners — surface password-required prompts and rollover
  // progress so the user isn't left guessing whether sealing finished.
  useEffect(() => {
    let disposed = false;
    let unlistenRequired: (() => void) | undefined;
    let unlistenLaunchDegraded: (() => void) | undefined;
    let unlistenStarted: (() => void) | undefined;
    let unlistenProgress: (() => void) | undefined;
    let unlistenCompleted: (() => void) | undefined;

    void (async () => {
      unlistenRequired = await listen(
        "profile-sync-e2e-password-required",
        () => {
          showToast({
            id: "e2e-password-required",
            type: "error",
            title: t("encryption.required.title"),
            description: t("encryption.required.description"),
            duration: 12000,
            action: {
              label: t("encryption.required.openSettings"),
              onClick: () => {
                setSettingsDialogOpen(true);
                setCurrentPage("settings");
              },
            },
          });
        },
      );

      // A synced profile opened without a verified remote state: the server was
      // unreachable, so its data may be behind another device's and nothing is
      // stopping a second device from opening it too. Silence here would look
      // exactly like a normal launch, so say it out loud.
      unlistenLaunchDegraded = await listen<{ profile_name?: string }>(
        "profile-launch-sync-degraded",
        (event) => {
          showToast({
            id: `launch-degraded-${event.payload?.profile_name ?? "profile"}`,
            type: "error",
            title: t("sync.deviceLock.degradedTitle"),
            description: t("sync.deviceLock.degradedDescription", {
              name: event.payload?.profile_name ?? "",
            }),
            duration: 12000,
          });
        },
      );

      unlistenStarted = await listen("e2e-rollover-started", () => {
        showToast({
          id: "e2e-rollover",
          type: "loading",
          title: t("encryption.rollover.startedTitle"),
          description: t("encryption.rollover.startedDescription"),
          duration: Number.POSITIVE_INFINITY,
        });
      });

      unlistenProgress = await listen<{
        stage: string;
        done: number;
        total: number;
      }>("e2e-rollover-progress", (event) => {
        const { stage, done, total } = event.payload;
        showToast({
          id: "e2e-rollover",
          type: "loading",
          title: t("encryption.rollover.progressTitle", {
            stage: t(`encryption.rollover.stage.${stage}`),
          }),
          description: t("encryption.rollover.progressDescription", {
            done,
            total,
          }),
          duration: Number.POSITIVE_INFINITY,
        });
      });

      unlistenCompleted = await listen("e2e-rollover-completed", () => {
        showToast({
          id: "e2e-rollover",
          type: "success",
          title: t("encryption.rollover.completedTitle"),
          description: t("encryption.rollover.completedDescription"),
          duration: 5000,
        });
      });

      // If the effect was torn down mid-setup, the cleanup below already ran
      // before these handles existed — unlisten them now so nothing leaks.
      if (disposed) {
        unlistenRequired?.();
        unlistenLaunchDegraded?.();
        unlistenStarted?.();
        unlistenProgress?.();
        unlistenCompleted?.();
      }
    })();

    return () => {
      disposed = true;
      unlistenRequired?.();
      unlistenLaunchDegraded?.();
      unlistenStarted?.();
      unlistenProgress?.();
      unlistenCompleted?.();
    };
  }, [t]);

  // Check permissions when they are initialized. During first-run onboarding
  // the welcome flow requests permissions, so the standalone dialog is deferred
  // until we know this isn't a first-run onboarding.
  useEffect(() => {
    if (isInitialized && firstRunOnboarding === false) {
      checkAllPermissions();
    }
  }, [isInitialized, firstRunOnboarding, checkAllPermissions]);

  // Check self-hosted sync config on mount and when cloud user changes
  useEffect(() => {
    void checkSelfHostedSync();
  }, [checkSelfHostedSync]);

  // Filter data by selected group and search query
  const filteredProfiles = useMemo(() => {
    let filtered = profiles;

    // Filter by group. "__all__" is a virtual filter that shows every
    // profile (including ungrouped ones). Any other value is a real
    // group id; ungrouped profiles only show through "All".
    if (!selectedGroupId || selectedGroupId === "__all__") {
      filtered = profiles;
    } else {
      filtered = profiles.filter(
        (profile) => profile.group_id === selectedGroupId,
      );
    }

    // Filter by search query
    if (searchQuery.trim()) {
      const query = searchQuery.toLowerCase().trim();
      filtered = filtered.filter((profile) => {
        // Search in profile name
        if (profile.name.toLowerCase().includes(query)) return true;

        // Search in note
        if (profile.note?.toLowerCase().includes(query)) return true;

        // Search in tags
        if (profile.tags?.some((tag) => tag.toLowerCase().includes(query)))
          return true;

        return false;
      });
    }

    return filtered;
  }, [profiles, selectedGroupId, searchQuery]);

  // Update loading states
  const isLoading = profilesLoading || groupsLoading || proxiesLoading;

  const subPageTitle =
    currentPage === "profiles"
      ? undefined
      : currentPage === "import"
        ? t("pageTitle.import")
        : t(`pageTitle.${currentPage}`);

  return (
    <div className="flex h-dvh flex-col bg-background font-(family-name:--font-geist-sans)">
      <CloseConfirmDialog />
      <HomeHeader
        onCreateProfileDialogOpen={setCreateProfileDialogOpen}
        searchQuery={searchQuery}
        onSearchQueryChange={setSearchQuery}
        groups={groupsData}
        totalProfiles={profiles.length}
        selectedGroupId={selectedGroupId}
        onGroupSelect={handleSelectGroup}
        pageTitle={subPageTitle}
        onKernelsOpen={() => {
          setKernelsManagementDialogOpen(true);
        }}
        currentPage={currentPage}
        onNavigate={handleRailNavigate}
      />
      <div className="flex min-h-0 flex-1">
        <RailNav currentPage={currentPage} onNavigate={handleRailNavigate} />
        <main className="flex min-w-0 flex-1 flex-col overflow-hidden">
          {currentPage === "profiles" && (
            <div className="flex min-h-0 flex-1 flex-col px-3 pt-2.5">
              {isLoading && groupsData.length === 0 ? null : null}
              <ProfilesDataTable
                profiles={filteredProfiles}
                infoDialogProfile={profileInfoDialog}
                onInfoDialogProfileChange={setProfileInfoDialog}
                onLaunchProfile={launchProfile}
                onKillProfile={handleKillProfile}
                onCloneProfile={handleCloneProfile}
                onSetPassword={handleSetPassword}
                onChangePassword={handleChangePassword}
                onRemovePassword={handleRemovePassword}
                onDeleteProfile={handleDeleteProfile}
                onRenameProfile={handleRenameProfile}
                onConfigureIdentity={handleConfigureIdentity}
                onCopyCookiesToProfile={handleCopyCookiesToProfile}
                onOpenCookieManagement={handleOpenCookieManagement}
                runningProfiles={runningProfiles}
                isUpdating={() => false}
                onDeleteSelectedProfiles={handleDeleteSelectedProfiles}
                onAssignProfilesToGroup={handleAssignProfilesToGroup}
                selectedGroupId={selectedGroupId}
                selectedProfiles={selectedProfiles}
                onSelectedProfilesChange={setSelectedProfiles}
                onBulkDelete={handleBulkDelete}
                onBulkGroupAssignment={handleBulkGroupAssignment}
                onBulkProxyAssignment={handleBulkProxyAssignment}
                onBulkCopyCookies={handleBulkCopyCookies}
                onBulkRun={handleBulkRun}
                onBulkStop={handleBulkStop}
                bulkActionsUnlocked={automationUnlocked}
                onBulkExtensionGroupAssignment={
                  handleBulkExtensionGroupAssignment
                }
                onAssignExtensionGroup={handleAssignExtensionGroup}
                onOpenProfileSyncDialog={handleOpenProfileSyncDialog}
                onToggleProfileSync={handleToggleProfileSync}
                crossOsUnlocked={crossOsUnlocked}
                syncUnlocked={syncUnlocked}
                getProfileSyncInfo={getProfileSyncInfo}
                onLaunchWithSync={(profile) => {
                  setSyncLeaderProfile(profile);
                }}
              />
            </div>
          )}

          {currentPage === "shortcuts" && (
            <ShortcutsPage groupTargets={orderedGroupTargets} />
          )}

          {settingsDialogOpen && (
            <SettingsDialog
              isOpen={settingsDialogOpen}
              onClose={() => {
                setSettingsDialogOpen(false);
                setCurrentPage("profiles");
              }}
              onIntegrationsOpen={() => {
                setSettingsDialogOpen(false);
                setIntegrationsDialogOpen(true);
                setCurrentPage("integrations");
              }}
              subPage={currentPage === "settings"}
            />
          )}

          {integrationsDialogOpen && (
            <IntegrationsDialog
              isOpen={integrationsDialogOpen}
              onClose={() => {
                setIntegrationsDialogOpen(false);
                setCurrentPage("profiles");
              }}
              subPage={currentPage === "integrations"}
              initialTab={integrationsInitialTab}
            />
          )}

          {proxyManagementDialogOpen && (
            <ProxyManagementDialog
              isOpen={proxyManagementDialogOpen}
              onClose={() => {
                setProxyManagementDialogOpen(false);
                setCurrentPage("profiles");
              }}
              subPage={currentPage === "proxies" || currentPage === "vpns"}
              initialTab={proxyManagementInitialTab}
            />
          )}

          {groupManagementDialogOpen && (
            <GroupManagementDialog
              isOpen={groupManagementDialogOpen}
              onClose={() => {
                setGroupManagementDialogOpen(false);
                setCurrentPage("profiles");
              }}
              onGroupManagementComplete={handleGroupManagementComplete}
              subPage={currentPage === "groups"}
            />
          )}

          {extensionManagementDialogOpen && (
            <ExtensionManagementDialog
              isOpen={extensionManagementDialogOpen}
              onClose={() => {
                setExtensionManagementDialogOpen(false);
                setCurrentPage("profiles");
              }}
              limitedMode={false}
              subPage={currentPage === "extensions"}
              initialTab={extensionManagementInitialTab}
            />
          )}

          {kernelsManagementDialogOpen && (
            <KernelsManagementDialog
              isOpen={kernelsManagementDialogOpen}
              onClose={() => {
                setKernelsManagementDialogOpen(false);
                setCurrentPage("profiles");
              }}
              subPage={currentPage === "kernels"}
            />
          )}

          {importProfileDialogOpen && (
            <ImportProfileDialog
              isOpen={importProfileDialogOpen}
              onClose={() => {
                setImportProfileDialogOpen(false);
                setCurrentPage("profiles");
              }}
              crossOsUnlocked={crossOsUnlocked}
              subPage={currentPage === "import"}
            />
          )}
        </main>
      </div>

      <CreateProfileDialog
        isOpen={createProfileDialogOpen}
        onClose={() => {
          setCreateProfileDialogOpen(false);
        }}
        onCreateProfile={handleCreateProfile}
        selectedGroupId={selectedGroupId}
        crossOsUnlocked={crossOsUnlocked}
      />

      <CommandPalette
        open={commandPaletteOpen}
        onOpenChange={setCommandPaletteOpen}
        onAction={runShortcut}
        groupTargets={orderedGroupTargets}
        onSelectGroup={(id) => {
          handleRailNavigate("profiles");
          handleSelectGroup(id);
        }}
        profiles={profiles}
        runningProfileIds={runningProfiles}
        onLaunchProfile={(profile) => {
          // The failure is already shown as a toast; swallow the rejection so it
          // does not surface as an unhandled rejection.
          void launchProfile(profile).catch(() => {});
        }}
        onKillProfile={(profile) => {
          void handleKillProfile(profile).catch(() => {});
        }}
        onShowProfileInfo={(profile) => {
          handleRailNavigate("profiles");
          setProfileInfoDialog(profile);
        }}
      />

      {pendingUrls.map((pendingUrl) => (
        <ProfileSelectorDialog
          key={pendingUrl.id}
          isOpen={true}
          onClose={() => {
            setPendingUrls((prev) =>
              prev.filter((u) => u.id !== pendingUrl.id),
            );
          }}
          url={pendingUrl.url}
          isUpdating={() => false}
          runningProfiles={runningProfiles}
        />
      ))}

      <PermissionDialog
        isOpen={permissionDialogOpen}
        onClose={() => {
          setPermissionDialogOpen(false);
        }}
        permissionType={currentPermissionType}
        onPermissionGranted={checkNextPermission}
      />

      <WelcomeDialog
        isOpen={welcomeOpen}
        needsSetup={profiles.length === 0}
        onComplete={handleWelcomeComplete}
      />
      <ThankYouDialog
        isOpen={thankYouOpen}
        onClose={() => setThankYouOpen(false)}
      />

      <CloneProfileDialog
        isOpen={!!cloneProfile}
        onClose={() => {
          setCloneProfile(null);
        }}
        profile={cloneProfile}
      />

      <ProfilePasswordDialog
        isOpen={!!passwordDialogProfile}
        onClose={() => {
          pendingLaunchAfterUnlockRef.current = null;
          setPasswordDialogProfile(null);
        }}
        profile={passwordDialogProfile}
        mode={passwordDialogMode}
        onSuccess={(p) => {
          // Resume pending launch after unlock.
          if (
            passwordDialogMode === "unlock" &&
            pendingLaunchAfterUnlockRef.current?.id === p.id
          ) {
            const target = pendingLaunchAfterUnlockRef.current;
            pendingLaunchAfterUnlockRef.current = null;
            void launchProfile(target).catch(() => {});
          }
          // On set/change/remove, the profile's encryption state changed.
          // Push that state to the sync server immediately so other devices
          // see the new envelope before they next pull. Skip if the profile
          // is currently running — its files would be in flux.
          if (
            (passwordDialogMode === "set" ||
              passwordDialogMode === "change" ||
              passwordDialogMode === "remove") &&
            !runningProfiles.has(p.id) &&
            p.sync_mode !== "Disabled"
          ) {
            void invoke("request_profile_sync", { profileId: p.id }).catch(
              (err: unknown) => {
                console.error("post-password sync failed", err);
              },
            );
          }
        }}
      />

      <FingerprintIdentityDialog
        isOpen={identityDialogOpen}
        onClose={() => {
          setIdentityDialogOpen(false);
          setCurrentProfileForIdentity(null);
        }}
        profile={currentProfileForIdentity}
        isRunning={
          currentProfileForIdentity
            ? runningProfiles.has(currentProfileForIdentity.id)
            : false
        }
        onSaved={(p) => {
          setCurrentProfileForIdentity(p);
        }}
      />

      <FingerprintAuditDialog
        isOpen={auditDialogOpen}
        onClose={() => {
          setAuditDialogOpen(false);
          setCurrentProfileForAudit(null);
        }}
        profile={currentProfileForAudit}
      />

      <GroupAssignmentDialog
        isOpen={groupAssignmentDialogOpen}
        onClose={() => {
          setGroupAssignmentDialogOpen(false);
        }}
        selectedProfiles={selectedProfilesForGroup}
        onAssignmentComplete={handleGroupAssignmentComplete}
        profiles={profiles}
      />

      <ExtensionGroupAssignmentDialog
        isOpen={extensionGroupAssignmentDialogOpen}
        onClose={() => {
          setExtensionGroupAssignmentDialogOpen(false);
        }}
        selectedProfiles={selectedProfilesForExtensionGroup}
        onAssignmentComplete={handleExtensionGroupAssignmentComplete}
        profiles={profiles}
      />

      <ProxyAssignmentDialog
        isOpen={proxyAssignmentDialogOpen}
        onClose={() => {
          setProxyAssignmentDialogOpen(false);
        }}
        selectedProfiles={selectedProfilesForProxy}
        onAssignmentComplete={handleProxyAssignmentComplete}
        profiles={profiles}
        storedProxies={storedProxies}
        vpnConfigs={vpnConfigs}
      />

      <CookieCopyDialog
        isOpen={cookieCopyDialogOpen}
        onClose={() => {
          setCookieCopyDialogOpen(false);
          setSelectedProfilesForCookies([]);
        }}
        selectedProfiles={selectedProfilesForCookies}
        profiles={profiles}
        runningProfiles={runningProfiles}
        onCopyComplete={() => {
          setSelectedProfilesForCookies([]);
        }}
      />

      <CookieManagementDialog
        isOpen={cookieManagementDialogOpen}
        onClose={() => {
          setCookieManagementDialogOpen(false);
          setCurrentProfileForCookieManagement(null);
        }}
        profile={currentProfileForCookieManagement}
      />

      <DeleteConfirmationDialog
        isOpen={pendingBulkAction !== null}
        onClose={() => {
          setPendingBulkAction(null);
        }}
        onConfirm={() => {
          if (!pendingBulkAction) return;
          if (pendingBulkAction.action === "run") {
            void executeBulkRun(pendingBulkAction.profiles);
          } else {
            void executeBulkStop(pendingBulkAction.profiles);
          }
        }}
        title={
          pendingBulkAction?.action === "stop"
            ? t("profiles.bulkStop.confirmTitle", {
                count: pendingBulkAction?.profiles.length ?? 0,
              })
            : t("profiles.bulkRun.confirmTitle", {
                count: pendingBulkAction?.profiles.length ?? 0,
              })
        }
        description={
          pendingBulkAction?.action === "stop"
            ? t("profiles.bulkStop.confirmDescription", {
                count: pendingBulkAction?.profiles.length ?? 0,
              })
            : t("profiles.bulkRun.confirmDescription", {
                count: pendingBulkAction?.profiles.length ?? 0,
              })
        }
        confirmButtonText={
          pendingBulkAction?.action === "stop"
            ? t("profiles.bulkStop.confirmButton", {
                count: pendingBulkAction?.profiles.length ?? 0,
              })
            : t("profiles.bulkRun.confirmButton", {
                count: pendingBulkAction?.profiles.length ?? 0,
              })
        }
        confirmButtonVariant="default"
        isLoading={isBulkActing}
      />
      <DeleteConfirmationDialog
        isOpen={showBulkDeleteConfirmation}
        onClose={() => {
          setShowBulkDeleteConfirmation(false);
        }}
        onConfirm={confirmBulkDelete}
        title={t("profiles.bulkDelete.title")}
        description={t("profiles.bulkDelete.description", {
          count: selectedProfiles.length,
        })}
        confirmButtonText={t("profiles.bulkDelete.confirmButton", {
          count: selectedProfiles.length,
        })}
        isLoading={isBulkDeleting}
        profileIds={selectedProfiles}
        profiles={profiles.map((p) => ({ id: p.id, name: p.name }))}
      />

      <SyncConfigDialog
        isOpen={syncConfigDialogOpen}
        onClose={() => {
          setSyncConfigDialogOpen(false);
          void checkSelfHostedSync();
        }}
      />

      <SyncAllDialog
        isOpen={syncAllDialogOpen}
        onClose={() => {
          setSyncAllDialogOpen(false);
        }}
      />

      <ProfileSyncDialog
        isOpen={profileSyncDialogOpen}
        onClose={() => {
          setProfileSyncDialogOpen(false);
          setCurrentProfileForSync(null);
        }}
        profile={currentProfileForSync}
        onSyncConfigOpen={() => {
          setSyncConfigDialogOpen(true);
        }}
      />

      <WindowResizeWarningDialog
        isOpen={windowResizeWarningOpen}
        onResult={(proceed) => {
          setWindowResizeWarningOpen(false);
          windowResizeWarningResolver.current?.(proceed);
          windowResizeWarningResolver.current = null;
        }}
      />

      <SyncFollowerDialog
        isOpen={syncLeaderProfile !== null}
        onClose={() => {
          setSyncLeaderProfile(null);
        }}
        leaderProfile={syncLeaderProfile}
        allProfiles={profiles}
        runningProfiles={runningProfiles}
      />
    </div>
  );
}
