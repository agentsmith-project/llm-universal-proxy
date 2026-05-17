# Field Mapping Matrix

- Layer: capability-diff matrix
- Status: active
- Vendor snapshot/captured date: 2026-04-16
- Proxy posture updated date: 2026-04-26
- Scope: high-risk field mappings, intentional drops, and safe-degradation notes

Legend: `Preserved` means the proxy can keep portable intent and client-visible semantics. `Warned safe degradation` means the proxy may emit a warning while preserving the safe portable representation. `Fail-closed` means there is no safe cross-provider translation and the proxy rejects the request before contacting upstream. `Warn/drop opaque carrier` means the proxy may warn and remove an opaque provider-owned carrier only when visible portable context remains. `Native-only` means the field is preserved only through native same-wire handling when no body mutation or response normalization is required and the same protocol can preserve native semantics.

Provider columns name the official field family on that surface. `Mapping status` is where cross-provider portability is judged.

| Intent | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Mapping status | Notes |
| --- | --- | --- | --- | --- | --- |
| Core conversation input | `input` | `messages` | `messages` plus top-level `system` | Warned safe degradation | Same concept, different wire shape |
| Typed media input parts | `input_image`, `input_audio`, `input_file` | image, audio, and file content parts | image and document blocks | Warned safe degradation / Fail-closed | Translate only supported media in the effective surface and only source transports the target can represent. HTTP(S) URLs are distinct from provider-native or local URIs such as `gs://`, `s3://`, and `file://`; unsupported media or source forms, provider `file_id`, unknown typed parts, and conflicting MIME provenance fail closed. |
| System / developer instruction | `instructions` or high-priority input roles | `system` message | top-level `system` | Warned safe degradation | Hierarchy and placement differ |
| Function tool definitions | `tools` | `tools` | `tools` with `input_schema` | Preserved | Function-only portability |
| Hosted / server tool definitions | Rich Responses tool families | No official hosted/server tool family on Chat create | Server tools and MCP connector | Native-only / Fail-closed | Keep provider-native same-wire handling only unless an explicit safe mapping exists. |
| Tool choice: auto / none | Native strings | Native strings | Object form | Warned safe degradation | Intent can be preserved, schema cannot |
| Tool choice: required / any / forced tool | Native | Native | `any` or `tool` | Warned safe degradation | Forced-tool semantics are not identical |
| Parallel tool use | `parallel_tool_calls` | `parallel_tool_calls` | `disable_parallel_tool_use` | Warned safe degradation | Inversion on Anthropic side |
| Reasoning request policy | `reasoning` | Model-specific | `thinking` | Fail-closed | Same idea, different execution contract |
| Reasoning output | Typed reasoning items / summaries | Model-specific output fields | `thinking` blocks | Warned safe degradation | Preserve summaries and usage where possible |
| Reasoning opaque state | `reasoning.encrypted_content`, reasoning item `encrypted_content` | No stable equivalent | No stable equivalent | Native-only / Warn/drop opaque carrier / Fail-closed | Provider-native same-wire handling preserves the carrier only when no body mutation or response normalization is required and the same protocol can preserve native semantics. In maximum-compatible request translation, warn/drop opaque carrier fields only when visible summary or visible transcript/history remains; opaque-only reasoning state fails closed. Never synthesize. Response-side reasoning encrypted_content has a separate Anthropic carrier recovery path. |
| Prompt-cache control | `prompt_cache_key`, retention policy | `prompt_cache_key` on supported surfaces | `cache_control` | Native-only / Fail-closed | Not the same primitive |
| Cached-token usage | `cached_tokens` | `cached_tokens` | cache read/write token fields | Warned safe degradation | Accounting models differ |
| Follow-up response handle | `previous_response_id`, conversations | No stable equivalent | No stable equivalent | Native-only / Fail-closed | Requires provider-owned state |
| Compaction | `context_management`, `/responses/compact`, compaction items | No stable equivalent | beta `context_management` compaction | Native-only / Warn/drop opaque carrier / Fail-closed | Native state surfaces are preserved only through native same-wire handling when no body mutation or response normalization is required and the same protocol can preserve native semantics. In maximum-compatible request translation, request-side compaction input items may warn/drop opaque carrier fields only when each degraded item has explicit visible summary text, or when non-compaction visible transcript/history remains; opaque-only compaction still fails closed, and one summarized compaction item does not permit another opaque-only compaction item to be silently dropped. Native Responses same-wire handling preserves compaction items unchanged. |
| Stream failure / incomplete terminal | `response.failed`, `response.incomplete` | finish reason or HTTP error | stop reason plus HTTP error | Warned safe degradation | Normalize for downstream needs |
| Context-window overflow signal | explicit failure shape | error / finish reason | `model_context_window_exceeded` stop reason | Warned safe degradation | Semantics differ materially |
| Metadata | `metadata` | `metadata` on compatible implementations | `metadata` | Warned safe degradation | Safe only within compatible families |
| Storage / persistence | `store` plus stored response resources | Official `store` field for stored completion artifacts | No official request-side persistence flag | Native-only / Fail-closed | Storage semantics differ |
| Service tier | `service_tier` | Official `service_tier` request field | Official `service_tier` request field on current Messages surfaces | Native-only | Native same-wire handling only; tier semantics are vendor-specific and should be preserved only when no body mutation or response normalization is required. |

Google OpenAI-compatible Gemini is treated as an OpenAI Chat-compatible upstream
in this active matrix. Native Google/Gemini mappings are retained only as
retired historical baseline material, not as an active proxy capability.

## Use this matrix with

| If you are deciding... | Read |
| --- | --- |
| Whether a feature should warn, drop, or normalize | [`../audits/2026-04-16-spec-refresh.md`](../audits/2026-04-16-spec-refresh.md) |
| Whether a capability is broadly portable at all | [`provider-capability-matrix.md`](provider-capability-matrix.md) |
