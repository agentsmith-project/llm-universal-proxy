# Configuration Guide

Ordinary user path: `install.sh` -> start the proxy -> `llmup codex-setup` -> `codex exec --profile llmup`.

Use that path when you want `codex-setup` to wire Codex to the proxy without editing Codex config by hand. This page is the advanced static YAML and server reference. It is for operators and developers who run `llm-universal-proxy --config` directly, generate checked-in server config, or need the full field-level config contract.

For the `codex-setup` user flow, see [Client Setup Guide](./clients.md). If you need to update config without restarting the process, see [Admin and Dynamic Config](./admin-dynamic-config.md).

Reasoning effort such as `xhigh` stays on the client request; it is not part
of the alias or upstream model name.

## Data Plane Security

Client-facing provider/model/resource routes use a required global auth mode
that is separate from the admin token. The mode is process-wide: one running
proxy process uses one `data_auth` state for all namespaces and
all provider/model/resource routes. It is not a per-upstream setting. If you need
a mixed deployment where some clients use a local proxy key and other clients
pass provider keys directly, run separate proxy instances.

The preferred static configuration is the top-level `data_auth` object:

- `mode: proxy_key`: clients send the proxy key as their normal SDK API key or
  as `Authorization: Bearer <proxy-key>`. The proxy uses each selected
  upstream's configured provider credential source for the real provider call.
- `mode: client_provider_key`: clients send the real provider key as their
  normal SDK API key or bearer token. The proxy forwards the client credential
  upstream.

In `proxy_key` mode, `data_auth.proxy_key` is required and must contain exactly
one secret source: `proxy_key.inline` or `proxy_key.env`. `proxy_key.inline`
stores the proxy key in the loaded config and is mainly useful for local tests
or tightly controlled generated config. `proxy_key.env` names an environment
variable that must resolve to the proxy key at startup or when an admin update is
applied.

If `data_auth` is omitted, the proxy keeps the backward-compatible
environment fallback: `LLM_UNIVERSAL_PROXY_AUTH_MODE` selects `proxy_key` or
`client_provider_key`, and `LLM_UNIVERSAL_PROXY_KEY` is required when that
fallback selects `proxy_key`. New deployments can still use the environment
fallback, but static `data_auth` is clearer for checked-in or
controller-rendered configuration.

Provider/model/resource routes reject missing client keys in both modes.
`/health` remains unauthenticated. Admin API routes use
`LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` and `Authorization: Bearer <admin-token>`.

Upstream provider credentials are configured per upstream and are used only when
the global mode is `proxy_key`. Supported provider credential sources are:

- `provider_key: { inline: "..." }`: inline provider key value.
- `provider_key: { env: "ENV" }`: structured environment variable source.

The pre-GA `provider_key_env: ENV` field has been removed. Static YAML and
Admin runtime payloads that still include it are rejected with a migration hint;
use `provider_key: { env: ENV }` instead.

`provider_key.inline` and `provider_key.env` are mutually exclusive on one
upstream. Inline and env source values must be non-empty. In `proxy_key` mode,
every upstream that can receive traffic must have one provider credential source,
and env sources must resolve in the proxy process environment.
Admin read views never return inline secret values.

In `client_provider_key` mode, provider credential sources are normally omitted.
`provider_key.inline` is rejected because it would embed a server-held provider
key that the mode will never use. `provider_key.env` is accepted but is not used
for request auth in this mode.

### Static YAML Auth Examples

In `proxy_key` mode, the proxy owns provider credentials. Clients only see the
local proxy key.

Set process environment before starting `llmup`:

```bash
export LLM_UNIVERSAL_PROXY_KEY="local-proxy-key"
export OPENAI_COMPATIBLE_API_KEY="real-openai-compatible-provider-key"
export ANTHROPIC_COMPATIBLE_API_KEY="real-anthropic-compatible-provider-key"
```

Static YAML:

```yaml
listen: 127.0.0.1:8080
upstream_timeout_secs: 120

data_auth:
  mode: proxy_key
  proxy_key:
    # The value of this env var is the client-facing proxy key.
    env: LLM_UNIVERSAL_PROXY_KEY

upstreams:
  PROXY-KEY-OPENAI-COMPATIBLE:
    # Provider API root; include the provider's version segment.
    api_root: https://openai-compatible.example/v1
    format: openai-chat-completions
    # Structured env source for this upstream's provider key.
    provider_key:
      env: OPENAI_COMPATIBLE_API_KEY

  PROXY-KEY-ANTHROPIC-COMPATIBLE:
    api_root: https://anthropic-compatible.example/v1
    format: anthropic-messages
    provider_key:
      env: ANTHROPIC_COMPATIBLE_API_KEY

model_aliases:
  coding-openai: "PROXY-KEY-OPENAI-COMPATIBLE:provider-openai-model"
  coding-anthropic: "PROXY-KEY-ANTHROPIC-COMPATIBLE:provider-anthropic-model"
```

Inline provider keys are accepted but should be reserved for generated local
fixtures or similarly controlled environments. Use an obvious fake value in
examples:

