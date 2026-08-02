/**
 * String references kept so Tauri commands that remain registered for
 * migration / diagnostics are still detected by the unused-command test.
 * Prefer the Kernels UI and its verified CloakBrowser install commands.
 */
export const LOCAL_FIRST_COMMAND_REFS = [
  "get_browser_release_types",
  "is_geoip_database_available",
  "download_geoip_database",
] as const;

// Explicit string forms for the unused-command scanner.
void "get_browser_release_types";
void "is_geoip_database_available";
void "download_geoip_database";
