# E2E Test Plan — llm-universal-proxy (real client ↔ proxy ↔ real provider)

> **Status:** PLAN only. Do not execute from this document. This defines the matrix, the run
> recipe, and the pass/fail + in-place-fix policy for the tester (human or agent) who runs it.

**Principles (read first).** KISS / DRY / YAGNI. One canonical way to run a given scenario class.
Don't test the tests. Mind functional boundaries: a provider returning odd/low-quality output is
the provider, not a proxy bug — unless the proxy's translation mangled the request or response.
Agents fix failures **in place** (root cause in code, structural, no interface change) and re-run;
no separate report / evidence / audit artifacts are produced. No scope creep: if a row isn't in the
matrix, it isn't run.

---

## 1. Goal & scope

**Goal.** Prove end-to-end correctness of `llmup` as a protocol-translation proxy pivoted through
OpenAI Chat: a **real** client (codex / claude / botified) talks its native wire protocol to the
proxy, the proxy translates to the configured upstream wire protocol, the **real** provider
responds, and the client renders a correct result — including streaming, tool round-trips, and an
image input.

**In scope (what E2E uniquely covers).**
- Each translation **direction** (Responses→Chat, Responses→Anthropic, Anthropic→Anthropic
  same-wire, Anthropic→Chat, Chat→Chat, Chat→Anthropic) at least once.
- Streaming vs non-streaming on the primary directions.
- One tool-use round-trip per agent client that calls tools.
- One vision input through `qwen-vl-plus`.

**Out of scope of E2E (already covered elsewhere).**
- Field-level translation correctness — covered by unit/integration tests under `src/translate/`,
  `src/streaming/tests/`, `src/server/tests/`. E2E does not re-verify individual field mappings.
- See §6 for the full functional-boundary list.

E2E = "does the whole chain actually work against real providers and real agent clients."

---

## 2. Environment & how-to-run

### 2.1 Provider credentials (from `~/.zshrc`)

The proxy config references these **env-var names** (never literal secrets). The tester must have
them exported in the shell that launches the proxy.

| Provider | Interface | Base URL env | Key env | Model env / value |
|---|---|---|---|---|
| DeepSeek | OpenAI Chat | `DEEPSEEK_OPENAI_BASE_URL` | `DEEPSEEK_API_KEY` | `DEEPSEEK_DEFAULT_MODEL` (`deepseek-v4-flash`), `DEEPSEEK_PRO_MODEL` (`deepseek-v4-pro`) |
| Qwen | OpenAI Chat (compatible-mode) | `QWEN_OPENAI_BASE_URL` | `QWEN_API_KEY` | `QWEN_VISION_MODEL` (`qwen-vl-plus`) |
| GLM | OpenAI Chat | `GLM_OPENAI_BASE_URL` | `GLM_API_KEY` | `GLM_MODEL` (`glm-5.2`) |
| GLM | Anthropic | `GLM_ANTHROPIC_BASE_URL` | `GLM_API_KEY` | `GLM_MODEL` (`glm-5.2`) |

> **Verify the GLM model id** — `glm-5.2[1m]` is rejected by GLM (HTTP 400 code 1211 "模型不存在"); use
> `glm-5.2` (or `glm-4.6` / `glm-4.5`). The `[1m]` context-window suffix is an internal tag, not a
> valid GLM model id.
| ~~Z_AI~~ | — | — | `Z_AI_API_KEY` only | **EXCLUDED** — no confirmed base URL / model id. Do not invent one. |

**Pre-run env check.** Before generating the config, confirm the base-URL env *values* are intact —
e.g. `echo $GLM_ANTHROPIC_BASE_URL` should end in `/anthropic` (not truncated). A truncated base URL
silently fails every row routed to that provider.

### 2.2 Run the proxy

The binary is not installed. argv0 `llm-universal-proxy` dispatches to the **server** (only
`llmup-codex` / `llmup-claude` / `llmup-config` dispatch to wrappers), so plain `cargo run` is
correct — no `LLMUP_FORCE_SERVER` needed.

