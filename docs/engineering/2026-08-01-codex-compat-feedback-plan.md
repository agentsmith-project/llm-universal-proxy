# Codex CLI (0.146.0) Compatibility Feedback — Plan

- Date: 2026-08-01
- Owner: lizhijian
- Input: Customer feedback from a Codex CLI 0.146.0 user testing against the proxy; the repo's own [`pre-ga-openai-responses-namespace-tool-bridge-plan.md`](./pre-ga-openai-responses-namespace-tool-bridge-plan.md) and [`pre-ga-agent-launcher-model-capability-metadata-plan.md`](./pre-ga-agent-launcher-model-capability-metadata-plan.md); PRD §2.4–2.6 compatibility contract.
- Status: handoff-ready plan. Deliberately short. Implementation is in-place changes + tests; no separate evidence/gate/audit artifacts.

## Principles (binding for this work)

- **KISS / DRY / YAGNI.** Reuse the existing warn-and-omit pipeline, the existing model-profile/catalog builder, and the existing alias config. Add a new mechanism only when the cheap fix is provably insufficient.
- **Structural fixes, not workarounds.** Align the implementation with the PRD §2.6 contract rather than patching symptoms. A code bug is fixed in the translation layer; a config gap is fixed in config; a deployment gap is fixed in deployment — each in its own layer.
- **In-place fix + test.** Each code change lands with its test in the same change. No parallel redesign, no second translation path.
- **Classify the fix, then size it.** Code bug → TDD. Compatibility improvement → detect + reuse. Config gap → guidance. Deployment gap → recipe. Nice-to-have → only if cheap.
- **No new governance layers.** No evidence/gate/report/audit docs for this work. The protocol-baselines audit cadence is the only doc ritual.

## 1. Goal & scope

Bring the proxy into line with its own compatibility contract for the Codex CLI scenarios in the feedback, by fixing the one real code bug (P0), confirming and fixing the suspected code bug (P1), and delivering the one compatibility improvement (P2). Deployment-side concerns are out of scope (see §4).

This plan is governed by the new CONSTITUTION Core Principle #7 (Maximum Client Compatibility) and Invariant #9 (maximum compatibility is the default posture). The proxy proactively absorbs client-specific needs rather than failing closed.

Scope classes:

- **Proxy code fix (TDD):** P0 namespace-tool warn-and-omit; P1 non-streaming Responses `model` field **and** `openai-model` response header.
- **Compatibility improvement (code):** P2 Codex-aware `/v1/models` extended catalog (`ModelsResponse` shape).

The pivot-through-Chat-Completions architecture stays. This plan does not add a Responses-native upstream path.

## 2. Issue-by-issue analysis

| # | Issue | Severity | Class | Root cause | Fix direction |
| --- | --- | --- | --- | --- | --- |
| 1 | Namespace tools (`multi_agent_v1`, `mcp__*`) cause whole-request rejection | Critical | **Code bug** | The tool-definition classifier tags `type:"namespace"` as a reject-triggering variant, unlike other non-function tools (`web_search`/`computer`/…). This violates PRD §2.4 row "Built-in / non-function tools" (`Won't` → warn-and-omit) and §2.6. | Route namespace tool **definitions** through the existing non-function warn-and-omit pipeline; drop them from the translated `tools` array with an `x-llmup-portability-warning`. |
| 2 | `/models` format incompatible with Codex 0.146.0 (degraded capability detection) | High | **Compat improvement** | The HTTP models endpoint returns standard OpenAI shape; Codex expects `ModelsResponse { models: Vec<ModelInfo> }` (see §2.1). No User-Agent sniffing today. | When the User-Agent contains "codex", return the `{models:[...]}` shape with `ModelInfo`-shaped entries from the existing `AgentModelCatalog` builder. |
| 4 | Non-streaming Responses `model: null`; Codex reads model from header, not body | Medium | **Code bug (confirmed)** | `openai_response_to_responses` builds the synthesized Responses object without a `model` key. Separately, Codex reads the model from the `openai-model` response **header**, which the proxy does not set today (see §2.1). | Add the `model` key to the body (parity with streaming) **and** set the `openai-model` response header to the client-facing alias on both streaming and non-streaming Responses. |

Issues 3 (alias name), 5 (HTTPS), and 6 (`/v1/*` path 404) from the original feedback are deployment-side configuration, not proxy code — see §4.

