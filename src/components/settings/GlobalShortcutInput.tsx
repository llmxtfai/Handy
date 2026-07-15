import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  formatKeyCombination,
  getKeyName,
  normalizeKey,
} from "../../lib/utils/keyboard";
import { ResetButton } from "../ui/ResetButton";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";
import { useOsType } from "../../hooks/useOsType";
import { commands } from "@/bindings";
import { toast } from "sonner";
import { ShortcutClearButton } from "./ShortcutClearButton";
import { assertCommandSucceeded, shortcutErrorMessage } from "./shortcutErrors";

interface GlobalShortcutInputProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  shortcutId: string;
  disabled?: boolean;
  clearable?: boolean;
}

const MODIFIERS = new Set([
  "ctrl",
  "control",
  "shift",
  "alt",
  "option",
  "meta",
  "command",
  "cmd",
  "super",
  "win",
  "windows",
]);

export const GlobalShortcutInput: React.FC<GlobalShortcutInputProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
  shortcutId,
  disabled = false,
  clearable = false,
}) => {
  const { t } = useTranslation();
  const tRef = useRef(t);
  tRef.current = t;
  const {
    getSetting,
    updateBinding,
    clearBinding,
    resetBinding,
    isUpdating,
    isLoading,
  } = useSettings();
  const [editing, setEditing] = useState(false);
  const [recordedKeys, setRecordedKeys] = useState<string[]>([]);
  const captureRef = useRef<HTMLDivElement>(null);
  const activeCaptureRef = useRef<string | null>(null);
  const mountedRef = useRef(false);
  const pressedKeysRef = useRef<Set<string>>(new Set());
  const recordedKeysRef = useRef<string[]>([]);
  const osType = useOsType();
  const bindings = getSetting("bindings") || {};

  const resetCaptureState = useCallback(() => {
    activeCaptureRef.current = null;
    pressedKeysRef.current.clear();
    recordedKeysRef.current = [];
    if (mountedRef.current) {
      setRecordedKeys([]);
      setEditing(false);
    }
  }, []);

  const abortCapture = useCallback(async (): Promise<boolean> => {
    const id = activeCaptureRef.current;
    if (!id) return true;
    try {
      assertCommandSucceeded(await commands.abortShortcutCapture(id));
      resetCaptureState();
      return true;
    } catch (error) {
      console.error("Failed to abort shortcut capture:", error);
      if (mountedRef.current) {
        toast.error(shortcutErrorMessage(error, tRef.current));
      }
      return false;
    }
  }, [resetCaptureState]);

  const retryAbortForCleanup = useCallback(async () => {
    for (let attempt = 0; attempt < 3 && activeCaptureRef.current; attempt++) {
      if (await abortCapture()) return;
    }
  }, [abortCapture]);

  // Capture ownership lives in a ref so unmount/window cleanup cannot be
  // skipped by a stale React closure.
  useEffect(() => {
    mountedRef.current = true;
    // A previous renderer may have disappeared before its async cleanup
    // completed. The backend command is idempotent when no capture exists.
    void commands
      .abortShortcutCapture(shortcutId)
      .then(assertCommandSucceeded)
      .catch(console.error);

    const handleWindowCleanup = () => void retryAbortForCleanup();
    window.addEventListener("pagehide", handleWindowCleanup);
    window.addEventListener("blur", handleWindowCleanup);
    return () => {
      mountedRef.current = false;
      window.removeEventListener("pagehide", handleWindowCleanup);
      window.removeEventListener("blur", handleWindowCleanup);
      void retryAbortForCleanup();
    };
  }, [retryAbortForCleanup, shortcutId]);

  useEffect(() => {
    if (!editing) return;

    const clearDuringCapture = async () => {
      try {
        await clearBinding(shortcutId);
        resetCaptureState();
      } catch (error) {
        toast.error(shortcutErrorMessage(error, t));
      }
    };

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.repeat) return;
      event.preventDefault();

      if (event.key === "Escape") {
        void abortCapture();
        return;
      }
      if (clearable && (event.key === "Delete" || event.key === "Backspace")) {
        void clearDuringCapture();
        return;
      }

      const key = normalizeKey(getKeyName(event, osType));
      pressedKeysRef.current.add(key);
      if (!recordedKeysRef.current.includes(key)) {
        recordedKeysRef.current = [...recordedKeysRef.current, key];
        setRecordedKeys(recordedKeysRef.current);
      }
    };

    const handleKeyUp = async (event: KeyboardEvent) => {
      event.preventDefault();
      const key = normalizeKey(getKeyName(event, osType));
      pressedKeysRef.current.delete(key);
      if (
        pressedKeysRef.current.size > 0 ||
        recordedKeysRef.current.length === 0
      ) {
        return;
      }

      const newShortcut = [...recordedKeysRef.current]
        .sort((a, b) => Number(MODIFIERS.has(b)) - Number(MODIFIERS.has(a)))
        .join("+");
      try {
        await updateBinding(shortcutId, newShortcut);
        // change_binding owns the successful backend capture teardown.
        resetCaptureState();
      } catch (error) {
        toast.error(shortcutErrorMessage(error, t));
        await abortCapture();
      }
    };

    const handleClickOutside = (event: MouseEvent) => {
      if (!captureRef.current?.contains(event.target as Node)) {
        void abortCapture();
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);
    window.addEventListener("click", handleClickOutside);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
      window.removeEventListener("click", handleClickOutside);
    };
  }, [
    abortCapture,
    clearBinding,
    clearable,
    editing,
    osType,
    resetCaptureState,
    shortcutId,
    t,
    updateBinding,
  ]);

  const startRecording = async () => {
    if (editing || disabled) return;
    try {
      assertCommandSucceeded(await commands.suspendBinding(shortcutId));
      activeCaptureRef.current = shortcutId;
      if (!mountedRef.current) {
        await retryAbortForCleanup();
        return;
      }
      pressedKeysRef.current.clear();
      recordedKeysRef.current = [];
      setRecordedKeys([]);
      setEditing(true);
    } catch (error) {
      toast.error(shortcutErrorMessage(error, t));
    }
  };

  const handleClear = async () => {
    try {
      await clearBinding(shortcutId);
    } catch (error) {
      toast.error(shortcutErrorMessage(error, t));
    }
  };

  const handleReset = async () => {
    try {
      await resetBinding(shortcutId);
    } catch (error) {
      toast.error(
        shortcutErrorMessage(
          error,
          t,
          "settings.general.shortcut.errors.reset",
        ),
      );
    }
  };

  const commonContainerProps = {
    descriptionMode,
    grouped,
  };
  if (isLoading) {
    return (
      <SettingContainer
        title={t("settings.general.shortcut.title")}
        description={t("settings.general.shortcut.description")}
        {...commonContainerProps}
      >
        <div className="text-sm text-mid-gray">
          {t("settings.general.shortcut.loading")}
        </div>
      </SettingContainer>
    );
  }

  const binding = bindings[shortcutId];
  if (!binding) {
    return (
      <SettingContainer
        title={t("settings.general.shortcut.title")}
        description={t("settings.general.shortcut.notFound")}
        {...commonContainerProps}
      >
        <div className="text-sm text-mid-gray">
          {t("settings.general.shortcut.none")}
        </div>
      </SettingContainer>
    );
  }

  const name = t(
    `settings.general.shortcut.bindings.${shortcutId}.name`,
    binding.name,
  );
  const description = t(
    `settings.general.shortcut.bindings.${shortcutId}.description`,
    binding.description,
  );
  const busy = disabled || isUpdating(`binding_${shortcutId}`);
  const currentBinding = binding.current_binding;
  const clearLabel = t("settings.general.shortcut.clear", { name });

  return (
    <SettingContainer
      title={name}
      description={description}
      {...commonContainerProps}
      disabled={disabled}
      layout="horizontal"
    >
      <div className="flex items-center space-x-1">
        {editing ? (
          <div
            ref={captureRef}
            data-testid={`shortcut-capture-${shortcutId}`}
            role="status"
            aria-live="polite"
            className="px-2 py-1 text-sm font-semibold border border-logo-primary bg-logo-primary/30 rounded-md"
          >
            {recordedKeys.length === 0
              ? t("settings.general.shortcut.pressKeys")
              : formatKeyCombination(recordedKeys.join("+"), osType)}
          </div>
        ) : (
          <button
            type="button"
            disabled={busy}
            onClick={(event) => {
              event.stopPropagation();
              void startRecording();
            }}
            className="px-2 py-1 text-sm font-semibold bg-mid-gray/10 border border-mid-gray/80 hover:bg-logo-primary/10 rounded-md cursor-pointer hover:border-logo-primary disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {currentBinding
              ? formatKeyCombination(currentBinding, osType)
              : t("settings.general.shortcut.set")}
          </button>
        )}
        {clearable && currentBinding && !editing && (
          <ShortcutClearButton
            label={clearLabel}
            disabled={busy}
            onClick={handleClear}
          />
        )}
        <ResetButton
          onClick={handleReset}
          disabled={busy || editing}
          ariaLabel={t("settings.general.shortcut.reset", { name })}
        />
      </div>
    </SettingContainer>
  );
};
