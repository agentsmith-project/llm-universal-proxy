import pathlib
import re
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
INSTALLER_COMMAND = (
    "curl --proto '=https' --tlsv1.2 -fsSL "
    "https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/install.sh | sh"
)
USER_ENTRY_DOCS = (
    "README.md",
    "README_CN.md",
    "docs/clients.md",
)
README_ENTRY_DOCS = (
    "README.md",
    "README_CN.md",
)
ADVANCED_DOC = "docs/advanced-usage.md"
CONFIG_DOC = "docs/configuration.md"
USER_TOOLING_PLAN_DOC = (
    "docs/engineering/pre-ga-user-tooling-install-config-agent-launch-plan.md"
)
ADVANCED_LINKS = (
    "docs/advanced-usage.md",
    "docs/clients.md",
    "docs/container.md",
    "docs/admin-dynamic-config.md",
)
USER_ENTRY_FORBIDDEN_SNIPPETS = (
    "git clone",
    "cargo build",
    "scripts/run_codex_proxy.sh",
    "scripts/run_claude_proxy.sh",
    "scripts/run_gemini_proxy.sh",
    "scripts/run_",
    "--config-source",
    "--env-file",
    "--proxy-base",
    "--dangerous-harness",
    "cat >",
    "provider_key:",
    "provider_key_env:",
    "model_aliases:",
    "data_auth:",
)


def normalized_whitespace(text: str) -> str:
    return " ".join(text.split())


