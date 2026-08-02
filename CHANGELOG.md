# Changelog

## Unreleased / Next v0.3.2 (not published)

- Added OpenAI Responses `agent_message` input-item translation so cross-provider Codex sub-agents receive inter-agent tasks instead of an "outside the portable cross-protocol subset" rejection (on `main` at commit `4b1ea7d`). The V2 encrypted `agent_message` form is accepted with a portability warning because its Fernet payload is opaque to anyone but the OpenAI server; the supported hybrid topology is V1 multi-agent, where an official main agent (ChatGPT login) dispatches to a local `llmup` sub-agent and the task arrives as a plain user message.
- Hardened maximum-compatibility encrypted-content handling: the fourth `encrypted_content` site (`function_call_output` / `custom_tool_call_output` output arrays) is now assessment-owned, mirroring the existing `agent_message` / `reasoning` / `compaction` sites — text-surviving payloads warn-and-drop the opaque blob while encrypted-only outputs fail closed; a degraded custom-tool replay path that could leak the opaque blob is fixed; and malformed input items lacking both `type` and `role` now surface an explicit rejection instead of vanishing silently.
- Advanced the main-branch release identity to Cargo package version `0.3.2` (release tag `v0.3.2`, not published), the next patch version after the published, occupied `v0.3.1` tag, without moving, deleting, or reusing the existing tag.

## v0.3.1 - 2026-08-01

- Fixed 25+ high-risk defects across protocol translation correctness, streaming stability (timeout/cancellation gaps, O(n²) algorithmic-complexity DoS, mid-stream error handling), security (proxy-credential leak in error bodies, forgeable tool-call attestation), and error-response shaping (200-on-error, finish-reason mapping, tool-result validation).
- Added unified reasoning effort with per-upstream `dialect` parameter: accept the union vocabulary (none→ultra) and map to each provider's native format (OpenAI reasoning_effort, Anthropic output_config.effort / thinking). Named presets (deepseek-openai, glm-openai, glm-anthropic, qwen-openai) for one-word config. Optional and additive — no behavior change without dialect.
- Refreshed protocol baselines to 2026-07-31 against current official OpenAI and Anthropic docs.
- Eliminated test-suite flakiness (process-global env mutation, wall-clock deadline races).
- Added Codex CLI compatibility for translated upstreams: warn-and-omit Responses namespace tool groups instead of hard-rejecting (P0), populate the synthesized Responses `model` field and emit the `openai-model` response header (P1), and serve a Codex-compatible `ModelsResponse` catalog from `/models` (P2).
- Bridged OpenAI Responses `{type:"namespace"}` tool groups (Codex `multi_agent_v1`) into flattened `<namespace>__<child>` function tools with streaming and non-streaming reversal, history replay, and tool_choice coverage, so Codex multi-agent sub-agents work through Chat/Anthropic upstreams; namespaces with non-function children stay fail-closed. Mirrors the existing custom-tool bridge (request-scoped context v2 to v3).
- 1212 tests, deterministically green.
- Advanced the main-branch release identity to Cargo package version `0.3.1` (release tag `v0.3.1`, not published), the next patch version after the published, occupied `v0.2.44` tag and the unpublished, tagged `v0.3.0` attempt, without moving, deleting, or reusing the existing tag.

## v0.2.44 - 2026-05-29

- Disabled Codex `features.multi_agent` automatically for launcher-managed aliases backed by translated upstream formats, avoiding Responses namespace tool failures on OpenAI Chat Completions and other non-native Responses targets while preserving multi-agent on native Responses upstreams.
- Switched launcher-managed Claude Code authentication from `ANTHROPIC_API_KEY` to gateway-style `ANTHROPIC_AUTH_TOKEN`, so `llmup-claude` can reach the local proxy without entering Claude Code's interactive API-key approval or browser-login path.
- Documented the researched `multi_agent_v1` namespace-tool bridge design, including a future whitelisted flat-function bridge path and the required unary, streaming, history, and tool-choice regression coverage before enabling it experimentally.
- Made the protected compatible provider live smoke advisory for release publication while still always uploading `compatible-provider-smoke.json`, so external compatible-provider outages do not block deterministic patch releases.
- Advanced the main-branch release identity to Cargo package version `0.2.44`, the next patch version after the published, occupied `v0.2.40` tag and the unpublished, failed `v0.2.41` / `v0.2.42` / `v0.2.43` tags, without moving, deleting, or reusing the existing tag.
- Kept the checked-in container publication manifest and container docs anchored to the published `v0.2.40` multi-arch image digest while using `0.2.44` / `v0.2.44` as the next release identity, not a published container tag yet.
- Recorded `v0.2.41` and `v0.2.43` as unpublished release attempts blocked by Compatible Provider Smoke and `v0.2.42` as an unpublished release attempt blocked by Python contract tests, all before GitHub Release creation.
- Verified the `v0.2.40` release workflow, main/tag CI, GitHub Release assets, online `install.sh` asset, and GHCR `v0.2.40` multi-arch image publication.

## v0.2.40 - 2026-05-20

- Stopped injecting `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1` by default in `llmup-claude`, preserving Claude Code's native permission and sandbox behavior for flags such as `--dangerously-skip-permissions` while keeping llmup's own startup-time provider secret scrubbing.
- Advanced the main-branch release identity to Cargo package version `0.2.40`, the next patch version after the published, occupied `v0.2.39` tag, without moving, deleting, or reusing the existing tag.
- Refreshed the checked-in container publication manifest and container docs around the published `v0.2.39` multi-arch image digest while keeping `0.2.40` / `v0.2.40` as the next release identity, not a published container tag yet.
- Verified the `v0.2.39` release workflow, main/tag CI, GitHub Release assets, online `install.sh` asset, and GHCR `v0.2.39` multi-arch image publication.