```bash
# 0. one-time build (skipped on subsequent runs if cached)
cargo build --bin llm-universal-proxy

# 1. generate the E2E config (base URLs expanded from the §2.1 env vars at generation time;
#    provider KEYS stay as env-name references, never literal)
#    This heredoc is the single source of the E2E runtime config.
source ~/.zshrc  # ensure provider env vars are present
cat > /tmp/llmup-e2e.yaml <<EOF
listen: 127.0.0.1:8080
upstream_timeout_secs: 120

upstreams:
  DEEPSEEK:
    api_root: ${DEEPSEEK_OPENAI_BASE_URL}
    format: openai-chat-completions
    provider_key: { env: DEEPSEEK_API_KEY }
    surface_defaults:
      modalities: { input: ["text"], output: ["text"] }
      tools: { supports_search: false, supports_view_image: false, apply_patch_transport: freeform, supports_parallel_calls: false }
  QWEN:
    api_root: ${QWEN_OPENAI_BASE_URL}
    format: openai-chat-completions
    provider_key: { env: QWEN_API_KEY }
    surface_defaults:
      modalities: { input: ["text","image"], output: ["text"] }
      tools: { supports_search: false, supports_view_image: false, apply_patch_transport: freeform, supports_parallel_calls: false }
  GLM_OPENAI:
    api_root: ${GLM_OPENAI_BASE_URL}
    format: openai-chat-completions
    provider_key: { env: GLM_API_KEY }
    surface_defaults:
      modalities: { input: ["text"], output: ["text"] }
      tools: { supports_search: false, supports_view_image: false, apply_patch_transport: freeform, supports_parallel_calls: false }
  GLM_ANTHROPIC:
    api_root: ${GLM_ANTHROPIC_BASE_URL}/v1   # proxy appends /messages, NOT /v1/messages
    format: anthropic
    provider_key: { env: GLM_API_KEY }
    surface_defaults:
      modalities: { input: ["text"], output: ["text"] }
      tools: { supports_search: false, supports_view_image: false, apply_patch_transport: freeform, supports_parallel_calls: false }

model_aliases:
  ds-flash:        DEEPSEEK:deepseek-v4-flash
  ds-pro:          DEEPSEEK:deepseek-v4-pro
  qwen-vl:         QWEN:qwen-vl-plus
  glm-chat:        GLM_OPENAI:glm-5.2
  glm-anthropic:   GLM_ANTHROPIC:glm-5.2
EOF

# 2. launch the proxy in proxy_key auth mode
LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key \
LLM_UNIVERSAL_PROXY_KEY=llmup-e2e-local \
cargo run --bin llm-universal-proxy -- --config /tmp/llmup-e2e.yaml
```

> `api_root` is a literal URL (no env indirection in the schema), so base URLs are expanded by the
> shell at generation time. Provider keys use `{ env: NAME }` indirection and are resolved by the
> proxy at runtime — no secret ever appears in the yaml. Auth is **proxy_key** mode (single shared
> key `llmup-e2e-local` for all clients); there is no IP/loopback passwordless mode.
>
> **Anthropic api_root must include `/v1`** — the proxy builds the Anthropic upstream URL as
> `{api_root}/messages` (NOT `/v1/messages`); see `src/config.rs` `build_upstream_url`. So GLM's
> anthropic base `${GLM_ANTHROPIC_BASE_URL}/v1` is required, mirroring the official
> `https://api.anthropic.com/v1`. A truncated root silently 404s every Anthropic-routed row.

### 2.3 Point each client at the proxy

All clients use the **same** proxy key value (`llmup-e2e-local`). The proxy base path differs per
client wire protocol.

**Client isolation (run before ANY client).** Every client invocation MUST run under a freshly-empty
HOME + dedicated config dirs with real keys scrubbed, so the client provably hits the proxy (no
ChatGPT/OAuth login, no real-key bypass). The freshly-empty config dirs are the proof that traffic can
only go through the proxy — provider keys are resolved by the proxy from its own env, not the client's.
For claude specifically this prevents the "multiple credentials" 400: a real `ANTHROPIC_API_KEY` plus
the proxy's `ANTHROPIC_AUTH_TOKEN` together make the proxy reject with a 400.

