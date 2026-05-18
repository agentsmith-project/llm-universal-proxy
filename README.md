# LLM Universal Proxy

[中文文档](./README_CN.md) · [Documentation](./docs/README.md)

`llmup` is a local proxy for model APIs and compatible endpoints. It lets Codex CLI and Claude Code talk to a provider through one local launcher path, while the real provider key stays on the proxy side.

It is built for maximum safe compatibility: when a feature can be translated safely, the proxy does that work locally; when it cannot, requests fail closed before the upstream call instead of being guessed into a provider shape.

> [!IMPORTANT]
> `llmup` works with provider APIs and compatible endpoints. It is not a bridge into vendor first-party app subscriptions or bundled first-party CLI entitlements unless that vendor explicitly documents that kind of third-party access.

## Quick Start

Install the binary and the three user commands:

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/install.sh | sh
```

Configure your provider:

```bash
llmup-config
```

Start the client you use:

```bash
llmup-codex
```

or:

```bash
llmup-claude
```

The real provider key is collected by `llmup-config` and kept in the local proxy configuration, not pasted into Codex or Claude Code. The launchers give the client a local proxy key and keep the upstream provider key on the proxy side.

The launchers also manage the local proxy process, client base URL, default model alias, and llmup-owned Codex/Claude state directories.

## Why There Is No `llmup` Command

There is intentionally no standalone `llmup` command. `llmup-config`, `llmup-codex`, and `llmup-claude` are the user commands.

That keeps the first-use path small: configure once, then launch the native client you already use. `llm-universal-proxy --config` remains the advanced server entrypoint.

## Compatibility

`llmup` gives clients a stable local protocol surface, not unlimited provider equivalence.

- Maximum safe compatibility means preserving the richest safe portable representation and refusing unsafe conversions.
- Provider-specific state, opaque reasoning carriers, and non-portable tool semantics may require native handling or fail closed before the upstream call.
- Reasoning effort such as `xhigh` is still a client/request setting; it is not part of a model name.

## Advanced

- [docs/clients.md](./docs/clients.md): launcher-managed Codex and Claude Code behavior
- [docs/advanced-usage.md](./docs/advanced-usage.md): manual proxy startup, YAML, manual Codex/Claude wiring, and auth modes
- [docs/max-compat-design.md](./docs/max-compat-design.md): compatibility design notes
- [docs/protocol-compatibility-matrix.md](./docs/protocol-compatibility-matrix.md): protocol conversion matrix
- [docs/DESIGN.md](./docs/DESIGN.md): system design
- [docs/container.md](./docs/container.md): container image usage
- [docs/admin-dynamic-config.md](./docs/admin-dynamic-config.md): admin and dynamic config reference
- [docs/ga-readiness-review.md](./docs/ga-readiness-review.md): GA scope and compatibility boundaries

## License

MIT License
