# Protocol Spec Online Recheck - 2026-07-31

- Layer: versioned audit
- Status: latest online recheck; historical audit, not an active implementation plan
- Compared against: repo baselines captured under `snapshots/2026-04-16` and last rechecked `2026-05-16` ([2026-05-16-online-recheck.md](./2026-05-16-online-recheck.md))
- Recheck date: 2026-07-31
- Scope: full-surface recheck of official provider docs for OpenAI Responses, OpenAI Chat Completions, and Anthropic Messages, plus current model-name verification across providers. Native Gemini `generateContent` is retired and out of scope (Gemini access uses Google's OpenAI-compatible endpoint).
- Note: this recheck did not add immutable snapshot artifacts. The 2026-04-16 snapshot bucket remains the archived evidence set. Per the update rules, baseline files were edited only where the official surface materially changed; this audit records the full diff.

## Sources Rechecked

| Provider | Official docs |
| --- | --- |
| OpenAI | `https://developers.openai.com/api/reference/resources/responses/index.md`, `https://developers.openai.com/api/reference/resources/chat/index.md`, `https://developers.openai.com/api/docs/changelog`, `https://developers.openai.com/api/docs/deprecations`, `https://developers.openai.com/api/docs/guides/reasoning`, `https://developers.openai.com/api/docs/guides/prompt-caching`, `https://developers.openai.com/api/docs/guides/latest-model`, `https://developers.openai.com/api/docs/models`, `https://developers.openai.com/api/docs/guides/responses-multi-agent` |
| Anthropic | `https://platform.claude.com/docs/en/api/messages`, `https://platform.claude.com/docs/en/api/streaming`, `https://platform.claude.com/docs/en/build-with-claude/extended-thinking`, `https://platform.claude.com/docs/en/build-with-claude/thinking`, `https://platform.claude.com/docs/en/about-claude/models/overview`, `https://platform.claude.com/docs/en/api/beta-headers`, `https://platform.claude.com/docs/en/release-notes/api`, `https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool` (note: `docs.anthropic.com` / `docs.claude.com` now 301-redirect to `platform.claude.com/docs`) |
| Google Gemini (OpenAI-compat) | `https://ai.google.dev/gemini-api/docs/openai`, `https://ai.google.dev/gemini-api/docs/models` |

## Change Summary

### OpenAI Responses

| Area | Current official-doc signal (vs 2026-05-16) | Local documentation action |
| --- | --- | --- |
| Models | GPT-5.6 family released 2026-07-09 (`gpt-5.6-sol`/`gpt-5.6-terra`/`gpt-5.6-luna`; alias `gpt-5.6` → `gpt-5.6-sol`). `gpt-5.1-codex*`, `gpt-5-codex`, `gpt-5.2-codex`, `gpt-5*-chat-latest` shut down 2026-07-23. | Baseline model-anchor notes refreshed away from shut-down `gpt-5.1-codex-max`/`gpt-5-pro` to the GPT-5.6 family. |
| Reasoning | `reasoning.effort` adds `max` (set: none/minimal/low/medium/high/xhigh/max). New `reasoning.mode` (`standard`/`pro`) and `reasoning.context` (`auto`/`current_turn`/`all_turns`; GPT-5.6 supports and defaults to `all_turns`). Persisted reasoning (`encrypted_content`) is now returned **by default** in stateless mode; legacy `include:["reasoning.encrypted_content"]` accepted but not required. | Reasoning section updated: full effort ladder incl. `max`, `reasoning.mode`/`reasoning.context`, default-on encrypted reasoning. |
| Prompt caching (major rework) | New `prompt_cache_options:{mode:"implicit"/"explicit"}` + per-block `prompt_cache_breakpoint:{mode:"explicit"}`. Non-ZDR default retention flipped to `24h` (2026-05-29, was `in_memory`). `prompt_cache_retention` **deprecated for GPT-5.6+** (use `prompt_cache_options.ttl`, only `"30m"`). `prompt_cache_key` required for reliable matching on GPT-5.6+. Cache writes cost 1.25× uncached input. | Caching section rewritten around `prompt_cache_options`/`prompt_cache_breakpoint`; old `in_memory`/`in-memory` spelling note demoted to legacy. |
| Tools | New built-ins: `programmatic_tool_calling`, `shell`, `local_shell`, `tool_search`, `apply_patch`, `computer`. MCP enriched (`connector_id` for 8 connectors, `tunnel_id`/Secure MCP Tunnel, `require_approval`, `allowed_tools`, `defer_loading`, `allowed_callers`). New item fields `phase` (`commentary`/`final_answer`), `caller`, `namespace`, `additional_tools`. | Tool surface updated; MCP bullet expanded. |
| Request envelope | New `moderation` object (2026-06-04); `prompt_cache_options` top-level; web search gained `return_token_budget` and image results. | Added to operational controls. |
| Orchestration | Multi-agent (beta, GPT-5.6 only); Fast mode (2026-07-30) replaces Priority Processing (`priority` auto-routes to `fast`). | New "Background, async, and orchestration" note; multi-agent request field marked TBD. |
| Deprecations | DALL·E 2/3 removed (2026-05-12); Realtime API Beta removed (2026-05-12); `prompt_cache_retention` deprecated for GPT-5.6+. | Recorded. |
| Lifecycle (unchanged) | Endpoint set, stateful lifecycle (manual/`previous_response_id`/`conversation`), 30-day TTL, `store:false`, compaction, SSE termination semantics all verified unchanged. | No change. |

### OpenAI Chat Completions

| Area | Current official-doc signal (vs 2026-05-16) | Local documentation action |
| --- | --- | --- |
| Status | Endpoint remains mainline supported; no Chat Completions deprecation date (Assistants API shuts down 2026-08-26). | Confirmed; no change. |
| Reasoning | `reasoning_effort` adds `max`; GPT-5.6/5.5 default `medium`. Old `xhigh`-anchored model notes referenced shut-down models. | Effort enum + defaults updated; shut-down model anchors removed. |
| New fields | `moderation`, `safety_identifier` (replacing `user`), `prompt_cache_options`/`prompt_cache_breakpoint`, `verbosity` enum (`low`/`medium`/`high`, top-level). | Added to request controls. |
| Caching | `prompt_cache_retention` deprecated for GPT-5.6+; non-ZDR default `24h`; `prompt_cache_key` required on GPT-5.6+. | Caching section updated. |
| `service_tier` | Enum: `auto`/`default`/`flex`/`scale`/`priority`/`fast` (Fast mode replaces Priority Processing). | Enumerated. |
| Tools | `tool_choice` now also supports an "allowed tools" option (`mode:auto`/`required` + `tools[]`) for GPT-5.6 programmatic tool calling. | Noted. |
| Unchanged | Endpoint, core envelope, message roles, multimodal parts, `response_format`, streaming shape + `[DONE]`, usage accounting, no server-side conversation state. | No change. |

### Anthropic Messages

| Area | Current official-doc signal (vs 2026-05-16) | Local documentation action |
| --- | --- | --- |
| Models | New: `claude-opus-4-8` (2026-05-28), `claude-sonnet-5` (2026-06-30), `claude-fable-5`/`claude-mythos-5` (2026-06-09), `claude-opus-5` (2026-07-24). Retired: `claude-sonnet-4-20250514`, `claude-opus-4-20250514` (2026-06-15); `claude-opus-4-1` deprecated (retires 2026-08-05). Haiku 4.5 is the only current model still supporting manual extended thinking. | Model examples/references refreshed; `claude-sonnet-4-6` example updated. |
| Request fields | New: `output_config` (`effort` `low/medium/high/xhigh/max` + `format` `json_schema`; GA, primary thinking-depth control on 4.6+), `inference_geo`, `speed` (fast mode, Opus 5/4.8 only), `fallbacks` (server-side refusal fallback), `mcp_servers` (MCP connector), `diagnostics`. **`temperature`/`top_p`/`top_k` return 400 on Opus 4.7+ and Sonnet 5/Fable 5/Opus 5 when non-default.** `anthropic-version` still `2023-06-01`. | Optional-fields list updated; sampling-param constraint recorded. |
| Response | New `stop_details` (only on `stop_reason:"refusal"`; `category` enum). `usage` expanded (`output_tokens_details.thinking_tokens`, `cache_creation.ephemeral_1h/5m_input_tokens`, `inference_geo`, `service_tier`, `server_tool_use.web_fetch_requests`, `speed`). | Response contract + usage subfields updated. |
| Content blocks | Added: `container_upload` (input+output), `mid_conv_system`, `tool_reference`, `compaction`, `fallback`, `web_fetch_tool_result`, `code_execution_tool_result`, `bash_code_execution_tool_result`, `text_editor_code_execution_tool_result`, `tool_search_tool_result`, `mcp_tool_use`, `mcp_tool_result`. | Block taxonomy updated. |
| Streaming | Event set unchanged. New: `fallback` block arrives as `content_block_start`/`content_block_stop` with no delta between. Error recovery diverges: 4.6+ forbids assistant-prefill continuation (use a user message). | Streaming section updated. |
| Extended thinking (rewrite) | Adaptive thinking default on 4.6+. `thinking.type:"enabled"`+`budget_tokens` removed (400) on 4.7+. `thinking.display` (`omitted`/`summarized`). Effort lives in `output_config.effort`. Raw CoT never returned on Fable 5/Mythos 5/Opus 5. `thinking:{type:"disabled"}` rules vary (400 on Fable 5; ≤`high` only on Opus 5). Task Budgets beta. | Extended-thinking section rewritten. |
| Context mgmt | New compaction feature (`compact_20260112`, beta `compact-2026-01-12`; `compaction` block echoed back). Mid-conversation system messages (`role:"system"` in `messages[]`, no beta; Opus 4.8/5, Fable 5, Mythos 5). | Added. |
| Tools | Web search: `web_search_20250305`/`_20260209`/`_20260318` (newest; `response_inclusion`, `allowed_callers`). Code execution `code_execution_20260120`/`_20260521`. MCP connector, advisor tool, tool-search, memory tool, mid-conversation tool changes (`mid-conversation-tool-changes-2026-07-01`). Fine-grained tool streaming now GA. | Tools section updated. |
| Stop reasons | 7 values unchanged; `refusal` is now classifier-driven on Fable 5/Opus 5/Sonnet 5. | Noted. |
| Beta headers | Concrete in-market list recorded (see baseline). `agent-memory-2026-07-22` replaces `managed-agents-2026-04-01` on memory-store endpoints (sending both → 400). | Beta-headers note enumerated. |
| Sources | `docs.anthropic.com`/`docs.claude.com` → `platform.claude.com/docs`. | Source URLs updated. |

### Cross-provider model names (for docs/examples refresh)

| Provider | Current (2026-07-31) | Notes |
| --- | --- | --- |
| OpenAI | `gpt-5.6-sol`/`gpt-5.6-terra`/`gpt-5.6-luna` (flagship); `gpt-5.4` still active | `gpt-5.1-codex*` shut down |
| Anthropic | `claude-fable-5`, `claude-opus-5`, `claude-sonnet-5`, `claude-haiku-4-5` | `claude-sonnet-5` (used in proxy docs) is current |
| Google Gemini (OpenAI-compat) | `gemini-3.6-flash` (current stable), `gemini-3.1-pro-preview` | **`gemini-2.0-flash` is shut down** — referenced in proxy examples, updated |
| DeepSeek | `deepseek-v4-flash` (current), `deepseek-v4-pro` | `deepseek-v4-flash` (used in proxy docs) is current |
| GLM (Zhipu) | `GLM-5.2` (flagship) | `glm-4.5/4.6`-as-flagship outdated |
| MiniMax | `MiniMax-M3` | — |
| Kimi (Moonshot) | `kimi-k3` (flagship) | `kimi-k2` outdated |
| Mistral | `mistral-large-2512`, `mistral-medium-2604` | — |

## Compatibility Impact

No cross-provider portability status changed. The recheck reinforces the existing proxy posture and adds a few provider constraints to track:

- Most new fields (OpenAI `moderation`/`prompt_cache_options`/Multi-agent; Anthropic `output_config`/`fallbacks`/compaction/new tools) are provider-native extensions: preserved on same-wire routes, warned-and-omitted or failed-closed on cross-protocol routes per existing policy. None is portable across protocol schemas.
- **Reasoning `max` is a request setting, not a model name** — consistent with the existing "reasoning effort is not part of a model name" stance; `xhigh`/`max` stay client-controlled.
- Prompt-cache rework is provider-native; the proxy preserves native cache controls on same-wire routes and does not translate cache handles/TTLs across providers (unchanged). `prompt_cache_retention` deprecation on GPT-5.6+ is a provider-native forwarding detail, not a new proxy product behavior.
- **Anthropic sampling-param constraint (new):** translating OpenAI→Anthropic requests that carry non-default `temperature`/`top_p`/`top_k` will 400 on Sonnet 5/Opus 5/Fable 5/Opus 4.7+ upstreams. See follow-up candidate below.
- New OpenAI/Anthropic tools (programmatic tool calling, MCP connector, code execution, compaction, fallback, tool search, memory) are non-portable and follow the existing tool-portability warn/omit/reject rules.
- Anthropic `refusal` becoming classifier-driven (HTTP 200 + `stop_details`) is a stop-reason/finish-reason mapping detail, already within the existing stop-reason translation surface.

## Implementation Follow-Up Candidates

| Candidate | Why it may help | Guardrail |
| --- | --- | --- |
| Anthropic sampling-param guard | Translated requests to Anthropic Sonnet 5/Opus 5/Fable 5/Opus 4.7+ upstreams carrying non-default `temperature`/`top_p`/`top_k` now 400. Strip/withhold them (or warn) for those models. | Model-gated; preserve caller-provided values for models that still accept them. |
| OpenAI explicit prompt-cache policy | Opt-in to set `prompt_cache_options`/`prompt_cache_breakpoint` for stable prefixes on GPT-5.6+ upstreams. | Opt-in per upstream/model; preserve caller-provided cache fields. |
| Reasoning pass-through | Same-wire OpenAI routes pass through `reasoning.effort` incl. `max`, `reasoning.mode`, `reasoning.context`; same-wire Anthropic routes pass through `output_config.effort`/`thinking.display`. | Preserve caller values; do not invent across protocols. |
| `stop_details`/refusal mapping | Map Anthropic `stop_reason:"refusal"` + `stop_details` on cross-protocol routes (warn-and-omit the non-portable category detail). | Keep it a mapping/warning, not a behavior switch. |

## Open Verification Items

- **OpenAI:** re-confirm the "previous_response_id and conversation cannot be used together" rule against the full create-method reference (could not be re-verified from the guide this cycle; no evidence of a change). Confirm the exact multi-agent request field name against the untruncated create reference.
- **Anthropic:** confirm whether `context_management` remains a top-level body field on the Messages reference page (page extraction omitted it this cycle; it is still documented under context-editing with beta `context-management-2025-06-27` — treated as present-but-beta-scoped).
- **Non-OpenAI/Anthropic model names** (GLM, MiniMax, Kimi, Mistral, Gemini) were verified against single official sources this cycle; re-confirm before any code/config hard-coding.