### 2.1 Codex source findings (grounding)

Findings from `reference/codex/codex-rs/` (in-tree). These pin the exact wire contracts P0–P2 must satisfy.

**Models endpoint (Issue 2).**

- Codex fetches `{base_url}/models?client_version=...` and expects `ModelsResponse { models: Vec<ModelInfo> }` — NOT standard OpenAI `{object:"list", data:[...]}`. (Source: `codex-rs/protocol/src/openai_models.rs:619-623`.)
- `ModelInfo` has many required fields: `slug`, `display_name`, `supported_reasoning_levels`, `shell_type`, `visibility`, `supported_in_api`, `priority`, `base_instructions`, `truncation_policy`. (Source: `:368-452`.)
- When the response fails to deserialize, Codex synthesizes a fallback and logs "Unknown model ... will use fallback metadata" — so it degrades, doesn't crash. (Source: `models-manager/src/model_info.rs:127-170`.)

**Namespace tools (Issue 1).**

- Wire format confirmed: `{type:"namespace", name:"multi_agent_v1", description:"...", tools:[{type:"function", name:"spawn_agent", ...}]}` in the top-level `tools` array. (Source: `tools/src/tool_spec.rs:17-53`, `tools/src/responses_api.rs:51-67`.)
- `multi_agent_v1` is at `core/src/tools/handlers/multi_agents_spec.rs:14`; MCP tools are namespaced under `mcp__<server>__`. (Source: `tools/src/responses_api.rs:107-125`.)
- Codex sends namespace tools when `provider.capabilities().namespace_tools` is true — inferred from `wire_api: "responses"`. (Source: `core/src/tools/spec_plan.rs:321,394-395`.)

**`model` field (Issue 4).**

- Codex does NOT read the `model` field from the Responses JSON body — it gets the model from the `openai-model` HTTP response header. (Source: `codex-api/src/sse/responses.rs:30,46-50,112-120`.)
- The body `model` field is harmless for Codex (no failure); fixing it is for other clients' benefit (logs, cost tracking, routing validation).
- **Implication:** Set the `openai-model` HTTP response header on Responses API responses to the client-facing model alias — this is what Codex actually reads. Captured as part of P1.

## 3. Prioritized work items

### P0 — Namespace tools: warn-and-omit (TDD bug fix)

**Contract.** PRD §2.4 ("Built-in / non-function tools → `Won't` → warn-and-omit unless a documented bridge preserves visible tool identity") and §2.6 (emit `x-llmup-portability-warning` when omitting non-portable detail). Today namespace tools fail-closed, which is stricter than every other non-function tool and contradicts the contract.

**Reject site (assessment).** `src/translate/internal/assessment.rs:2458-2467` — `assess_request_translation_with_dialect` calls `responses_nonportable_tool_definition_message` and `assessment.reject(message)` first; the `else if responses_has_warning_only_nonportable_tool_definitions(body)` branch is the existing warn-and-omit path we want namespace to use.

**Why namespace alone is rejected.** `responses_nonportable_tool_definition_message` (`assessment.rs:767-780`) returns a reject message for any tool the classifier yields as `Ok(Some(NormalizedOpenAiFamilyToolDef::Namespace(...)))`. The classifier `normalized_responses_tool_definition` (`src/translate/internal/tools.rs:1229-1285`, namespace arm at `1270-1282`) is the only non-function type that yields a non-`None` variant; all other non-function types (`web_search`, `computer`, `code_interpreter`, `file_search`, unknown) fall to the `_ => Ok(None)` catch-all at `1283`, which the warn-only detector (`responses_has_warning_only_nonportable_tool_definitions`, `assessment.rs:782-794`) treats as warn-and-omit.

**Fix (preferred — DRY, one change).** In `normalized_responses_tool_definition`, make `type:"namespace"` tool **definitions** classify as the warn-and-omit non-function category — i.e., do not yield the reject-triggering `Namespace` variant for definitions (fall through to the `Ok(None)` non-portable path that `web_search`/`computer` already use). Because the emit loop `normalized_responses_tool_definitions_from_request` (`tools.rs:1334-1348`) already silently skips `Ok(None)` entries, the tool is automatically omitted from the translated `tools` array, and the assessment's existing warn-and-omit branch emits the portability warning and the `x-llmup-portability-warning` header (`src/server/errors.rs:303-315`). No new mechanism.

