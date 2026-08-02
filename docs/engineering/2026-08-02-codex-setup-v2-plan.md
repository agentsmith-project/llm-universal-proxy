# `codex-setup` v2 — Hotfix Bugs + Reasoning Validation + Interactive Wizard — Plan

- Date: 2026-08-02
- Owner: lizhijian
- Input: two user-reported bugs against `codex-setup` (v0.3.3, on `main`) + two user-requested improvements (reasoning-effort levels, TUI wizard UX). Code references below are verified against `main`.
- Status: **handoff-ready** (2-round review: correctness + scope/handoff; MUST-FIX items applied). Concise, executable. Implementation is in-place changes + tests; no separate evidence/gate/audit artifacts are produced for this work.

## Principles (binding for this work)

- **KISS / DRY / YAGNI.** Fix what users hit; reuse existing write/install logic for the wizard; do not build a full-screen TUI where sequential prompts suffice.
- **In-place fix + test (TDD).** Each bug/feature lands with its test in the same change. Bug repro first, then fix.
- **Idempotency is a contract.** `codex-setup` must be safely re-runnable; the profile rebuild must be a pure function of its inputs.
- **One input vocabulary.** Reasoning effort uses the single union ladder already defined in `dialect.rs` — no second enum, no stringly-typed CLI.
- **No new governance layers.** No evidence/gate/report/audit docs for this work.

## Priority

- **HOTFIX (users hitting them now):** Bug 1, Bug 2. Ship together, ahead of features.
- **Same release (features):** Feature 1 (small), Feature 2 (medium). Feature 2 depends on Bug 2's fix (the wizard runs the connection test to discover models).

---

## HOTFIX 1 — `model_catalog_json` duplicated on re-run with `--force-v1`

**Symptom:** re-running `codex-setup ... --force-v1` twice produces a profile with two top-level `model_catalog_json = "..."` keys. Codex `--strict-config` then rejects the profile as a duplicate-key TOML error.

**Root cause (confirmed):** `strip_managed_tables()` only strips TOML **table headers** (lines beginning with `[`):

- `src/user_tools/codex_setup.rs:222-237` — the stripper matches on `trimmed.starts_with('[')`; bare top-level keys are never removed.
- `src/user_tools/codex_setup.rs:198-201` — `build_profile_content()` writes the bare key `model_catalog_json = "..."` *before* the managed tables. It is not a table header, so it survives stripping. The next run prepends a second copy → duplicate TOML key.

**Fix (approach (b) — unify):** teach the stripper to drop managed **bare keys** in addition to managed tables. Concretely:

- Rename/generalize `strip_managed_tables()` → `strip_managed()` taking two lists: `headers: &[&str]` (as today) and `bare_keys: &[&str]` (e.g. `["model_catalog_json"]`).
- A bare-key line matches when, after trimming, it is `<key> =` (equals possibly preceded/followed by whitespace). Drop such lines only for keys in the managed set. Keep all other top-level keys (`env_key`, user-owned keys) verbatim.
- `build_profile_content()` (`:193`) calls `strip_managed(existing, &["[model_providers.llmup]", "[features]"], &["model_catalog_json"])`.

**Why not a separate `strip_managed_keys()`:** two passes over the same content is more code and an extra allocation for no clarity gain; one function that owns "managed lines" is DRY and keeps the idempotency contract in one place.

**Tests (TDD, add to `codex_setup.rs` `#[cfg(test)] mod tests`):**

1. Repro first: `build_profile_content(Some(existing_with_one_catalog_key), url, Some(path))` then call again feeding the first result back as `existing` → assert exactly **one** line starting with `model_catalog_json`.
2. `strip_managed()` preserves a user-owned top-level bare key (e.g. `env_key = "X"`) while dropping a managed `model_catalog_json`.
3. Existing `strip_managed_tables_removes_only_named_tables` (`:1025`) still passes (rename the call site; semantics unchanged for tables).

**Verification:** `cargo test --lib codex_setup`; manual two-pass `--force-v1` run → `grep -c '^model_catalog_json' ~/.codex/llmup.config.toml` == 1.

---

## HOTFIX 2 — connection test reports "0 models reachable" despite server returning models

**Symptom:** `codex-setup --provider-key ...` prints `connection test: OK (0 models reachable).` even though the llmup proxy `/models` endpoint returns a non-empty model list.

