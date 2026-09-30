<!--
One logical change per pull request. If you are doing two unrelated things, open
two - a PR that is easy to review is a PR that gets merged.
-->

## What this changes

<!-- A few sentences. What behaves differently after this, and why. -->

## Why

<!-- The reason, not just the symptom. Link the issue it closes: Closes #123 -->

## How it was verified

<!-- What you ran, and what you observed. Delete what does not apply. -->

- [ ] `cd src-tauri && cargo test -p cpum-core`
- [ ] `cd src-tauri && cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cd src-tauri && cargo fmt --all -- --check`
- [ ] `npx vue-tsc --noEmit`
- [ ] `.\build.bat` (only if you touched the bundle, the installer hooks or
      `tauri.conf.json`)
- [ ] Tests added for new behaviour
- [ ] New user-visible strings added to **both** locales in `src/i18n.ts`
- [ ] README updated if behaviour visible to users changed
- [ ] `CHANGELOG.md` updated under `## [Unreleased]`

## Notes for the reviewer

<!--
Anything that will otherwise cost the reviewer ten minutes: a trade-off you
chose, a platform you could not test, a follow-up you deliberately left out.
-->

## Screenshots

<!-- Required for UI changes. -->
