/**
 * Local-first build: Wayfern third-party license dialog is not required.
 * Terms are treated as accepted so the app never blocks on Wayfern legal UI.
 */
export function useWayfernTerms(): {
  termsAccepted: boolean | null;
  isLoading: boolean;
  checkTerms: () => Promise<void>;
} {
  return {
    termsAccepted: true,
    isLoading: false,
    checkTerms: async () => {},
  };
}
