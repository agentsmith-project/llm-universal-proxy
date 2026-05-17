# Field Mapping Matrix

- Layer: capability-diff matrix
- Status: active
- Vendor snapshot/captured date: 2026-04-16
- Proxy posture updated date: 2026-04-26
- Scope: high-risk field mappings, intentional omissions, and portability actions

Legend: `Portability action` describes what llmup does at the boundary. `Map portable semantics` keeps portable intent and client-visible semantics. `Warn and omit non-portable detail` emits a portability warning while preserving the safe portable representation. `Fail closed` rejects the request before contacting upstream. `Warn and omit opaque carrier` removes an opaque provider-owned carrier only when visible portable context remains. `Same-wire preserve only` preserves the field only through native same-wire handling when no body mutation or response normalization is required and the same protocol can preserve native semantics.

Provider columns name the official field family on that surface. `Portability action` records llmup behavior; it is not a product grade.

| Intent | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Portability action | Notes |
| --- | --- | --- | --- | --- | --- |
| Core conversation input | `input` | `messages` | `messages` plus top-level `system` | Map portable semantics | Same concept, different wire shape |
| Typed media input parts | `input_image`, `input_audio`, `input_file` | image, audio, and file content parts | image and document blocks | Map supported media / Fail closed | Translate only supported media in the effective surface and only source transports the target can represent. HTTP(S) URLs are distinct from provider-native or local URIs such as `gs://`, `s3://`, and `file://`; unsupported media or source forms, provider `file_id`, unknown typed parts, and conflicting MIME provenance fail closed. |
| System / developer instruction | `instructions` or high-priority input roles | `system` message | top-level `system` | Warn and omit non-portable detail | Hierarchy and placement differ |
| Function tool definitions | `tools` | `tools` | `tools` with `input_schema` | Map portable function contract | Function-only portability |
| Hosted / server tool definitions | Rich Responses tool families | No official hosted/server tool family on Chat create | Server tools and MCP connector | Same-wire preserve only / Fail closed | Keep provider-native same-wire handling only unless an explicit safe mapping exists. |
| Tool choice: auto / none | Native strings | Native strings | Object form | Warn and omit non-portable detail | Intent can be preserved, schema cannot |
| Tool choice: required / any / forced tool | Native | Native | `any` or `tool` | Warn and omit non-portable detail | Forced-tool semantics are not identical |
| Parallel tool use | `parallel_tool_calls` | `parallel_tool_calls` | `disable_parallel_tool_use` | Warn and omit non-portable detail | Inversion on Anthropic side |
| Reasoning request policy | `reasoning` | Model-specific | `thinking` | Fail closed | Same idea, different execution contract |
| Reasoning output | Typed reasoning items / summaries | Model-specific output fields | `thinking` blocks | Warn and omit non-portable detail | Preserve summaries and usage where possible |
| Reasoning opaque state | `reasoning.encrypted_content`, reasoning item `encrypted_content` | No stable equivalent | No stable equivalent | Same-wire preserve only / Warn and omit opaque carrier / Fail closed | Provider-native same-wire handling preserves the carrier only when no body mutation or response normalization is required and the same protocol can preserve native semantics. Under maximum safe compatibility, emit a portability warning and omit opaque carrier fields only when visible summary or visible transcript/history remains; opaque-only reasoning state fails closed. Never synthesize. Response-side reasoning encrypted_content has a separate Anthropic carrier recovery path. |
| Prompt-cache control | `prompt_cache_key`, `prompt_cache_retention` | `prompt_cache_key`, `prompt_cache_retention` on supported surfaces | `cache_control` | Same-wire preserve only / Documented explicit mapping / Warn and omit non-portable detail / Fail closed | Provider prompt-cache controls are not an `llmup` cache. Documented explicit mappings are OpenAI-family `extra_body.anthropic.cache_control` to Anthropic top-level `cache_control`, and Anthropic `extra_body.openai.prompt_cache_key` plus optional `prompt_cache_retention` to OpenAI-family top-level fields. Other native controls are not semantic equivalents and are not synthesized; wrong-target explicit extensions fail closed. |
| Cached-token usage | `cached_tokens` | `cached_tokens` | cache read/write token fields | Warn and omit non-portable detail | Accounting models differ |
| Follow-up response handle | `previous_response_id`, conversations | No stable equivalent | No stable equivalent | Same-wire preserve only / Fail closed | Requires provider-owned state |
| Compaction | `context_management`, `/responses/compact`, compaction items | No stable equivalent | beta `context_management` compaction | Same-wire preserve only / Warn and omit opaque carrier / Fail closed | Native state surfaces are preserved only through native same-wire handling when no body mutation or response normalization is required and the same protocol can preserve native semantics. Under maximum safe compatibility, request-side compaction input items may emit a portability warning and omit opaque carrier fields only when each affected item has explicit visible summary text, or when non-compaction visible transcript/history remains; opaque-only compaction still fails closed, and one summarized compaction item does not permit another opaque-only compaction item to be silently omitted. Native Responses same-wire handling preserves compaction items unchanged. |
| Stream failure / incomplete terminal | `response.failed`, `response.incomplete` | finish reason or HTTP error | stop reason plus HTTP error | Warn and omit non-portable detail | Normalize for downstream needs |
| Context-window overflow signal | explicit failure shape | error / finish reason | `model_context_window_exceeded` stop reason | Warn and omit non-portable detail | Semantics differ materially |
| Metadata | `metadata` | `metadata` on compatible implementations | `metadata` | Warn and omit non-portable detail | Safe only within compatible families |
| Storage / persistence | `store` plus stored response resources | Official `store` field for stored completion artifacts | No official request-side persistence flag | Same-wire preserve only / Fail closed | Storage semantics differ |
| Service tier | `service_tier` | Official `service_tier` request field | Official `service_tier` request field on current Messages surfaces | Same-wire preserve only | Native same-wire handling only; service-class semantics are vendor-specific and should be preserved only when no body mutation or response normalization is required. |

Google OpenAI-compatible Gemini is treated as an OpenAI Chat-compatible upstream
in this active matrix. Native Google/Gemini mappings are retained only as
retired historical baseline material, not as an active proxy capability.

## Use this matrix with

| If you are deciding... | Read |
| --- | --- |
| Whether a feature should warn, omit, or normalize | [`../audits/2026-04-16-spec-refresh.md`](../audits/2026-04-16-spec-refresh.md) |
| Whether a capability is broadly portable at all | [`provider-capability-matrix.md`](provider-capability-matrix.md) |
