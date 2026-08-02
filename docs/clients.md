# Client Setup Guide

This guide covers the `codex-setup` subcommand and the Codex V1 hybrid sub-agent topology.
`llmup` does not install Codex CLI or Claude Code. Install the native client you plan to use first.

The single user-facing entrypoint is:

```bash
llmup codex-setup --base-url <proxy-url> --model <alias> --provider-key <local-proxy-key>
```

`llmup` is a convenience alias the installer creates for the `llm-universal-proxy` binary, so `llmup codex-setup` and `llm-universal-proxy codex-setup` are the same command.

## What `codex-setup` Generates

`codex-setup` writes Codex configuration files under `~/.codex` (or `$CODEX_HOME`) so a sub-agent can route through the local proxy while the official Codex main agent keeps using its own credentials:

- `~/.codex/llmup.config.toml` — a profile that defines the `[model_providers.llmup]` block (base URL, `wire_api = "responses"`, `env_key`) and pins `[features] multi_agent_v2 = false` so Codex stays on the default V1 multi-agent behavior.
- `~/.codex/agents/llmup-<model>.toml` — a custom sub-agent that sets `model_provider = "llmup"` for the model you pass to `--model`. It deliberately omits `fork_turns` and `fork_context`: V1 no-fork is the default, so fresh-context sub-agents are used.
- `~/.codex/llmup/state.json` — a manifest of the files `codex-setup` owns, plus the detected Codex version. It never stores API keys.

After install, export the proxy key into the environment variable the provider block references and run Codex with the profile:

```bash
codex exec --profile llmup
```

When configuring the proxy itself, use the API shape your provider documents: `openai-chat-completions`, `openai-responses`, or `anthropic-messages`.

## V1 Hybrid Sub-Agent Topology

Codex supports a hybrid V1 multi-agent setup where the main agent runs the official model via ChatGPT login and a custom sub-agent runs a local model through `llmup`. Codex routes each agent by its `model_provider`, so the two use different providers in one session.

`codex-setup` is the supported way to build this topology. The main agent keeps `model_provider = "openai"` with the official model and ChatGPT login. The `llmup-<model>` sub-agent pins `model_provider = "llmup"`, so Codex routes its turns to the local proxy.

Do not enable `[features] multi_agent_v2` — use the default V1 multi-agent. V2 encrypts the inter-agent task with an OpenAI server-held key (Fernet), so a non-OpenAI sub-agent cannot decrypt it and never receives the task. V1 delivers the task as a plain user message, which the local model receives normally. The generated profile keeps `multi_agent_v2 = false` defensively.

Prefer fresh-context sub-agents over full-history forks. Compaction is safe for long-running sub-agents: Codex uses local compaction for non-OpenAI providers and never calls the `/responses/compact` endpoint through `llmup`, so sub-agents compact normally.

## Auth Boundary

The real provider key belongs to the proxy, not to Codex. `codex-setup` writes only the `llmup` provider block that points at the local proxy; it never writes a provider key into `~/.codex`. Codex authenticates to the proxy through the env var named in the provider block (`LLMUP_PROXY_KEY` by default), which you set to the local proxy key.

## Managing The Installation

- `llmup codex-setup --status` prints the files `codex-setup` owns and the detected Codex version.
- `llmup codex-setup --uninstall` removes every managed file listed in `state.json`, then the manifest itself.
- Re-running `codex-setup` with new flags regenerates the managed files and preserves any unrelated tables you added to the profile.

For manual proxy startup, explicit base URLs, multi-endpoint YAML, or auth modes, use [Advanced Usage](./advanced-usage.md).