**Alternative (preserve the `Namespace` variant for the future bridge).** Keep classification as-is but (a) make `responses_nonportable_tool_definition_message` return `None` for `Namespace`, (b) extend `responses_has_warning_only_nonportable_tool_definitions` to also warn for `Namespace`, and (c) make the emit arm `normalized_tool_definition_to_openai` (`tools.rs:1350-1390`, currently hard-`Err` for `Namespace`) omit instead of error. This is more sites for the same behavior; choose only if the team wants to keep the `Namespace` classification seam live for the deferred reversible bridge.

**Dead code.** With the preferred fix, `NormalizedOpenAiFamilyToolDef::Namespace` / `NormalizedOpenAiFamilyNamespaceTool` and the defensive `Namespace` arm in `normalized_tool_definition_to_openai` may become unreachable from the definition path. Leave them as a guarded seam for the deferred bridge, or remove them (YAGNI) and re-introduce when the bridge lands. Confirm against the separate input-item path (see scope note) before removing.

**Scope note — namespaced input items.** The user-reported failure is tool **definitions** (the `tools` array). Namespaced tool-call **input items** in history are governed by a separate function, `responses_nonportable_input_item_message` (`assessment.rs:976-1019`), covered by test `translate_request_responses_to_non_responses_rejects_namespaced_tool_calls` (`src/translate/internal/tests/mod.rs:1919-1942`). With multi-agent disabled, no such history is sent, so P0's primary fix is definitions only. During implementation, verify whether namespaced input items also need warn-and-omit and record the finding; do not expand P0 silently.

**TDD — see §5.**

### P1 — Non-streaming Responses `model` field + `openai-model` header (TDD bug fix, confirmed)

Codex reads the model from the **`openai-model` HTTP response header**, not the body (`codex-api/src/sse/responses.rs:30,46-50,112-120`, see §2.1). So this work item has two parts: the body field (parity, for non-Codex clients) and the header (what Codex actually reads).

**Part A — body `model` field (confirmed bug).** `src/translate/internal/openai_responses.rs`, function `openai_response_to_responses` (line `556`), builds the result object at lines `682-690` with **no `model` key**. Call chain: `translate_response_with_context` (`internal.rs:851`) → `openai_response_to_client` (`internal.rs:868/880`) → production caller `src/server/proxy.rs:2017`.

**Parity references (both correct).** Reverse direction `responses_response_to_openai_impl` at `openai_responses.rs:541` sets `"model": body.get("model").cloned().unwrap_or(Null)`. Streaming sets `state.model` from the upstream chunk's `model` (`src/streaming/openai_sink.rs:1459-1470`; field at `src/streaming/state.rs:150`) and emits it at the `created` (`1489`) and terminal (`831`) events. The streaming comment at `openai_sink.rs:1460-1461` claiming the non-streaming path already surfaces `model` is the stale claim this fix makes true.

**Fix A.** Add one key to the `json!` block at `openai_responses.rs:682-690`:

```rust
"model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
```

This surfaces the **upstream-returned model**, matching streaming and the reverse path. It eliminates the `null` for non-Codex clients (logs, cost tracking, routing validation). For Codex this body field is harmless either way — Codex ignores it.

**Part B — `openai-model` response header (what Codex reads).** Set the `openai-model` HTTP response header to the **client-facing alias** (e.g. `ds-flash`) on Responses API responses, both streaming and non-streaming. The alias is already resolved in the request path (`requested_model`/`resolved_model`, `src/server/proxy.rs:885`, `980`); thread it to the non-streaming response builder and the streaming SSE sink and set the header there. This is the lightweight alias-surfacing path — it satisfies Codex without changing the body contract, and replaces the earlier "thread the alias into the body across both paths" follow-on.

**PF-2 is unrelated.** The `PF-2` markers in the codebase (`src/streaming/openai_sink.rs:633`, `src/streaming/tests/responses_sink/terminal.rs:387`) are about the encrypted reasoning carrier, not the `model` field. The streaming `model`-field test has no PF-2 tag.

**TDD — see §5.**

### P2 — Codex-aware `/v1/models` extended catalog (compat improvement)

