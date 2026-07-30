/**
 * Local-first build: commercial trial modal is permanently acknowledged.
 * No remote commercial license lifecycle is enforced.
 */

export interface TrialStatusActive {
  type: "Active";
  remaining_seconds: number;
  days_remaining: number;
  hours_remaining: number;
  minutes_remaining: number;
}

export interface TrialStatusExpired {
  type: "Expired";
}

export type TrialStatus = TrialStatusActive | TrialStatusExpired;

export function useCommercialTrial(): {
  trialStatus: TrialStatus | null;
  hasAcknowledged: boolean;
  isLoading: boolean;
  checkTrialStatus: () => Promise<void>;
} {
  return {
    trialStatus: {
      type: "Active",
      remaining_seconds: 0,
      days_remaining: 0,
      hours_remaining: 0,
      minutes_remaining: 0,
    },
    hasAcknowledged: true,
    isLoading: false,
    checkTrialStatus: async () => {},
  };
}
