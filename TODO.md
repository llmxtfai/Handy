# Independent Recording Shortcuts: Handoff and Follow-up

Last updated: 2026-07-15

## Current status

The implementation is complete, independently reviewed, tested, and stored on clean local branches. Nothing has been pushed and no pull request or GitHub Discussion post has been created yet.

- App repository: `C:\Users\hiros\app\Handy-handoff`
- App branch: `feat/independent-recording-shortcuts`
- App head: `f8f2ba9` (`docs: clarify shortcut backend switching contract`)
- App base: `upstream/main` at `b9925d0`
- Website repository: `C:\Users\hiros\app\handy.computer`
- Website branch: `docs/independent-recording-shortcuts`
- Website head: `708c512` (`docs: refresh post-processing shortcut screenshot`)
- Website base: `upstream/main` at `07008c4`

This file is intentionally a local handoff document. Do not include it in the upstream app PR unless the maintainer explicitly wants project TODO notes in the repository.

## What was built

Handy now models recording interaction as four independent shortcut bindings instead of one shortcut plus a global Push To Talk boolean:

1. **Transcribe Shortcut**: press once to start recording and press it again to stop and transcribe.
2. **Push To Talk Shortcut**: hold to record and release to stop and transcribe.
3. **Transcribe with Post-Processing**: press once to start and again to stop, transcribe, and post-process.
4. **Push To Talk with Post-Processing**: hold to record and release to transcribe and post-process.

The design intentionally does **not** use double-tap detection. Interaction type is determined by the binding identity, which keeps behavior explicit and avoids timing ambiguity.

### UX and compatibility rules

- Empty shortcut means that action is disabled.
- Each row has an actual **Set shortcut** action and a clear `x` action when assigned.
- Both normal actions can remain assigned and be used interchangeably.
- The two normal actions cannot share the same key combination.
- At least one normal transcription shortcut must remain assigned.
- Either, both, or neither post-processing shortcut may be assigned.
- New installations default to Push To Talk assigned and Transcribe unassigned, including the mirrored post-processing pair.
- Existing installations preserve their old mapping and interaction exactly:
  - Legacy Push To Talk enabled becomes the Push To Talk binding.
  - Legacy Push To Talk disabled becomes the Transcribe binding.
- The Cancel shortcut is shown on macOS/Windows when either usable press-once binding is assigned. It can cancel any recording. Escape is the default; remapping Cancel replaces Escape.
- Switching keyboard implementations preflights every assigned binding. An incompatible binding rejects the switch without rewriting or resetting settings.

## Implementation summary

### Backend and persisted settings

- Added settings schema v2 and one-time migration in `src-tauri/src/settings.rs`.
- Removed the persisted global `push_to_talk` field after migration.
- Added the four binding IDs and new-install defaults in `src-tauri/resources/default_settings.json`.
- Routed all recording shortcut input through `TranscriptionCoordinator`.
- Binding identity now selects toggle versus hold/release behavior.
- Added serialized recording admission and shortcut-mutation ownership.
- Shortcut changes are rejected while recording, processing, or another capture/mutation owns the coordinator.
- Added atomic unregister/register transactions, rollback, preflight validation, clear/reset rules, and capture ownership in `src-tauri/src/shortcut/mutation.rs`.
- Added the X11 50 ms release grace needed to distinguish synthetic release/press bursts from a genuine Push To Talk release.
- Cancellation suppresses repeated held-key input until a real release.
- Capture-suspension failure no longer tries to re-register a shortcut that may still be live.
- Post-processing enable/disable registration changes are transactional.
- Keyboard backend switching rejects incompatible bindings and preserves the current backend and bindings.

### Frontend

- General settings now show separate Transcribe and Push To Talk rows.
- Post Process settings show the mirrored pair under a plural **Shortcuts** heading.
- Removed the old `PushToTalk.tsx` toggle component.
- Added visible clear controls, localized errors, contextual accessible names, and announced capture status.
- Added lifecycle protection for:
  - delayed capture startup resolving after unmount;
  - failed abort/stop retaining ownership for retry;
  - pagehide, blur, and unmount cleanup retries;
  - stale capture recovery on mount;
  - duplicate Handy Keys release events;
  - rapid double activation while backend startup is pending;
  - assignment-versus-clear overlap while a commit is pending;
  - delayed event-listener registration after unmount.
- A rejected post-processing toggle command now rolls optimistic Zustand state back instead of leaving UI and backend state divergent.
- Added genuine localized strings for all 21 non-English catalogs plus English, with placeholder parity.

