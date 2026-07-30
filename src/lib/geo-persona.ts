import { invoke } from "@tauri-apps/api/core";
import type { BrowserProfile } from "@/types";

/** Align a profile persona timezone/locale to the current proxy exit IP. */
export async function matchProfilePersonaToExit(
  profileId: string,
): Promise<BrowserProfile> {
  return invoke<BrowserProfile>("match_profile_persona_to_exit", {
    profileId,
  });
}
