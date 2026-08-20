"use client";

import { invoke } from "@tauri-apps/api/core";
import * as React from "react";
import { useTranslation } from "react-i18next";
import MultipleSelector, { type Option } from "@/components/multiple-selector";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import {
  isBackendErrorCode,
  parseBackendError,
  translateBackendError,
} from "@/lib/backend-errors";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import type { BrowserProfile, ProfileGroup } from "@/types";
import { DeleteConfirmationDialog } from "./delete-confirmation-dialog";

const NO_GROUP = "__none__";

interface InstalledKernel {
  id: string;
  version: string;
}

/** One `<Select>` option: which kernel, and which build of it. */
interface KernelTarget {
  kernelId: string;
  version: string;
}

const targetValue = (target: KernelTarget) =>
  `${target.kernelId}|${target.version}`;

const parseTargetValue = (value: string): KernelTarget => {
  const separator = value.indexOf("|");
  return {
    kernelId: value.slice(0, separator),
    version: value.slice(separator + 1),
  };
};

interface ProfileBasicsEditorProps {
  profile: BrowserProfile;
  /** Name and group are structural, so they stay read-only while running. */
  isRunning: boolean;
}

/**
 * In-place editor for the profile fields that used to be read-only cards:
 * name, group, tags and note. Each field saves on its own — the backend
 * emits `profiles-changed`, so the table and this dialog both refresh
 * without any manual reload here.
 */