```bash
# isolation preamble — run before ANY client (codex/claude/botified)
E2E_HOME="$(mktemp -d -t llmup-e2e-home-XXXX)"
export HOME="$E2E_HOME"
export XDG_CONFIG_HOME="$E2E_HOME/.config" XDG_CACHE_HOME="$E2E_HOME/.cache" \
       XDG_DATA_HOME="$E2E_HOME/.local/share" XDG_STATE_HOME="$E2E_HOME/.local/state"
export CODEX_HOME="$E2E_HOME/.codex"          # empty -> no ChatGPT login
export CLAUDE_CONFIG_DIR="$E2E_HOME/.claude"  # empty -> no saved claude login
mkdir -p "$E2E_HOME/.codex" "$E2E_HOME/.claude" "$E2E_HOME/.config" "$E2E_HOME/.cache"
# codex errors if CODEX_HOME points at a path that does not exist ("CODEX_HOME points to ... but that
# path does not exist"); create all client dirs up front.
unset ANTHROPIC_API_KEY OPENAI_API_KEY ANTHROPIC_AUTH_TOKEN  # scrub real keys from ~/.zshrc
export NO_PROXY=127.0.0.1,localhost
# THEN set the proxy-pointing vars (PROXY_KEY / the codex -c flags / etc.) from below
```

Then set the per-client vars:

```bash
PROXY_KEY=llmup-e2e-local
PROXY=http://127.0.0.1:8080

# codex (OpenAI Responses client) → proxy /openai/v1/*
# NOTE: codex 0.146 IGNORES OPENAI_BASE_URL — setting just OPENAI_BASE_URL+OPENAI_API_KEY makes codex
# hit api.openai.com directly (401). codex needs a CONFIG-BASED provider via -c flags. The project's
# own launcher (src/user_tools/agent_launcher.rs build_client_argv) wires codex exactly this way.
export OPENAI_API_KEY="$PROXY_KEY"   # proxy auth token; codex reads it via env_key below
codex exec \
  -c model_provider=proxy \
  -c "openai_base_url=$PROXY/openai/v1" \
  -c model_providers.proxy.name=proxy \
  -c "model_providers.proxy.base_url=$PROXY/openai/v1" \
  -c model_providers.proxy.env_key=OPENAI_API_KEY \
  -c model_providers.proxy.wire_api=responses \
  -c model_providers.proxy.supports_websockets=false \
  -c features.multi_agent=false \
  -c tools.web_search=false \
  -m <alias> "<prompt>"

# claude / Claude Code (Anthropic Messages client) → proxy /anthropic/v1/*
export ANTHROPIC_BASE_URL="$PROXY/anthropic"
export ANTHROPIC_AUTH_TOKEN="$PROXY_KEY"
export ANTHROPIC_MODEL="<alias>"
claude --print "<prompt>"

# botified (OpenAI Chat client) → proxy /openai/v1/*
# Configure its runtime config providers[].base_url = "$PROXY/openai/v1"
# and providers[].api_key_env = BOTIFIED_PROXY_KEY, then:
BOTIFIED_PROXY_KEY="$PROXY_KEY" botified serve --config /tmp/botified-e2e.yaml
```

Notes:
- codex appends `/responses` to the configured provider `base_url` (`$PROXY/openai/v1`) → route
  `/openai/v1/responses`. ✓ It does NOT read `OPENAI_BASE_URL` (see the codex block above).
- `features.multi_agent=false` + `tools.web_search=false` are REQUIRED when the upstream is NOT
  OpenAI Responses — codex otherwise sends the non-portable `multi_agent_v1` namespace tool, which
  the proxy correctly rejects → the whole request fails. The project launcher disables them for
  non-Responses upstreams. `supports_websockets=false` is also required (the proxy is HTTP/SSE;
  without it codex attempts a websocket and fails).
- claude appends `/v1/messages` to `ANTHROPIC_BASE_URL` → route `/anthropic/v1/messages`. ✓
- botified appends `/chat/completions` to its `base_url` → route `/openai/v1/chat/completions`. ✓

---

