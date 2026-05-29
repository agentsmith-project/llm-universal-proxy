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
- project the selected llmup model alias and supported client hints
- keep native Codex or Claude Code arguments available for the real client
- keep client state in llmup-owned directories

Codex state is kept under `~/.llmup-codex` by default and exposed to Codex through `CODEX_HOME`. Claude Code state is kept under `~/.llmup-claude` by default and exposed through `CLAUDE_CONFIG_DIR`.

In managed profile projection, `--llmup-model <alias>` selects the llmup alias to expose to the native client. The default alias is `main`.

The config protocol values are explicit: `openai-chat-completions`, `openai-responses`, and `anthropic-messages`.

For Codex, the launcher still points a `proxy` provider at the local llmup base URL and local proxy key. It also generates a Codex model catalog and supported tool hints from the configured aliases' limits and surface, the same contract exposed as `llmup.surface` metadata. The Codex catalog contains every configured llmup alias. Codex UI may or may not display every catalog alias in a given release; llmup does not make that a hard promise. The hard contract is that the selected alias can start the Codex session.

For Claude Code, the launcher does not append an automatic native `--model default` argument because `default` is Claude Code's own special model setting. `main` is the normal selected alias for Claude Code. It projects the selected alias through `ANTHROPIC_CUSTOM_MODEL_OPTION` and `ANTHROPIC_MODEL`, and projects configured limits into Claude Code limit env when they are available.

If the llmup config contains official Claude family aliases, the launcher maps them through Claude Code's documented env variables: `ANTHROPIC_DEFAULT_HAIKU_MODEL=haiku`, `ANTHROPIC_DEFAULT_SONNET_MODEL=sonnet`, and `ANTHROPIC_DEFAULT_OPUS_MODEL=opus`. `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1` is not enabled by default.

These projected hints help the native clients present the selected model correctly. Protocol shaping and request enforcement stay in the proxy configuration and server-side conversion path.

The original home directory is not rewritten by default, so tools that run inside Codex or Claude Code can still find normal git, SSH, package-manager, and language-tool caches.

## Inheritance Boundary

The V1 launcher contract is narrow: the main Codex or Claude Code session goes through the local proxy, and native Codex subagents or Claude Code Task/subagent calls created inside the same client runtime inherit that proxy wiring.

V1 does not manage every process below the client. It does not guarantee proxy inheritance for Claude agent teams, bare `codex` or `claude` commands started from a shell, arbitrary shell child processes such as MCP servers, hooks, or scripts, or background tasks that continue after the launcher-managed parent exits.

## Native Arguments

After llmup consumes its own launcher controls, remaining arguments are passed through to the native client. That means common Codex and Claude Code workflows such as resume, help, MCP management, profiles, or permission modes stay native client behavior.

Managed profile projection owns llmup model selection. Use `--llmup-model <alias>` to choose another llmup alias. If you need to pass native model, provider, or catalog controls yourself while still using proxy plumbing, add `--llmup-no-profile-projection` and manage those native options explicitly.

For Codex, native `--profile` can load model, provider, or catalog overrides. Under managed profile projection, the launcher treats it as a conflict and fails fast. Use `--llmup-no-profile-projection` to keep proxy plumbing while managing native profile settings yourself, or `--llmup-no-proxy` for fully native Codex profile behavior.

Use `llmup-codex --llmup-help` or `llmup-claude --llmup-help` for launcher-specific help. Native client help remains owned by Codex CLI or Claude Code.

For commands that should not go through the proxy, such as login, native help, native configuration, or MCP management, use `llmup-codex --llmup-no-proxy -- <native args>` or `llmup-claude --llmup-no-proxy -- <native args>`. The launcher does not auto-detect native subcommands.

## Auth Boundary

`llmup-config` stores provider credentials for the proxy. The launchers give Codex or Claude Code only the local proxy credential needed to call the proxy. This keeps the provider key on the proxy side instead of putting it into the client environment.

For Claude Code, the launcher uses its gateway bearer-token authentication path so a managed `llmup-claude` session does not need a Claude browser login just to reach the local proxy. Native Claude login and account management commands should still be run with `--llmup-no-proxy`.

For manual auth modes, explicit base URLs, direct proxy startup, multi-endpoint YAML, Gemini configured with `format: openai-chat-completions`, or container/admin links, use [Advanced Usage](./advanced-usage.md).