## v0.2.39 - 2026-05-20

- Fixed the release-blocking Clippy `large_enum_variant` finding in launcher profile projection by boxing the model catalog inside `ProfileProjection::Enabled`; this keeps the Codex/Claude launcher behavior unchanged while satisfying the release gate.
- Advanced the main-branch release identity to Cargo package version `0.2.39`, the next patch version after the published, occupied `v0.2.37` tag and the unpublished, failed `v0.2.38` tag, without moving, deleting, or reusing the existing tag.

## v0.2.38 - 2026-05-20

- Reworked the launcher-managed `llmup-config` path around a lower-cognitive-load interactive setup: new configs use the `main` local model name, strict service types (`openai-chat-completions`, `openai-responses`, `anthropic-messages`), and a single visible add-model flow instead of hidden compatibility commands.
- Added multi-model user tooling support: `llmup-config list` and `add-model` can manage multiple services and aliases, `llmup-codex` projects all configured aliases into the Codex model catalog, and `llmup-claude` projects the selected alias plus configured `haiku` / `sonnet` / `opus` family aliases.
- Hardened Codex/Claude launcher metadata and subagent behavior with generated Codex catalog descriptions, selected-model environment projection, Claude family model environment isolation, and focused fake-client/full-flow regression coverage.
- Updated README / README_CN / clients / advanced docs around the install -> configure -> launch path, explicit protocol names, and the `main` default model; advanced server and manual YAML usage remain separate from the beginner path.
- Removed the hidden `llmup-config init --non-interactive` surface from the product CLI, keeping tests on the same interactive setup path that users run while preserving internal Rust helpers for focused generation tests.
- Advanced the main-branch release identity to Cargo package version `0.2.38`, the next patch version after the published, occupied `v0.2.37` tag, without moving, deleting, or reusing the existing tag. This release identity was not published because CI failed on Clippy and was superseded by `v0.2.39`.

## v0.2.37 - 2026-05-19

- Added launcher-managed model capability projection for Codex CLI and Claude Code, including configured context windows, maximum output tokens, Codex auto-compact thresholds, web-search capability, and Claude custom model metadata.
- Added `llmup-config set-limits` so users can update alias or upstream model limits without hand-editing YAML, while preserving existing targets and surface metadata.
- Moved CLI matrix and interactive harness setup onto the Rust launcher launch-plan path, keeping tests aligned with real `llmup-codex` / `llmup-claude` behavior instead of duplicating projection logic in Python.
- Hardened launcher behavior around native model/profile conflicts, true `--llmup-no-proxy` passthrough, generated Codex model catalogs, Anthropic model limit exposure, and focused regression coverage for the new agent metadata path.
- Advanced the main-branch release identity to Cargo package version `0.2.37`, the next patch version after the unpublished `0.2.36` development identity and the published, occupied `v0.2.35` tag, without moving, deleting, or reusing the existing tag.

## v0.2.36 - 2026-05-19

- Advanced the main-branch release identity to Cargo package version `0.2.36`, the next patch version after the published, occupied `v0.2.35` tag, without moving, deleting, or reusing the existing tag.
- Refreshed the checked-in container publication manifest and docs around the actual published `v0.2.35` multi-arch image digest while keeping `0.2.36` / `v0.2.36` as the next release identity, not a published container tag yet.
- Verified the `v0.2.35` release workflow, tag-triggered CI, GitHub Release, GHCR `v0.2.35`, `0.2.35`, and `latest` image tags, plus the online `install.sh` path after publication.

## v0.2.35 - 2026-05-19

- Clarified OpenAI-family format naming across config, user tooling, traces, fixtures, and examples: Chat Completions now emits `openai-chat-completions`, Responses remains `openai-responses`, and the old `openai-completion` spelling is kept only as an input alias.
- Advanced the main-branch release identity to Cargo package version `0.2.35`, the next patch version after the published, occupied `v0.2.34` tag, without moving, deleting, or reusing the existing tag.
- Refreshed the checked-in container publication manifest and docs around the actual published `v0.2.34` multi-arch image digest while keeping `0.2.35` / `v0.2.35` as the next release identity, not a published container tag yet.
- Verified the `v0.2.34` release workflow, GitHub Release, GHCR `v0.2.34`, `0.2.34`, and `latest` image tags, plus the online `install.sh` path and no-argument `llmup-config` setup after publication.

## v0.2.34 - 2026-05-18

- Fixed `llmup-config` with no arguments so it now runs the first-use interactive setup instead of only printing usage, and added regression coverage for the installed `llmup-config` entrypoint.
- Advanced the main-branch release identity to Cargo package version `0.2.34`, the next patch version after the published, occupied `v0.2.33` tag, without moving, deleting, or reusing the existing tag.
- Refreshed the checked-in container publication manifest and docs around the actual published `v0.2.33` multi-arch image digest while keeping `0.2.34` / `v0.2.34` as the next release identity, not a published container tag yet.
- Verified the `v0.2.33` release workflow, GitHub Release, GHCR `v0.2.33`, `0.2.33`, and `latest` image tags, and the online `install.sh` path after publication.

## v0.2.33 - 2026-05-18

- Restored full repository documentation contract coverage after the user-facing README / clients simplification: README now keeps pointers to the advanced compatibility, protocol matrix, and design docs while the quick start remains focused on install -> configure -> launch.
- Clarified that launcher-managed Codex setup uses live `llmup.surface` metadata when available, preserving configured model capability truth without reintroducing manual proxy wiring into the beginner path.
- Advanced the main-branch release identity to Cargo package version `0.2.33`, the next patch version after the unpublished `0.2.32` failed release identity and the published, occupied `v0.2.30` tag, without moving, deleting, or reusing the existing tag.