## 3. The matrix

Priority: **P0** = every translation direction + streaming. **P1** = tool-use round-trip.
**P2** = vision + botified breadth + error-translation. All rows are in scope unless marked.

| # | Pri | Client (wire) | Upstream (wire) | Direction | Model alias | Stream? | Tool? | Vision? |
|---|---|---|---|---|---|---|---|---|
| 1 | P0 | codex (Responses) | DeepSeek (Chat) | Responses→Chat | `ds-flash` | no | – | – |
| 2 | P0 | codex (Responses) | DeepSeek (Chat) | Responses→Chat | `ds-flash` | **yes** | – | – |
| 3 | P0 | codex (Responses) | GLM-Anthropic | Responses→Anthropic | `glm-anthropic` | no | – | – |
| 4 | P0 | codex (Responses) | GLM-Anthropic | Responses→Anthropic | `glm-anthropic` | **yes** | – | – |
| 5 | P0 | claude (Anthropic) | GLM-Anthropic | Anthropic→Anthropic (same-wire) | `glm-anthropic` | no | – | – |
| 6 | P0 | claude (Anthropic) | GLM-Anthropic | Anthropic→Anthropic (same-wire) | `glm-anthropic` | **yes** | – | – |
| 7 | P0 | claude (Anthropic) | DeepSeek (Chat) | Anthropic→Chat | `ds-flash` | no | – | – |
| 8 | P0 | claude (Anthropic) | DeepSeek (Chat) | Anthropic→Chat | `ds-flash` | **yes** | – | – |
| 9 | P1 | codex (Responses) | GLM-OpenAI (Chat) | Responses→Chat | `glm-chat` | no | **yes** | – |
| 10 | P1 | claude (Anthropic) | GLM-Anthropic | Anthropic→Anthropic | `glm-anthropic` | no | **yes** | – |
| 11 | P2 | claude (Anthropic) | Qwen (Chat) | Anthropic→Chat | `qwen-vl` | no | – | **yes** |
| 12 | P2 | botified (Chat) | DeepSeek (Chat) | Chat→Chat (same-wire) | `ds-flash` | no | – | – |
| 13 | P2 | botified (Chat) | GLM-Anthropic | Chat→Anthropic | `glm-anthropic` | no | – | – |
| 14 | P2 | codex (Responses) | any | error-translation | `<bad-alias>` | no | – | – |

**Why these rows / why not more.** One representative upstream per direction for stream/non-stream
avoids provider combinatorial explosion (DeepSeek and Qwen are both OpenAI Chat — DeepSeek stands in
for the P0 Chat upstream, Qwen appears via the vision row). GLM is the only provider offering both
wires, so it carries every Anthropic-side and cross-protocol-into-Anthropic case. botified gets two
rows only — enough to cover both its directions (Chat→Chat and Chat→Anthropic). Tool-use uses one row
per tool-calling client. Vision is a single row (one image input satisfies the requirement).
Error-translation is one row (14): a bad alias forces an error that must reach the client as a
well-formed error in its native wire format (not a crash/hang/200-with-error).

---

## 4. Per-combo procedure (terse, one way per scenario class)

Standard task shapes (DRY — reuse across rows):
- **Text completion:** `"Reply with exactly the word PONG."` → expect `PONG` (or clearly containing it).
- **Tool-use:** give the agent a trivial tool (e.g. a `get_timestamp`/`echo` shell or MCP tool) and
  instruct `"Use the <tool> tool, then report its result."` → expect a tool call + a follow-up that
  relays the tool output.
- **Vision:** a small local PNG; prompt `"Describe this image in one sentence."` → expect a
  plausible description (not a refusal/error about missing image).

