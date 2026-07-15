import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";

type Invocation = { cmd: string; args: Record<string, unknown> };

const defaultSettings = JSON.parse(
  readFileSync(
    new URL("../src-tauri/resources/default_settings.json", import.meta.url),
    "utf8",
  ),
);

const settings = {
  ...defaultSettings,
  onboarding_completed: true,
  experimental_enabled: true,
  post_process_enabled: true,
  keyboard_implementation: "tauri",
  bindings: {
    ...defaultSettings.bindings,
    transcribe: {
      ...defaultSettings.bindings.transcribe,
      current_binding: "",
    },
    push_to_talk: {
      ...defaultSettings.bindings.push_to_talk,
      current_binding: "ctrl+space",
    },
    transcribe_with_post_process: {
      ...defaultSettings.bindings.transcribe_with_post_process,
      current_binding: "",
    },
    push_to_talk_with_post_process: {
      ...defaultSettings.bindings.push_to_talk_with_post_process,
      current_binding: "ctrl+shift+space",
    },
    cancel: {
      id: "cancel",
      name: "Cancel Keyboard Shortcut",
      description: "Cancels recording.",
      default_binding: "escape",
      current_binding: "escape",
    },
  },
};

async function installTauriMock(
  page: Page,
  overrides: Partial<typeof settings> = {},
) {
  await page.addInitScript(
    (initialSettings) => {
      const invocations: Invocation[] = [];
      const callbacks = new Map<number, (value: unknown) => void>();
      const eventHandlers = new Map<string, Set<number>>();
      let callbackId = 1;
      const currentSettings = structuredClone(initialSettings);
      const failures: Record<string, string | string[]> = {};
      const delays: Record<string, number> = {};
      const commandGates = new Map<
        string,
        { promise: Promise<void>; release: () => void }
      >();
      const commandKey = (cmd: string, args: Record<string, unknown>) => {
        const target =
          typeof args.id === "string"
            ? args.id
            : typeof args.bindingId === "string"
              ? args.bindingId
              : null;
        return target ? `${cmd}:${target}` : cmd;
      };
      const holdCommand = (key: string) => {
        if (commandGates.has(key)) return;
        let release = () => {};
        const promise = new Promise<void>((resolve) => {
          release = resolve;
        });
        commandGates.set(key, { promise, release });
      };
      const releaseCommand = (key: string) => {
        const gate = commandGates.get(key);
        if (!gate) throw new Error(`Command is not held: ${key}`);
        commandGates.delete(key);
        gate.release();
      };
      const emit = (event: string, payload: unknown) => {
        for (const handler of eventHandlers.get(event) || []) {
          callbacks.get(handler)?.({ event, id: 1, payload });
        }
      };

      Object.assign(window, {
        __HANDY_TEST__: {
          invocations,
          settings: currentSettings,
          failures,
          delays,
          holdCommand,
          releaseCommand,
          emit,
        },
        __TAURI_OS_PLUGIN_INTERNALS__: {
          platform: "windows",
          os_type: "windows",
          family: "windows",
          arch: "x86_64",
          version: "test",
          exe_extension: "exe",
          eol: "\r\n",
        },
        __TAURI_INTERNALS__: {
          transformCallback(callback: (value: unknown) => void, once = false) {
            const id = callbackId++;
            callbacks.set(id, (value) => {
              callback(value);
              if (once) callbacks.delete(id);
            });
            return id;
          },
          unregisterCallback(id: number) {
            callbacks.delete(id);
          },
          async invoke(cmd: string, args: Record<string, unknown> = {}) {
            invocations.push({ cmd, args });
            const gate =
              commandGates.get(commandKey(cmd, args)) ?? commandGates.get(cmd);
            if (gate) await gate.promise;
            if (delays[cmd]) {
              await new Promise((resolve) => setTimeout(resolve, delays[cmd]));
            }
            const targetedFailureKey =
              typeof args.id === "string" ? `${cmd}:${args.id}` : cmd;
            const failureKey = failures[targetedFailureKey]
              ? targetedFailureKey
              : cmd;
            const failure = failures[failureKey];
            if (Array.isArray(failure) && failure.length > 0) {
              throw failure.shift();
            }
            if (typeof failure === "string") throw failure;
            if (cmd === "plugin:event|listen") {
              const event = String(args.event);
              const handlers = eventHandlers.get(event) || new Set<number>();
              handlers.add(Number(args.handler));
              eventHandlers.set(event, handlers);
              return Number(args.handler);
            }
            if (cmd === "plugin:event|unlisten") {
              eventHandlers
                .get(String(args.event))
                ?.delete(Number(args.eventId));
              return null;
            }
            if (cmd.startsWith("plugin:event|")) return null;
            if (cmd === "get_app_settings" || cmd === "get_default_settings") {
              return structuredClone(currentSettings);
            }
            if (cmd === "get_available_models") return [];
            if (cmd === "get_current_model") return null;
            if (cmd === "get_transcription_model_status") {
              return { status: "unloaded", model_id: null, error: null };
            }
            if (cmd === "get_windows_microphone_permission_status") {
              return { supported: false };
            }
            if (cmd === "get_audio_devices" || cmd === "get_output_devices") {
              return [];
            }
            if (cmd === "is_laptop" || cmd === "is_portable") return false;
            if (cmd === "check_custom_sounds") {
              return { start: false, stop: false };
            }
            if (cmd === "clear_binding") {
              const id = String(args.id);
              currentSettings.bindings[id].current_binding = "";
              return { success: true, binding: currentSettings.bindings[id] };
            }
            if (cmd === "change_binding") {
              const id = String(args.id);
              currentSettings.bindings[id].current_binding = String(
                args.binding,
              );
              return { success: true, binding: currentSettings.bindings[id] };
            }
            if (cmd === "reset_binding") {
              const id = String(args.id);
              currentSettings.bindings[id].current_binding =
                currentSettings.bindings[id].default_binding;
              return { success: true, binding: currentSettings.bindings[id] };
            }
            return null;
          },
        },
        __TAURI_EVENT_PLUGIN_INTERNALS__: {
          unregisterListener(event: string, id: number) {
            callbacks.delete(id);
            eventHandlers.get(event)?.delete(id);
          },
        },
      });
    },
    { ...settings, ...overrides },
  );
}