## v0.2.32 - 2026-05-18

- Added the user-facing tooling layer: `llmup-config`, `llmup-codex`, and `llmup-claude` now provide a low-friction local setup path while preserving the existing `llm-universal-proxy --config ...` advanced server mode.
- Added a POSIX `install.sh` release asset path that installs the main binary, creates the three user-tool aliases, verifies checksums, rejects unsafe archives, avoids `sudo`, and prints absolute next-step commands when the install directory is not on the current `PATH`.
- Reworked README / README_CN around the new install -> configure -> launch flow and moved manual proxy startup, multi-endpoint YAML, and manual Codex/Claude wiring into `docs/advanced-usage.md`.
- Hardened release, governance, and documentation contracts around the new user-tooling path, including installer smoke coverage, help/version checks, README leakage checks, and launcher-managed versus manual-proxy documentation boundaries.
- Advanced the main-branch release identity to Cargo package version `0.2.32`, the next patch version after the unpublished `0.2.31` development identity and the published, occupied `v0.2.30` tag, without moving, deleting, or reusing the existing tag.

## v0.2.31 - 2026-05-18

- Advanced the main-branch release identity to Cargo package version `0.2.31`, the next patch version after the published, occupied `v0.2.30` tag, without moving, deleting, or reusing the existing tag.
- Refreshed the checked-in container publication manifest and docs around the actual published `v0.2.30` multi-arch image digest while keeping `0.2.31` / `v0.2.31` as the next release identity, not a published container tag yet.
- Verified the `v0.2.30` release workflow, GitHub Release, GHCR `v0.2.30`, `0.2.30`, and `latest` image tags, and tag-triggered CI after publication.

## v0.2.30 - 2026-05-18

- Maximized Claude Code / Anthropic Messages compatibility with OpenAI-compatible Chat targets by allowing complete visible thinking and context-management history to translate instead of failing closed on provider-native request controls.
- Hardened Anthropic context-management handling, prompt-cache control synthesis contracts, and debug trace redaction so `llmup` only forwards or synthesizes provider request controls it can represent safely.
- Strengthened the real CLI and trace verifier matrix for Claude Code, OpenAI Chat, and OpenAI Responses targets, including duplicate request id rejection, Responses tool-call pairing evidence, and Python workspace behavior checks.
- Added the pre-GA context editing / trace / cache contract correction handoff plan under `docs/engineering`, covering single prepared request bodies, provider prompt-cache defaults, Anthropic beta headers, and focused TDD gates.
- Advanced from the unpublished `0.2.29` development identity to Cargo package version `0.2.30` while preserving the published, occupied `v0.2.28` tag; this keeps the release identity on the next patch version available for publication without moving, deleting, or reusing the existing tag.

## v0.2.29 - 2026-05-17

- Advanced the main-branch release identity to Cargo package version `0.2.29`, the next patch version after the published, occupied `v0.2.28` tag, without moving, deleting, or reusing the existing tag.
- Refreshed the checked-in container publication manifest and docs around the actual published `v0.2.28` multi-arch image digest while keeping `0.2.29` / `v0.2.29` as the next release identity, not a published container tag yet.
- Verified the `v0.2.28` release workflow, GitHub Release, GHCR `v0.2.28`, `0.2.28`, and `latest` image tags, and tag-triggered CI after publication.

## v0.2.28 - 2026-05-17

- Removed the native Gemini wire/client/provider format path. Gemini is now treated only as a Google OpenAI-compatible upstream, reducing product surface area and translation complexity.
- Removed public compatibility tiers and mode selection in favor of one maximum-safe compatibility target; same-protocol routes preserve raw request/response forwarding when no mutation is required.
- Added a lightweight in-memory conversation state bridge for mapping stateful OpenAI Responses requests, including `previous_response_id` and conversation state, onto stateless providers with necessary replay.
- Added provider-native prompt-cache support and observability by mapping controls such as Anthropic `cache_control` and OpenAI `prompt_cache_key`, emitting cache usage telemetry, and redacting debug traces; `llmup` still does not implement its own prompt cache.
- Hardened compatibility, observability, and release confidence with portability warnings on upstream errors, prompt-cache trace guardrails, debug trace replay metadata, and refreshed Codex/Claude end-to-end smoke evidence.
- Advanced the main-branch release identity to Cargo package version `0.2.28`, the next patch version after the published, occupied `v0.2.27` tag, without moving, deleting, or reusing the existing tag.

## v0.2.27 - 2026-05-01

- Made the single maximum-compatibility translation path repair OpenAI Chat Completions target requests by downgrading `system` and `developer` messages to annotated `user` turns, so narrow OpenAI-compatible chat providers that reject high-priority roles can still serve Codex, Claude Code, Gemini, and Responses clients.
- Kept native OpenAI Responses-compatible upstreams on their own protocol surface: `instructions`, `system`, and `developer` roles remain intact when the target upstream format is `openai-responses`.
- Expanded translation regression coverage for Responses-to-Chat compaction/instruction history, same-format OpenAI Chat maximum-compatibility handling, Gemini/Anthropic-to-Chat instruction lowering, and rejection of removed legacy compatibility input.
- Advanced the main-branch release identity to Cargo package version `0.2.27`, the next patch version after the unpublished `0.2.26` development identity and the published, occupied `v0.2.25` tag, without moving, deleting, or reusing the existing tag.