### 4.1 codex rows (1–4, 9, 14)
```bash
# rows 1–4 (non-stream + stream pairs): codex exec ALWAYS streams — ONE invocation covers BOTH rows
# of a pair. Observe incremental chunk rendering for the stream row (2,4); the completed output
# satisfies the non-stream row (1,3). There is no --no-stream flag (disregard any old "non-streaming
# by default" wording — codex streams).
#
# codex IGNORES OPENAI_BASE_URL (§2.3) — EVERY codex invocation needs the config-based provider
# flags below. Define them once (matches src/user_tools/agent_launcher.rs build_client_argv) and
# reuse across rows:
PROXY=http://127.0.0.1:8080
CODEX_CFG=(
  -c model_provider=proxy
  -c "openai_base_url=$PROXY/openai/v1"
  -c model_providers.proxy.name=proxy
  -c "model_providers.proxy.base_url=$PROXY/openai/v1"
  -c model_providers.proxy.env_key=OPENAI_API_KEY
  -c model_providers.proxy.wire_api=responses
  -c model_providers.proxy.supports_websockets=false
  -c features.multi_agent=false
  -c tools.web_search=false
)

# rows 1–4: text completion
codex exec "${CODEX_CFG[@]}" -m <alias> "Count slowly from 1 to 5, one number per line."
# tool (row 9): use the built-in shell tool under a WRITABLE sandbox (read-only blocks execution;
# codex has no echo tool). --skip-git-repo-check is needed because the sandbox dir is not a git repo.
codex exec "${CODEX_CFG[@]}" -m glm-chat \
  --sandbox workspace-write -C /tmp/llmup-e2e-codex --skip-git-repo-check \
  "Run this shell command: echo hi . Then report the exact stdout you observed."
# error (row 14): unknown alias -> client must receive a well-formed error in its native wire format
codex exec "${CODEX_CFG[@]}" -m this-alias-does-not-exist "Reply with exactly the word PONG."
```

### 4.2 claude rows (5–8, 10, 11)
```bash
# rows 5–8 (non-stream + stream pairs): claude --print ALWAYS streams — ONE invocation covers BOTH
# rows of a pair. Observe incremental chunk rendering for the stream row (6,8); the completed output
# satisfies the non-stream row (5,7). There is no --no-stream flag.
ANTHROPIC_MODEL=<alias> claude --print "Reply with exactly the word PONG."
# tool (row 10): allow one tool and require its use.
# NOTE: --allowedTools/--allowed-tools is VARIADIC (<tools...>) — the space form
# `--allowed-tools "Bash(echo *)" "prompt"` consumes the prompt as a tool name ("no input" error).
# Use the equals form with a single value (--allowedTools=Bash), OR pipe the prompt via stdin.
ANTHROPIC_MODEL=glm-anthropic claude --print --allowedTools=Bash "Use echo to print hi, then report it."
# vision (row 11): attach an image file. Driving an image through `claude --print <file>` is
# unreliable; the image-translation path can instead be verified by a direct curl to
# /anthropic/v1/messages with an Anthropic image block -> Qwen (this is what execution used
# successfully):
#   curl -s "$PROXY/anthropic/v1/messages" \
#     -H "x-api-key: $PROXY_KEY" -H "anthropic-version: 2023-06-01" \
#     -H "content-type: application/json" \
#     -d "{\"model\":\"qwen-vl\",\"max_tokens\":256,\"messages\":[{\"role\":\"user\",\"content\":[
#          {\"type\":\"text\",\"text\":\"Describe this image in one sentence.\"},
#          {\"type\":\"image\",\"source\":{\"type\":\"base64\",\"media_type\":\"image/png\",
#           \"data\":\"$(base64 -w0 /tmp/sample.png)\"}}]}]}"
ANTHROPIC_MODEL=qwen-vl claude --print "Describe this image in one sentence." /tmp/sample.png
```
If Claude Code rejects the unknown model alias, also export
`ANTHROPIC_CUSTOM_MODEL_OPTION=<alias>` (and `ANTHROPIC_CUSTOM_MODEL_OPTION_NAME` /
`ANTHROPIC_CUSTOM_MODEL_DESCRIPTION` if needed) so the alias is accepted.

### 4.3 botified rows (12, 13) — MANUAL / TUI-driven

botified has no one-shot chat CLI (only `serve`), so rows 12 and 13 are human/TUI-driven, not
scriptable like codex/claude. An agent automating this would need the WS frame protocol (out of KISS
scope).