class DocsHomepageContractTests(unittest.TestCase):
    def read_text(self, relative_path: str) -> str:
        return (REPO_ROOT / relative_path).read_text(encoding="utf-8")

    def assert_in_order(self, text: str, snippets: tuple[str, ...]) -> None:
        cursor = -1
        for snippet in snippets:
            next_index = text.find(snippet)
            self.assertNotEqual(next_index, -1, f"missing snippet: {snippet}")
            self.assertGreater(
                next_index,
                cursor,
                f"expected `{snippet}` after previous snippet",
            )
            cursor = next_index

    def assert_doc_mentions(self, relative_path: str, snippets: tuple[str, ...]) -> None:
        text = self.read_text(relative_path)
        for snippet in snippets:
            with self.subTest(path=relative_path, snippet=snippet):
                self.assertIn(snippet, text)

    def test_readmes_make_the_three_step_launcher_path_the_homepage_story(self):
        expectations = {
            "README.md": (
                "## Quick Start",
                INSTALLER_COMMAND,
                "llmup-config",
                "llmup-codex",
                "llmup-claude",
                "## Why There Is No `llmup` Command",
                "## Advanced",
            ),
            "README_CN.md": (
                "## 三步开始",
                INSTALLER_COMMAND,
                "llmup-config",
                "llmup-codex",
                "llmup-claude",
                "## 为什么没有独立的 `llmup` 命令",
                "## 高级用法",
            ),
        }

        for relative_path, snippets in expectations.items():
            with self.subTest(path=relative_path):
                self.assert_in_order(self.read_text(relative_path), snippets)

    def test_readmes_explain_no_standalone_llmup_command_is_intentional(self):
        self.assert_doc_mentions(
            "README.md",
            (
                "There is intentionally no standalone `llmup` command.",
                "`llmup-config`, `llmup-codex`, and `llmup-claude` are the user commands.",
                "`llm-universal-proxy --config` remains the advanced server entrypoint.",
            ),
        )
        self.assert_doc_mentions(
            "README_CN.md",
            (
                "第一版有意不提供独立的 `llmup` 主命令。",
                "普通用户只需要记住 `llmup-config`、`llmup-codex` 和 `llmup-claude`。",
                "`llm-universal-proxy --config` 仍然保留给高级服务端用法。",
            ),
        )

    def test_user_entry_docs_do_not_leak_developer_or_manual_wiring_paths(self):
        for relative_path in USER_ENTRY_DOCS:
            text = self.read_text(relative_path)
            for forbidden in USER_ENTRY_FORBIDDEN_SNIPPETS:
                with self.subTest(path=relative_path, forbidden=forbidden):
                    self.assertNotIn(forbidden, text)
            self.assertIsNone(
                re.search(r"(?m)^\s*export\s+\w*(?:API|KEY)\w*=", text),
                f"{relative_path} should not ask users to export API keys by hand",
            )

    def test_user_entry_docs_keep_provider_secrets_in_the_config_tool_story(self):
        for relative_path in README_ENTRY_DOCS:
            text = self.read_text(relative_path)
            with self.subTest(path=relative_path):
                self.assertIn("llmup-config", text)
                self.assertNotIn("REPLACE_WITH_YOUR", text)
                self.assertNotRegex(text, r"sk-(?:cp|ant|proj|live|test)-[A-Za-z0-9_-]+")

        self.assert_doc_mentions(
            "README.md",
            (
                "The real provider key is collected by `llmup-config` and kept in the local proxy configuration, not pasted into Codex or Claude Code.",
                "The launchers give the client a local proxy key and keep the upstream provider key on the proxy side.",
            ),
        )
        self.assert_doc_mentions(
            "README_CN.md",
            (
                "真实模型服务 Key 由 `llmup-config` 保存到本机代理配置里，不需要粘到 Codex CLI 或 Claude Code 里。",
                "launcher 只把本地代理密码交给客户端，真实 provider key 留在 proxy 侧。",
            ),
        )

    def test_user_entry_docs_explain_native_clients_are_external(self):
        self.assert_doc_mentions(
            "README.md",
            (
                "`llmup` does not install Codex CLI or Claude Code.",
                "Install the native client you plan to use first.",
            ),
        )
        self.assert_doc_mentions(
            "README_CN.md",
            (
                "`llmup` 不会自动安装 Codex CLI 或 Claude Code。",
                "请先安装你要使用的原生客户端。",
            ),
        )
        self.assert_doc_mentions(
            "docs/clients.md",
            (
                "`llmup` does not install Codex CLI or Claude Code.",
                "Install the native client you plan to use first.",
            ),
        )

    def test_readmes_keep_only_short_advanced_links(self):
        for relative_path in README_ENTRY_DOCS:
            text = self.read_text(relative_path)
            for link in ADVANCED_LINKS:
                with self.subTest(path=relative_path, link=link):
                    self.assertIn(link, text)
            self.assertLess(
                normalized_whitespace(text).count("llm-universal-proxy --config"),
                2,
                f"{relative_path} should only point to the advanced server entrypoint briefly",
            )

    def test_docs_index_points_to_launcher_and_advanced_user_docs(self):
        self.assert_doc_mentions(
            "docs/README.md",
            (
                "[clients.md](./clients.md)",
                "[advanced-usage.md](./advanced-usage.md)",
                "[container.md](./container.md)",
                "[admin-dynamic-config.md](./admin-dynamic-config.md)",
                "Launcher-managed Codex and Claude Code setup",
                "Manual proxy startup, multi-endpoint YAML, manual Codex/Claude wiring",
            ),
        )

    def test_clients_guide_is_launcher_managed_overview_not_manual_tutorial(self):
        text = self.read_text("docs/clients.md")

        for snippet in (
            "`llmup-codex`",
            "`llmup-claude`",
            "`~/.llmup-codex`",
            "`~/.llmup-claude`",
            "`CODEX_HOME`",
            "`CLAUDE_CONFIG_DIR`",
            "native Codex or Claude Code arguments",
            "[Advanced Usage](./advanced-usage.md)",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

        for forbidden in (
            "OPENAI_BASE_URL=",
            "ANTHROPIC_BASE_URL=",
            "OPENAI_API_KEY=",
            "ANTHROPIC_API_KEY=",
            "provider_key_env:",
            "Manual Wiring Without Wrappers",
            "```yaml",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, text)

    def test_clients_guide_keeps_codex_launcher_injection_fixed_not_live_surface_driven(
        self,
    ):
        text = self.read_text("docs/clients.md")

        for snippet in (
            "fixed minimal provider injection",
            "does not read live `llmup.surface` metadata",
            "native Codex client does not see live surface metadata",
            "Model identity, capability truth, and protocol shaping stay in the proxy configuration and server-side conversion path.",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

        for forbidden in (
            "launcher-generated provider hints use live `llmup.surface` metadata",
            "native client sees the capability shape",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, text)

    def test_clients_guide_documents_explicit_no_proxy_escape_hatch(self):
        text = self.read_text("docs/clients.md")

        for snippet in (
            "login, native help, native configuration, or MCP management",
            "`llmup-codex --llmup-no-proxy -- <native args>`",
            "`llmup-claude --llmup-no-proxy -- <native args>`",
            "The launcher does not auto-detect native subcommands.",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_configuration_doc_is_advanced_static_yaml_reference_not_quickstart(self):
        text = self.read_text(CONFIG_DOC)

        self.assert_in_order(
            text,
            (
                "# Configuration Guide",
                "Ordinary user path:",
                "install.sh",
                "llmup-config",
                "llmup-codex",
                "llmup-claude",
                "This page is the advanced static YAML and server reference.",
            ),
        )
        self.assertNotIn("## Quick Start", text)
        self.assertNotIn("export PRESET_", text)
        self.assertNotIn("scripts/run_", text)

    def test_user_tooling_plan_keeps_provider_neutral_first_run_and_cli_smoke_caveats(self):
        text = self.read_text(USER_TOOLING_PLAN_DOC)

        for snippet in (
            "首次配置示例必须保持 provider-neutral",
            "MiniMax 只能作为可替换的 OpenAI-compatible 示例",
            "模型服务地址，例如 `https://api.example.com/v1`",
            "模型名，例如 `provider-model-id`",
            "`llmup-codex --llmup-no-proxy -- <native args>`",
            "`llmup-claude --llmup-no-proxy -- <native args>`",
            "不要让 launcher 自动识别子命令",
            "不读取 live `llmup.surface` metadata",
            "模型、能力和协议转换真相仍由 proxy 配置和服务端转换承担",
            "Codex 和 Claude Code 的固定 `InjectionPrelude + NativeArgv` 都必须由真实 CLI smoke 保护",
            "如果不可行，优先改用位置无关的配置注入方式，不维护子命令表",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

        for forbidden in (
            "$MINIMAX_API_KEY",
            "https://api.minimaxi.com/v1",
            "https://api.minimax.io/v1",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, text)

    def test_advanced_usage_contains_the_manual_user_contract(self):
        text = self.read_text(ADVANCED_DOC)

        for snippet in (
            "llm-universal-proxy --config",
            "Manual Proxy Startup",
            "Multi-Endpoint YAML",
            "Manual Codex Wiring",
            "Manual Claude Wiring",
            "The provider key belongs to the proxy, not to the client.",
            "`data_auth.proxy_key` protects the local proxy",
            "`client_provider_key`",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "format: openai-completion",
            "[Admin and Dynamic Config](./admin-dynamic-config.md)",
            "[Container Guide](./container.md)",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

        for forbidden in (
            "GET /admin/state",
            "POST /admin/namespaces/:namespace/config",
            "PUT /admin/data-auth",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, text)

    def test_user_entry_docs_retain_bounded_compatibility_language(self):
        self.assert_doc_mentions(
            "README.md",
            (
                "maximum safe compatibility",
                "fail closed before the upstream call",
                "provider APIs and compatible endpoints",
            ),
        )
        self.assert_doc_mentions(
            "README_CN.md",
            (
                "最大安全兼容",
                "先在本地失败",
                "模型 API 或兼容 endpoint",
            ),
        )


if __name__ == "__main__":
    unittest.main()
