# Advanced Usage

The normal path is `llmup-config`, then `llmup-codex` or `llmup-claude`. Use this page when you intentionally want to start the proxy yourself, maintain YAML by hand, or wire a client without the launchers.

## Manual Proxy Startup

Create a config file, then start the server entrypoint directly:

```bash
llm-universal-proxy --config ~/.llmup/manual.yaml
```

The proxy listens on the address in that file. Keep it running while manually wired clients use it.

## Multi-Endpoint YAML

This example exposes one OpenAI-compatible upstream, one Anthropic Messages-compatible upstream, and Gemini through Google's OpenAI-compatible endpoint. Replace URLs, model names, and environment variable names for your provider.

```yaml
listen: 127.0.0.1:18888
upstream_timeout_secs: 120

data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY

upstreams:
  OPENAI_COMPATIBLE:
    api_root: https://openai-compatible.example/v1
    format: openai-completion
    provider_key:
      env: OPENAI_COMPATIBLE_API_KEY

  ANTHROPIC_COMPATIBLE:
    api_root: https://anthropic-compatible.example
    format: anthropic
    provider_key:
      env: ANTHROPIC_COMPATIBLE_API_KEY

  GOOGLE_OPENAI_COMPATIBLE:
    api_root: https://generativelanguage.googleapis.com/v1beta/openai
    format: openai-completion
    provider_key_env: GEMINI_API_KEY

model_aliases:
  default: "OPENAI_COMPATIBLE:provider-model"
  claude-like: "ANTHROPIC_COMPATIBLE:provider-model"
  gemini-flash: "GOOGLE_OPENAI_COMPATIBLE:gemini-2.0-flash"
```

The provider key belongs to the proxy, not to the client. In `proxy_key` mode, the provider keys are read by the proxy from the configured env sources, and clients only send the local proxy key.

## Auth Modes

`data_auth.proxy_key` protects the local proxy. In that mode, clients use the local proxy key as their SDK key, while each upstream provider key stays server-side in `provider_key.inline`, `provider_key.env`, or `provider_key_env`.

In `client_provider_key` mode, the client SDK key is the real provider key. Use that mode only when you intentionally want clients to hold provider credentials and send them through the proxy for the selected upstream.

## Manual Codex Wiring

For a proxy started on `127.0.0.1:18888` in `proxy_key` mode, Codex's SDK key is the local proxy key:

```bash
OPENAI_API_KEY=$LLM_UNIVERSAL_PROXY_KEY \
OPENAI_BASE_URL=http://127.0.0.1:18888/openai/v1 \
codex \
  -c model_provider=proxy \
  -c model_providers.proxy.name=Proxy \
  -c model_providers.proxy.env_key=OPENAI_API_KEY \
  -c model_providers.proxy.base_url=http://127.0.0.1:18888/openai/v1 \
  -c model_providers.proxy.wire_api=responses \
  --model default
```

Codex appends its Responses path to the configured base URL, so the proxy receives OpenAI-style Responses traffic.

## Manual Claude Wiring

For Claude Code in `proxy_key` mode, the SDK key is also the local proxy key:

```bash
ANTHROPIC_API_KEY=$LLM_UNIVERSAL_PROXY_KEY \
ANTHROPIC_BASE_URL=http://127.0.0.1:18888/anthropic \
claude --model claude-like
```

Claude Code appends its Messages path to the configured base URL, so the proxy receives Anthropic-style Messages traffic.

## Google OpenAI-Compatible Gemini

Gemini is supported through Google's OpenAI-compatible endpoint. Configure it as an OpenAI-compatible upstream with `format: openai-completion` and the API root `https://generativelanguage.googleapis.com/v1beta/openai`.

Native Gemini `generateContent` routing is not the active user surface. If a Gemini request depends on native-only semantics that cannot be safely represented through the configured protocol, the proxy should fail closed instead of inventing behavior.

## Admin And Containers

For runtime admin operations, use [Admin and Dynamic Config](./admin-dynamic-config.md). For Docker and GHCR usage, use the [Container Guide](./container.md).

This page intentionally links to the Admin API reference instead of copying the full endpoint list, so the source of truth stays in one place.
