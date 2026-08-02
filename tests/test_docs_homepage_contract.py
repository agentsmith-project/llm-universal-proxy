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
AGENT_MODEL_METADATA_PLAN_DOC = (
    "docs/engineering/pre-ga-agent-launcher-model-capability-metadata-plan.md"
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

    def test_readmes_make_the_codex_setup_path_the_homepage_story(self):
        expectations = {
            "README.md": (
                "## Quick Start",
                INSTALLER_COMMAND,
                "codex-setup",
                "## Why a `llmup` Alias",
                "## Advanced",
            ),
            "README_CN.md": (
                "## 快速开始",
                INSTALLER_COMMAND,
                "codex-setup",
                "## 关于 `llmup` 别名",
                "## 高级用法",
            ),
        }

        for relative_path, snippets in expectations.items():
            with self.subTest(path=relative_path):
                self.assert_in_order(self.read_text(relative_path), snippets)

    def test_readmes_explain_the_llmup_alias(self):
        self.assert_doc_mentions(
            "README.md",
            (
                "The installer creates a `llmup` alias that points at the single `llm-universal-proxy` binary",
                "`llmup codex-setup` is the one user-facing command.",
                "`llm-universal-proxy --config` remains the advanced server entrypoint.",
            ),
        )
        self.assert_doc_mentions(
            "README_CN.md",
            (
                "安装脚本会创建一个 `llmup` 别名，指向唯一的 `llm-universal-proxy` 二进制",
                "所以面向用户的命令只有 `llmup codex-setup`。",
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

    def test_readmes_keep_provider_secrets_server_side(self):
        for relative_path in README_ENTRY_DOCS:
            text = self.read_text(relative_path)
            with self.subTest(path=relative_path):
                self.assertIn("codex-setup", text)
                self.assertNotIn("REPLACE_WITH_YOUR", text)
                self.assertNotRegex(text, r"sk-(?:cp|ant|proj|live|test)-[A-Za-z0-9_-]+")

        self.assert_doc_mentions(
            "README.md",
            (
                "The real provider key stays server-side; Codex only receives the local proxy key.",
            ),
        )
        self.assert_doc_mentions(
            "README_CN.md",
            (
                "真实 provider key 留在服务端，Codex 只拿到本地代理密钥。",
            ),
        )

    def test_readme_new_user_model_name_is_main_not_default(self):
        expected = {
            "README.md": (
                "The default local model name is `main`; pass it to `codex-setup --model` unless you configured another alias.",
            ),
            "README_CN.md": (
                "默认本地模型名是 `main`；除非你配置了别的别名，否则把它传给 `codex-setup --model`。",
            ),
        }

        for relative_path, snippets in expected.items():
            text = self.read_text(relative_path)
            for snippet in snippets:
                with self.subTest(path=relative_path, snippet=snippet):
                    self.assertIn(snippet, text)

            for forbidden in (
                "`default` as the model",
                "`default` model",
                "`default` alias",
                "--alias default",
                "--llmup-model default",
            ):
                with self.subTest(path=relative_path, forbidden=forbidden):
                    self.assertNotIn(forbidden, text)

    def test_readme_and_clients_use_explicit_protocol_names(self):
        for relative_path in ("README.md", "README_CN.md", "docs/clients.md"):
            text = self.read_text(relative_path)
            for snippet in (
                "`openai-chat-completions`",
                "`openai-responses`",
                "`anthropic-messages`",
            ):
                with self.subTest(path=relative_path, snippet=snippet):
                    self.assertIn(snippet, text)

            for forbidden in (
                "OpenAI-compatible",
                "OpenAI compatible",
                "OpenAI Chat Completions-compatible",
                "Anthropic Messages-compatible",
            ):
                with self.subTest(path=relative_path, forbidden=forbidden):
                    self.assertNotIn(forbidden, text)

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

    def test_docs_index_points_to_codex_setup_and_advanced_user_docs(self):
        self.assert_doc_mentions(
            "docs/README.md",
            (
                "[clients.md](./clients.md)",
                "[advanced-usage.md](./advanced-usage.md)",
                "[container.md](./container.md)",
                "[admin-dynamic-config.md](./admin-dynamic-config.md)",
                "`codex-setup` subcommand flow",
                "Manual proxy startup, multi-endpoint YAML, manual Codex/Claude wiring",
            ),
        )

    def test_clients_guide_is_codex_setup_overview_not_manual_tutorial(self):
        text = self.read_text("docs/clients.md")

        for snippet in (
            "codex-setup",
            "`llmup codex-setup`",
            "`~/.codex/llmup.config.toml`",
            "`~/.codex/agents/llmup-<model>.toml`",
            "`~/.codex/llmup/state.json`",
            "`$CODEX_HOME`",
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

    def test_configuration_doc_is_advanced_static_yaml_reference_not_quickstart(self):
        text = self.read_text(CONFIG_DOC)

        self.assert_in_order(
            text,
            (
                "# Configuration Guide",
                "Ordinary user path:",
                "install.sh",
                "codex-setup",
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
            "MiniMax 只能作为可替换的 OpenAI Chat Completions 兼容服务示例",
            "协议格式，可直接回车使用默认 `openai-chat-completions`",
            "模型服务地址，例如 `https://api.example.com/v1`",
            "模型名，例如 `provider-model-id`",
            "`llmup-codex --llmup-no-proxy -- <native args>`",
            "`llmup-claude --llmup-no-proxy -- <native args>`",
            "不要让 launcher 自动识别子命令",
            "不维护子命令表",
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
            "Advanced Model Limits",
            "structured alias `limits`",
            'main: "openai_chat:provider-model"',
            'fast: "openai_chat:provider-fast-model"',
            'sonnet: "anthropic_messages:claude-sonnet-like"',
            'opus: "anthropic_messages:claude-opus-like"',
            "Manual Codex Wiring",
            "Manual Claude Wiring",
            "The provider key belongs to the proxy, not to the client.",
            "`data_auth.proxy_key` protects the local proxy",
            "`client_provider_key`",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "format: openai-chat-completions",
            "[Admin and Dynamic Config](./admin-dynamic-config.md)",
            "[Container Guide](./container.md)",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

        for forbidden in (
            "GET /admin/state",
            "POST /admin/namespaces/:namespace/config",
            "PUT /admin/data-auth",
            'default: "',
            "--alias default",
            "llmup-config set-limits",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, text)

    def test_old_engineering_plans_no_longer_deliver_default_alias_as_the_default(self):
        for relative_path in (USER_TOOLING_PLAN_DOC, AGENT_MODEL_METADATA_PLAN_DOC):
            text = self.read_text(relative_path)
            for forbidden in (
                "本地模型名固定为 `default`",
                "默认 alias 是 `default`",
                "默认值为 `default`",
                "默认 alias `default`",
                "--model-alias default",
                "LLMUP_PROVIDER_DEFAULT_API_KEY",
                'default: "DEFAULT:<model-name>"',
                "`--llmup-model default`",
            ):
                with self.subTest(path=relative_path, forbidden=forbidden):
                    self.assertNotIn(forbidden, text)

        self.assertIn("本地模型名固定为 `main`", self.read_text(USER_TOOLING_PLAN_DOC))
        self.assertIn(
            "提供 `--llmup-model <alias>`，作为 llmup-managed alias 的唯一入口；默认值为 `main`。",
            self.read_text(AGENT_MODEL_METADATA_PLAN_DOC),
        )

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