## v0.2.26 - 2026-04-29

- Advanced the main-branch release identity to Cargo package version `0.2.26`, the next patch version after the published and occupied `v0.2.25` tag, without moving, deleting, or reusing the existing tag.
- Refreshed the checked-in container publication manifest and docs around the actual published `v0.2.25` multi-arch image digest while keeping `0.2.26` / `v0.2.26` as the next release identity, not a published container tag yet.
- Verified the `v0.2.25` release workflow, GHCR `v0.2.25` and `0.2.25` image tags, and tag-triggered CI after publication.

## v0.2.25 - 2026-04-28

- Added static and Admin API support for structured `data_auth` and upstream
  provider credential sources, including inline/env proxy keys,
  inline/env/legacy-env provider keys, and the CAS-based `/admin/data-auth`
  control-plane endpoint.
- Simplified data-plane auth to the two GA modes, `proxy_key` and
  `client_provider_key`, with request-scoped auth/runtime snapshots so Admin
  updates affect new requests without mixing old auth decisions with new
  provider credentials.
- Hardened known-credential redaction across public JSON and SSE responses,
  error paths, debug traces, hooks, dashboard metrics/logs, model/resource
  metadata, allowed upstream response headers, and hook delivery logs.
- Documented static YAML and runtime Admin API configuration paths, including
  container replay requirements for runtime-only Admin API writes, and expanded
  docs contracts for the GA data-auth shape.
- Advanced the main-branch release identity to Cargo package version `0.2.25`, the next patch version after the occupied `v0.2.24` tag, without moving, deleting, or reusing the existing tag.
- Refreshed the checked-in container publication manifest and docs around the actual published `v0.2.24` multi-arch image digest while keeping `0.2.25` / `v0.2.25` as the next release identity, not a published container tag yet.
- Updated the container guide and GA docs contract to treat API bootstrap, `/health`, `/ready`, and no-mount Admin API startup as current published `v0.2.24` image behavior.

## v0.2.24 - 2026-04-28

- Advanced the main-branch release identity to Cargo package version `0.2.24`, the next patch version after the occupied `v0.2.23` tag, without moving, deleting, or reusing the existing tag, so the published `v0.2.23` container tag remains immutable.
- Refreshed the checked-in container publication manifest and docs around the actual published `v0.2.23` image digest while keeping `0.2.24` / `v0.2.24` as the next release identity, not a published container tag yet.
- Added container API bootstrap support for control-plane-managed deployments: the image now ships a secret-free empty `/etc/llmup/config.yaml`, `/health` remains liveness, `/ready` reports readiness only after at least one namespace is loaded, and Docker health checks target `/ready`.
- Added the explicit `--admin-bootstrap` binary mode for direct all-Admin-API startup, requiring a non-empty `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` before serving an initially empty runtime.
- Expanded container smoke coverage to exercise both static config mounts and the no-mount bootstrap path: start from the built-in empty config, apply runtime config through the Admin API, wait for readiness, and complete a streaming proxy request through a mock upstream.
- Clarified GA container and configuration docs for static YAML versus runtime Admin API payloads, `proxy_key` versus `client_provider_key` data-plane auth, and the boundary between the currently published `v0.2.23` image and the next-release bootstrap behavior.

## v0.2.23 - 2026-04-28

- Kept the release identity on Cargo package version `0.2.23`, the next patch version after the occupied `v0.2.22` tag, without moving, deleting, or reusing the existing tag and without jumping to `0.2.24` while `v0.2.23` remains available.
- Stabilized the real CLI matrix by inferring tool-loop requirements from workspace templates, long-horizon fixtures, and nested verifiers, so Codex, Claude Code, and Gemini workspace-edit cases are classified consistently.
- Aligned Codex wrapper provider configuration on `model_providers.proxy.env_key="OPENAI_API_KEY"` while preserving the hermetic proxy base URL, Responses wire API, and scripted interactive gate behavior shared by the matrix and interactive wrapper tests.
- Tightened expected fail-closed handling by canonicalizing upstream format aliases such as `chat`, `openai`, `claude`, and `gemini`, and by requiring complete Gemini thought-signature error evidence instead of treating bare marker strings as expected failures.
- Split checked-in container publication state from the next release identity: the new release container manifest records the current published `v0.2.22` tag and digest, while docs and Compose pin that published image and describe `0.2.23` / `v0.2.23` as not published yet.
- Added the release workflow digest artifact path for the pushed GHCR image, including a machine-readable `artifacts/container-image.json` with image, release/version tags, digest, package version, git SHA, publish timestamp, run URL, and post-release next identity.
- Expanded docs and governance coverage for README/container current-vs-next release wording, manifest schema and digest invariants, workflow digest artifact upload, protected release gates, and the real CLI matrix fail-closed contracts.

## v0.2.22 - 2026-04-27

- Documented provider-neutral preset naming as the portable GA path, keeping named official and compatible providers as operator examples rather than release-blocking dependencies.
- Clarified Responses reasoning/compaction continuity boundaries for opaque state: visible summaries or transcript context may be preserved with warnings under the single maximum-compatibility path, while opaque-only continuity still fails closed outside internal raw forwarding or native provider handling.
- Recorded the hermetic Codex wrapper interaction gate as the deterministic release check for scripted two-turn wrapper behavior.
- Aligned GA docs alignment around protected `COMPAT_*` smoke evidence, precise chat-completions/messages route coverage, and Actions artifact retention.

## v0.2.21 - 2026-04-26

