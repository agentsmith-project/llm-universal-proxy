# Client Setup Guide

This guide covers the launcher-managed path for Codex CLI and Claude Code.
`llmup` does not install Codex CLI or Claude Code. Install the native client you plan to use first.

Start from the same user flow as the homepage:

```bash
llmup-config
```

Then choose the client you use:

```bash
llmup-codex
# or
llmup-claude
```

Run the launcher for the client you want. `llmup-codex` behaves like Codex CLI with llmup proxy setup added. `llmup-claude` behaves like Claude Code with llmup proxy setup added.

## What The Launchers Manage

The launchers own the repetitive local wiring:

- start and stop the local proxy for the session
- inject the client base URL and local proxy key
- keep the real provider secret out of the client process
- set the default llmup model alias
- keep native Codex or Claude Code arguments available for the real client
- keep client state in llmup-owned directories

Codex state is kept under `~/.llmup-codex` by default and exposed to Codex through `CODEX_HOME`. Claude Code state is kept under `~/.llmup-claude` by default and exposed through `CLAUDE_CONFIG_DIR`.

For Codex, the launcher uses fixed minimal provider injection: a `proxy` provider pointed at the local llmup base URL, the local proxy key, and the `default` model alias. It does not read live `llmup.surface` metadata, and the native Codex client does not see live surface metadata from llmup. Model identity, capability truth, and protocol shaping stay in the proxy configuration and server-side conversion path.

The original home directory is not rewritten by default, so tools that run inside Codex or Claude Code can still find normal git, SSH, package-manager, and language-tool caches.

## Native Arguments

After llmup consumes its own launcher controls, remaining arguments are passed through to the native client. That means common Codex and Claude Code workflows such as resume, help, MCP management, profiles, permission modes, or one-off model overrides stay native client behavior.

Use `llmup-codex --llmup-help` or `llmup-claude --llmup-help` for launcher-specific help. Native client help remains owned by Codex CLI or Claude Code.

For commands that should not go through the proxy, such as login, native help, native configuration, or MCP management, use `llmup-codex --llmup-no-proxy -- <native args>` or `llmup-claude --llmup-no-proxy -- <native args>`. The launcher does not auto-detect native subcommands.

## Auth Boundary

`llmup-config` stores provider credentials for the proxy. The launchers give Codex or Claude Code only the local proxy credential needed to call the proxy. This keeps the provider key on the proxy side instead of putting it into the client environment.

For manual auth modes, explicit base URLs, direct proxy startup, multi-endpoint YAML, Gemini through Google's OpenAI-compatible endpoint as OpenAI Chat Completions format, or container/admin links, use [Advanced Usage](./advanced-usage.md).