```yaml
listen: 127.0.0.1:8080

data_auth:
  mode: proxy_key
  proxy_key:
    inline: "DEMO_PROXY_KEY_DO_NOT_USE"

upstreams:
  INLINE-DEMO:
    api_root: https://openai-compatible.example/v1
    format: openai-chat-completions
    provider_key:
      inline: "DEMO_PROVIDER_KEY_DO_NOT_USE"

model_aliases:
  inline-demo: "INLINE-DEMO:provider-model-id"
```

In `client_provider_key` mode, clients send the real provider key through their normal SDK API key or bearer-token path. The proxy does not need server-side provider key env vars for these upstream calls, so static YAML normally leaves `provider_key` out.

Static YAML:

```yaml
listen: 127.0.0.1:8080
upstream_timeout_secs: 120

data_auth:
  mode: client_provider_key

upstreams:
  CLIENT-KEY-OPENAI-COMPATIBLE:
    api_root: https://openai-compatible.example/v1
    format: openai-chat-completions
    # provider_key is normally omitted.

  CLIENT-KEY-ANTHROPIC-COMPATIBLE:
    api_root: https://anthropic-compatible.example/v1
    format: anthropic-messages
    # The client key is forwarded upstream.

model_aliases:
  bring-your-openai-key: "CLIENT-KEY-OPENAI-COMPATIBLE:provider-openai-model"
  bring-your-anthropic-key: "CLIENT-KEY-ANTHROPIC-COMPATIBLE:provider-anthropic-model"
```

CORS is off by default. To allow browser callers, set `LLM_UNIVERSAL_PROXY_CORS_ALLOWED_ORIGINS` to a comma-separated list of exact origins, for example `https://app.example,https://console.example`. Wildcard origins are not accepted, and CORS is not an auth mechanism.

## The Main Sections

### `listen`

The address the proxy binds to, for example `127.0.0.1:8080`.

### `upstream_timeout_secs`

The request timeout for upstream calls.

### `resource_limits`

Resource boundaries for client request bodies, upstream response bodies, and streaming SSE translation. All values must be greater than zero.

- `max_request_body_bytes`: maximum client JSON request body size
- `max_non_stream_response_bytes`: maximum successful non-stream upstream response body size
- `max_upstream_error_body_bytes`: maximum upstream error body captured before returning a bounded proxy error
- `max_sse_frame_bytes`: maximum size of a single upstream SSE frame
- `stream_idle_timeout_secs`: maximum idle time between upstream stream chunks
- `stream_max_duration_secs`: maximum total lifetime for an upstream stream
- `stream_max_events`: maximum upstream SSE frames processed for one stream
- `max_accumulated_stream_state_bytes`: maximum accumulated translator state for one stream

### Compatibility Behavior

Translated paths use one product behavior: maximum safe compatibility. The proxy preserves the most complete client-visible representation it can, emits portability warnings when omitting non-portable detail, and rejects requests before upstream when a field would require unsafe approximation.

Fail-closed behavior is a hard portability boundary, not a lower compatibility setting. Responses reasoning or compaction carriers may be omitted with a portability warning only when visible summary text or visible transcript history remains. Requests with opaque-only reasoning, opaque-only compaction, provider-owned state, unsupported media, and unsupported source transports fail closed. Provider-owned state is preserved only when same-wire-protocol handling can remain byte-preserving internally.

### `conversation_state_bridge`

`conversation_state_bridge` controls the retention bounds for llmup's built-in
local transcript replay. It is part of the single maximum safe compatibility
behavior, not user-selectable behavior:

```yaml
conversation_state_bridge:
  ttl_seconds: 3600
  max_bytes: 268435456
```

The bridge may save short-lived, process-local transcript state behind
llmup-owned `resp_llmup_*` response IDs when an OpenAI Responses request needs
cross-protocol request construction for OpenAI Chat Completions or Anthropic
Messages upstreams. Later non-streaming continuations with a matching local
`previous_response_id` are expanded back into explicit Responses `input` before
normal translation. The replayable surface is text message input, assistant text
message output, regular `function_call` / `function_call_output`, portable
`custom_tool_call` / `custom_tool_call_output`, visible reasoning summary text,
and first-response streaming completed visible output capture.

Boundaries:

- state is process memory only, bounded by `ttl_seconds` and `max_bytes`; process restart drops all entries
- `store: false` is honored and does not save replay state
- unknown, expired, non-local, owner-mismatched, route/config-mismatched, and external provider-owned IDs fail closed
- streaming first responses can commit completed visible text and reasoning summaries for later local replay, but streaming continuation with `previous_response_id` still fails closed
- native OpenAI Responses forwarding keeps provider IDs and provider-owned state unchanged
- visible reasoning summary replay stores only `summary[].summary_text.text`; `encrypted_content`, Anthropic thinking signatures, redacted/omitted thinking, and provider-private reasoning fields are not saved or replayed
- there is no persistence, retrieval, cache, Conversations API bridge, background lifecycle emulation, compaction replay, provider-owned opaque state replay, or Gemini native state bridge

### `upstreams`