- Bumped the release identity past the occupied `v0.2.20` tag without moving, deleting, or reusing the existing tag.
- Hardened governance so CI and release fetch full tag history before checking release identity, and fail closed when a shallow checkout makes tag visibility unsafe.
- Collected the recent provider-neutral CLI matrix, compatible-provider smoke, streaming translation, and release-governance fixes under the forward-bumped release line.

## v0.2.20 - 2026-04-25

- Split the supply-chain gate into a Cargo-supported lockfile integrity check via `cargo metadata --locked --format-version 1 --no-deps` and a cargo-audit execution that no longer passes unsupported audit flags.
- Routed both CI and release through the shared `scripts/supply_chain_audit.sh` contract while keeping release SBOM generation and upload in place.
- Bumped the release identity after the failed occupied `v0.2.19` run.

## v0.2.19 - 2026-04-25

- Changed the repo-side release workflow to require the provider-neutral compatible GA smoke gate before publishing, using the protected `release-compatible-provider` environment while leaving the four official provider live smoke as optional extended evidence rather than a protected release blocker.
- Locked the `compatible-provider-smoke` invocation, JSON artifact upload, and release publish dependencies under `scripts/check-governance.sh` and Python release-gate contract tests.
- Added release identity governance that fails when `refs/tags/v$VERSION` already exists at a commit other than current `HEAD`, preventing continued development on an occupied tag/version pair.

## v0.2.18 - 2026-04-25

- Hardened the release workflow so Docker Buildx diagnostic records do not get uploaded as release artifacts.
- Limited GitHub Release artifact downloads to packaged `llm-universal-proxy-*` binaries, preventing Docker build records from breaking release creation.

## v0.2.17 - 2026-04-25

- Added an embedded Web Admin Dashboard at `/dashboard` as a single-binary static shell. `/dashboard` shell and static assets are public UI resources; admin work stays behind the existing `/admin/*` APIs, where `/admin/*` API calls require `Authorization: Bearer <admin-token>` using `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN`. The data-plane token is separate, and the dashboard does not introduce service keys, multi-user auth, sessions, or a separate frontend runtime.
- Added dashboard regression coverage for public shell loading, admin-token-protected API access, static asset content types, no inherited CORS, redacted-state read-only guidance, and responsive mobile layout boundaries.
- Productionized container support with a non-root Docker image, `/etc/llmup/config.yaml` default config path, OCI labels, `/health` Docker `HEALTHCHECK`, Makefile Docker targets, and a container smoke test that verifies image metadata, admin-token gating, Docker health, and a streaming proxy path through a mock upstream.
- Added GHCR release publishing for `ghcr.io/agentsmith-project/llm-universal-proxy`, including CI image build/smoke without push, tag-only multi-arch release publishing, container examples, and container/admin dashboard documentation.

## v0.2.16 - 2026-04-25

- Hardened translated multimodal media-source boundaries so polluted URI-like references fail closed before upstream routing, including raw and percent-encoded control characters, Unicode separators, zero-width spaces, and invalid `data:` URI fallbacks across Anthropic, OpenAI Chat, OpenAI Responses, and Gemini paths.
- Centralized canonical inline base64 validation for translated media inputs, requiring non-empty canonical payloads for data URIs, bare base64, Anthropic `source.data`, Gemini `inlineData.data`, OpenAI `input_audio.data`, and tool-result media instead of trimming or forwarding noncanonical payloads.
- Extended OpenAI Chat <-> Responses media conversion checks so translated `image_url`, `input_image.image_url`, `file_data`, `file_url`, and `input_audio.data` fields share the same sanitizer while preserving same-format forwarding behavior.
- Added broad unit and end-to-end regression coverage for polluted multimodal URLs, provider/local file URIs, data URI metadata, empty media payloads, and raw inline base64 across OpenAI, Anthropic, and Gemini translation paths.

## v0.2.15 - 2026-04-25

- Supported native OpenAI Responses retrieval streaming for `GET /v1/responses/{response_id}?stream=true`, including upstream `Accept: text/event-stream`, guarded same-format SSE forwarding, and fail-closed handling when an upstream returns non-SSE success bodies.
- Hardened OpenAI Chat `file` and OpenAI Responses `input_file` MIME provenance checks so conflicting `mime_type` / `mimeType`, `file_data` data URI MIME, or filename-derived hints are rejected before upstream routing, including same-format Responses forwarding and OpenAI-to-Gemini translation paths.
- Expanded MIME provenance regression coverage for camelCase metadata, top-level versus nested file metadata, filename conflicts, and Gemini conversion paths, then re-verified Codex, Claude Code, and Gemini real-client smoke coverage plus a Codex long-horizon 6502 emulator task through MiniMax paths.
- Documented the typed-media MIME provenance safety rule across user, configuration, compatibility, and architecture docs so conflicting `mime_type` / `mimeType`, data URI MIME, or filename hints are clearly described as fail-closed request errors.

## v0.2.14 - 2026-04-24

- Tightened the maximum-compatibility public boundary so client-visible tool identity stays stable, while proxy-private `__llmup_custom__*` transport names and `_llmup_tool_bridge_context` state no longer leak across or get trusted at external boundaries.
- Made translated SSE handling frame-aware, failing closed on malformed raw artifact frames and artifact event types while preserving literal boundary text inside successful text, schema, and metadata payloads.
- Hardened Responses resource and lifecycle success framing: non-204 / 205 empty success bodies now map to `502`, and 204 / 205 validation uses a no-auto-decompression client to reject illegal `Content-Length` / `Transfer-Encoding` no-content framing.
- Added downstream-disconnect cancellation handling, expanded CLI matrix owned-health diagnostics, and stabilized real Codex `apply_patch` routing on the original public tool name.
- Refined upstream egress proxy configuration docs, examples, CI Python unittest governance, and test-report generation so release validation and operational guidance stay reproducible.

