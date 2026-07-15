import type { TFunction } from "i18next";

const STABLE_ERROR_KEYS: Record<string, string> = {
  final_normal_binding_required:
    "settings.general.shortcut.errors.finalNormalBindingRequired",
  recording_in_progress: "settings.general.shortcut.errors.recordingInProgress",
};

export function shortcutErrorMessage(
  error: unknown,
  t: TFunction,
  fallbackKey = "settings.general.shortcut.errors.set",
): string {
  const detail = error instanceof Error ? error.message : String(error);
  const stableKey = STABLE_ERROR_KEYS[detail];
  return stableKey ? t(stableKey) : t(fallbackKey, { error: detail });
}

export function assertCommandSucceeded(
  result: { status: "ok" } | { status: "error"; error: string },
): void {
  if (result.status === "error") throw new Error(result.error);
}