A map of named upstreams. Each upstream defines where the real provider lives and which protocol shape the proxy should use when talking to it.

Each upstream usually needs:

- `api_root`: the provider API root, including its version segment
- `format`: the expected upstream protocol when you want to pin it
- a provider credential source in `proxy_key` mode, usually `provider_key.env`

Practical rules:

- `api_root` should point at the provider API root, not a model-specific path
- include the version segment such as `/v1` or `/v1beta`
- `upstream_headers` may add non-secret routing or tenant headers, but cannot override auth/secret headers such as `authorization`, `proxy-authorization`, `x-api-key`, `api-key`, `openai-api-key`, or `anthropic-api-key`
- use exactly one of `provider_key.inline` or `provider_key.env` when the
  global mode is `proxy_key`
- normally omit provider credential sources when the global mode is
  `client_provider_key`; `provider_key.env` is accepted but ignored, while
  `provider_key.inline` is rejected

Provider-specific static headers belong inside the upstream's `headers` field.

### `model_aliases`

`model_aliases` lets you present one stable local model name to clients even if the real upstream models change over time.

Static aliases use concrete upstream names and concrete provider model IDs:

```yaml
model_aliases:
  coding-anthropic: "PROXY-KEY-ANTHROPIC-COMPATIBLE:provider-anthropic-model"
  coding-openai: "PROXY-KEY-OPENAI-COMPATIBLE:provider-openai-model"
```

Those are local names. They do not need to match provider model IDs. If you start `llm-universal-proxy --config` directly, use concrete `UPSTREAM:MODEL` alias targets.

If you want more explicit metadata, switch to the structured alias form:

```yaml
model_aliases:
  coding-openai:
    target: PROXY-KEY-OPENAI-COMPATIBLE:provider-openai-model
    limits:
      context_window: 200000
      max_output_tokens: 128000
```

Model resolution rules:

- if the client requests an alias such as `coding-openai`, the proxy resolves it through `model_aliases`
- if the client requests `UPSTREAM:MODEL`, the proxy routes directly to that named upstream and model
- if multiple upstreams exist and the requested model is neither an alias nor an explicit `UPSTREAM:MODEL`, the proxy rejects the request instead of guessing

### `surface_defaults` and alias `surface`

Use `limits` when you want the proxy and client wrappers to know things such as:

- `context_window`
- `max_output_tokens`

Use `surface_defaults` on an upstream, or `surface` on an alias, when you want to describe the client-visible behavior of a routed model.

Raw HTTP smoke tests can omit these fields, but wrapper/live-profile flows should provide at least the minimal text-only surface used by the quickstart. Add richer values only when the model surface really supports them.

Supported `surface.modalities.input` values:

| Value | Meaning |
| --- | --- |
| `text` | Plain text input. |
| `image` | Image input parts. |
| `audio` | Audio input parts. |
| `pdf` | Narrow PDF input capability. Use this when the model supports PDF documents but not arbitrary files. |
| `file` | Generic file input capability. This includes PDF; use `pdf` when you need to advertise only the narrower PDF shape. |
| `video` | Video input capability. In the current first multimodal phase, video is primarily a request-policy gate and is not a promise of cross-provider video translation. |

These values describe the proxy's protocol compatibility layer and the configured client-visible alias surface. They do not prove that a real upstream provider or model accepts that media; configure only what the selected upstream model actually supports.

`surface.modalities.input` gates media types, not every possible source transport for that media. HTTP(S) URLs are explicit remote URLs; provider-native or local URIs such as `gs://`, `s3://`, and `file://`, and provider-owned identifiers such as `file_id`, are different source identities and are not portable unless a documented adapter supports them.

Unsupported media and unsupported source transports must fail closed. The proxy should reject unsupported or unknown typed media parts instead of silently dropping them, flattening them into text, or forwarding them to an upstream surface that cannot represent them.

Media metadata must also be self-consistent. For OpenAI Chat `file` parts and OpenAI Responses `input_file` parts, the proxy compares explicit `mime_type` / `mimeType`, MIME-bearing `file_data` data URIs, and filename-derived hints. If those sources disagree, the request is rejected before any upstream call.

## Upstream Egress Proxy

The public config shape is:

- top-level `proxy`
- per-upstream `proxy`

Each `proxy` value is either:

- `direct`
- `{ url: ... }`

If both levels are omitted, the proxy falls back to the standard environment proxy variables.

Resolution order:

1. `upstreams.<NAME>.proxy`
2. top-level `proxy`
3. environment proxy settings

Supported `proxy.url` schemes:

- `http://`
- `https://`
- `socks5://`
- `socks5h://`

For a fuller proxy example, see [examples/upstream-proxy.yaml](../examples/upstream-proxy.yaml).

## Optional Hooks and Debug Trace

These are optional and should usually come after the basic routing config is already working.

### `hooks`

Use hooks when you want best-effort outbound reporting for usage or exchanges.

### `debug_trace`

Use `debug_trace` when you want a local JSONL trail for debugging request and response behavior.

For client attachment and wrapper details, see [Client Setup Guide](./clients.md).