## v0.2.13 - 2026-04-24

- Hardened the Codex real-client prework / work-summary verifier so read-only classification now fails closed around shell startup hooks, PATH-based command resolution, wrapper aliases, shell redirection and control operators, shell expansions, and environment/config-driven helper execution.
- Restricted Codex prework read-only evidence to trusted direct system commands or isolated `python3 -I -S -c` snippets with explicit parser allowlists, while rejecting shell wrappers, bare PATH commands, `egrep` / `fgrep` wrappers, unsafe `rg` helpers, mutating `sed` / `find` forms, and zsh expansion edge cases.
- Expanded TDD coverage for direct dangerous command cases, including shell operator bypasses, tilde / equals / extended-glob expansion, Python startup and AST boundaries, `rg --no-config` enforcement, and direct parser regressions that are no longer hidden by shell-wrapper rejection.
- Re-verified the Codex prework signal work-summary matrix case against `minimax-openai` after the verifier contract tightened, preserving the intended UI `Working` state recovery coverage while avoiding unsafe read-only false positives.

## v0.2.12 - 2026-04-21

- Fixed translated Responses commentary message lifecycle and `phase` semantics used by Codex `Working` state recovery, so mid-turn preambles now close as `commentary` and terminal text after tool-boundary finishes is no longer mislabeled as `final_answer`.
- Preserved completed Responses `response.output` ordering across commentary, tool calls, and final answers, so terminal payloads keep mid-turn commentary instead of dropping it after the stream-level `done` event.
- Tightened Anthropic translated streaming semantics: reasoning deltas now stay incremental, while generic OpenAI chat / Responses structured tool calls are only finalized when the block's final JSON remains valid, avoiding exposure of revocable tool calls to general clients.
- Added focused streaming and proxy integration regressions around commentary/tool boundaries, terminal output ordering, and translated Anthropic tool-call validity transitions.

## v0.2.11 - 2026-04-21

### Configuration And Routing

- Added upstream egress proxy configuration at both the namespace and per-upstream levels, so you can route provider traffic through a forward proxy or explicitly bypass it with `direct`.
- Exposed resolved upstream proxy source and mode in the admin state views, and added a copyable `examples/upstream-proxy.yaml` example for static config setups.

### Docs And Quickstart

- Reworked the public README/docs around the `llmup` name and provider-API-only usage scope, with a docs index plus dedicated guides for configuration, client setup, and admin-driven dynamic config.
- Replaced the old homepage path with a simpler two-upstream OpenAI + DeepSeek quickstart and matching `examples/quickstart-openai-deepseek.yaml`, including stable alias examples such as `gpt-5-4` and `gpt-5-4-mini`.
- Clarified Codex, Claude Code, and Gemini wrapper routing so the docs now distinguish wrapper base URLs from the proxy endpoints they eventually hit.

### Tooling

- Updated the interactive wrappers and real CLI matrix to auto-pick the newest available proxy binary from `target/debug` or `target/release` instead of assuming a release build.

## v0.2.10 - 2026-04-21

- Closed the remaining public `proxec` -> `llmup` rename drift across model-catalog list, object, and direct-upstream object responses so public `owned_by`, Google `version`, Google `description`, and embedded metadata all expose one consistent `llmup` namespace.
- Aligned live `llmup.surface` metadata with the proxy's effective model surface source of truth, so public model payloads and wrapper-generated runtime config now agree on surfaced limits, modalities, search support, and related tool metadata instead of drifting through parallel metadata paths.
- Restored the public Codex `apply_patch` contract to `freeform` on surfaced metadata and wrapper catalogs, keeping the internal function-wrapper bridge transport private instead of exposing it as the user-visible tool transport.
- Hardened the OpenAI Responses -> Google custom/grammar tool bridge so maximum-compatibility translated paths preserve stable public tool identity such as `apply_patch` while using the canonical single-string wrapper only as an internal transport detail for Gemini compatibility.
- Preserved Gemini live custom-tool streaming on the native SSE path instead of silently downgrading to unary transport, with regression coverage around `:streamGenerateContent` routing and streamed Responses tool-call behavior.

## v0.2.9 - 2026-04-21

- Fixed wrapper-generated runtime config serialization so managed proxy launches now preserve upstream `surface_defaults` and structured alias `surface` overrides instead of silently dropping model-surface metadata when rewriting the source config for test and interactive runs.
- Corrected Codex metadata resolution to follow the proxy's effective-surface precedence: alias `surface` now overrides upstream surface defaults on client-facing metadata, while legacy `codex` fields only fill gaps instead of incorrectly overriding the effective model surface.
- Updated the interactive client wrappers and their regression coverage around maximum-permission / yolo startup flags so Codex, Claude, and Gemini launch with the intended non-interactive approval posture for these proxy-driven harnesses.

## v0.2.8 - 2026-04-20

- Historical pre-removal note: the runtime briefly exposed named compatibility postures for translated agent-facing paths; current runtime has removed those behavior choices and rejects the old compatibility input instead of treating it as a migration no-op.
- Preserved stable public tool identity across translated live paths: client-visible and model-visible surfaces now keep original tool names such as `apply_patch` instead of exposing proxy-private `__llmup_custom__*` transport artifacts, with matching smoke and regression coverage around that contract.
- Stopped trusting client-supplied `_llmup_tool_bridge_context` payloads at proxy ingress; the bridge context is now enforced as an internal-only request-scoped field so external callers cannot spoof custom/freeform tool decoding state.
- Synced Codex-facing model surface metadata from the same source-of-truth model surface used by the proxy, so generated catalogs and wrapper defaults stay aligned on text-only modalities, search support, and `apply_patch` tool surfacing instead of drifting through parallel metadata paths.
- Fixed OpenAI Responses tool-call sink finalization so pending translated tool calls emit consistent `done` / `response.output_item.done` events with the correct payload and proxied tool metadata during flush and teardown paths.
- Removed the dead Claude-to-OpenAI single-argument conversion wrapper after the last test-only caller moved to the real implementation, clearing the remaining Rust `dead_code` warning without changing translation behavior.