**Root cause (confirmed):** the Codex-UA catalog emitted by the server keys model identity as `slug`, never `id`:

- `src/user_tools/codex_setup.rs:891-899` — `connection_test()` reads `model.get("id")` only.
- `src/user_tools/agent_model_profile.rs:287` — catalog builder emits `entry.insert("slug", ...)`; it never emits `id`. So every entry fails the `id` lookup → 0 models.

**Note (distinct "0 models" cause):** this fix addresses the slug/id field mismatch on a *reachable* endpoint. A separate "0 models" cause is a wrong `base_url`: the proxy exposes no bare `/models` route, only `/openai/v1/models`. The symptom therefore assumes the user supplied the `/openai/v1` base-url convention (which the docs and the wizard default both do); without the suffix the connection test 404s before any slug/id read happens.

**Fix:**

1. **Read `slug` with `id` fallback** (`codex_setup.rs:891-899`): replace the `filter_map` with `model.get("slug").or_else(|| model.get("id"))`. This keeps the function correct for both the Codex-UA catalog (slug) and any future/id upstream that returns id.
2. **Do not print "OK" on zero models** (`codex_setup.rs:980-988`): when `models.is_empty()`, print a `WARNING — 0 models discovered` line (same channel as the existing error branch) instead of `OK`. An empty list is not a success state. Keep the `Err` branch unchanged.
3. **(Optional, small) validate `--model`:** after discovery, if the user passed `--model <alias>`, warn when the alias is absent from the discovered list. Non-fatal (the alias may resolve server-side); do not abort install.

**Tests (TDD):**

1. Unit-test the extraction by refactoring the parse into a pure helper `extract_model_ids(&serde_json::Value) -> Vec<String>` (reads slug→id). Feed it `{models:[{slug:"ds-flash"},{id:"gpt-x"}]}` → `["ds-flash","gpt-x"]`. Feed `{models:[]}` → `[]`.
2. (If discovery wiring is unit-testable without a live socket) assert the caller emits the WARNING wording on empty and OK wording on non-empty.

**Verification:** against a running llmup proxy with configured aliases, `codex-setup --provider-key ...` reports `OK (<n> models reachable)` with n > 0.

---

## Feature 1 — Reasoning effort: all 8 levels selectable + validated

**Current gap:** `--reasoning-effort` (`codex_setup.rs:519-523`) accepts any string with no validation; a typo (e.g. `hight`) is written verbatim into `model_reasoning_effort` and silently mis-routed. `HELP_TEXT` (`:477`) does not enumerate valid values.

**Source of truth (already in the repo):** `src/config/dialect.rs:18-34` defines `ReasoningLevel` with the 8-level union vocabulary — `none, minimal, low, medium, high, xhigh, max, ultra` — plus `FromStr` (`:54`) and `Display` (`:61`). Reuse this enum; do not introduce a second ladder.

**Required:**