**Target schema (pinned by §2.1).** Codex fetches `{base_url}/models?client_version=...` and expects `ModelsResponse { models: Vec<ModelInfo> }` — NOT standard OpenAI `{object:"list", data:[...]}`. `ModelInfo` carries many required fields (`slug`, `display_name`, `supported_reasoning_levels`, `shell_type`, `visibility`, `supported_in_api`, `priority`, `base_instructions`, `truncation_policy`). On deserialization failure Codex degrades to fallback metadata (logs "Unknown model ... will use fallback metadata") rather than crashing.

**Gap.** The HTTP models endpoint returns standard OpenAI shape: `openai_model_list` (`src/server/models.rs:289-297`) → `{object:"list", data:[...]}`; each model via `openai_model_value` (`304-313`) with `llmup:{upstream_name, upstream_model, limits, surface}` metadata. There is **no User-Agent / client sniffing** today (`src/detect.rs` is path/body format detection only; `src/server/headers.rs:224-238` does not forward `user-agent`; model handlers pass `&HeaderMap::new()` to the redactor at `models.rs:207`).

**Reuse, do not duplicate.** Build `ModelInfo`-shaped entries from the existing `AgentModelCatalog` builder in `src/user_tools/agent_model_profile.rs` (`codex_catalog_entry` `128-173`, `default_codex_catalog_entry` `262-315`, `write_codex_model_catalog_for_profiles` `219-248`, from `AgentModelCatalog::from_config`) — the same builder the managed `llmup-codex` launcher writes as a file via `-c model_catalog_json`. Merge in the per-model `llmup.surface` metadata already produced for the standard endpoint so HTTP callers get the same richness file-side callers get.

**Why HTTP matters here.** Managed launches inject the catalog via file, so they don't need the HTTP endpoint. The HTTP endpoint matters for users who point Codex at the proxy **manually** (the reporter's scenario), where Codex 0.146.0 otherwise falls back to "defaulting to fallback metadata".

**Fix direction.** When the User-Agent contains "codex", return `{models:[...]}` with `ModelInfo`-shaped entries from the existing `AgentModelCatalog` builder; non-Codex clients keep today's standard OpenAI shape exactly (additive, no regression). Wire the UA in by threading `HeaderMap` into the OpenAI models handlers (`models.rs:22-114`; the `HeaderMap` import is already present at line `6`) following the pattern at `src/server/body_limits.rs:47-49` / `src/server/admin.rs:155`, or via a small UA middleware alongside `data_auth::require_data_access` (`src/server/mod.rs:423-426`).

**TDD — see §5.**

> **Out of scope (deployment-side).** Model aliasing, HTTPS/TLS, and API path discoverability are deployment-side configuration choices, not proxy code improvements (detailed in §4).

## 4. Non-goals