### Documentation and website

- Updated the app `README.md` shortcut behavior and migration guidance.
- Updated public website pages covering General, Getting Started, Advanced, Debug, FAQ, Post-Processing, About, and the homepage.
- Updated the platform shortcut table and Cancel behavior.
- Added a visible notice before the legacy homepage video explaining that its old single-shortcut/toggle UI is obsolete. The transcript remains verbatim because it describes the recorded video.
- Replaced the General and Post Process screenshots with the current rendered UI.
- The latest Post Process screenshot shows the plural **Shortcuts** heading and both independent controls.

## Important files

### Backend

- `src-tauri/src/settings.rs`
- `src-tauri/src/shortcut/mod.rs`
- `src-tauri/src/shortcut/mutation.rs`
- `src-tauri/src/shortcut/handy_keys.rs`
- `src-tauri/src/shortcut/tauri_impl.rs`
- `src-tauri/src/transcription_coordinator.rs`
- `src-tauri/resources/default_settings.json`

### Frontend and tests

- `src/components/settings/GlobalShortcutInput.tsx`
- `src/components/settings/HandyKeysShortcutInput.tsx`
- `src/components/settings/general/GeneralSettings.tsx`
- `src/components/settings/post-processing/PostProcessingSettings.tsx`
- `src/components/settings/ShortcutClearButton.tsx`
- `src/stores/settingsStore.ts`
- `src/bindings.ts`
- `src/i18n/locales/*/translation.json`
- `tests/app.spec.ts`

### Website

- `C:\Users\hiros\app\handy.computer\src\content\docs\general.mdx`
- `C:\Users\hiros\app\handy.computer\src\content\docs\getting-started.mdx`
- `C:\Users\hiros\app\handy.computer\src\content\docs\post-processing.mdx`
- `C:\Users\hiros\app\handy.computer\src\content\docs\advanced.mdx`
- `C:\Users\hiros\app\handy.computer\src\content\docs\debug.mdx`
- `C:\Users\hiros\app\handy.computer\src\components\PlatformShortcuts.tsx`
- `C:\Users\hiros\app\handy.computer\src\pages\index.astro`
- `C:\Users\hiros\app\handy.computer\public\docs\handy-general.png`
- `C:\Users\hiros\app\handy.computer\public\docs\handy-post.png`

## Verification evidence

The following checks passed on the final app implementation:

- `bun run lint`
- `bun run check:translations`: all 21 non-English languages complete; 395 English source keys
- Changed-file Prettier check
- `bun run build`: TypeScript and Vite production build passed
- `bun run test:playwright`: **37/37 passed**
- `cargo fmt --manifest-path src-tauri/Cargo.toml --check`
- `cargo test --manifest-path src-tauri/Cargo.toml --lib`: **148/148 passed**
- `git diff --check upstream/main...HEAD`
- Independent backend, frontend, migration, accessibility, documentation, and adversarial reviews completed with all findings addressed

Known unrelated Rust warnings remain in upstream code:

- Unused import in `src-tauri/src/helpers/clamshell.rs`
- Unused assignment in `src-tauri/src/managers/transcription.rs`

Website verification:

- Astro generated all **17 pages** and printed `Complete!`.
- On Windows, the process then exits with the libuv assertion `!(handle->flags & UV_HANDLE_CLOSING)`.
- The same post-build assertion was reproduced on an unchanged detached worktree at website upstream commit `07008c4`, so it is pre-existing tooling behavior rather than a change introduced by this feature.
- Generated HTML was checked for the current shortcut, migration, keyboard-backend, Cancel, and legacy-video guidance.
- Both updated screenshots were visually inspected.

## Commits

### App branch

- `cde2c0d feat: add independent recording shortcuts`
- `7462814 fix: preserve shortcut registration on capture failure`
- `ec68f6c fix: close shortcut lifecycle and accessibility gaps`
- `f8f2ba9 docs: clarify shortcut backend switching contract`

### Website branch

- `2ceb7f0 docs: explain independent recording shortcuts`
- `d249e71 docs: align shortcut guidance with current behavior`
- `708c512 docs: refresh post-processing shortcut screenshot`

Backup branches preserve earlier full working history and internal spec artifacts:

- App: `backup/independent-recording-shortcuts-full-history`
- Website: `backup/independent-recording-shortcuts-docs-history`

The internal design and execution plan exist only on the app backup branch because upstream does not have the `docs/superpowers` convention:

- `docs/superpowers/specs/2026-07-15-independent-recording-shortcuts-design.md`
- `docs/superpowers/plans/2026-07-15-independent-recording-shortcuts.md`