Write `/tmp/botified-e2e.yaml` with a SINGLE text provider pointed at the proxy (`api_compat: standard`
so it emits plain OpenAI Chat):
```yaml
version: 1
providers:
  - name: via-proxy
    api_compat: standard
    base_url: http://127.0.0.1:8080/openai/v1   # proxy Chat route
    model: ds-flash            # row 12; use glm-anthropic for row 13
    api_key_env: BOTIFIED_PROXY_KEY
    capabilities: [text]
    context_window_tokens: 131072
    max_output_tokens: 32768
service: { host: 127.0.0.1, port: 17777, service_key_env: BOTIFIED_SERVICE_KEY }
tools: { enabled: [] }
```

Launch and drive one chat turn via the TUI:
```bash
BOTIFIED_PROXY_KEY=llmup-e2e-local BOTIFIED_SERVICE_KEY=anything \
  botified serve --config /tmp/botified-e2e.yaml &
botified-tui --base-url http://127.0.0.1:17777 --service-key-env BOTIFIED_SERVICE_KEY
# in the TUI: send the PONG prompt, observe the reply
```
**Reach proof** = the proxy log shows the request to the upstream host (DeepSeek for row 12,
GLM-Anthropic for row 13).

### 4.4 What to verify (every row)
1. **Reach:** proxy logs show the request hitting the expected upstream (provider host in logs).
2. **Translate:** client receives a well-formed response in its **native** wire format and renders it
   (no protocol/parse error on the client side).
3. **Content:** the task's expected content appears (PONG / 1–5 / tool result / image description).
4. **Stream rows:** incremental chunks arrive and the stream closes cleanly (no hang, no truncation).
5. **Tool rows:** a tool-call is issued by the model, the tool result is fed back, and the final
   answer references the tool output.
6. **Vision row:** the image is accepted and described (proves image blocks crossed the translation).

---

## 5. Pass/fail criteria + in-place-fix policy

**Pass** = all four §4.4 checks green for the row.

**Fail handling — fix in place, no artifacts.** When a row fails:
1. Determine whether the failure is a **proxy bug** (translation/streaming/auth/request-routing) or a
   **provider/client quirk** (§6). Only proxy bugs are actioned here.
   - **Tie-breaker:** the deciding signal is the request the proxy *actually emitted* (proxy logs), not
     the prompt. It is a **proxy bug** if the upstream request shape is wrong or the response
     translation mangles the client's wire format; it is a **provider quirk** if the proxy's emitted
     request is well-formed but the provider rejects/limits it.
2. If proxy bug: fix the **root cause** in code (structural, TDD-style — add/adjust a unit test that
   reproduces the fault at the field/streaming level, then fix). Do **not** change any external
   interface (client flags, config schema, provider contract). Do **not** weaken assertions.
3. Re-run the failing row. Repeat until green.
4. No separate report, evidence, or audit document is created. The commit history + the matrix above
   are the record.

**Whole-suite pass** = every in-scope matrix row green after the final run.

---

## 6. Out-of-scope / functional boundaries

Not the proxy's job (a failure here is **not** an E2E failure of the proxy unless translation
mangled the payload):
- **Provider-side output quality** — odd/refused/hallucinated model answers, provider rate limits,
  provider downtime, or provider-specific quirks (e.g. a provider ignoring a parameter it doesn't
  support). If the same prompt misbehaves directly against the provider, it's the provider.
- **Client-side behavior** — codex/claude/botified UI rendering bugs, their own tool-execution
  sandboxing, or client config mistakes unrelated to the proxy.
- **Performance / load testing** — latency, throughput, concurrency under load. E2E is correctness
  only, one request at a time.
- **The deferred PF-5 streaming-bridge id case** — the known-deferred streaming-bridge identifier
  scenario is explicitly out of scope; do not expand the matrix to chase it here.
- **Z_AI** — excluded: only `Z_AI_API_KEY` is available with no confirmed base URL or model id.
- **Field-level translation exhaustiveness** — covered by unit/integration suites, not re-checked
  here (see §1).
- **Interface changes** — adding new clients/providers/routes/wire formats is scope creep for this
  pass; the matrix above is the entire scope.
