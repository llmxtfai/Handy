import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { formatKeyCombination } from "../../lib/utils/keyboard";
import { ResetButton } from "../ui/ResetButton";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";
import { useOsType } from "../../hooks/useOsType";
import { commands } from "@/bindings";
import { toast } from "sonner";
import { ShortcutClearButton } from "./ShortcutClearButton";
import { assertCommandSucceeded, shortcutErrorMessage } from "./shortcutErrors";

interface HandyKeysShortcutInputProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  shortcutId: string;
  disabled?: boolean;
  clearable?: boolean;
}

interface HandyKeysEvent {
  modifiers: string[];
  key: string | null;
  is_key_down: boolean;
  hotkey_string: string;
}

export const HandyKeysShortcutInput: React.FC<HandyKeysShortcutInputProps> = ({
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
  const [isRecording, setIsRecording] = useState(false);
  const [currentKeys, setCurrentKeys] = useState("");
  const captureRef = useRef<HTMLDivElement>(null);
  const activeCaptureRef = useRef(false);
  const mountedRef = useRef(false);
  const pendingTeardownRef = useRef(false);
  const commitInFlightRef = useRef(false);
  const stopInFlightRef = useRef<Promise<boolean> | null>(null);
  const unlistenRef = useRef<(() => void) | null>(null);
  const currentKeysRef = useRef("");
  const osType = useOsType();
  const bindings = getSetting("bindings") || {};

  const resetCaptureState = useCallback(() => {
    activeCaptureRef.current = false;
    pendingTeardownRef.current = false;
    commitInFlightRef.current = false;
    currentKeysRef.current = "";
    if (mountedRef.current) {
      setCurrentKeys("");
      setIsRecording(false);
    }
  }, []);

  const stopCapture = useCallback((): Promise<boolean> => {
    if (!activeCaptureRef.current) return Promise.resolve(true);
    if (stopInFlightRef.current) return stopInFlightRef.current;
    const stopPromise = (async () => {
      try {
        assertCommandSucceeded(await commands.stopHandyKeysRecording());
        unlistenRef.current?.();
        unlistenRef.current = null;
        resetCaptureState();
        return true;
      } catch (error) {
        console.error("Failed to stop Handy Keys capture:", error);
        if (mountedRef.current) {
          toast.error(shortcutErrorMessage(error, tRef.current));
        }
        return false;
      } finally {
        stopInFlightRef.current = null;
      }
    })();
    stopInFlightRef.current = stopPromise;
    return stopPromise;
  }, [resetCaptureState]);

  const retryStopForCleanup = useCallback(async () => {
    for (let attempt = 0; attempt < 3 && activeCaptureRef.current; attempt++) {
      if (await stopCapture()) return;
    }
  }, [stopCapture]);

  useEffect(() => {
    mountedRef.current = true;
    // Idempotently recover a capture left behind by a renderer that vanished.
    void commands
      .stopHandyKeysRecording()
      .then(assertCommandSucceeded)
      .catch(console.error);

    const handleWindowCleanup = () => void retryStopForCleanup();
    window.addEventListener("pagehide", handleWindowCleanup);
    window.addEventListener("blur", handleWindowCleanup);
    return () => {
      mountedRef.current = false;
      window.removeEventListener("pagehide", handleWindowCleanup);
      window.removeEventListener("blur", handleWindowCleanup);
      void retryStopForCleanup();
    };
  }, [retryStopForCleanup]);

  useEffect(() => {
    if (!isRecording) return;
    let disposed = false;
    void listen<HandyKeysEvent>("handy-keys-event", async (event) => {
      if (disposed) return;
      if (commitInFlightRef.current) return;
      if (pendingTeardownRef.current) {
        await stopCapture();
        return;
      }
      const {
        hotkey_string: hotkey,
        is_key_down: isKeyDown,
        key,
      } = event.payload;
      const clearKey = (key || hotkey).toLowerCase();

      if (
        isKeyDown &&
        clearable &&
        event.payload.modifiers.length === 0 &&
        (clearKey === "delete" || clearKey === "backspace")
      ) {
        commitInFlightRef.current = true;
        try {
          await clearBinding(shortcutId);
          pendingTeardownRef.current = true;
          await stopCapture();
        } catch (error) {
          toast.error(shortcutErrorMessage(error, t));
        } finally {
          commitInFlightRef.current = false;
        }
        return;
      }

      if (isKeyDown && hotkey) {
        currentKeysRef.current = hotkey;
        setCurrentKeys(hotkey);
      } else if (!isKeyDown && currentKeysRef.current) {
        commitInFlightRef.current = true;
        try {
          await updateBinding(shortcutId, currentKeysRef.current);
          pendingTeardownRef.current = true;
          await stopCapture();
        } catch (error) {
          toast.error(shortcutErrorMessage(error, t));
          pendingTeardownRef.current = true;
          await stopCapture();
        } finally {
          commitInFlightRef.current = false;
        }
      }
    }).then((removeListener) => {
      if (disposed) removeListener();
      else unlistenRef.current = removeListener;
    });

    return () => {
      disposed = true;
      unlistenRef.current?.();
      unlistenRef.current = null;
    };
  }, [
    clearBinding,
    clearable,
    isRecording,
    shortcutId,
    stopCapture,
    t,
    updateBinding,
  ]);

  useEffect(() => {
    if (!isRecording) return;
    const handleClickOutside = (event: MouseEvent) => {
      if (!captureRef.current?.contains(event.target as Node)) {
        void stopCapture();
      }
    };
    window.addEventListener("click", handleClickOutside);
    return () => window.removeEventListener("click", handleClickOutside);
  }, [isRecording, stopCapture]);

  const startRecording = async () => {
    if (isRecording || disabled) return;
    try {
      assertCommandSucceeded(
        await commands.startHandyKeysRecording(shortcutId),
      );
      activeCaptureRef.current = true;
      if (!mountedRef.current) {
        await retryStopForCleanup();
        return;
      }
      pendingTeardownRef.current = false;
      currentKeysRef.current = "";
      setCurrentKeys("");
      setIsRecording(true);
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

  const containerProps = { descriptionMode, grouped };
  if (isLoading) {
    return (
      <SettingContainer
        title={t("settings.general.shortcut.title")}
        description={t("settings.general.shortcut.description")}
        {...containerProps}
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
        {...containerProps}
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

  return (
    <SettingContainer
      title={name}
      description={description}
      {...containerProps}
      disabled={disabled}
      layout="horizontal"
    >
      <div className="flex items-center space-x-1">
        {isRecording ? (
          <div
            ref={captureRef}
            data-testid={`shortcut-capture-${shortcutId}`}
            role="status"
            aria-live="polite"
            className="px-2 py-1 text-sm font-semibold border border-logo-primary bg-logo-primary/30 rounded-md"
          >
            {currentKeys
              ? formatKeyCombination(currentKeys, osType)
              : t("settings.general.shortcut.pressKeys")}
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
        {clearable && currentBinding && !isRecording && (
          <ShortcutClearButton
            label={t("settings.general.shortcut.clear", { name })}
            disabled={busy}
            onClick={handleClear}
          />
        )}
        <ResetButton
          onClick={handleReset}
          disabled={busy || isRecording}
          ariaLabel={t("settings.general.shortcut.reset", { name })}
        />
      </div>
    </SettingContainer>
  );
};