## Contribution constraints

- Handy is under a feature freeze. Feature proposals require community support in GitHub Discussions before a PR is opened.
- Relevant community thread: <https://github.com/cjpais/Handy/discussions/211>
- On 2026-01-31, the maintainer said new shortcuts would not be merged until the project had a unified, non-confusing shortcut-management UI. This implementation is specifically designed around that concern, but it still needs to be presented and validated with the community.
- Before opening an app PR, read `.github/PULL_REQUEST_TEMPLATE.md` again and follow every section exactly.
- The template's **Human Written Description** must be written by the human contributor. Do not let an AI invent that paragraph; leave a TODO until it is supplied.
- Include the required AI Assistance disclosure.
- Feature requests belong in Discussions, not GitHub Issues.
- Read `CONTRIBUTING.md` before submission.
- Use conventional commit prefixes.
- Open the app PR first. The website PR should reference and follow the app PR so public docs are not published ahead of unreleased behavior.

## Remaining work

- [ ] Read the latest posts in Discussion #211 and confirm no newer maintainer direction or competing implementation changes the proposal.
- [ ] Search current upstream open PRs and branches for overlapping shortcut-management work or merge conflicts.
- [ ] Post a concise proposal to Discussion #211 that explains:
  - the user problem;
  - the two explicit interaction rows instead of timing-based double tap;
  - minimal migration impact for existing users;
  - the clear-to-disable model and final-normal-binding rule;
  - screenshots of General and Post Process settings;
  - how the design answers the maintainer's unified-UI concern.
- [ ] Gather or link community feedback required by the PR template.
- [ ] Decide whether maintainers want one app PR containing code plus README, followed by a website PR. This is the current recommendation.
- [ ] Rebase or merge the latest `upstream/main` into both local branches immediately before final verification. Do not rewrite or force-push shared history.
- [ ] Resolve any conflicts and rerun the complete verification matrix below.
- [ ] Read `.github/PULL_REQUEST_TEMPLATE.md` immediately before drafting the app PR.
- [ ] Ask the human contributor to write the mandatory **Human Written Description**.
- [ ] Draft the remaining PR template sections, including community feedback links, test evidence, migration behavior, platform considerations, screenshots, and AI Assistance disclosure.
- [ ] Push `feat/independent-recording-shortcuts` to the fork only after explicit user approval.
- [ ] Open the app PR as a draft only after explicit user approval and after the Discussion/community requirement is satisfied.
- [ ] Watch app PR CI and address only failures caused by this branch.
- [ ] After the app PR is accepted or maintainers request website docs, push `docs/independent-recording-shortcuts` and open the website PR referencing the app PR.
- [ ] Verify the deployed website after merge.
- [ ] Keep this `TODO.md` out of the upstream PR unless maintainers explicitly request it.

## Final verification commands before pushing

From `C:\Users\hiros\app\Handy-handoff`:

```powershell
$env:PATH='C:\Users\hiros\.bun\bin;C:\VulkanSDK\1.4.350.0\Bin;' + $env:PATH
$env:CARGO_TARGET_DIR='E:\codex-cache\Handy-target'
$env:VULKAN_SDK='C:\VulkanSDK\1.4.350.0'

bun run lint
bun run check:translations
bun run build
bun run test:playwright
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml --lib
git diff --check upstream/main...HEAD
git status --short
```

The repository-wide `bun run format:check` currently reports pre-existing formatting differences in many unchanged upstream files on Windows. Use a changed-file Prettier check for this branch and keep `cargo fmt --check` separate unless upstream fixes the baseline.

From `C:\Users\hiros\app\handy.computer`:

```powershell
$env:PATH='C:\Users\hiros\.bun\bin;' + $env:PATH
bun run build
git diff --check upstream/main...HEAD
git status --short
```

Expect the Windows Astro process to emit the documented libuv assertion only after all 17 pages have been generated. Recheck against upstream if that behavior changes.

## Recommended pickup sequence

1. Open this file and confirm both branches and heads still match the values above.
2. Fetch both upstream repositories and inspect changes since the recorded base commits.
3. Review Discussion #211 and current open PRs.
4. Prepare the community proposal with the updated screenshots.
5. Incorporate maintainer/community feedback if it does not compromise migration safety or explicit interaction behavior.
6. Rebase/merge upstream and rerun every verification command.
7. Prepare the app PR template with the human-written section left for the contributor.
8. Request explicit approval before pushing or opening either PR.