- **Full Responses-native upstream support.** The pivot-through-Chat-Completions architecture stays. No new wire protocol or response translation path.
- **Reversible namespace-tool mapping bridge** (user suggestion #2). Defer as YAGNI until warn-and-omit (P0) proves insufficient in the field. The existing `pre-ga-openai-responses-namespace-tool-bridge-plan.md` remains the design record for that future opt-in, whitelist-gated, unary+streaming+history+tool_choice-complete bridge. P0 deliberately takes the contract-compliant warn-and-omit path instead.
- **Alias in the Responses body `model` field.** P1 Part A surfaces the upstream model in the body (parity with streaming). Surfacing the client-facing alias *in the body* across both streaming and non-streaming remains out of scope; the alias reaches Codex via the `openai-model` header (P1 Part B) instead.
- **Deployment-side configuration (out of scope).** Model aliasing, HTTPS/TLS, and API path discoverability are deployment-side configuration choices, not proxy code improvements. They are addressed in deployment documentation, not this plan.
- **New governance / evidence / audit artifacts** for this work.

## 5. TDD sequencing

### P0 — namespace warn-and-omit (Red → Green → Refactor)

1. **Red.** In `src/translate/internal/tests/mod.rs`, flip `translate_request_responses_to_non_responses_rejects_namespace_tool_groups` (lines `1891-1917`) from `expect_err(… contains "namespace")` to asserting success **with** a portability warning that mentions the dropped non-function tool. Add a sibling case for `multi_agent_v1` as the namespace `name` (and `mcp__*` if a representative value is known). Both must fail red.
2. **Red (integration).** In `tests/integration_test.rs`, mirror `responses_translated_portability_warnings_emit_headers` (lines `7848-7882`, which today sends `{"type":"web_search"}`) with a `type:"namespace"` tool: assert `status().is_success()` and an `x-llmup-portability-warning` header containing "non-function Responses tools", and assert the translated upstream `tools` array does **not** contain the namespace tool. Must fail red.
3. **Green.** Apply the preferred fix: route namespace tool definitions through the warn-and-omit non-function category in `normalized_responses_tool_definition` (`tools.rs:1270-1282`). Both unit and integration tests go green.
4. **Refactor.** Handle resulting dead code per the P0 dead-code note (keep as guarded seam or remove). Leave the input-item reject path (`assessment.rs:976-1019`) and its test (`tests/mod.rs:1919-1942`) untouched unless the scope-note check decides otherwise.
5. **Regression sweep.** `cargo test` (translation + server + streaming suites). The `web_search`/`computer` warn-and-omit behavior must be unchanged.

### P1 — non-streaming `model` field + `openai-model` header (Red → Green)

**Part A — body field.**

1. **Red.** In `src/translate/internal/tests/mod.rs`, adjacent to `translate_response_openai_to_responses_maps_usage_fields` (line `10984`), add `translate_response_openai_to_responses_propagates_model`: feed a Chat completion body with `"model":"gpt-4o-2024"` and assert `out["model"] == "gpt-4o-2024"`. Must fail red. (Also scan sibling Chat→Responses tests at lines `11014`/`11049`/`11155`/`11217`/`11527`/`11574`/`11621`/`11656`/`11705` for the same gap.)
2. **Green.** Add `"model": body.get("model").cloned().unwrap_or(serde_json::Value::Null)` to the `json!` block at `src/translate/internal/openai_responses.rs:682-690`. Test goes green.
3. **Consistency check.** Compare against the streaming reference test `openai_chunk_to_responses_sse_propagates_model_to_created_and_terminal` (`src/streaming/tests/responses_sink/lifecycle.rs:66`) — non-streaming must now surface the same upstream-model value streaming does.

**Part B — `openai-model` header (alias).**

1. **Red.** In `tests/integration_test.rs`, add Responses cases (streaming and non-streaming) asserting the response carries header `openai-model: <client-facing alias>` (e.g. `ds-flash`) resolved from the configured alias, not the upstream model id. Must fail red.
2. **Green.** Thread the resolved alias (`requested_model`/`resolved_model`, `src/server/proxy.rs:885`, `980`) into the non-streaming response builder and the streaming SSE sink, and set `openai-model` in both. Tests go green.

### P2 — Codex-aware `/v1/models` catalog (Red → Green)

1. **Red.** In `src/server/tests/models.rs`, add a test sending `User-Agent: codex/0.146.0` and asserting the response is `{"models":[...]}` (not `{object:"list",data:[...]}`) with `ModelInfo`-shaped entries carrying the required fields (`slug`, `display_name`, `supported_reasoning_levels`, `shell_type`, `visibility`, `supported_in_api`, `priority`, `base_instructions`, `truncation_policy`) built from `AgentModelCatalog` + `llmup.surface`. Add a paired test asserting a non-Codex UA still returns today's standard OpenAI shape verbatim (no regression).
2. **Green.** When the UA contains "codex", build `{models:[...]}` from the existing `AgentModelCatalog::from_config` builder (`agent_model_profile.rs`) merged with `llmup.surface` metadata; otherwise keep the standard shape. No second catalog implementation.

## 6. Verification criteria (the user's acceptance criteria)

- **Issue 1:** A Responses request carrying `multi_agent_v1` and/or `mcp__*` namespace tools **succeeds** (HTTP 2xx), the translated upstream request omits those tools, and the response carries an `x-llmup-portability-warning` header. The user can leave Codex multi-agent/MCP enabled without the proxy rejecting the request.
- **Issue 2:** With a Codex User-Agent, `/v1/models` returns the `ModelsResponse { models: [ModelInfo] }` shape Codex 0.146.0 expects (with the required `ModelInfo` fields); Codex no longer logs "defaulting to fallback metadata". Non-Codex clients see the standard OpenAI shape unchanged.
- **Issue 4:** A non-streaming Responses request returns a non-null body `model` field (parity with streaming) **and** both streaming and non-streaming Responses responses carry an `openai-model` header set to the client-facing alias (e.g. `ds-flash`), which is what Codex reads.
