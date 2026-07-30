import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import type { DeviceIdentity, ProfileLock } from "@/types";

/**
 * Cross-device locks on synced profiles.
 *
 * A lock exists only while a profile is open on some device, so this list is
 * normally empty or has a single entry. Expired locks (left by a device that died
 * without releasing) are filtered out by the backend.
 *
 * Fails soft: with sync unconfigured or the server unreachable the backend returns
 * an empty list, which reads as "nothing locked" — the same behaviour as a device
 * that cannot check, and consistent with the launch path, which warns rather than
 * refusing when it cannot verify.
 */
export function useProfileLocks() {
  const [locks, setLocks] = useState<ProfileLock[]>([]);
  const [deviceId, setDeviceId] = useState<string | null>(null);

  const fetchLocks = useCallback(async () => {
    try {
      setLocks(await invoke<ProfileLock[]>("get_profile_locks"));
    } catch {
      setLocks([]);
    }
  }, []);

  useEffect(() => {
    void (async () => {
      try {
        const identity = await invoke<DeviceIdentity>("get_device_identity");
        setDeviceId(identity.id);
      } catch {
        setDeviceId(null);
      }
    })();
  }, []);

  useEffect(() => {
    void fetchLocks();
    const unlisten = listen("profile-locks-changed", () => void fetchLocks());
    return () => {
      void unlisten.then((fn) => {
        fn();
      });
    };
  }, [fetchLocks]);

  /**
   * True only for a lock held by a *different* device. A lock this device owns is
   * not an obstacle — it is the normal state of a profile open right here.
   */
  const isLockedByAnotherDevice = useCallback(
    (profileId: string): boolean => {
      const lock = locks.find((l) => l.profile_id === profileId);
      if (!lock) return false;
      if (deviceId && lock.device_id === deviceId) return false;
      return true;
    },
    [locks, deviceId],
  );

  const getLockDeviceName = useCallback(
    (profileId: string): string | undefined =>
      locks.find((l) => l.profile_id === profileId)?.device_name,
    [locks],
  );

  return {
    locks,
    deviceId,
    isLockedByAnotherDevice,
    getLockDeviceName,
    refetchLocks: fetchLocks,
  };
}