## v0.2.7 - 2026-04-20

- Routed dashboard-mode runtime logs into an in-memory TUI log buffer and rendered them inside a new `Runtime Logs` panel, so live `warn!` / `info!` / `error!` output no longer overwrites the alternate-screen dashboard while the proxy is serving traffic.
- Rebalanced the dashboard activity area so recent latency is shown as a compact trend sparkline and recent-request tables stay readable across tighter terminal heights while sharing space cleanly with live runtime logs.
- Fixed OpenAI Responses sink tool-stream finalization so pending tool calls emit the expected done events promptly on tool-call finishes instead of leaving completion semantics to later terminal cleanup paths.
- Improved the Codex interactive wrapper flow by generating explicit `apply_patch_tool_type: freeform` metadata in temporary catalogs, disabling `view_image` for text-only defaults, and refreshing the English / Chinese manual-testing docs around those safer defaults.

## v0.2.6 - 2026-04-19

- Propagated configured `max_output_tokens` defaults from resolved model limits into request translation when clients omit an explicit output cap, so Anthropic, OpenAI Completions, and sibling target protocols no longer silently fall back to incorrect hard-coded defaults such as `4096`.
- Updated the generated Codex custom model catalog to follow the current real schema and to compute default `auto_compact_token_limit` from available input budget: `0.85 * (context_window - max_output_tokens)` when both limits are known, while keeping the older `0.85 * context_window` fallback only when no output budget is available.
- Hardened long-session tool replay boundaries by marking incomplete or truncated tool calls as non-replayable and intentionally degrading later replay / bridge paths instead of pretending the partial call is still valid structured history.
- Fixed custom-tool bridge trust transfer so representation rewrites re-sign non-replayable markers after verification instead of literally copying a marker that no longer matches the bridged `name` / raw payload.
- Verified fixes against known failures observed on the Anthropic and OpenAI-completions Codex yolo mainlines, including long-session translation, replay, and compaction regressions, without claiming cross-provider full fidelity for every long-horizon path.
- Tightened cross-protocol compatibility handling so more non-portable request and typed-item semantics fail closed or surface explicit compatibility warnings instead of silently widening behavior across OpenAI, Responses, Anthropic, and Gemini paths.
- Hardened runtime-chain observability during streaming teardown: hooks and `debug_trace` now preserve protocol-level terminal outcomes through disconnects and error endings, while bounded background capture paths surface explicit truncation / overflow accounting instead of silently dropping data or accumulating unbounded exchange payloads.
- Expanded `debug_trace` coverage for Google / Gemini client-format streaming so traces record protocol-level terminal, error, text, and tool-call summaries rather than only the final transport outcome.
- Normalized Gemini CLI matrix runner workspace handling so smoke and long-horizon cases use stable absolute `--include-directories` / isolated runner-state paths even when reports are launched from relative directories.

## v0.2.5 - 2026-04-17

- Added `scripts/real_cli_matrix.py` as the reusable real-client CLI matrix runner for repeatable end-to-end proxy testing across real `codex`, `claude`, and `gemini` processes, including stable matrix listing and case targeting.
- Kept `scripts/test_cli_clients.sh` as a compatibility shim for older local flows and wrappers by forwarding it directly to the Python runner.
- Isolated Codex, Claude Code, and Gemini runs with runner-managed home/config/cache state and per-client environment wiring, while reusing a runner-managed Gemini bootstrap home instead of the user's normal profile.
- Added timestamped report artifacts under `test-reports/cli-matrix/`, including JSON and Markdown summaries, per-case logs, captured workspaces, and a `latest` symlink for quick inspection.
- Tightened long-horizon verification so the Python bugfix fixture must both repair the `calc.py` implementation and preserve the expected `main.py` behavior, rejecting comment-only or non-functional edits.
- Kept `qwen-local` as optional coverage enabled only when local env is configured, with the default matrix limiting it to smoke coverage and excluding long-horizon code-edit cases.

## v0.2.4 - 2026-04-07

- Added `rust-toolchain.toml` as the repository's pinned Rust toolchain source and wired CI / release jobs to install Rust from that value instead of implicit `stable`.
- Added repository governance checks for `Cargo.toml` / `Cargo.lock` / `CHANGELOG.md` version alignment, `--locked` cargo usage, Dockerfile toolchain parity, and workflow smoke wiring.
- Added lightweight binary smoke coverage to CI and the Linux release build so tagged artifacts are exercised before packaging.
- Normalized repository-wide Rust formatting so the release `cargo fmt --check` gate passes again.
- Fixed `clippy -D warnings` failures in the debug trace helpers, request handler linting, and shared test mock utilities.
- Re-ran the local release gate successfully with:
  - `cargo fmt --all -- --check`
  - `cargo clippy --locked --all-targets --all-features -- -D warnings`
  - `cargo test --locked --verbose`
- Carries forward the real CLI E2E smoke-script work documented in `v0.2.3`, including real `codex` / `claude` proxy smoke coverage and release-note documentation updates.

## v0.2.3 - 2026-04-07

