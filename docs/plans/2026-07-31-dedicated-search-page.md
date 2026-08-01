# Full-Page Search Results Implementation Plan

> **For agentic workers:** Implement this plan task-by-task, write each regression test before production code, and commit each task separately.

**Goal:** Preserve Nira's current Ctrl+F / Cmd+F / Alt+Space search overlay, but make Enter close it and open a full-page view of the same query and results.

**Architecture:** Install the existing search state once at the app root so the always-mounted overlay and the page consume one query, debounce, and result set. Add an internal `Section::Search` route without a sidebar item, reuse the existing artist and playable-row primitives, and keep row clicks responsible for playback.

**Tech Stack:** Rust 2024, Dioxus 0.7, existing hooks/components, existing mt-ui CSS tokens, Anvil tasks.

## Global Constraints

- Keep the overlay's appearance, hotkeys, Escape behavior, row playback, context menus, and artist navigation unchanged.
- Enter on the overlay opens the results page; it no longer starts playback.
- Do not add Search to the sidebar and do not add filters, history, sorting, pagination, ranking changes, or dependencies.
- Keep visible copy and shared code provider-neutral. Private-provider code and wiring remain private-only.
- Preserve the existing 250 ms debounce, stale-query guard, provider fallback, artist merge, and track interleave.
- Use existing `--bg`, `--raise1`, `--raise2`, `--raise3`, `--fg`, `--sub`, `--faint`, `--r`, and `--rs` tokens. No borders, shadows, glows, gradients, hue accents, or pills.
- Use only the named `anvil tests`, `anvil check`, `anvil dev`, and `anvil release` tasks for resource-heavy verification.
- Make shared commits on public, audit them for private paths and private-provider wiring, cherry-pick only those commits into private master, preserve private provider wiring, and never push private history.

### Task 1: Share the existing search state

**Files:**

- Modify: `hooks/src/use_search.rs`
- Modify: `hooks/src/lib.rs`
- Test: `pages/tests/ui_regressions.rs`

**Interfaces:**

- Produces: `pub(crate) fn install_search()` and unchanged `pub fn use_search() -> UseSearch`.

- [x] Add a regression that requires one root installer and a context-only `use_search()` accessor.
- [x] Run `anvil tests` and confirm the new regression fails because each caller still creates its own state.
- [x] Split the existing hook into root installation plus context lookup without changing search behavior.
- [x] Run `anvil tests` and `anvil check` and confirm they pass.
- [x] Commit only the hook, installer wiring, test, and this plan.

### Task 2: Open full-page results on Enter

**Files:**

- Create: `pages/src/search.rs`
- Modify: `pages/src/lib.rs`
- Modify: `pages/src/search_overlay.rs`
- Modify: `pages/src/parts.rs`
- Modify: `components/src/lib.rs`
- Modify: `nira/src/main.rs`
- Test: `pages/tests/ui_regressions.rs`

**Interfaces:**

- Produces: internal `Section::Search`, `pages::search::Search`, shared `SearchTrackRow`, and `SearchOverlay(open, on_search)`.

- [x] Add regressions requiring the internal route, no Search sidebar item, and an overlay `on_search` callback instead of Enter-to-play.
- [x] Run `anvil tests` and confirm the new regressions fail for the missing route/page/handoff.
- [x] Add the internal route and full-page view using the shared root state, existing `SearchBar`, `ArtistResults`, `PlayableLi`, and `ArtistLinks`.
- [x] Move the existing overlay row markup into the shared row component while retaining its current overlay class and behavior.
- [x] On non-empty submit, close detail/overlay state and select `Section::Search`; leave row clicks as playback.
- [x] Reuse the existing responsive page and track-list styles; do not restyle the overlay or shared SearchBar.
- [x] Run `anvil tests`, `anvil check`, and `anvil release` on public.
- [ ] Inspect real dark/light rendered Search pages without mouse or keyboard automation.
- [ ] Commit the page, handoff, CSS, and regressions.

### Task 3: Propagate and verify safely

- [ ] Audit `origin/main..public` for private-provider wiring, private paths, credentials, and unrelated changes.
- [ ] Cherry-pick the exact Search commits into private master and resolve only provider-context overlap while preserving private providers.
- [ ] Run `anvil tests`, `anvil check`, and `anvil release` on private.
- [ ] Verify `/home/mt/.local/bin/nira` resolves to the private release.
- [ ] Leave unrelated pre-existing files untouched and report any remaining dirty state explicitly.

## Deliberately Skipped

- Sidebar Search navigation, overlay preview caps, a separate "View all" button, album search, filters, history, sorting, pagination, and ranking changes. Add one only after actual use proves it is needed.
