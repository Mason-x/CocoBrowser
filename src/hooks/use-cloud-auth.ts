import type { CloudUser } from "@/types";

interface UseCloudAuthReturn {
  user: CloudUser | null;
  isLoggedIn: boolean;
  isLoading: boolean;
  logout: () => Promise<void>;
}

/**
 * Local-first build: there is no hosted account tier, so this always reports
 * a signed-out session and never issues a `cloud_*` IPC call. Consumers keep
 * their existing signed-out branches; nothing reaches the network.
 */
export function useCloudAuth(): UseCloudAuthReturn {
  return {
    user: null,
    isLoggedIn: false,
    isLoading: false,
    logout: () => Promise.resolve(),
  };
}