export function ProfileBasicsEditor({
  profile,
  isRunning,
}: ProfileBasicsEditorProps) {
  const { t } = useTranslation();

  const [name, setName] = React.useState(profile.name);
  const [note, setNote] = React.useState(profile.note ?? "");
  const [groups, setGroups] = React.useState<ProfileGroup[]>([]);
  const [allTags, setAllTags] = React.useState<string[]>([]);
  const [kernels, setKernels] = React.useState<InstalledKernel[]>([]);
  const [downgrade, setDowngrade] = React.useState<{
    target: KernelTarget;
    from: string;
    to: string;
  } | null>(null);
  const [downgradeLoading, setDowngradeLoading] = React.useState(false);

  React.useEffect(() => {
    setName(profile.name);
  }, [profile.name]);

  React.useEffect(() => {
    setNote(profile.note ?? "");
  }, [profile.note]);

  React.useEffect(() => {
    void (async () => {
      try {
        setGroups(await invoke<ProfileGroup[]>("get_profile_groups"));
      } catch (error) {
        console.error("Failed to load groups:", error);
      }
      try {
        setAllTags(await invoke<string[]>("get_all_tags"));
      } catch (error) {
        console.error("Failed to load tags:", error);
      }
      try {
        setKernels(await invoke<InstalledKernel[]>("list_installed_kernels"));
      } catch (error) {
        console.error("Failed to load installed kernels:", error);
      }
    })();
  }, []);

  const commitName = React.useCallback(async () => {
    const next = name.trim();
    if (next === profile.name) return;
    if (!next) {
      showErrorToast(t("profileInfo.basics.nameEmpty"));
      setName(profile.name);
      return;
    }
    try {
      await invoke("rename_profile", {
        profileId: profile.id,
        newName: next,
      });
    } catch (error) {
      showErrorToast(translateBackendError(t as never, error));
      setName(profile.name);
    }
  }, [name, profile.id, profile.name, t]);

  const commitNote = React.useCallback(async () => {
    const next = note.trim();
    if (next === (profile.note ?? "")) return;
    try {
      await invoke("update_profile_note", {
        profileId: profile.id,
        note: next || null,
      });
    } catch (error) {
      showErrorToast(translateBackendError(t as never, error));
      setNote(profile.note ?? "");
    }
  }, [note, profile.id, profile.note, t]);

  const onGroupChange = React.useCallback(
    async (value: string) => {
      try {
        await invoke("assign_profiles_to_group", {
          profileIds: [profile.id],
          groupId: value === NO_GROUP ? null : value,
        });
      } catch (error) {
        showErrorToast(translateBackendError(t as never, error));
      }
    },
    [profile.id, t],
  );

  const kernelLabel = React.useCallback(
    (kernelId: string) => {
      const key = `createProfile.kernelNames.${kernelId}`;
      const label = t(key);
      // Kernels that predate the name table (or arrive from a synced profile)
      // have no key; showing the raw id beats showing the key path.
      return label === key ? kernelId : label;
    },
    [t],
  );

  // The profile's own kernel always appears, even when its build is missing
  // from the install registry, so the control never renders blank.
  const kernelTargets = React.useMemo<KernelTarget[]>(() => {
    const current: KernelTarget = {
      kernelId: profile.browser,
      version: profile.version,
    };
    const targets = kernels.map((kernel) => ({
      kernelId: kernel.id,
      version: kernel.version,
    }));
    if (
      !targets.some((target) => targetValue(target) === targetValue(current))
    ) {
      targets.unshift(current);
    }
    return targets.sort((a, b) =>
      a.kernelId === b.kernelId
        ? b.version.localeCompare(a.version, undefined, { numeric: true })
        : a.kernelId.localeCompare(b.kernelId),
    );
  }, [kernels, profile.browser, profile.version]);

  const switchKernel = React.useCallback(
    async (target: KernelTarget, allowDowngrade: boolean) => {
      try {
        await invoke("switch_profile_kernel", {
          profileId: profile.id,
          kernelId: target.kernelId,
          version: target.version,
          allowDowngrade,
        });
        showSuccessToast(
          t("profileInfo.kernel.switchSuccess", {
            kernel: kernelLabel(target.kernelId),
            version: target.version,
          }),
        );
      } catch (error) {
        if (isBackendErrorCode(error, "KERNEL_DOWNGRADE_BLOCKED")) {
          const params = parseBackendError(error)?.params;
          setDowngrade({
            target,
            from: params?.from ?? "",
            to: params?.to ?? target.version,
          });
          return;
        }
        showErrorToast(translateBackendError(t as never, error));
      }
    },
    [kernelLabel, profile.id, t],
  );

  const onKernelChange = React.useCallback(
    (value: string) => {
      const target = parseTargetValue(value);
      if (
        target.kernelId === profile.browser &&
        target.version === profile.version
      ) {
        return;
      }
      void switchKernel(target, false);
    },
    [profile.browser, profile.version, switchKernel],
  );

  const confirmDowngrade = React.useCallback(async () => {
    if (!downgrade) return;
    setDowngradeLoading(true);
    const target = downgrade.target;
    setDowngrade(null);
    await switchKernel(target, true);
    setDowngradeLoading(false);
  }, [downgrade, switchKernel]);

  const onTagsChange = React.useCallback(
    async (options: Option[]) => {
      const seen = new Set<string>();
      const tags: string[] = [];
      for (const option of options) {
        const value = option.value.trim();
        if (value && !seen.has(value)) {
          seen.add(value);
          tags.push(value);
        }
      }
      try {
        await invoke("update_profile_tags", { profileId: profile.id, tags });
        setAllTags((prev) => Array.from(new Set([...prev, ...tags])).sort());
      } catch (error) {
        showErrorToast(translateBackendError(t as never, error));
      }
    },
    [profile.id, t],
  );

  const tagOptions = React.useMemo<Option[]>(
    () => (profile.tags ?? []).map((tag) => ({ value: tag, label: tag })),
    [profile.tags],
  );
  const allTagOptions = React.useMemo<Option[]>(
    () => allTags.map((tag) => ({ value: tag, label: tag })),
    [allTags],
  );

  return (
    <div className="flex flex-col gap-3">
      <span className="text-[10px] tracking-wide text-muted-foreground uppercase">
        {t("profileInfo.basics.title")}
      </span>

      <div className="space-y-1.5">
        <Label htmlFor="profile-basics-name">{t("common.labels.name")}</Label>
        <Input
          id="profile-basics-name"
          value={name}
          disabled={isRunning}
          onChange={(e) => {
            setName(e.target.value);
          }}
          onBlur={() => void commitName()}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
            if (e.key === "Escape") setName(profile.name);
          }}
        />
      </div>

      <div className="space-y-1.5">
        <Label htmlFor="profile-basics-group">
          {t("profileInfo.fields.group")}
        </Label>
        <Select
          value={profile.group_id ?? NO_GROUP}
          disabled={isRunning}
          onValueChange={(value) => void onGroupChange(value)}
        >
          <SelectTrigger id="profile-basics-group" className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NO_GROUP}>
              {t("profileInfo.values.none")}
            </SelectItem>
            {groups.map((group) => (
              <SelectItem key={group.id} value={group.id}>
                {group.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="space-y-1.5">
        <Label htmlFor="profile-basics-kernel">
          {t("profileInfo.kernel.title")}
        </Label>
        <Select
          value={targetValue({
            kernelId: profile.browser,
            version: profile.version,
          })}
          disabled={isRunning}
          onValueChange={onKernelChange}
        >
          <SelectTrigger id="profile-basics-kernel" className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {kernelTargets.map((target) => (
              <SelectItem key={targetValue(target)} value={targetValue(target)}>
                {t("profileInfo.kernel.option", {
                  kernel: kernelLabel(target.kernelId),
                  version: target.version,
                })}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <p className="text-xs text-muted-foreground">
          {t("profileInfo.kernel.switchHint")}
        </p>
      </div>

      <div className="space-y-1.5">
        <Label>{t("profileInfo.fields.tags")}</Label>
        <MultipleSelector
          value={tagOptions}
          options={allTagOptions}
          onChange={(options) => void onTagsChange(options)}
          creatable
          selectFirstItem={false}
          placeholder={t("profileTable.addTagsPlaceholder")}
        />
      </div>

      <div className="space-y-1.5">
        <Label htmlFor="profile-basics-note">
          {t("profileInfo.fields.note")}
        </Label>
        <Textarea
          id="profile-basics-note"
          value={note}
          rows={3}
          placeholder={t("profileInfo.basics.notePlaceholder")}
          onChange={(e) => {
            setNote(e.target.value);
          }}
          onBlur={() => void commitNote()}
        />
      </div>

      {isRunning && (
        <p className="text-xs text-muted-foreground">
          {t("profileInfo.basics.runningHint")}
        </p>
      )}

      <DeleteConfirmationDialog
        isOpen={downgrade !== null}
        onClose={() => {
          setDowngrade(null);
        }}
        onConfirm={() => void confirmDowngrade()}
        title={t("profileInfo.kernel.downgradeTitle")}
        description={t("profileInfo.kernel.downgradeDescription", {
          from: downgrade?.from ?? "",
          to: downgrade?.to ?? "",
        })}
        confirmButtonText={t("profileInfo.kernel.downgradeConfirm")}
        isLoading={downgradeLoading}
      />
    </div>
  );
}