1. **Validate** in `parse_args()` (`:519-523`): parse the value through `ReasoningLevel::from_str`. On failure, wrap/replace its error with a clear message naming the bad value and listing all 8 valid levels. Do this **locally in `codex_setup.rs`** — enumerate the levels in a local const array / format string right at the call site (e.g. `"valid: none|minimal|low|medium|high|xhigh|max|ultra"`) and append it to the error. Do **NOT** edit shared `dialect.rs`: `ReasoningLevel::parse` (`dialect.rs:39-51`) returns only `"unknown reasoning level \`{other}\`"` with no enumeration, and the single source of truth for the enum stays there. The helpful listing lives only in the CLI layer.
2. **Help text** (`HELP_TEXT`, `:477`): enumerate the 8 levels inline, e.g. `--reasoning-effort <s>   one of: none|minimal|low|medium|high|xhigh|max|ultra (optional)`.
3. **Default (recommendation):** keep current behavior — if not specified, **omit** the key (Codex's own default applies). The user's ask ("全部挡位可选" / all levels selectable) is satisfied by validation + enumeration; forcing a default would change behavior for existing users. The wizard (Feature 2) will surface "medium" as a highlighted default *choice* without changing the CLI default.

**Known issue — deferred (NOT in this plan's scope, flag only):** `default_codex_catalog_entry()` at `src/user_tools/agent_model_profile.rs:293-313` hardcodes only 4 levels (`low/medium/high/xhigh`) in `supported_reasoning_levels`. This is server-side catalog metadata and should eventually be reconciled with the 8-level union (see `2026-08-01-reasoning-unification-dialect-plan.md`). Track separately; do not bundle into this hotfix/feature release.

**Tests (TDD):**

1. `parse_args(["--reasoning-effort","ultra", ...])` → `Ok` with `reasoning_effort == Some("ultra")`.
2. `parse_args(["--reasoning-effort","hight", ...])` → `Err` whose message contains all 8 valid level strings (so the user sees the menu).
3. Omitted → `reasoning_effort == None` (regression guard for the default-omit decision).

---

## Feature 2 — Interactive wizard mode (`--interactive`) via `dialoguer`

**User intent:** "使用tui的方式配置（降低使用者心智负担。ux/ui你要好好设计，要简单，但是信息全面）" — lower cognitive load; simple but complete.

**Design decision — `dialoguer`, not ratatui:**

- `ratatui` (already in `Cargo.toml:34`, used read-only by `src/dashboard.rs`) has **no text-input widget**. A full-screen wizard would need ~500 lines of hand-rolled input handling — violates KISS/YAGNI.
- `dialoguer` (not yet a dependency) gives `Input` / `Password` / `Select` / `Confirm` out of the box. A full sequential wizard is ~100-150 lines. It also wins on **security**: `Password` masks the key, unlike `--provider-key` which leaks via shell history.
- Reserve `ratatui`/`crossterm` for the existing read-only dashboard. The wizard is sequential prompts, not a full-screen TUI. (Consistent with `2026-08-02-codex-subagent-tui-plan.md` §"subcommand, not standalone TUI binary".)

**Add dependency:** `dialoguer = { version = "0.11", default-features = false }` to `Cargo.toml` `[dependencies]`. The wizard uses only `Input`/`Password`/`Select`/`Confirm`, which rely on `console` (pulled automatically with default features off); disabling default features avoids pulling `fuzzy-matcher`/`editor`, which the wizard never uses.

**Flag:** `--interactive` (alias `--tui` is misleading since it's not full-screen; use `--interactive` only). In `parse_args`, when set, route to a new `Action::Interactive` arm in `run_with` (`codex_setup.rs:922-1009`). `--interactive` is mutually exclusive with `--status`/`--uninstall` (it's a generate-mode flag); `--help` always works.

**Wizard flow (sequential prompts) — UX principle: every step has a sensible default, ESC/Ctrl-C cancels cleanly:**

1. **Input — `base_url`:** default `http://127.0.0.1:8080/openai/v1` (the proxy exposes no bare `/models` route, only `/openai/v1/models` — omitting the `/openai/v1` suffix 404s both the connection test and the Codex runtime); validate non-empty + URL shape (reuse `normalize_base_url`, `:83`).
2. **Password — `provider_key`:** masked. Optional (user may skip; install proceeds without connection test). State explicitly it is used only for the test, never stored.
3. **Connection test — fetch `/models`** (reuses HOTFIX 2 `connection_test`). Show result inline. If it fails or returns 0, offer "continue anyway / change URL / abort". Conditional chain: if step 2 (key) is skipped, step 3 is skipped as well and step 4 falls back to free-text model entry.
4. **Select — `model`:** populated from step 3's discovered list (arrow-key menu). If step 3 was skipped/empty, fall back to a free-text `Input` validated with `is_valid_model_name` (`codex_setup.rs:572`).
5. **Select — `reasoning_effort`:** the 8 levels as a menu (Feature 1 enum), with a leading "[omit — use Codex default]" choice highlighted as the default. This is where "all gears selectable" lands in the UX without changing the CLI default. **Known cosmetic mismatch (deferred):** the wizard offers all 8 levels, but the discovered catalog advertises only 4 (`low/medium/high/xhigh`, see `agent_model_profile.rs:293-313`); picking a non-advertised level causes Codex to degrade to fallback model metadata. Track under catalog reconciliation (see Non-goals), do not block this release.
6. **Input — `context_window`:** optional, numeric (`u64`); blank = omit.
7. **Confirm — `force-v1?`** (y/n, default n). Explain one line: pins `multi_agent_version = v1`. **If yes, call `run_codex_debug_models_bundled()` (`codex_setup.rs:313`) here** to derive the bundled catalog; its result is the 3rd arg to `install()` (`bundled_catalog_json: Option<&str>`). Without this call, force-v1 through the wizard silently skips catalog derivation.
8. **Preview:** print the *exact* `llmup.config.toml` + agent file bodies that would be written to stdout (reuse `build_profile_content` + `generate_agent_content` — no new write logic).
9. **Confirm — `proceed with install?`** (y/n). On yes → call the existing `install()` (`codex_setup.rs:707`) with the collected `SetupInput` and the step-7 bundled catalog (if any). On no → print nothing written, exit 0.

**Implementation notes:**

- Reuse: `parse_args` (for the few flags still allowed alongside `--interactive`, e.g. `--force-v1` can be pre-set and the wizard's step 7 reflects/edits it), `normalize_base_url`, `connection_test`, `build_profile_content`, `generate_agent_content`, `run_codex_debug_models_bundled` (`:313`), `install`. **No new file-write logic.**
- All wizard prompts return into the existing `CliOptions`/`SetupInput` structs — the install path is unchanged.
- **Non-TTY guard:** `--interactive` must check `std::io::IsTerminal` (or dialoguer's own TTY check) on stdin. On non-TTY stdin (CI, piped, `nohup`) dialoguer reads EOF → errors or hangs. If not a TTY, reject with guidance ("`--interactive` requires a terminal; use flags for scripting") or fall through to flag-mode.
- Keep the non-interactive path (today's flags) fully working; `--interactive` is additive. Mutually-exclusive: if `--interactive` is set, `--base-url`/`--model`/`--provider-key`/`--reasoning-effort` act as **pre-filled defaults** for their wizard steps (powers scripted + overridden-by-prompt workflows).

**Tests:**

- The wizard is interactive I/O — do not unit-test the prompt sequence. Instead, factor the *decision logic* (e.g. "given discovered models + chosen alias, build `SetupInput`") into a pure helper and test that. The prompts are thin wrappers.
- Manual verification checklist (in the plan, executed by implementer): run with and without prefilled flags; ESC/Ctrl-C mid-flow writes nothing; a full pass produces the same files as the equivalent one-liner.

---

## Non-goals (scope guard)

- **Deferred:** reconcile `default_codex_catalog_entry().supported_reasoning_levels` (4 levels) with the 8-level union (`agent_model_profile.rs:293-313`). Server-side, separate concern; track under the reasoning-unification plan.
- **Deferred:** ratatui full-screen wizard / animated TUI. `dialoguer` sequential prompts are the chosen UX.
- **Out of scope:** catalog-derivation changes, `model_catalog_json` semantics beyond the bare-key strip fix, AGENTS.md management, Claude Code auto-config. (See `2026-08-02-codex-subagent-tui-plan.md` for the broader subcommand direction.)
- No new evidence/gate/report/audit docs for this work.
- Do not change the CLI default for `--reasoning-effort` (omit); only validate + document it.

## Risks (consolidated)

- **Terminal / cross-platform:** `dialoguer` relies on `console`, which handles Unix + Windows raw-mode I/O; no platform-specific code is added by this work. Residual risk is non-TTY stdin (CI/piped), addressed by the `IsTerminal` guard above (F5).
- **Cancellation safety:** every wizard prompt returns before `install()` is called, so ESC/Ctrl-C at any step writes nothing — `install()` runs only at the final confirm (step 9).
- **Secret handling:** the provider key is used solely for the step-3 connection test and is never persisted — `SetupInput` has no key field, so there is no path by which the key reaches disk or state.

## Sequencing

1. **HOTFIX 1** (bare-key strip, TDD) → commit. Unblocks safe `--force-v1` re-runs.
2. **HOTFIX 2** (slug/id read + no-OK-on-zero, TDD) → commit. Prerequisite for the wizard's model-discovery step.
3. **Feature 1** (reasoning validation + help, TDD) → commit. Small.
4. **Feature 2** (`dialoguer` dep + `--interactive` wizard, reusing `install()`) → commit. Medium.
5. Bump version + release notes covering both hotfixes + both features.

Each step is independently shippable and test-backed. Steps 1-2 may fast-ship as a patch ahead of 3-4 if release cadence wants the hotfixes out sooner.
