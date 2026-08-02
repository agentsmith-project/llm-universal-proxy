# LLM Universal Proxy

[中文文档](./README_CN.md) · [Documentation](./docs/README.md)

`llmup` is a local proxy for model APIs and compatible endpoints. It lets Codex CLI talk to a provider through one local proxy while the real provider key stays on the proxy side.

It is built for maximum safe compatibility: when a feature can be translated safely, the proxy does that work locally; when it cannot, requests fail closed before the upstream call instead of being guessed into a provider shape.

> [!IMPORTANT]
> `llmup` works with provider APIs and compatible endpoints. It is not a bridge into vendor first-party app subscriptions or bundled first-party CLI entitlements unless that vendor explicitly documents that kind of third-party access.

## Quick Start

Install the native client you plan to use first. `llmup` does not install Codex CLI or Claude Code.

Install the binary and the `llmup` convenience alias with the one-line installer:

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/install.sh | sh
```

The installer creates a `llmup` alias that points at `llm-universal-proxy`, so the single user-facing command is `llmup codex-setup`. The proxy is a local server, so start it with your provider configured (see the [Configuration Guide](./docs/configuration.md) for static YAML). Then generate the Codex config that routes a sub-agent through the proxy:

```bash
llmup codex-setup --base-url http://127.0.0.1:8080/openai/v1 --model main --provider-key <local-proxy-key>
```

`codex-setup` writes a `llmup` provider, a custom sub-agent, and a `llmup` profile under `~/.codex` so the official Codex main agent keeps its own credentials while a sub-agent routes through the proxy. The real provider key stays server-side; Codex only receives the local proxy key.

Then run Codex with the generated profile:

```bash
codex exec --profile llmup
```

When the proxy config asks for a service type, use the API shape your provider documents: `openai-chat-completions`, `openai-responses`, or `anthropic-messages`. The default local model name is `main`; pass it to `codex-setup --model` unless you configured another alias.

## Why a `llmup` Alias

The installer creates a `llmup` alias that points at the single `llm-universal-proxy` binary, so `llmup codex-setup` is the one user-facing command. `llm-universal-proxy --config` remains the advanced server entrypoint.

## Compatibility

`llmup` gives clients a stable local protocol surface, not unlimited provider equivalence.

- Maximum safe compatibility means preserving the richest safe portable representation and refusing unsafe conversions.
- Provider-specific state, opaque reasoning carriers, and non-portable tool semantics may require native handling or fail closed before the upstream call.
- Reasoning effort such as `xhigh` is still a client/request setting; it is not part of a model name.

## Advanced

- [docs/clients.md](./docs/clients.md): `codex-setup` flow and the Codex V1 hybrid sub-agent topology
- [docs/advanced-usage.md](./docs/advanced-usage.md): manual proxy startup, YAML, manual Codex/Claude wiring, and auth modes
- [docs/container.md](./docs/container.md): container image usage
- [docs/admin-dynamic-config.md](./docs/admin-dynamic-config.md): admin and dynamic config reference

## License

MIT License