async function openGeneral(
  page: Page,
  overrides: Partial<typeof settings> = {},
) {
  await installTauriMock(page, overrides);
  await page.goto("/");
  await expect(
    page.getByText("General", { exact: true }).first(),
  ).toBeVisible();
  const recoveryCommand =
    overrides.keyboard_implementation === "handy_keys"
      ? "stop_handy_keys_recording"
      : "abort_shortcut_capture";
  await expect
    .poll(() =>
      page.evaluate(
        (cmd) =>
          window.__HANDY_TEST__.invocations.filter(
            (invocation) => invocation.cmd === cmd,
          ).length,
        recoveryCommand,
      ),
    )
    .toBeGreaterThanOrEqual(4);
}

test.describe("independent recording shortcuts", () => {
  test("renders both normal shortcut actions and a real Set shortcut button", async ({
    page,
  }) => {
    await openGeneral(page);

    await expect(
      page.getByText("Transcribe Shortcut", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText("Push To Talk Shortcut", { exact: true }),
    ).toBeVisible();
    const setShortcut = page.getByRole("button", { name: "Set shortcut" });
    await expect(setShortcut).toBeVisible();
    await setShortcut.focus();
    await expect(setShortcut).toBeFocused();
  });

  test("activates an empty shortcut with Enter and aborts abandoned capture", async ({
    page,
  }) => {
    await openGeneral(page);

    const setShortcut = page.getByRole("button", { name: "Set shortcut" });
    await setShortcut.focus();
    await page.keyboard.press("Enter");
    await expect(
      page.getByText("Press keys...", { exact: true }),
    ).toBeVisible();

    await page.getByText("General", { exact: true }).first().click();
    await expect
      .poll(() =>
        page.evaluate(() =>
          window.__HANDY_TEST__.invocations.some(
            ({ cmd, args }) =>
              cmd === "abort_shortcut_capture" && args.id === "transcribe",
          ),
        ),
      )
      .toBe(true);
  });

  test("activates an empty shortcut with Space", async ({ page }) => {
    await openGeneral(page);

    const setShortcut = page.getByRole("button", { name: "Set shortcut" });
    await setShortcut.focus();
    await page.keyboard.press("Space");
    await expect(
      page.getByText("Press keys...", { exact: true }),
    ).toBeVisible();
  });

  test("successful assignment commits through the backend without aborting", async ({
    page,
  }) => {
    await openGeneral(page);
    const abortsBefore = await page.evaluate(
      () =>
        window.__HANDY_TEST__.invocations.filter(
          ({ cmd, args }) =>
            cmd === "abort_shortcut_capture" && args.id === "transcribe",
        ).length,
    );
    await page.getByRole("button", { name: "Set shortcut" }).click();
    await page.keyboard.press("Control+KeyK");

    await expect
      .poll(() =>
        page.evaluate(() =>
          window.__HANDY_TEST__.invocations.some(
            ({ cmd, args }) =>
              cmd === "change_binding" &&
              args.id === "transcribe" &&
              args.binding === "ctrl+k",
          ),
        ),
      )
      .toBe(true);
    expect(
      await page.evaluate(
        () =>
          window.__HANDY_TEST__.invocations.filter(
            ({ cmd, args }) =>
              cmd === "abort_shortcut_capture" && args.id === "transcribe",
          ).length,
      ),
    ).toBe(abortsBefore);
  });

  test("aborts capture when its settings page unmounts", async ({ page }) => {
    await openGeneral(page);
    const abortsBefore = await page.evaluate(
      () =>
        window.__HANDY_TEST__.invocations.filter(
          ({ cmd, args }) =>
            cmd === "abort_shortcut_capture" && args.id === "transcribe",
        ).length,
    );
    await page.getByRole("button", { name: "Set shortcut" }).click();
    await page.getByText("Post Process", { exact: true }).click();

    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd, args }) =>
                cmd === "abort_shortcut_capture" && args.id === "transcribe",
            ).length,
        ),
      )
      .toBeGreaterThan(abortsBefore);
  });

  test("Global abort failure retains capture and a later outside click retries", async ({
    page,
  }) => {
    await openGeneral(page);
    await page.getByRole("button", { name: "Set shortcut" }).click();
    await page.evaluate(() => {
      window.__HANDY_TEST__.failures.abort_shortcut_capture =
        "temporary_failure";
    });

    await page.getByText("General", { exact: true }).first().click();
    await expect(
      page.getByText("Press keys...", { exact: true }),
    ).toBeVisible();

    await page.evaluate(() => {
      delete window.__HANDY_TEST__.failures.abort_shortcut_capture;
    });
    await page.getByText("General", { exact: true }).first().click();
    await expect(
      page.getByRole("button", { name: "Set shortcut" }),
    ).toBeVisible();
  });

  test("Global pagehide retries teardown and remount recovers stale capture", async ({
    page,
  }) => {
    await openGeneral(page);
    await page.getByRole("button", { name: "Set shortcut" }).click();
    const beforePagehide = await page.evaluate(
      () =>
        window.__HANDY_TEST__.invocations.filter(
          ({ cmd, args }) =>
            cmd === "abort_shortcut_capture" && args.id === "transcribe",
        ).length,
    );
    await page.evaluate(() => {
      window.__HANDY_TEST__.failures["abort_shortcut_capture:transcribe"] = [
        "temporary_failure",
        "temporary_failure",
        "temporary_failure",
        "temporary_failure",
        "temporary_failure",
        "temporary_failure",
      ];
      window.dispatchEvent(new Event("pagehide"));
    });
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd, args }) =>
                cmd === "abort_shortcut_capture" && args.id === "transcribe",
            ).length,
        ),
      )
      .toBeGreaterThanOrEqual(beforePagehide + 3);
    await expect(page.getByTestId("shortcut-capture-transcribe")).toBeVisible();
    expect(
      await page.evaluate(
        () =>
          (
            window.__HANDY_TEST__.failures[
              "abort_shortcut_capture:transcribe"
            ] as string[]
          ).length,
      ),
    ).toBe(3);

    // Invoke the React handler directly so this test isolates unmount cleanup
    // from the separate outside-click abort path.
    await page.evaluate(() => {
      let item: Element | null =
        [...document.querySelectorAll("p")].find(
          (element) => element.textContent?.trim() === "Post Process",
        ) || null;
      let propsKey: string | undefined;
      while (item && !propsKey) {
        propsKey = Object.keys(item).find(
          (key) =>
            key.startsWith("__reactProps$") &&
            typeof (item as unknown as Record<string, { onClick?: unknown }>)[
              key
            ]?.onClick === "function",
        );
        if (!propsKey) item = item.parentElement;
      }
      if (!item || !propsKey) throw new Error("Post Process button not found");
      (item as unknown as Record<string, { onClick: () => void }>)[
        propsKey
      ].onClick();
    });
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd, args }) =>
                cmd === "abort_shortcut_capture" && args.id === "transcribe",
            ).length,
        ),
      )
      .toBeGreaterThanOrEqual(beforePagehide + 6);
    expect(
      await page.evaluate(
        () =>
          (
            window.__HANDY_TEST__.failures[
              "abort_shortcut_capture:transcribe"
            ] as string[]
          ).length,
      ),
    ).toBe(0);
    const beforeRemount = await page.evaluate(
      () =>
        window.__HANDY_TEST__.invocations.filter(
          ({ cmd, args }) =>
            cmd === "abort_shortcut_capture" && args.id === "transcribe",
        ).length,
    );
    await page.evaluate(() => {
      delete window.__HANDY_TEST__.failures[
        "abort_shortcut_capture:transcribe"
      ];
    });
    await page.getByText("General", { exact: true }).first().click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd, args }) =>
                cmd === "abort_shortcut_capture" && args.id === "transcribe",
            ).length,
        ),
      )
      .toBeGreaterThan(beforeRemount);
  });

  test("clears an assigned shortcut from a visible accessible control", async ({
    page,
  }) => {
    await openGeneral(page);

    const clear = page.getByRole("button", {
      name: "Clear Push To Talk Shortcut",
    });
    await expect(clear).toBeVisible();
    await clear.click();
    await expect(
      page.getByRole("button", { name: "Set shortcut" }),
    ).toHaveCount(2);
    expect(
      await page.evaluate(() =>
        window.__HANDY_TEST__.invocations.some(
          ({ cmd, args }) =>
            cmd === "clear_binding" && args.id === "push_to_talk",
        ),
      ),
    ).toBe(true);
  });

  test("Delete clears an optional shortcut while capturing", async ({
    page,
  }) => {
    await openGeneral(page);

    await page
      .getByRole("button", { name: /Ctrl.*Space/i })
      .first()
      .click();
    await page.keyboard.press("Delete");

    await expect
      .poll(() =>
        page.evaluate(() =>
          window.__HANDY_TEST__.invocations.some(
            ({ cmd, args }) =>
              cmd === "clear_binding" && args.id === "push_to_talk",
          ),
        ),
      )
      .toBe(true);
  });

  test("Backspace clears an optional shortcut while capturing", async ({
    page,
  }) => {
    await openGeneral(page);
    await page
      .getByRole("button", { name: /Ctrl.*Space/i })
      .first()
      .click();
    await page.keyboard.press("Backspace");
    await expect
      .poll(() =>
        page.evaluate(() =>
          window.__HANDY_TEST__.invocations.some(
            ({ cmd }) => cmd === "clear_binding",
          ),
        ),
      )
      .toBe(true);
  });

  test("does not enter Global capture when suspension fails", async ({
    page,
  }) => {
    await openGeneral(page);
    await page.evaluate(() => {
      window.__HANDY_TEST__.failures.suspend_binding = "recording_in_progress";
    });
    await page.getByRole("button", { name: "Set shortcut" }).click();

    await expect(
      page.getByText(
        "Shortcuts can’t be changed while Handy is recording or transcribing.",
        { exact: true },
      ),
    ).toBeVisible();
    await expect(page.getByText("Press keys...", { exact: true })).toHaveCount(
      0,
    );
  });

  test("Global tears down a delayed suspension that resolves after unmount", async ({
    page,
  }) => {
    await openGeneral(page);
    await page.evaluate(() => {
      window.__HANDY_TEST__.holdCommand("suspend_binding:transcribe");
    });
    await page.getByRole("button", { name: "Set shortcut" }).click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd, args }) =>
                cmd === "suspend_binding" && args.id === "transcribe",
            ).length,
        ),
      )
      .toBe(1);
    const abortsBeforeCreation = await page.evaluate(
      () =>
        window.__HANDY_TEST__.invocations.filter(
          ({ cmd, args }) =>
            cmd === "abort_shortcut_capture" && args.id === "transcribe",
        ).length,
    );
    await page.getByText("About", { exact: true }).first().click();
    expect(
      await page.evaluate(
        () =>
          window.__HANDY_TEST__.invocations.filter(
            ({ cmd, args }) =>
              cmd === "abort_shortcut_capture" && args.id === "transcribe",
          ).length,
      ),
    ).toBe(abortsBeforeCreation);
    await page.evaluate(() => {
      window.__HANDY_TEST__.releaseCommand("suspend_binding:transcribe");
    });
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd, args }) =>
                cmd === "abort_shortcut_capture" && args.id === "transcribe",
            ).length,
        ),
      )
      .toBeGreaterThan(abortsBeforeCreation);
  });

  test("Global capture exposes an announced status", async ({ page }) => {
    await openGeneral(page);
    await page.getByRole("button", { name: "Set shortcut" }).click();

    const capture = page.getByRole("status");
    await expect(capture).toHaveText("Press keys...");
    await expect(capture).toHaveAttribute("aria-live", "polite");
  });

  test("does not enter Handy Keys capture when backend start fails", async ({
    page,
  }) => {
    await openGeneral(page, { keyboard_implementation: "handy_keys" });
    await page.evaluate(() => {
      window.__HANDY_TEST__.failures.start_handy_keys_recording =
        "recording_in_progress";
    });
    await page.getByRole("button", { name: "Set shortcut" }).click();

    await expect(
      page.getByText(
        "Shortcuts can’t be changed while Handy is recording or transcribing.",
        { exact: true },
      ),
    ).toBeVisible();
    await expect(page.getByText("Press keys...", { exact: true })).toHaveCount(
      0,
    );
  });

  test("Handy Keys tears down a delayed start that resolves after unmount", async ({
    page,
  }) => {
    await openGeneral(page, { keyboard_implementation: "handy_keys" });
    await page.evaluate(() => {
      window.__HANDY_TEST__.holdCommand(
        "start_handy_keys_recording:transcribe",
      );
    });
    await page.getByRole("button", { name: "Set shortcut" }).click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd, args }) =>
                cmd === "start_handy_keys_recording" &&
                args.bindingId === "transcribe",
            ).length,
        ),
      )
      .toBe(1);
    const stopsBeforeCreation = await page.evaluate(
      () =>
        window.__HANDY_TEST__.invocations.filter(
          ({ cmd }) => cmd === "stop_handy_keys_recording",
        ).length,
    );
    await page.getByText("About", { exact: true }).first().click();
    expect(
      await page.evaluate(
        () =>
          window.__HANDY_TEST__.invocations.filter(
            ({ cmd }) => cmd === "stop_handy_keys_recording",
          ).length,
      ),
    ).toBe(stopsBeforeCreation);
    await page.evaluate(() => {
      window.__HANDY_TEST__.releaseCommand(
        "start_handy_keys_recording:transcribe",
      );
    });
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd }) => cmd === "stop_handy_keys_recording",
            ).length,
        ),
      )
      .toBeGreaterThan(stopsBeforeCreation);
  });

  test("Handy Keys capture exposes an announced status", async ({ page }) => {
    await openGeneral(page, { keyboard_implementation: "handy_keys" });
    await page.getByRole("button", { name: "Set shortcut" }).click();

    const capture = page.getByRole("status");
    await expect(capture).toHaveText("Press keys...");
    await expect(capture).toHaveAttribute("aria-live", "polite");
  });

  test("Handy Keys commits a captured shortcut and stops the backend", async ({
    page,
  }) => {
    await openGeneral(page, { keyboard_implementation: "handy_keys" });
    await page.getByRole("button", { name: "Set shortcut" }).click();
    await page.evaluate(() => {
      window.__HANDY_TEST__.emit("handy-keys-event", {
        modifiers: ["ctrl"],
        key: "k",
        is_key_down: true,
        hotkey_string: "ctrl+k",
      });
      window.__HANDY_TEST__.emit("handy-keys-event", {
        modifiers: ["ctrl"],
        key: "k",
        is_key_down: false,
        hotkey_string: "ctrl+k",
      });
    });

    await expect
      .poll(() =>
        page.evaluate(() => {
          const invocations = window.__HANDY_TEST__.invocations;
          const changeIndex = invocations.findIndex(
            ({ cmd }) => cmd === "change_binding",
          );
          return invocations
            .slice(changeIndex + 1)
            .filter(({ cmd }) => cmd === "stop_handy_keys_recording").length;
        }),
      )
      .toBeGreaterThanOrEqual(1);
    expect(
      await page.evaluate(() =>
        window.__HANDY_TEST__.invocations.some(
          ({ cmd, args }) =>
            cmd === "change_binding" && args.binding === "ctrl+k",
        ),
      ),
    ).toBe(true);
  });

  test("Handy Keys commits and stops once when release repeats during a delayed change", async ({
    page,
  }) => {
    await openGeneral(page, {
      keyboard_implementation: "handy_keys",
      bindings: {
        ...settings.bindings,
        transcribe: {
          ...settings.bindings.transcribe,
          current_binding: "alt+space",
        },
      },
    });
    await page
      .getByRole("button", { name: /Alt.*Space/i })
      .first()
      .click();
    await page.evaluate(() => {
      window.__HANDY_TEST__.delays.change_binding = 100;
      window.__HANDY_TEST__.emit("handy-keys-event", {
        modifiers: ["ctrl"],
        key: "k",
        is_key_down: true,
        hotkey_string: "ctrl+k",
      });
      const release = {
        modifiers: ["ctrl"],
        key: "k",
        is_key_down: false,
        hotkey_string: "ctrl+k",
      };
      window.__HANDY_TEST__.emit("handy-keys-event", release);
      window.__HANDY_TEST__.emit("handy-keys-event", release);
    });
    await expect(page.getByTestId("shortcut-capture-transcribe")).toHaveCount(
      0,
    );

    expect(
      await page.evaluate(
        () =>
          window.__HANDY_TEST__.invocations.filter(
            ({ cmd }) => cmd === "change_binding",
          ).length,
      ),
    ).toBe(1);
    expect(
      await page.evaluate(() => {
        const invocations = window.__HANDY_TEST__.invocations;
        const firstChange = invocations.findIndex(
          ({ cmd }) => cmd === "change_binding",
        );
        return invocations
          .slice(firstChange + 1)
          .filter(({ cmd }) => cmd === "stop_handy_keys_recording").length;
      }),
    ).toBe(1);
  });

  test("Handy Keys stop failure retains capture and outside click retries", async ({
    page,
  }) => {
    await openGeneral(page, { keyboard_implementation: "handy_keys" });
    await page.getByRole("button", { name: "Set shortcut" }).click();
    await page.evaluate(() => {
      window.__HANDY_TEST__.failures.stop_handy_keys_recording =
        "temporary_failure";
    });
    await page.getByText("General", { exact: true }).first().click();
    await expect(page.getByTestId("shortcut-capture-transcribe")).toBeVisible();

    await page.evaluate(() => {
      delete window.__HANDY_TEST__.failures.stop_handy_keys_recording;
    });
    await page.getByText("General", { exact: true }).first().click();
    await expect(
      page.getByRole("button", { name: "Set shortcut" }),
    ).toBeVisible();
  });

  test("Handy Keys does not reapply a committed binding when stop retry is needed", async ({
    page,
  }) => {
    await openGeneral(page, { keyboard_implementation: "handy_keys" });
    await page.getByRole("button", { name: "Set shortcut" }).click();
    await page.evaluate(() => {
      window.__HANDY_TEST__.failures.stop_handy_keys_recording =
        "temporary_failure";
      window.__HANDY_TEST__.emit("handy-keys-event", {
        modifiers: ["ctrl"],
        key: "k",
        is_key_down: true,
        hotkey_string: "ctrl+k",
      });
      window.__HANDY_TEST__.emit("handy-keys-event", {
        modifiers: ["ctrl"],
        key: "k",
        is_key_down: false,
        hotkey_string: "ctrl+k",
      });
    });
    await expect(page.getByTestId("shortcut-capture-transcribe")).toBeVisible();
    await page.evaluate(() => {
      delete window.__HANDY_TEST__.failures.stop_handy_keys_recording;
    });
    await page.evaluate(() => {
      window.__HANDY_TEST__.emit("handy-keys-event", {
        modifiers: [],
        key: "x",
        is_key_down: true,
        hotkey_string: "x",
      });
    });
    await expect(page.getByTestId("shortcut-capture-transcribe")).toHaveCount(
      0,
    );
    expect(
      await page.evaluate(
        () =>
          window.__HANDY_TEST__.invocations.filter(
            ({ cmd }) => cmd === "change_binding",
          ).length,
      ),
    ).toBe(1);
  });

  for (const clearKey of ["delete", "backspace"] as const) {
    test(`Handy Keys ${clearKey} clears and stops capture`, async ({
      page,
    }) => {
      await openGeneral(page, { keyboard_implementation: "handy_keys" });
      await page
        .getByRole("button", { name: /Ctrl.*Space/i })
        .first()
        .click();
      const stopsBefore = await page.evaluate(
        () =>
          window.__HANDY_TEST__.invocations.filter(
            ({ cmd }) => cmd === "stop_handy_keys_recording",
          ).length,
      );
      await page.evaluate((key) => {
        window.__HANDY_TEST__.emit("handy-keys-event", {
          modifiers: [],
          key,
          is_key_down: true,
          hotkey_string: key,
        });
      }, clearKey);

      await expect
        .poll(() =>
          page.evaluate(
            () =>
              window.__HANDY_TEST__.invocations.filter(
                ({ cmd }) => cmd === "stop_handy_keys_recording",
              ).length,
          ),
        )
        .toBe(stopsBefore + 1);
      expect(
        await page.evaluate(() =>
          window.__HANDY_TEST__.invocations.some(
            ({ cmd }) => cmd === "clear_binding",
          ),
        ),
      ).toBe(true);
    });
  }

  test("Handy Keys pagehide retries stop and remount performs recovery stop", async ({
    page,
  }) => {
    await openGeneral(page, { keyboard_implementation: "handy_keys" });
    await page.getByRole("button", { name: "Set shortcut" }).click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd, args }) =>
                cmd === "plugin:event|listen" &&
                args.event === "handy-keys-event",
            ).length,
        ),
      )
      .toBeGreaterThan(0);
    const before = await page.evaluate(
      () =>
        window.__HANDY_TEST__.invocations.filter(
          ({ cmd }) => cmd === "stop_handy_keys_recording",
        ).length,
    );
    const unlistensBefore = await page.evaluate(
      () =>
        window.__HANDY_TEST__.invocations.filter(
          ({ cmd, args }) =>
            cmd === "plugin:event|unlisten" &&
            args.event === "handy-keys-event",
        ).length,
    );
    await page.evaluate(() => {
      window.__HANDY_TEST__.failures.stop_handy_keys_recording = [
        "temporary_failure",
        "temporary_failure",
        "temporary_failure",
        "temporary_failure",
        "temporary_failure",
        "temporary_failure",
      ];
      window.dispatchEvent(new Event("pagehide"));
    });
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd }) => cmd === "stop_handy_keys_recording",
            ).length,
        ),
      )
      .toBeGreaterThanOrEqual(before + 3);

    await page.getByText("Post Process", { exact: true }).click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd }) => cmd === "stop_handy_keys_recording",
            ).length,
        ),
      )
      .toBeGreaterThanOrEqual(before + 6);
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd, args }) =>
                cmd === "plugin:event|unlisten" &&
                args.event === "handy-keys-event",
            ).length,
        ),
      )
      .toBeGreaterThan(unlistensBefore);
    const beforeRemount = await page.evaluate(
      () =>
        window.__HANDY_TEST__.invocations.filter(
          ({ cmd }) => cmd === "stop_handy_keys_recording",
        ).length,
    );
    await page.getByText("General", { exact: true }).first().click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd }) => cmd === "stop_handy_keys_recording",
            ).length,
        ),
      )
      .toBeGreaterThan(beforeRemount);
  });

  test("Handy Keys unregisters a listener that resolves after unmount", async ({
    page,
  }) => {
    await openGeneral(page, { keyboard_implementation: "handy_keys" });
    await page.evaluate(() => {
      window.__HANDY_TEST__.delays["plugin:event|listen"] = 100;
    });
    const unlistensBefore = await page.evaluate(
      () =>
        window.__HANDY_TEST__.invocations.filter(
          ({ cmd, args }) =>
            cmd === "plugin:event|unlisten" &&
            args.event === "handy-keys-event",
        ).length,
    );
    await page.getByRole("button", { name: "Set shortcut" }).click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd, args }) =>
                cmd === "plugin:event|listen" &&
                args.event === "handy-keys-event",
            ).length,
        ),
      )
      .toBeGreaterThan(0);
    await page.getByText("Post Process", { exact: true }).click();

    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__HANDY_TEST__.invocations.filter(
              ({ cmd, args }) =>
                cmd === "plugin:event|unlisten" &&
                args.event === "handy-keys-event",
            ).length,
        ),
      )
      .toBeGreaterThan(unlistensBefore);
  });

  for (const [code, message] of [
    [
      "final_normal_binding_required",
      "Assign another recording shortcut before clearing this one.",
    ],
    [
      "recording_in_progress",
      "Shortcuts can’t be changed while Handy is recording or transcribing.",
    ],
  ] as const) {
    test(`shows localized copy for ${code}`, async ({ page }) => {
      await openGeneral(page);
      await page.evaluate((errorCode) => {
        window.__HANDY_TEST__.failures.clear_binding = errorCode;
      }, code);

      await page
        .getByRole("button", { name: "Clear Push To Talk Shortcut" })
        .click();
      await expect(page.getByText(message, { exact: true })).toBeVisible();
    });
  }

  test("renders both post-processing shortcut actions", async ({ page }) => {
    await openGeneral(page);
    await page.getByText("Post Process", { exact: true }).click();

    await expect(
      page.getByText("Post-Processing Shortcut", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText("Push To Talk with Post-Processing Shortcut", {
        exact: true,
      }),
    ).toBeVisible();
  });
});

declare global {
  interface Window {
    __HANDY_TEST__: {
      invocations: Invocation[];
      settings: typeof settings;
      failures: Record<string, string | string[]>;
      delays: Record<string, number>;
      holdCommand: (key: string) => void;
      releaseCommand: (key: string) => void;
      emit: (event: string, payload: unknown) => void;
    };
  }
}