- Added a real-client E2E smoke script at `scripts/test_cli_clients.sh` that exercises the proxy with actual `codex` and `claude` CLI processes against mixed upstream aliases.
- Verified cross-protocol routing for Codex CLI over OpenAI Responses and Claude Code over Anthropic Messages, including Anthropic-compatible and OpenAI-compatible upstreams behind one proxy.
- Isolated Claude Code smoke tests from the user's global configuration by running them with a temporary `CLAUDE_CONFIG_DIR` and `--bare` mode, without modifying `~/.claude/settings.json`.
- Fixed the Claude smoke-test base URL handling so the client points at `/anthropic` and lets Claude append `/v1/messages` itself.
- Fixed Codex multi-turn smoke tests in temporary workspaces by adding `--skip-git-repo-check`.
- Marked the local `qwen-local` alias as an intentional skip for multi-turn code-edit tasks where the model is not reliable enough, while keeping its single-turn smoke coverage enabled.
- Documented the new smoke script and its constraints in `README.md` and `README_CN.md`.

## v0.2.2 - 2026-03-20

- Fixed streaming request telemetry so downstream client disconnects are recorded as `cancelled` instead of being misreported as `500` errors.
- Added `cancelled` counts to the dashboard and per-upstream traffic panels, and excluded cancelled requests from error-rate accounting.
- Extended `usage` and `exchange` hook payloads with `cancelled_by_client`, `partial`, and `termination_reason` to make interrupted streaming requests observable without draining upstream generation.
- Added regression coverage for request tracker cancellation, hook stream-drop finalization, and early client disconnects against a slow SSE mock.

## v0.2.1 - 2026-03-20

- Added a protocol-namespaced API surface as the formal public interface: `/openai/v1/...`, `/anthropic/v1/...`, and `/google/v1beta/...`.
- Removed the legacy mixed `/v1/...` downstream routes to reduce code complexity and user-facing ambiguity.
- Added local model catalog endpoints under each protocol namespace, including list and retrieve operations.
- Added a terminal dashboard powered by `ratatui` / `crossterm` to expose runtime configuration, request activity, hook state, routing, and upstream traffic at a glance.
- Added isolated CLI examples for Codex CLI, Claude Code, and Gemini CLI, with temporary-home patterns that avoid modifying user-level configuration.
- Fixed Gemini-to-OpenAI request translation so Gemini-style `contents` blocks without an explicit `role` still preserve input text correctly.
- Expanded test coverage for namespaced routing, model catalog endpoints, dashboard plumbing, and Gemini request translation edge cases.

## v0.2.0 - 2026-03-20

- Added product-grade multi-upstream audit hooks with asynchronous `exchange` and `usage` delivery, request/response capture, normalized usage reporting, credential fingerprinting, and per-hook circuit breaker / pending-byte protections.
- Added upstream credential policy controls: `credential_actual`, `auth_policy`, and force-server credential enforcement.
- Added first-class Anthropic Messages client support at `POST /v1/messages`, including request detection, translation, and streaming.
- Improved upstream URL construction to support both versionless roots and versioned compatibility bases such as `.../api/paas/v4`.
- Expanded reasoning / thinking support across OpenAI Chat, OpenAI Responses, Anthropic Messages, and Gemini translation paths, including non-streaming response mapping and streaming lifecycle conversion.
- Added extensive regression coverage for reasoning/thinking, streaming, hooks, and protocol translation, plus a real upstream smoke script covering Anthropic-compatible and OpenAI-compatible services.

## v0.1.4 - 2026-03-20

- Fixed `run_with_listener()` to validate config before serving, which prevents invalid-config startup hangs and unblocks the `missing_upstreams_config_is_rejected` integration test in CI.
- Re-ran the release gate after the YAML/CLI configuration work: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --locked`.

## v0.1.3 - 2026-03-20

- Replaced environment-variable-based proxy configuration with a YAML config file loaded via `--config` / `-c`.
- Added named multi-upstream routing, local unique model aliases, and per-upstream fallback credential env support in the YAML schema.
- Standardized upstream base URLs on versionless roots and moved `/v1` / `/v1beta` path composition into the proxy.
- Removed the legacy single-upstream configuration path to reduce user-facing configuration ambiguity.
- Added targeted tests for YAML parsing, config-file loading, CLI argument parsing, multi-upstream routing, alias resolution, fallback credentials, and startup failure when no upstreams are configured.

## v0.1.2 - 2026-03-18

- Tracked `Cargo.lock` in git so `cargo check/test/clippy --locked` works in CI and release jobs.
- Updated GitHub Actions checkout steps to `actions/checkout@v5` to avoid the Node.js 20 deprecation path on hosted runners.
- Kept release gating on `cargo fmt`, `cargo clippy -D warnings`, and `cargo test` before building tagged artifacts.

## v0.1.1 - 2026-03-18

- Added `UPSTREAM_API_KEY` and `UPSTREAM_HEADERS` so the proxy can authenticate to upstreams and inject required protocol headers.
- Defaulted unspecified `stream` requests to non-streaming behavior to match OpenAI Chat Completions and Responses semantics.
- Corrected OpenAI Responses usage mapping and expanded streaming lifecycle conversion for content, reasoning, and function calls.
- Switched Google Gemini SSE upstream routing to `streamGenerateContent?alt=sse` and tightened related integration coverage.
- Hardened upstream format discovery by probing with auth and protocol headers instead of treating any non-`404` response as support.
- Added real-world documentation for running Codex CLI against Anthropic-compatible upstream services through the proxy.
- Release CI now gates on `cargo fmt`, `cargo clippy -D warnings`, and `cargo test` before building tagged artifacts.
