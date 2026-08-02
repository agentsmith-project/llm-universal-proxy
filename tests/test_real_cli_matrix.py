import importlib.util
import dataclasses
import io
import json
import os
import pathlib
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import textwrap
import unittest
from unittest import mock


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "real_cli_matrix.py"
DEFAULT_CONFIG_PATH = (
    REPO_ROOT
    / "scripts"
    / "fixtures"
    / "cli_matrix"
    / "default_proxy_test_matrix.yaml"
)
TOOL_IDENTITY_FIXTURE_PATH = (
    REPO_ROOT
    / "scripts"
    / "fixtures"
    / "cli_matrix"
    / "smoke"
    / "tool_identity_public_contract.json"
)
CLAUDE_OPENAI_MULTI_TURN_SHAPE_FIXTURE_PATH = (
    REPO_ROOT
    / "scripts"
    / "fixtures"
    / "cli_matrix"
    / "smoke"
    / "claude_openai_multi_turn_request_shape_contract"
    / "task.json"
)
LOCKED_TOOL_CONTRACT_SPEC_PATHS = (
    REPO_ROOT / "docs" / "DESIGN.md",
    REPO_ROOT / "docs" / "PRD.md",
    REPO_ROOT / "docs" / "CONSTITUTION.md",
    REPO_ROOT / "docs" / "protocol-baselines" / "capabilities" / "tools.md",
    REPO_ROOT / "docs" / "max-compat-design.md",
    REPO_ROOT / "docs" / "engineering" / "max-compat-development-plan.md",
)
LOCKED_TOOL_CONTRACT_LINES = (
    "The proxy must not rewrite the visible tool name supplied by the client.",
    "`__llmup_custom__*` is an internal transport artifact, not a public contract.",
    "`apply_patch` remains a public freeform tool on client-visible surfaces.",
)
README_DOC_ENTRY_SNIPPETS = (
    "[docs/clients.md](./docs/clients.md)",
    "[docs/advanced-usage.md](./docs/advanced-usage.md)",
    "[docs/container.md](./docs/container.md)",
    "[docs/admin-dynamic-config.md](./docs/admin-dynamic-config.md)",
)
README_FORBIDDEN_RESERVED_PREFIX_PUBLIC_CONTRACT_PATTERNS = (
    r"`__llmup_custom__[^`]*`\s+is\s+(?:an?\s+)?public contract",
    r"`__llmup_custom__[^`]*`\s+is\s+(?:an?\s+)?public tool(?: name)?",
)
FORBIDDEN_DANGEROUS_ARGS = (
    "--dangerously-bypass-approvals-and-sandbox",
    "--dangerously-skip-permissions",
    "--sandbox=false",
    "--yolo",
)
FORBIDDEN_LEGACY_TOOL_IDENTITY_LANGUAGE = {
    REPO_ROOT / "README.md": (
        "Important current limitation:",
        "The current Responses custom-tool bridge still uses reserved synthetic names such as `__llmup_custom__apply_patch` on some translated live paths.",
        "there is also a known current limitation: Responses custom tools may still be bridged with reserved proxy names visible to the upstream model.",
    ),
    REPO_ROOT / "docs" / "DESIGN.md": (
        "It is acceptable only as an internal or stateless fallback",
    ),
    REPO_ROOT / "docs" / "max-compat-design.md": (
        "The current live bridge for OpenAI Responses custom tools rewrites:",
        "the current name-based encoding is not valid as a live model-visible contract for agent clients.",
        "Current behavior is intentional.",
        "keep reserved-prefix bridge names only as legacy/stateless fallback machinery",
    ),
}
REQUIRED_TOOL_BRIDGE_DIRECTION_LANGUAGE = {
    REPO_ROOT / "docs" / "max-compat-design.md": (
        "The intended translated-path bridge preserves the stable visible tool name and carries bridge provenance in request-scoped translation context.",
    ),
    REPO_ROOT / "docs" / "engineering" / "max-compat-development-plan.md": (
        "Phase 0 and Phase 1 together define the intended translated-path bridge: preserve the stable visible tool name on live requests and carry bridge provenance in request-scoped translation context.",
    ),
}


def load_module():
    spec = importlib.util.spec_from_file_location("real_cli_matrix", SCRIPT_PATH)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def codex_catalog_probe(catalog_payload, timeout_secs=2):
    if shutil.which("codex") is None:
        raise unittest.SkipTest("codex binary is not available")

    with tempfile.TemporaryDirectory() as temp_dir:
        root = pathlib.Path(temp_dir)
        catalog_path = root / "catalog.json"
        catalog_path.write_text(
            json.dumps(catalog_payload, indent=2) + "\n",
            encoding="utf-8",
        )
        env = dict(os.environ)
        env["HOME"] = str(root / "home")
        command = [
            "codex",
            "exec",
            "reply with ok",
            "--model",
            "minimax-openai",
            "--ephemeral",
            "--json",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "-C",
            "/tmp",
            "-c",
            'model_provider="proxy"',
            "-c",
            'model_providers.proxy.name="Proxy"',
            "-c",
            'model_providers.proxy.base_url="http://127.0.0.1:1/openai/v1"',
            "-c",
            'model_providers.proxy.wire_api="responses"',
            "-c",
            f'model_catalog_json="{catalog_path}"',
            "-c",
            "tools.web_search=false",
        ]
        try:
            completed = subprocess.run(
                command,
                env=env,
                input="",
                text=True,
                capture_output=True,
                timeout=timeout_secs,
            )
            return completed.returncode, completed.stdout + completed.stderr, False
        except subprocess.TimeoutExpired as error:
            stdout = error.stdout.decode() if isinstance(error.stdout, bytes) else (error.stdout or "")
            stderr = error.stderr.decode() if isinstance(error.stderr, bytes) else (error.stderr or "")
            return 124, stdout + stderr, True


def make_target(
    module,
    *,
    name="minimax-anth",
    required=True,
    enabled=True,
    proxy_model=None,
    upstream_name="MINIMAX-ANTHROPIC",
    upstream_format="anthropic",
):
    return module.MatrixTarget(
        name=name,
        required=required,
        enabled=enabled,
        proxy_model=proxy_model or name,
        upstream_name=upstream_name,
        upstream_format=upstream_format,
        skip_reason=None,
    )


def make_fixture(
    module,
    *,
    fixture_id="smoke_pong",
    prompt="Reply with PONG",
    prompt_template=None,
    verifier=None,
    timeout_secs=5,
    requires_tool_loop=False,
):
    return module.TaskFixture(
        fixture_id=fixture_id,
        kind="smoke",
        prompt=prompt,
        prompt_template=prompt_template,
        verifier=verifier or {"type": "contains", "value": "PONG"},
        timeout_secs=timeout_secs,
        workspace_template=None,
        requires_tool_loop=requires_tool_loop,
    )


def make_expected_fail_closed_fixture(module):
    return make_fixture(
        module,
        fixture_id="negative_provider_state_resource",
        verifier={
            "type": "expected_fail_closed",
            "category": "provider_owned_state_resource",
            "reason": "provider-owned state/resource must fail closed",
            "required_all": ["provider-owned state"],
            "required_any": ["conversation", "previous_response_id"],
        },
    )


def make_context(module, client_name: str):
    return module.VerifierContext(client_name=client_name)


def preset_endpoint_env(**overrides):
    values = {
        "PRESET_ENDPOINT_API_KEY": "proxy-only-secret",
        "PRESET_OPENAI_ENDPOINT_BASE_URL": "https://openai-compatible.example/v1",
        "PRESET_ANTHROPIC_ENDPOINT_BASE_URL": "https://anthropic-compatible.example/v1",
        "PRESET_ENDPOINT_MODEL": "provider-configured-model",
    }
    values.update(overrides)
    return values


def write_preset_endpoint_env_file(path: pathlib.Path, **overrides) -> pathlib.Path:
    values = preset_endpoint_env(**overrides)
    path.write_text(
        "".join(f'export {key}="{value}"\n' for key, value in values.items()),
        encoding="utf-8",
    )
    return path


def make_case(module, *, client_name, target=None, fixture=None, case_id=None):
    target = target or make_target(module)
    fixture = fixture or make_fixture(module)
    return module.MatrixCase(
        client_name=client_name,
        target=target,
        fixture=fixture,
        case_id=case_id or f"{client_name}__{target.name}__{fixture.fixture_id}",
    )


class RealCliMatrixTests(unittest.TestCase):
    def fake_launch_plan_side_effect(self, module):
        def fake_launch_plan(**kwargs):
            client_name = kwargs["client_name"]
            env = {}
            if client_name == "codex":
                env = {
                    "CODEX_HOME": str(kwargs["client_home"] / ".codex"),
                    "OPENAI_API_KEY": kwargs["proxy_key"],
                    "OPENAI_BASE_URL": f"{kwargs['proxy_base']}/openai/v1",
                }
                argv = ["-m", kwargs["model"], *kwargs["native_args"]]
            elif client_name == "claude":
                env = {
                    "CLAUDE_CONFIG_DIR": str(kwargs["client_home"] / ".claude"),
                    "ANTHROPIC_AUTH_TOKEN": kwargs["proxy_key"],
                    "ANTHROPIC_BASE_URL": f"{kwargs['proxy_base']}/anthropic",
                    "ANTHROPIC_CUSTOM_MODEL_OPTION": kwargs["model"],
                    "ANTHROPIC_MODEL": kwargs["model"],
                }
                argv = [*kwargs["native_args"]]
            else:
                raise ValueError(f"unknown client: {client_name}")
            return module.ClientLaunchPlan(
                program=client_name,
                argv=argv,
                env=env,
                projection={"enabled": True, "profile": {"alias": kwargs["model"]}},
                artifacts={"codex_model_catalog": None},
            )

        return fake_launch_plan

    def assert_path_not_within(self, path: pathlib.Path, root: pathlib.Path) -> None:
        resolved_path = pathlib.Path(path).resolve()
        resolved_root = pathlib.Path(root).resolve()
        self.assertFalse(
            resolved_path == resolved_root or resolved_root in resolved_path.parents,
            f"expected {resolved_path} not to be within {resolved_root}",
        )

    def assert_path_uses_opaque_case_token(
        self,
        path: pathlib.Path,
        case,
    ) -> None:
        resolved_path = pathlib.Path(path).resolve()
        basename = resolved_path.name
        self.assertNotIn(case.case_id, str(resolved_path))
        self.assertNotIn(case.client_name, basename)
        self.assertNotIn(case.target.name, basename)
        self.assertNotIn(case.fixture.fixture_id, basename)

    def test_trace_tool_name_helpers_ignore_removed_native_gemini_schema(self):
        module = load_module()
        trace_value = {
            "tools": [{"functionDeclarations": [{"name": "legacy_gemini_tool"}]}],
            "toolConfig": {
                "functionCallingConfig": {
                    "allowedFunctionNames": ["legacy_gemini_tool"]
                }
            },
        }

        self.assertEqual(module._tool_names_from_trace_value(trace_value), [])
        self.assertEqual(module._tool_selector_names_from_trace_value(trace_value), [])
        self.assertEqual(module._trace_summary_tool_selector_names(trace_value), [])

        active_trace_value = {"tool_choice": {"allowed_tool_names": ["Edit"]}}
        self.assertEqual(
            module._trace_summary_tool_selector_names(active_trace_value), ["Edit"]
        )

    def test_load_fixtures_rejects_prompt_template_with_unsupported_placeholder(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            fixtures_root = pathlib.Path(temp_dir)
            (fixtures_root / "bad.json").write_text(
                json.dumps(
                    {
                        "id": "bad_prompt_template",
                        "kind": "smoke",
                        "prompt": "Fallback prompt",
                        "prompt_template": "Current target: {target_name}",
                        "verifier": {"type": "contains", "value": "PONG"},
                        "timeout_secs": 30,
                    }
                ),
                encoding="utf-8",
            )

            with self.assertRaisesRegex(
                ValueError,
                "unsupported placeholder .*target_name.*client_name",
            ):
                module.load_fixtures(fixtures_root)

    def test_load_fixtures_rejects_prompt_template_with_invalid_format_syntax(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            fixtures_root = pathlib.Path(temp_dir)
            (fixtures_root / "bad.json").write_text(
                json.dumps(
                    {
                        "id": "bad_prompt_template",
                        "kind": "smoke",
                        "prompt": "Fallback prompt",
                        "prompt_template": "Current client: {client_name",
                        "verifier": {"type": "contains", "value": "PONG"},
                        "timeout_secs": 30,
                    }
                ),
                encoding="utf-8",
            )

            with self.assertRaisesRegex(
                ValueError,
                "invalid prompt_template.*bad.json",
            ):
                module.load_fixtures(fixtures_root)

    def test_render_fixture_prompt_prefers_prompt_template_with_client_name(self):
        module = load_module()
        fixture = make_fixture(
            module,
            prompt="Generic prompt",
            prompt_template=(
                "Current client: {client_name}. "
                "Reply with only the exact public editing tool names visible here."
            ),
        )

        rendered = module.render_fixture_prompt(fixture, "claude")

        self.assertEqual(
            rendered,
            "Current client: claude. Reply with only the exact public editing tool names visible here.",
        )

    def test_load_fixtures_infers_tool_loop_from_workspace_capabilities(self):
        module = load_module()

        fixtures = {
            fixture.fixture_id: fixture
            for fixture in module.load_fixtures(DEFAULT_CONFIG_PATH.parent)
        }

        self.assertTrue(
            fixtures[
                "public_editing_tool_workspace_edit_contract"
            ].requires_tool_loop
        )
        self.assertTrue(fixtures["codex_observable_edit_contract"].requires_tool_loop)
        self.assertTrue(
            fixtures["codex_prework_signal_work_summary_contract"].requires_tool_loop
        )
        self.assertTrue(fixtures["python_bugfix"].requires_tool_loop)
        self.assertTrue(fixtures["rust_6502_cpu"].requires_tool_loop)
        self.assertFalse(fixtures["smoke_pong"].requires_tool_loop)
        self.assertFalse(fixtures["tool_identity_public_contract"].requires_tool_loop)
        self.assertTrue(
            fixtures[
                "claude_openai_multi_turn_request_shape_contract"
            ].requires_tool_loop
        )

    def test_claude_openai_multi_turn_shape_fixture_targets_chat_and_responses(
        self,
    ):
        module = load_module()

        def trace_contracts(verifier):
            if verifier.get("type") == "trace_request_contract":
                return [verifier]
            if verifier.get("type") != "all_of":
                return []
            contracts = []
            for nested in verifier.get("verifiers", []):
                if isinstance(nested, dict):
                    contracts.extend(trace_contracts(nested))
            return contracts

        self.assertTrue(CLAUDE_OPENAI_MULTI_TURN_SHAPE_FIXTURE_PATH.exists())
        fixtures = module.load_fixtures(DEFAULT_CONFIG_PATH.parent)
        shape_fixture_ids = {
            fixture.fixture_id
            for fixture in fixtures
            if "multi_turn_request_shape_contract" in fixture.fixture_id
        }
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )
        targets = module.resolve_matrix_targets(parsed, preset_endpoint_env())

        cases = module.expand_matrix(
            module.CLIENT_NAMES,
            targets,
            fixtures,
            phase="basic",
            skip_slow=True,
        )
        shape_case_ids = [
            case.case_id
            for case in cases
            if "multi_turn_request_shape_contract" in case.fixture.fixture_id
        ]

        fixtures_by_id = {fixture.fixture_id: fixture for fixture in fixtures}
        chat_fixture = fixtures_by_id["claude_openai_multi_turn_request_shape_contract"]
        self.assertIn(
            "claude_openai_multi_turn_request_shape_contract",
            shape_fixture_ids,
        )
        self.assertIn(
            "claude_openai_responses_multi_turn_request_shape_contract",
            shape_fixture_ids,
        )
        for fixture_id in shape_fixture_ids:
            self.assertEqual(fixtures_by_id[fixture_id].supported_clients, ("claude",))
        self.assertEqual(
            shape_case_ids,
            [
                "claude__preset-openai-compatible__claude_openai_multi_turn_request_shape_contract",
                "claude__preset-openai-responses-compatible__claude_openai_responses_multi_turn_request_shape_contract",
            ],
        )
        [chat_trace] = trace_contracts(chat_fixture.verifier)
        self.assertEqual(chat_trace["upstream_format"], "openai-chat-completions")
        self.assertEqual(chat_trace["request_index"], -1)
        self.assertTrue(chat_trace["require_response_for_each_request"])
        self.assertIn("upstream_summary.message_roles", chat_trace["required_request_fields"])
        self.assertIn("upstream_summary.messages_tail", chat_trace["required_request_fields"])
        self.assertIn(
            {
                "field": "upstream_summary.messages_tail",
                "contains": {"role": "tool"},
            },
            chat_trace["required_request_objects"],
        )

        responses_fixture = fixtures_by_id[
            "claude_openai_responses_multi_turn_request_shape_contract"
        ]
        [responses_trace] = trace_contracts(responses_fixture.verifier)
        self.assertEqual(responses_trace["upstream_format"], "openai-responses")
        self.assertEqual(responses_trace["request_index"], -1)
        self.assertTrue(responses_trace["require_response_for_each_request"])
        self.assertIn("upstream_summary.input_tail", responses_trace["required_request_fields"])
        self.assertIn(
            {
                "field": "upstream_summary.input_tail",
                "contains": {"type": "function_call_output"},
            },
            responses_trace["required_request_objects"],
        )

    def test_load_fixtures_infers_tool_loop_for_nested_workspace_verifier_without_manual_flag(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            fixtures_root = pathlib.Path(temp_dir)
            (fixtures_root / "nested.json").write_text(
                json.dumps(
                    {
                        "id": "nested_workspace_edit",
                        "kind": "smoke",
                        "prompt": "Fix calc.py",
                        "timeout_secs": 30,
                        "workspace_template": "workspace",
                        "verifier": {
                            "type": "all_of",
                            "verifiers": [
                                {"type": "contains", "value": "done"},
                                {
                                    "type": "python_source_and_output",
                                    "source": {
                                        "path": "calc.py",
                                        "function": "add",
                                        "args": ["a", "b"],
                                        "returns": {
                                            "kind": "binary_op",
                                            "operator": "+",
                                            "left": "a",
                                            "right": "b",
                                        },
                                    },
                                    "entrypoint": {
                                        "path": "main.py",
                                        "expect_stdout_contains": ["2 + 3 = 5"],
                                    },
                                },
                            ],
                        },
                    }
                ),
                encoding="utf-8",
            )

            [fixture] = module.load_fixtures(fixtures_root)

        self.assertTrue(fixture.requires_tool_loop)

    def test_default_proxy_binary_path_prefers_newer_debug_build(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            release_binary = root / "target" / "release" / "llm-universal-proxy"
            debug_binary = root / "target" / "debug" / "llm-universal-proxy"
            release_binary.parent.mkdir(parents=True, exist_ok=True)
            debug_binary.parent.mkdir(parents=True, exist_ok=True)
            release_binary.write_text("", encoding="utf-8")
            debug_binary.write_text("", encoding="utf-8")
            os.utime(release_binary, (100, 100))
            os.utime(debug_binary, (200, 200))

            resolved = module.default_proxy_binary_path(
                release_binary=release_binary,
                debug_binary=debug_binary,
            )

        self.assertEqual(resolved, debug_binary)

    def test_resolve_cli_args_uses_resolved_default_proxy_binary(self):
        module = load_module()

        with mock.patch.object(
            module,
            "default_proxy_binary_path",
            return_value=pathlib.Path("/tmp/fresh-proxy"),
        ):
            args = module.resolve_cli_args([])

        self.assertEqual(args.binary, "/tmp/fresh-proxy")

    def test_parse_dotenv_exports_reads_export_lines(self):
        module = load_module()

        parsed = module.parse_dotenv_exports(
            textwrap.dedent(
                """
                # comment
                export PRESET_ENDPOINT_API_KEY="real-key"
                LOCAL_QWEN_BASE_URL=http://127.0.0.1:9997/v1
                LOCAL_QWEN_MODEL='qwen3.5-9b-awq'
                """
            )
        )

        self.assertEqual(parsed["PRESET_ENDPOINT_API_KEY"], "real-key")
        self.assertEqual(parsed["LOCAL_QWEN_BASE_URL"], "http://127.0.0.1:9997/v1")
        self.assertEqual(parsed["LOCAL_QWEN_MODEL"], "qwen3.5-9b-awq")

    def test_parse_proxy_source_extracts_upstreams_and_aliases(self):
        module = load_module()

        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstream_timeout_secs: 120
                upstreams:
                  MINIMAX-ANTHROPIC:
                    api_root: "https://api.minimaxi.com/anthropic/v1"
                    format: anthropic
                    provider_key:
                      env: TEST_PROVIDER_API_KEY
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-chat-completions
                    provider_key:
                      env: TEST_PROVIDER_API_KEY
                model_aliases:
                  minimax-anth: "MINIMAX-ANTHROPIC:MiniMax-M2.7-highspeed"
                  minimax-openai: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                  claude-opus-4-6: "LOCAL-QWEN:qwen3.5-9b-awq"
                debug_trace:
                  path: /tmp/trace.jsonl
                  max_text_chars: 16384
                """
            )
        )

        self.assertEqual(parsed.listen, "127.0.0.1:18888")
        self.assertEqual(parsed.upstream_timeout_secs, 120)
        self.assertEqual(parsed.upstreams["MINIMAX-ANTHROPIC"]["format"], "anthropic")
        self.assertEqual(
            parsed.model_aliases["minimax-openai"],
            "MINIMAX-OPENAI:MiniMax-M2.7-highspeed",
        )
        self.assertEqual(parsed.debug_trace["path"], "/tmp/trace.jsonl")

    def test_parse_proxy_source_extracts_model_alias_limits(self):
        module = load_module()

        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-chat-completions
                    provider_key:
                      env: TEST_PROVIDER_API_KEY
                    limits:
                      context_window: 200000
                      max_output_tokens: 128000
                model_aliases:
                  minimax-openai:
                    target: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                    limits:
                      max_output_tokens: 64000
                """
            )
        )

        self.assertEqual(
            parsed.model_aliases["minimax-openai"],
            "MINIMAX-OPENAI:MiniMax-M2.7-highspeed",
        )
        self.assertEqual(parsed.upstream_limits["MINIMAX-OPENAI"].context_window, 200000)
        self.assertEqual(parsed.upstream_limits["MINIMAX-OPENAI"].max_output_tokens, 128000)
        self.assertEqual(
            parsed.model_alias_configs["minimax-openai"].limits.max_output_tokens,
            64000,
        )

    def test_parse_proxy_source_rejects_provider_key_env_with_migration_hint(self):
        module = load_module()

        with self.assertRaises(ValueError) as raised:
            module.parse_proxy_source(
                textwrap.dedent(
                    """
                    listen: 127.0.0.1:18888
                    upstreams:
                      LEGACY:
                        api_root: "https://api.example.com/v1"
                        format: openai-chat-completions
                        provider_key_env: TEST_PROVIDER_API_KEY
                    """
                )
            )

        message = str(raised.exception)
        self.assertIn("provider_key_env was removed", message)
        self.assertIn("provider_key: { env: ENV }", message)

    def test_parse_proxy_source_extracts_upstream_surface_defaults_and_alias_surface(self):
        module = load_module()

        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-chat-completions
                    provider_key:
                      env: TEST_PROVIDER_API_KEY
                    surface_defaults:
                      modalities:
                        input: ["text"]
                        output: ["text"]
                      tools:
                        supports_search: false
                        supports_view_image: false
                        apply_patch_transport: function
                        supports_parallel_calls: true
                model_aliases:
                  vision-openai:
                    target: "MINIMAX-OPENAI:MiniMax-Vision"
                    surface:
                      modalities:
                        input: ["text", "image"]
                        output: ["text"]
                      tools:
                        supports_search: true
                        supports_view_image: true
                        apply_patch_transport: freeform
                        supports_parallel_calls: false
                """
            )
        )

        self.assertEqual(
            parsed.upstream_surface_defaults["MINIMAX-OPENAI"].input_modalities,
            ("text",),
        )
        self.assertEqual(
            parsed.upstream_surface_defaults["MINIMAX-OPENAI"].output_modalities,
            ("text",),
        )
        self.assertFalse(
            parsed.upstream_surface_defaults["MINIMAX-OPENAI"].supports_search
        )
        self.assertFalse(
            parsed.upstream_surface_defaults["MINIMAX-OPENAI"].supports_view_image
        )
        self.assertEqual(
            parsed.upstream_surface_defaults["MINIMAX-OPENAI"].apply_patch_transport,
            "function",
        )
        self.assertTrue(
            parsed.upstream_surface_defaults[
                "MINIMAX-OPENAI"
            ].supports_parallel_calls
        )
        self.assertEqual(
            parsed.model_alias_configs["vision-openai"].surface.input_modalities,
            ("text", "image"),
        )
        self.assertEqual(
            parsed.model_alias_configs["vision-openai"].surface.output_modalities,
            ("text",),
        )
        self.assertTrue(
            parsed.model_alias_configs["vision-openai"].surface.supports_search
        )
        self.assertTrue(
            parsed.model_alias_configs["vision-openai"].surface.supports_view_image
        )
        self.assertEqual(
            parsed.model_alias_configs["vision-openai"].surface.apply_patch_transport,
            "freeform",
        )
        self.assertFalse(
            parsed.model_alias_configs["vision-openai"].surface.supports_parallel_calls
        )

    def test_resolve_matrix_targets_marks_qwen_optional_when_env_missing(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )

        targets = {
            target.name: target
            for target in module.resolve_matrix_targets(parsed, preset_endpoint_env())
        }

        self.assertNotIn("minimax-anth", targets)
        self.assertNotIn("minimax-openai", targets)
        self.assertTrue(targets["preset-anthropic-compatible"].required)
        self.assertTrue(targets["preset-openai-compatible"].required)
        self.assertFalse(targets["qwen-local"].required)
        self.assertFalse(targets["qwen-local"].enabled)
        self.assertIn("LOCAL_QWEN", targets["qwen-local"].skip_reason)
        self.assertEqual(
            targets["preset-openai-compatible"].limits.context_window,
            200000,
        )
        self.assertEqual(
            targets["preset-openai-compatible"].limits.max_output_tokens,
            128000,
        )
        self.assertEqual(
            targets["preset-openai-compatible"].codex_metadata.input_modalities,
            ("text",),
        )
        self.assertFalse(
            targets["preset-openai-compatible"].codex_metadata.supports_search_tool
        )

    def test_resolve_matrix_targets_hydrates_preset_upstream_model_from_dotenv(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )

        targets = {
            target.name: target
            for target in module.resolve_matrix_targets(
                parsed,
                preset_endpoint_env(PRESET_ENDPOINT_MODEL="provider-live-model"),
            )
        }

        self.assertEqual(
            targets["preset-openai-compatible"].upstream_model,
            "provider-live-model",
        )
        self.assertEqual(
            targets["preset-openai-responses-compatible"].upstream_model,
            "provider-live-model",
        )
        self.assertEqual(
            targets["preset-anthropic-compatible"].upstream_model,
            "provider-live-model",
        )

    def test_resolve_matrix_targets_exposes_upstream_format_from_config(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )

        targets = {
            target.name: target
            for target in module.resolve_matrix_targets(parsed, preset_endpoint_env())
        }

        self.assertEqual(
            targets["preset-anthropic-compatible"].upstream_format,
            "anthropic",
        )
        self.assertEqual(
            targets["preset-openai-compatible"].upstream_format,
            "openai-chat-completions",
        )
        self.assertEqual(
            targets["preset-openai-responses-compatible"].upstream_format,
            "openai-responses",
        )

        qwen_targets = {
            target.name: target
            for target in module.resolve_matrix_targets(
                parsed,
                preset_endpoint_env(
                    LOCAL_QWEN_BASE_URL="http://127.0.0.1:9997/v1",
                    LOCAL_QWEN_MODEL="qwen3.5-9b-awq",
                    LOCAL_QWEN_API_KEY="not-needed",
                ),
            )
        }

        self.assertEqual(qwen_targets["qwen-local"].upstream_format, "openai-chat-completions")

    def test_preset_trace_filter_keeps_real_provider_model_after_target_resolution(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )
        targets = {
            target.name: target
            for target in module.resolve_matrix_targets(
                parsed,
                preset_endpoint_env(PRESET_ENDPOINT_MODEL="provider-live-model"),
            )
        }
        case = make_case(
            module,
            client_name="codex",
            target=targets["preset-openai-compatible"],
            fixture=make_fixture(module),
        )

        filtered = module.filter_trace_entries_for_case(
            [
                {
                    "timestamp_ms": 1000,
                    "request_id": "req_case",
                    "phase": "request",
                    "path": "/openai/v1/responses",
                    "client_format": "openai-responses",
                    "client_model": "preset-openai-compatible",
                    "upstream_name": "PRESET-OPENAI-COMPATIBLE",
                    "upstream_model": "provider-live-model",
                },
                {
                    "timestamp_ms": 1000,
                    "request_id": "req_case",
                    "phase": "response",
                    "path": "/openai/v1/responses",
                    "upstream_name": "PRESET-OPENAI-COMPATIBLE",
                    "upstream_model": "provider-live-model",
                },
            ],
            case,
            started_ms=950,
            finished_ms=1050,
        )

        self.assertEqual([entry["request_id"] for entry in filtered], ["req_case", "req_case"])

    def test_resolve_matrix_targets_fails_fast_when_preset_endpoint_env_is_missing(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )

        with self.assertRaises(ValueError) as raised:
            module.resolve_matrix_targets(parsed, {})

        message = str(raised.exception)
        self.assertIn("PRESET_OPENAI_ENDPOINT_BASE_URL", message)
        self.assertIn("PRESET_ANTHROPIC_ENDPOINT_BASE_URL", message)
        self.assertIn("PRESET_ENDPOINT_MODEL", message)
        self.assertIn("PRESET_ENDPOINT_API_KEY", message)

    def test_resolve_matrix_targets_enables_qwen_when_env_present(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )

        targets = {
            target.name: target
            for target in module.resolve_matrix_targets(
                parsed,
                preset_endpoint_env(
                    LOCAL_QWEN_BASE_URL="http://127.0.0.1:9997/v1",
                    LOCAL_QWEN_MODEL="qwen3.5-9b-awq",
                    LOCAL_QWEN_API_KEY="not-needed",
                ),
            )
        }

        self.assertTrue(targets["qwen-local"].enabled)
        self.assertEqual(targets["qwen-local"].proxy_model, "qwen-local")
        self.assertEqual(targets["qwen-local"].upstream_name, "LOCAL-QWEN")

    def test_resolve_matrix_targets_skips_qwen_when_provider_key_env_is_missing(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )

        targets = {
            target.name: target
            for target in module.resolve_matrix_targets(
                parsed,
                preset_endpoint_env(
                    LOCAL_QWEN_BASE_URL="http://127.0.0.1:9997/v1",
                    LOCAL_QWEN_MODEL="qwen3.5-9b-awq",
                ),
            )
        }

        self.assertFalse(targets["qwen-local"].enabled)
        self.assertIn("LOCAL_QWEN_API_KEY", targets["qwen-local"].skip_reason)

    def test_build_runtime_config_overrides_listen_and_injects_qwen(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )

        rendered = module.build_runtime_config_text(
            parsed,
            preset_endpoint_env(
                LOCAL_QWEN_BASE_URL="http://127.0.0.1:9997/v1",
                LOCAL_QWEN_MODEL="qwen3.5-9b-awq",
                LOCAL_QWEN_API_KEY="not-needed",
            ),
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertIn("listen: 127.0.0.1:19999", rendered)
        self.assertIn("LOCAL-QWEN:", rendered)
        self.assertIn('qwen-local: "LOCAL-QWEN:qwen3.5-9b-awq"', rendered)
        self.assertIn("path: /tmp/cli-matrix-trace.jsonl", rendered)

    def test_build_runtime_config_hydrates_provider_neutral_preset_endpoint_from_dotenv(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )

        rendered = module.build_runtime_config_text(
            parsed,
            {
                "PRESET_ENDPOINT_API_KEY": "proxy-only-secret",
                "PRESET_OPENAI_ENDPOINT_BASE_URL": "https://openai-compatible.example/v1",
                "PRESET_ANTHROPIC_ENDPOINT_BASE_URL": "https://anthropic-compatible.example/v1",
                "PRESET_ENDPOINT_MODEL": "provider-configured-model",
            },
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertIn("PRESET-OPENAI-COMPATIBLE:", rendered)
        self.assertIn("api_root: https://openai-compatible.example/v1", rendered)
        self.assertIn("PRESET-OPENAI-RESPONSES-COMPATIBLE:", rendered)
        self.assertIn("PRESET-ANTHROPIC-COMPATIBLE:", rendered)
        self.assertIn("api_root: https://anthropic-compatible.example/v1", rendered)
        self.assertEqual(rendered.count("env: PRESET_ENDPOINT_API_KEY"), 3)
        self.assertIn(
            'preset-openai-compatible: "PRESET-OPENAI-COMPATIBLE:provider-configured-model"',
            rendered,
        )
        self.assertIn(
            'preset-openai-responses-compatible: "PRESET-OPENAI-RESPONSES-COMPATIBLE:provider-configured-model"',
            rendered,
        )
        self.assertIn(
            'preset-anthropic-compatible: "PRESET-ANTHROPIC-COMPATIBLE:provider-configured-model"',
            rendered,
        )
        self.assertNotIn("proxy-only-secret", rendered)
        self.assertNotIn("PRESET_ENDPOINT_MODEL", rendered)
        self.assertNotIn("api_root: PRESET_", rendered)
        self.assertNotIn("MINIMAX", rendered.upper())

    def test_build_runtime_config_fails_fast_when_preset_endpoint_env_is_missing(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )

        with self.assertRaises(ValueError) as raised:
            module.build_runtime_config_text(
                parsed,
                {},
                listen_host="127.0.0.1",
                listen_port=19999,
                trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
            )

        message = str(raised.exception)
        self.assertIn("PRESET_OPENAI_ENDPOINT_BASE_URL", message)
        self.assertIn("PRESET_ANTHROPIC_ENDPOINT_BASE_URL", message)
        self.assertIn("PRESET_ENDPOINT_MODEL", message)
        self.assertIn("PRESET_ENDPOINT_API_KEY", message)

    def test_build_runtime_config_injects_qwen_surface_defaults_for_live_profile_truth_chain(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )

        rendered = module.build_runtime_config_text(
            parsed,
            preset_endpoint_env(
                LOCAL_QWEN_BASE_URL="http://127.0.0.1:9997/v1",
                LOCAL_QWEN_MODEL="qwen3.5-9b-awq",
                LOCAL_QWEN_API_KEY="not-needed",
            ),
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )
        reparsed = module.parse_proxy_source(rendered)
        qwen_surface = reparsed.upstream_surface_defaults["LOCAL-QWEN"]
        qwen_metadata = module.resolve_codex_model_metadata(reparsed, "qwen-local")

        self.assertEqual(qwen_surface.input_modalities, ("text",))
        self.assertEqual(qwen_surface.output_modalities, ("text",))
        self.assertFalse(qwen_surface.supports_search)
        self.assertFalse(qwen_surface.supports_view_image)
        self.assertEqual(qwen_surface.apply_patch_transport, "freeform")
        self.assertEqual(qwen_metadata.input_modalities, ("text",))
        self.assertFalse(qwen_metadata.supports_search_tool)

    def test_build_runtime_config_does_not_render_local_qwen_env_secret(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-chat-completions
                    provider_key:
                      env: MINIMAX_API_KEY
                model_aliases:
                  minimax-openai: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                """
            )
        )

        rendered = module.build_runtime_config_text(
            parsed,
            {
                "LOCAL_QWEN_BASE_URL": "http://127.0.0.1:9997/v1",
                "LOCAL_QWEN_MODEL": "qwen3.5-9b-awq",
                "LOCAL_QWEN_API_KEY": "sk-local-secret-that-must-not-be-rendered",
            },
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertIn("env: LOCAL_QWEN_API_KEY", rendered)
        self.assertNotIn("sk-local-secret-that-must-not-be-rendered", rendered)

    def test_build_runtime_config_omits_local_qwen_when_provider_key_env_source_is_missing(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )

        rendered = module.build_runtime_config_text(
            parsed,
            preset_endpoint_env(
                LOCAL_QWEN_BASE_URL="http://127.0.0.1:9997/v1",
                LOCAL_QWEN_MODEL="qwen3.5-9b-awq",
            ),
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertNotIn("LOCAL-QWEN:", rendered)
        self.assertNotIn('qwen-local: "LOCAL-QWEN:', rendered)
        self.assertNotIn("env: LOCAL_QWEN_API_KEY", rendered)

    def test_build_runtime_config_preserves_structured_model_alias_limits(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-chat-completions
                    provider_key:
                      env: TEST_PROVIDER_API_KEY
                    limits:
                      context_window: 200000
                      max_output_tokens: 128000
                model_aliases:
                  minimax-openai:
                    target: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                    limits:
                      max_output_tokens: 64000
                debug_trace:
                  path: /tmp/trace.jsonl
                  max_text_chars: 16384
                """
            )
        )

        rendered = module.build_runtime_config_text(
            parsed,
            preset_endpoint_env(),
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertIn("minimax-openai:", rendered)
        self.assertIn('target: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"', rendered)
        self.assertIn("limits:", rendered)
        self.assertIn("context_window: 200000", rendered)
        self.assertIn("max_output_tokens: 64000", rendered)

    def test_build_runtime_config_serializes_upstream_surface_defaults_and_alias_surface(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-chat-completions
                    provider_key:
                      env: TEST_PROVIDER_API_KEY
                    surface_defaults:
                      modalities:
                        input: ["text"]
                        output: ["text"]
                      tools:
                        supports_search: false
                        supports_view_image: false
                        apply_patch_transport: function
                        supports_parallel_calls: true
                model_aliases:
                  vision-openai:
                    target: "MINIMAX-OPENAI:MiniMax-Vision"
                    surface:
                      modalities:
                        input: ["text", "image"]
                        output: ["text"]
                      tools:
                        supports_search: true
                        supports_view_image: true
                        apply_patch_transport: freeform
                        supports_parallel_calls: false
                """
            )
        )

        rendered = module.build_runtime_config_text(
            parsed,
            preset_endpoint_env(),
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertIn("surface_defaults:", rendered)
        self.assertIn('input: ["text"]', rendered)
        self.assertIn('output: ["text"]', rendered)
        self.assertIn("supports_search: false", rendered)
        self.assertIn("supports_view_image: false", rendered)
        self.assertIn("apply_patch_transport: function", rendered)
        self.assertIn("supports_parallel_calls: true", rendered)
        self.assertIn("surface:", rendered)
        self.assertIn('input: ["text", "image"]', rendered)
        self.assertIn('output: ["text"]', rendered)
        self.assertIn("supports_search: true", rendered)
        self.assertIn("supports_view_image: true", rendered)
        self.assertIn("apply_patch_transport: freeform", rendered)
        self.assertIn("supports_parallel_calls: false", rendered)

    def test_parse_and_render_round_trip_nested_proxy_objects_and_surface_fields(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                proxy:
                  url: http://corp-proxy.example:8080
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-chat-completions
                    provider_key:
                      env: TEST_PROVIDER_API_KEY
                    proxy:
                      url: http://upstream-proxy.example:8080
                    surface_defaults:
                      modalities:
                        input: ["text"]
                      tools:
                        supports_search: false
                        supports_view_image: false
                        apply_patch_transport: freeform
                        supports_parallel_calls: true
                model_aliases:
                  vision-openai:
                    target: "MINIMAX-OPENAI:MiniMax-Vision"
                    surface:
                      modalities:
                        input: ["text", "image"]
                      tools:
                        supports_search: true
                        supports_view_image: true
                        apply_patch_transport: freeform
                        supports_parallel_calls: false
                debug_trace:
                  path: /tmp/trace.jsonl
                """
            )
        )

        self.assertEqual(parsed.proxy["url"], "http://corp-proxy.example:8080")
        self.assertEqual(
            parsed.upstreams["MINIMAX-OPENAI"]["proxy"]["url"],
            "http://upstream-proxy.example:8080",
        )
        self.assertFalse(
            parsed.upstream_surface_defaults["MINIMAX-OPENAI"].supports_view_image
        )
        self.assertEqual(
            parsed.upstream_surface_defaults["MINIMAX-OPENAI"].apply_patch_transport,
            "freeform",
        )
        self.assertTrue(
            parsed.upstream_surface_defaults["MINIMAX-OPENAI"].supports_parallel_calls
        )
        self.assertTrue(
            parsed.model_alias_configs["vision-openai"].surface.supports_view_image
        )
        self.assertEqual(
            parsed.model_alias_configs["vision-openai"].surface.apply_patch_transport,
            "freeform",
        )
        self.assertFalse(
            parsed.model_alias_configs["vision-openai"].surface.supports_parallel_calls
        )

        rendered = module.build_runtime_config_text(
            parsed,
            {},
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )
        reparsed = module.parse_proxy_source(rendered)

        self.assertIn("proxy:", rendered)
        self.assertIn("url: http://corp-proxy.example:8080", rendered)
        self.assertIn("url: http://upstream-proxy.example:8080", rendered)
        self.assertIn("supports_view_image: false", rendered)
        self.assertIn("apply_patch_transport: freeform", rendered)
        self.assertIn("supports_parallel_calls: true", rendered)
        self.assertNotIn("codex:", rendered)
        self.assertEqual(reparsed.proxy["url"], "http://corp-proxy.example:8080")
        self.assertEqual(
            reparsed.upstreams["MINIMAX-OPENAI"]["proxy"]["url"],
            "http://upstream-proxy.example:8080",
        )
        self.assertEqual(
            reparsed.model_alias_configs["vision-openai"].surface.input_modalities,
            ("text", "image"),
        )

    def test_resolve_model_limits_inherits_upstream_defaults_and_alias_overrides(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-chat-completions
                    provider_key:
                      env: TEST_PROVIDER_API_KEY
                    limits:
                      context_window: 200000
                      max_output_tokens: 128000
                model_aliases:
                  minimax-openai:
                    target: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                    limits:
                      max_output_tokens: 64000
                """
            )
        )

        limits = module.resolve_model_limits(parsed, "minimax-openai")

        self.assertEqual(limits.context_window, 200000)
        self.assertEqual(limits.max_output_tokens, 64000)

    def test_resolve_model_limits_supports_direct_upstream_model_targets(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-chat-completions
                    provider_key:
                      env: TEST_PROVIDER_API_KEY
                    limits:
                      context_window: 200000
                      max_output_tokens: 128000
                model_aliases:
                  minimax-openai: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                """
            )
        )

        limits = module.resolve_model_limits(
            parsed, "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
        )

        self.assertEqual(limits.context_window, 200000)
        self.assertEqual(limits.max_output_tokens, 128000)

    def test_resolve_codex_model_metadata_defaults_proxy_models_to_text_only_and_search_disabled(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-chat-completions
                    provider_key:
                      env: TEST_PROVIDER_API_KEY
                    limits:
                      context_window: 200000
                model_aliases:
                  minimax-openai: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                """
            )
        )

        metadata = module.resolve_codex_model_metadata(parsed, "minimax-openai")

        self.assertEqual(metadata.input_modalities, ("text",))
        self.assertFalse(metadata.supports_search_tool)

    def test_resolve_codex_model_metadata_supports_surface_defaults_and_alias_surface(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-chat-completions
                    provider_key:
                      env: TEST_PROVIDER_API_KEY
                    surface_defaults:
                      modalities:
                        input: ["text"]
                      tools:
                        supports_search: false
                model_aliases:
                  vision-openai:
                    target: "MINIMAX-OPENAI:MiniMax-Vision"
                    surface:
                      modalities:
                        input: ["text", "image"]
                      tools:
                        supports_search: true
                """
            )
        )

        metadata = module.resolve_codex_model_metadata(parsed, "vision-openai")

        self.assertEqual(metadata.input_modalities, ("text", "image"))
        self.assertTrue(metadata.supports_search_tool)

    def test_build_runtime_config_omits_local_qwen_aliases_when_env_missing(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            DEFAULT_CONFIG_PATH.read_text(encoding="utf-8")
        )

        rendered = module.build_runtime_config_text(
            parsed,
            preset_endpoint_env(),
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertNotIn("LOCAL-QWEN:", rendered)
        self.assertNotIn('qwen-local: "LOCAL-QWEN:', rendered)
        self.assertNotIn('claude-opus-4-6: "LOCAL-QWEN:', rendered)

    def test_python_harness_no_longer_owns_codex_projection_helpers(self):
        module = load_module()
        forbidden_names = (
            "default_auto_compact_token_limit",
            "build_codex_model_catalog",
            "build_codex_catalog_args",
            "build_codex_proxy_provider_args",
        )

        for script_path in (
            module.REPO_ROOT / "scripts" / "real_cli_matrix.py",
            module.REPO_ROOT / "scripts" / "interactive_cli.py",
        ):
            source = script_path.read_text(encoding="utf-8")
            for name in forbidden_names:
                with self.subTest(script=script_path.name, name=name):
                    self.assertNotRegex(source, rf"\bdef\s+{name}\b")
                    self.assertNotRegex(source, rf"\b{name}\(")

    def test_matrix_native_args_leave_model_projection_to_rust_plan(self):
        module = load_module()
        fixture = make_fixture(module, prompt="Reply with PONG")
        workspace = pathlib.Path("/tmp/workspace").resolve()

        codex_args = module.build_matrix_native_args(
            "codex",
            fixture,
            workspace,
        )
        claude_args = module.build_matrix_native_args(
            "claude",
            fixture,
            workspace,
        )

        self.assertEqual(
            codex_args,
            [
                "exec",
                "Reply with PONG",
                "--ephemeral",
                "--json",
                "--skip-git-repo-check",
                "-C",
                str(workspace),
            ],
        )
        self.assertNotIn("--model", codex_args)
        self.assertNotIn("-m", codex_args)
        self.assertEqual(
            claude_args,
            [
                "--bare",
                "--print",
                "--output-format",
                "text",
                "--setting-sources",
                "user",
                "--no-session-persistence",
                "--add-dir",
                str(workspace),
            ],
        )
        self.assertNotIn("--model", claude_args)

    def test_build_client_launch_plan_uses_llmup_alias_for_universal_proxy_binary(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            binary = root / "llm-universal-proxy"
            config_path = root / "config.yaml"
            env_file_path = root / "secrets.env"
            artifact_dir = root / "artifacts"
            client_home = root / "client-home"
            record_path = root / "launcher-record.json"
            config_path.write_text("model_aliases: {}\n", encoding="utf-8")
            env_file_path.write_text(
                "PROVIDER_API_KEY=provider-secret\n",
                encoding="utf-8",
            )
            binary.write_text(
                f"""#!/usr/bin/env python3
import json
import os
import pathlib
import sys

record_path = pathlib.Path({json.dumps(str(record_path))})
record = {{
    "argv0": pathlib.Path(sys.argv[0]).name,
    "args": sys.argv[1:],
    "env": {{
        key: os.environ.get(key)
        for key in (
            "LLMUP_INTERNAL_LAUNCH_PLAN",
            "LLMUP_HOME",
            "LLMUP_CODEX_HOME",
            "LLMUP_CLAUDE_CONFIG_DIR",
        )
    }},
}}
record_path.write_text(json.dumps(record, indent=2) + "\\n", encoding="utf-8")
print(json.dumps({{
    "schema_version": 1,
    "agent": "codex",
    "program": "codex",
    "argv": [
        "-c",
        "model_catalog_json=\\\"/tmp/catalog.json\\\"",
        "-c",
        "tools.web_search=false",
        "-m",
        "preset-chat",
        "exec",
        "Reply with PONG",
    ],
    "env": {{
        "CODEX_HOME": str({json.dumps(str(client_home / ".codex"))}),
        "OPENAI_API_KEY": "matrix-proxy-key",
    }},
    "projection": {{"enabled": True, "profile": {{"alias": "preset-chat"}}}},
    "artifacts": {{"codex_model_catalog": "/tmp/catalog.json"}},
}}))
""",
                encoding="utf-8",
            )
            binary.chmod(0o755)

            plan = module.build_client_launch_plan(
                client_name="codex",
                proxy_binary=binary,
                config_path=config_path,
                env_file_path=env_file_path,
                proxy_base="http://127.0.0.1:18888/",
                proxy_key="matrix-proxy-key",
                artifact_dir=artifact_dir,
                client_home=client_home,
                model="preset-chat",
                native_args=["exec", "Reply with PONG"],
                base_env={"PATH": os.environ.get("PATH", ""), "HOME": "/home/user"},
            )
            record = json.loads(record_path.read_text(encoding="utf-8"))

        self.assertEqual(record["argv0"], "llmup-codex")
        self.assertEqual(record["env"]["LLMUP_INTERNAL_LAUNCH_PLAN"], "1")
        self.assertEqual(record["env"]["LLMUP_CODEX_HOME"], str(client_home / ".codex"))
        self.assertIn("--llmup-internal-launch-plan-json", record["args"])
        self.assertIn("--llmup-model", record["args"])
        self.assertIn("preset-chat", record["args"])
        self.assertEqual(plan.program, "codex")
        self.assertIn("model_catalog_json", " ".join(plan.argv))
        self.assertIn("tools.web_search=false", plan.argv)
        self.assertIn("-m", plan.argv)
        self.assertNotIn("provider-secret", json.dumps(dataclasses.asdict(plan)))

    def test_fetch_live_model_profile_reads_llmup_surface_for_direct_upstream_model(self):
        module = load_module()
        payload = {
            "id": "MINIMAX-OPENAI:MiniMax-Vision",
            "llmup": {
                "upstream_name": "MINIMAX-OPENAI",
                "upstream_model": "MiniMax-Vision",
                "surface": {
                    "limits": {
                        "context_window": 200000,
                        "max_output_tokens": 128000,
                    },
                    "modalities": {"input": ["text", "image"]},
                    "tools": {
                        "supports_search": True,
                        "supports_view_image": True,
                        "apply_patch_transport": "function",
                        "supports_parallel_calls": True,
                    },
                },
            },
        }

        with mock.patch.object(
            module,
            "http_get_json",
            return_value=payload,
        ) as http_get_json:
            profile = module.fetch_live_model_profile(
                "http://127.0.0.1:18888",
                "MINIMAX-OPENAI:MiniMax-Vision",
                proxy_key="profile-proxy-key",
            )

        self.assertEqual(http_get_json.call_args.args[0], "http://127.0.0.1:18888/openai/v1/models/MINIMAX-OPENAI:MiniMax-Vision")
        self.assertEqual(http_get_json.call_args.kwargs["bearer_token"], "profile-proxy-key")
        self.assertEqual(profile.limits.context_window, 200000)
        self.assertEqual(profile.limits.max_output_tokens, 128000)
        self.assertEqual(profile.codex_metadata.input_modalities, ("text", "image"))
        self.assertTrue(profile.codex_metadata.supports_search_tool)
        self.assertTrue(profile.codex_metadata.supports_view_image)
        self.assertEqual(profile.codex_metadata.apply_patch_tool_type, "freeform")
        self.assertTrue(profile.codex_metadata.supports_parallel_tool_calls)

    def test_fetch_live_model_profile_rejects_legacy_proxec_payloads(self):
        module = load_module()

        with mock.patch.object(
            module,
            "http_get_json",
            return_value={"proxec": {"surface": {"limits": {"context_window": 1}}}},
        ):
            with self.assertRaisesRegex(RuntimeError, "llmup"):
                module.fetch_live_model_profile(
                    "http://127.0.0.1:18888",
                    "minimax-openai",
                    proxy_key=module.DEFAULT_PROXY_KEY,
                )

    def test_fetch_live_model_profile_requires_llmup_surface_as_the_live_truth_source(self):
        module = load_module()

        with mock.patch.object(
            module,
            "http_get_json",
            return_value={
                "id": "minimax-openai",
                "llmup": {
                    "limits": {
                        "context_window": 200000,
                        "max_output_tokens": 128000,
                    },
                },
            },
        ):
            with self.assertRaisesRegex(RuntimeError, "llmup\\.surface"):
                module.fetch_live_model_profile(
                    "http://127.0.0.1:18888",
                    "minimax-openai",
                    proxy_key=module.DEFAULT_PROXY_KEY,
                )

    def test_fetch_live_model_profile_rejects_missing_critical_surface_fields(self):
        module = load_module()

        with mock.patch.object(
            module,
            "http_get_json",
            return_value={
                "id": "minimax-openai",
                "llmup": {
                    "surface": {
                        "modalities": {},
                        "tools": {
                            "supports_view_image": True,
                            "apply_patch_transport": "function",
                            "supports_parallel_calls": True,
                        },
                    },
                },
            },
        ):
            with self.assertRaisesRegex(
                RuntimeError,
                "llmup\\.surface\\.modalities\\.input.*llmup\\.surface\\.tools\\.supports_search",
            ):
                module.fetch_live_model_profile(
                    "http://127.0.0.1:18888",
                    "minimax-openai",
                    proxy_key=module.DEFAULT_PROXY_KEY,
                )

    def test_refresh_target_model_profiles_uses_live_models_for_enabled_targets(self):
        module = load_module()
        enabled_target = make_target(
            module,
            name="vision-openai",
            proxy_model="MINIMAX-OPENAI:MiniMax-Vision",
            upstream_name="MINIMAX-OPENAI",
        )
        disabled_target = make_target(
            module,
            name="disabled-openai",
            enabled=False,
            proxy_model="disabled-openai",
        )
        live_profile = module.LiveModelProfile(
            limits=module.ModelLimits(
                context_window=200000,
                max_output_tokens=128000,
            ),
            codex_metadata=module.CodexModelMetadata(
                input_modalities=("text", "image"),
                supports_search_tool=True,
                supports_view_image=True,
                apply_patch_tool_type="freeform",
                supports_parallel_tool_calls=True,
            ),
        )

        with mock.patch.object(
            module,
            "fetch_live_model_profile",
            return_value=live_profile,
        ) as fetch_live_model_profile:
            module.refresh_target_model_profiles(
                "http://127.0.0.1:18888",
                [enabled_target, disabled_target],
                proxy_key="profile-proxy-key",
            )

        fetch_live_model_profile.assert_called_once_with(
            "http://127.0.0.1:18888",
            "MINIMAX-OPENAI:MiniMax-Vision",
            proxy_key="profile-proxy-key",
        )
        self.assertEqual(enabled_target.limits, live_profile.limits)
        self.assertEqual(enabled_target.codex_metadata, live_profile.codex_metadata)
        self.assertIsNone(disabled_target.limits)
        self.assertIsNone(disabled_target.codex_metadata)

    def test_real_codex_rejects_previous_minimal_catalog_shape(self):
        _module = load_module()

        returncode, output, timed_out = codex_catalog_probe(
            {
                "models": [
                    {
                        "slug": "minimax-openai",
                        "display_name": "minimax-openai",
                        "context_window": 200000,
                        "auto_compact_token_limit": 170000,
                        "input_modalities": ["text"],
                        "supports_search_tool": False,
                    }
                ]
            }
        )

        self.assertFalse(timed_out)
        self.assertEqual(returncode, 1)
        self.assertIn("failed to parse model_catalog_json", output)
        self.assertIn("as JSON", output)

    def test_real_codex_accepts_rust_launch_plan_generated_catalog(self):
        # The argv[0] launcher (`llmup-codex`) and its
        # `--llmup-internal-launch-plan-json` protocol were removed when the
        # client-config tools were deprecated in favor of the `codex-setup`
        # subcommand. This legacy matrix case exercised the deleted launcher
        # against the real proxy binary, so it is kept as an explicit skip
        # rather than dropped, to document the superseded coverage.
        self.skipTest(
            "launcher --llmup-internal-launch-plan-json protocol removed; "
            "superseded by the codex-setup subcommand"
        )
        module = load_module()
        if shutil.which("codex") is None:
            self.skipTest("codex binary is not available")
        proxy_binary = module.default_proxy_binary_path()
        if not proxy_binary.exists():
            self.skipTest(f"proxy binary is not available: {proxy_binary}")

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            config_path = root / "config.yaml"
            env_file_path = root / "secrets.env"
            artifact_dir = root / "artifacts"
            client_home = root / "client-home"
            config_path.write_text(
                textwrap.dedent(
                    """
                    listen: 127.0.0.1:18888
                    upstreams:
                      DEFAULT:
                        api_root: https://api.example.com/v1
                        format: openai-responses
                        provider_key:
                          env: LLMUP_PROVIDER_DEFAULT_API_KEY
                        limits:
                          context_window: 200000
                          max_output_tokens: 128000
                        surface_defaults:
                          modalities:
                            input: ["text"]
                            output: ["text"]
                          tools:
                            supports_search: false
                            supports_view_image: false
                            apply_patch_transport: freeform
                            supports_parallel_calls: false
                    model_aliases:
                      default: DEFAULT:test-model
                    """
                ).lstrip(),
                encoding="utf-8",
            )
            env_file_path.write_text(
                "LLMUP_PROVIDER_DEFAULT_API_KEY=provider-secret\n",
                encoding="utf-8",
            )

            plan = module.build_client_launch_plan(
                client_name="codex",
                proxy_binary=proxy_binary,
                config_path=config_path,
                env_file_path=env_file_path,
                proxy_base="http://127.0.0.1:1",
                proxy_key="matrix-proxy-key",
                artifact_dir=artifact_dir,
                client_home=client_home,
                model="default",
                native_args=[],
                base_env={"PATH": os.environ.get("PATH", ""), "HOME": str(root / "home")},
            )
            catalog_path = pathlib.Path(plan.artifacts["codex_model_catalog"])
            catalog_exists = catalog_path.exists()
            (root / "codex-debug-home" / ".codex").mkdir(parents=True)
            env = dict(os.environ)
            env["HOME"] = str(root / "codex-debug-home")
            env["CODEX_HOME"] = str(root / "codex-debug-home" / ".codex")
            completed = subprocess.run(
                [
                    "codex",
                    "debug",
                    "models",
                    "-c",
                    f'model_catalog_json="{catalog_path}"',
                ],
                env=env,
                text=True,
                capture_output=True,
                timeout=5,
                check=False,
            )

        self.assertIn(f'model_catalog_json="{catalog_path}"', plan.argv)
        self.assertTrue(catalog_exists)
        self.assertEqual(
            completed.returncode,
            0,
            msg=completed.stderr or completed.stdout,
        )
        catalog = json.loads(completed.stdout)
        models = catalog["models"] if isinstance(catalog, dict) else catalog
        default_model = next(
            model for model in models if model.get("slug") == "default"
        )
        self.assertEqual(default_model["context_window"], 200000)
        self.assertEqual(default_model["auto_compact_token_limit"], 61200)
        self.assertEqual(default_model["input_modalities"], ["text"])
        self.assertFalse(default_model["supports_search_tool"])

    def test_resolve_codex_model_metadata_uses_surface_defaults_and_alias_surface(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-chat-completions
                    provider_key:
                      env: TEST_PROVIDER_API_KEY
                    limits:
                      context_window: 200000
                    surface_defaults:
                      modalities:
                        input: ["text"]
                      tools:
                        supports_search: false
                        supports_view_image: false
                        apply_patch_transport: function
                        supports_parallel_calls: true
                model_aliases:
                  vision-openai:
                    target: "MINIMAX-OPENAI:MiniMax-Vision"
                    surface:
                      modalities:
                        input: ["text", "image"]
                      tools:
                        supports_search: true
                        supports_view_image: true
                        apply_patch_transport: freeform
                        supports_parallel_calls: false
                """
            )
        )
        model_limits = module.resolve_model_limits(parsed, "vision-openai")
        codex_metadata = module.resolve_codex_model_metadata(parsed, "vision-openai")

        self.assertEqual(codex_metadata.input_modalities, ("text", "image"))
        self.assertTrue(codex_metadata.supports_search_tool)
        self.assertTrue(codex_metadata.supports_view_image)
        self.assertEqual(codex_metadata.apply_patch_tool_type, "freeform")
        self.assertFalse(codex_metadata.supports_parallel_tool_calls)

    def test_parse_proxy_source_rejects_alias_legacy_codex_schema(self):
        module = load_module()

        with self.assertRaisesRegex(
            ValueError,
            "legacy codex: schema.*model_aliases\\.vision-openai.*use limits/surface",
        ):
            module.parse_proxy_source(
                textwrap.dedent(
                    """
                    listen: 127.0.0.1:18888
                    upstreams:
                      MINIMAX-OPENAI:
                        api_root: "https://api.minimaxi.com/v1"
                        format: openai-chat-completions
                        provider_key:
                          env: TEST_PROVIDER_API_KEY
                    model_aliases:
                      vision-openai:
                        target: "MINIMAX-OPENAI:MiniMax-Vision"
                        codex:
                          supports_search_tool: true
                    """
                )
            )

    def test_parse_proxy_source_rejects_upstream_legacy_codex_schema(self):
        module = load_module()

        with self.assertRaisesRegex(
            ValueError,
            "legacy codex: schema.*upstreams\\.MINIMAX-OPENAI.*use limits/surface",
        ):
            module.parse_proxy_source(
                textwrap.dedent(
                    """
                    listen: 127.0.0.1:18888
                    upstreams:
                      MINIMAX-OPENAI:
                        api_root: "https://api.minimaxi.com/v1"
                        format: openai-chat-completions
                        provider_key:
                          env: TEST_PROVIDER_API_KEY
                        codex:
                          input_modalities: ["text", "image"]
                          supports_search_tool: true
                    """
                )
            )

    def test_build_client_command_uses_rust_launch_plan_command(self):
        module = load_module()
        fixture = make_fixture(module)
        plan = module.ClientLaunchPlan(
            program="codex",
            argv=[
                "-c",
                'model_provider="proxy"',
                "-c",
                'model_catalog_json="/tmp/catalog.json"',
                "-c",
                "tools.web_search=false",
                "-m",
                "preset-chat",
                "exec",
                "Reply with PONG",
                "--ephemeral",
                "--json",
                "--skip-git-repo-check",
                "-C",
                "/tmp/workspace",
            ],
            env={"OPENAI_API_KEY": "matrix-proxy-key"},
            projection={"enabled": True},
            artifacts={"codex_model_catalog": "/tmp/catalog.json"},
        )

        command = module.build_client_command(
            "codex",
            "http://127.0.0.1:18888",
            make_target(module, name="preset-chat", proxy_model="preset-chat"),
            fixture,
            pathlib.Path("/tmp/workspace").resolve(),
            launch_plan=plan,
        )

        self.assertEqual(command, ["codex", *plan.argv])
        self.assertIn('model_catalog_json="/tmp/catalog.json"', command)
        self.assertIn("tools.web_search=false", command)
        self.assertIn("-m", command)
        self.assertNotIn("--model", command)

    def test_build_client_env_is_base_plus_rust_launch_plan_env(self):
        module = load_module()
        plan = module.ClientLaunchPlan(
            program="claude",
            argv=["--bare"],
            env={
                "CLAUDE_CONFIG_DIR": "/tmp/claude-config",
                "ANTHROPIC_AUTH_TOKEN": "matrix-proxy-key",
                "ANTHROPIC_BASE_URL": "http://127.0.0.1:18888/anthropic",
                "ANTHROPIC_CUSTOM_MODEL_OPTION": "preset-anthropic-compatible",
                "ANTHROPIC_MODEL": "preset-anthropic-compatible",
            },
            projection={"enabled": True},
            artifacts={"codex_model_catalog": None},
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            env = module.build_client_env(
                "claude",
                {
                    "PATH": "/usr/bin",
                    "HOME": "/home/user",
                    "ANTHROPIC_API_KEY": "host-anthropic-key",
                    "PRESET_ENDPOINT_API_KEY": "provider-secret",
                },
                "http://127.0.0.1:18888",
                pathlib.Path(temp_dir) / "claude-home",
                launch_plan=plan,
            )

        self.assertEqual(env["PATH"], "/usr/bin")
        self.assertNotEqual(env["HOME"], "/home/user")
        self.assertEqual(env["ANTHROPIC_AUTH_TOKEN"], "matrix-proxy-key")
        self.assertNotIn("ANTHROPIC_API_KEY", env)
        self.assertEqual(
            env["ANTHROPIC_CUSTOM_MODEL_OPTION"],
            "preset-anthropic-compatible",
        )
        self.assertEqual(env["ANTHROPIC_MODEL"], "preset-anthropic-compatible")
        self.assertNotIn("PRESET_ENDPOINT_API_KEY", env)

    def test_run_matrix_case_uses_rust_launch_plan_for_command_and_env(self):
        module = load_module()
        target = make_target(module, name="preset-chat", proxy_model="preset-chat")
        fixture = make_fixture(module, prompt="Reply with PONG")
        case = make_case(module, client_name="codex", target=target, fixture=fixture)
        captured = {}

        def fake_launch_plan(**kwargs):
            native_args = list(kwargs["native_args"])
            captured["native_args"] = native_args
            captured["model"] = kwargs["model"]
            captured["client_home"] = kwargs["client_home"]
            return module.ClientLaunchPlan(
                program="codex",
                argv=[
                    "-c",
                    'model_catalog_json="/tmp/rust-catalog.json"',
                    "-c",
                    "tools.web_search=false",
                    "-m",
                    kwargs["model"],
                    *native_args,
                ],
                env={
                    "CODEX_HOME": str(kwargs["client_home"] / ".codex"),
                    "OPENAI_API_KEY": kwargs["proxy_key"],
                    "OPENAI_BASE_URL": f"{kwargs['proxy_base']}/openai/v1",
                },
                projection={"enabled": True, "profile": {"alias": kwargs["model"]}},
                artifacts={"codex_model_catalog": "/tmp/rust-catalog.json"},
            )

        with tempfile.TemporaryDirectory() as temp_dir, mock.patch.object(
            module,
            "build_client_launch_plan",
            side_effect=fake_launch_plan,
        ) as build_client_launch_plan, mock.patch.object(
            module.subprocess,
            "run",
            return_value=subprocess.CompletedProcess(
                args=["codex"],
                returncode=0,
                stdout="PONG\n",
                stderr="",
            ),
        ) as run_command:
            report_dir = pathlib.Path(temp_dir)
            result = module.run_matrix_case(
                case,
                "http://127.0.0.1:18888",
                report_dir,
                {
                    "PATH": "/usr/bin",
                    "HOME": "/home/user",
                    "PRESET_ENDPOINT_API_KEY": "provider-secret",
                    module.PROXY_KEY_ENV: "matrix-proxy-key",
                },
                proxy_binary=pathlib.Path("/tmp/llm-universal-proxy"),
                config_path=pathlib.Path("/tmp/config.yaml"),
                env_file_path=pathlib.Path("/tmp/secrets.env"),
            )

        build_client_launch_plan.assert_called_once()
        self.assertEqual(result["status"], "passed")
        self.assertEqual(captured["model"], "preset-chat")
        self.assertEqual(
            captured["native_args"],
            [
                "exec",
                "Reply with PONG",
                "--ephemeral",
                "--json",
                "--skip-git-repo-check",
                "-C",
                str(pathlib.Path(run_command.call_args.kwargs["cwd"])),
            ],
        )
        self.assertEqual(result["command"][:6], ["codex", "-c", 'model_catalog_json="/tmp/rust-catalog.json"', "-c", "tools.web_search=false", "-m"])
        self.assertIn("preset-chat", result["command"])
        self.assertNotIn("--model", result["command"])
        self.assertNotIn("PRESET_ENDPOINT_API_KEY", run_command.call_args.kwargs["env"])
        self.assertEqual(
            run_command.call_args.kwargs["env"]["OPENAI_API_KEY"],
            "matrix-proxy-key",
        )

    def test_build_client_command_rejects_internal_tool_artifacts_from_launch_plan(self):
        module = load_module()
        fixture = make_fixture(module)
        plan = module.ClientLaunchPlan(
            program="codex",
            argv=["-c", 'tool_identity_contract="__llmup_custom__apply_patch"'],
            env={},
            projection={},
            artifacts={},
        )

        with self.assertRaisesRegex(ValueError, "__llmup_custom__apply_patch"):
            module.build_client_command(
                "codex",
                "http://127.0.0.1:18888",
                make_target(module),
                fixture,
                pathlib.Path("/tmp/workspace").resolve(),
                launch_plan=plan,
            )

    def test_build_client_command_native_fallback_does_not_project_model(self):
        module = load_module()
        fixture = make_fixture(module)
        command = module.build_client_command(
            "codex",
            "http://127.0.0.1:18888",
            make_target(module, name="preset-chat", proxy_model="preset-chat"),
            fixture,
            pathlib.Path("/tmp/workspace").resolve(),
        )

        self.assertEqual(command[0], "codex")
        self.assertIn("exec", command)
        self.assertNotIn("-m", command)
        self.assertNotIn("--model", command)

    def test_prepare_proxy_env_keeps_dotenv_scoped_to_proxy_only(self):
        module = load_module()
        base_env = {
            "PATH": "/usr/bin",
            "HOME": "/home/user",
            "OPENAI_API_KEY": "real-openai",
        }
        dotenv_env = {
            "PRESET_ENDPOINT_API_KEY": "proxy-only-secret",
            "LOCAL_QWEN_BASE_URL": "http://127.0.0.1:9997/v1",
        }

        proxy_env = module.prepare_proxy_env(base_env, dotenv_env)

        with tempfile.TemporaryDirectory() as temp_dir:
            client_env = module.build_client_env(
                "codex",
                base_env,
                "http://127.0.0.1:18888",
                pathlib.Path(temp_dir) / "codex-home",
            )

        self.assertEqual(proxy_env["PRESET_ENDPOINT_API_KEY"], "proxy-only-secret")
        self.assertEqual(proxy_env["LOCAL_QWEN_BASE_URL"], "http://127.0.0.1:9997/v1")
        self.assertEqual(proxy_env["LLM_UNIVERSAL_PROXY_AUTH_MODE"], "proxy_key")
        self.assertEqual(proxy_env["LLM_UNIVERSAL_PROXY_KEY"], module.DEFAULT_PROXY_KEY)
        self.assertNotIn("PRESET_ENDPOINT_API_KEY", client_env)
        self.assertNotIn("LOCAL_QWEN_BASE_URL", client_env)

    def test_prepare_proxy_env_persists_replay_marker_key_within_run_root(self):
        module = load_module()
        base_env = {"PATH": "/usr/bin", "HOME": "/home/user"}

        with tempfile.TemporaryDirectory() as temp_dir:
            runtime_root = pathlib.Path(temp_dir)
            first_env = module.prepare_proxy_env(base_env, {}, runtime_root)
            second_env = module.prepare_proxy_env(base_env, {}, runtime_root)
            key_path = runtime_root / module.REPLAY_MARKER_KEY_FILENAME

            self.assertTrue(key_path.exists())
            self.assertEqual(
                first_env[module.REPLAY_MARKER_KEY_ENV],
                second_env[module.REPLAY_MARKER_KEY_ENV],
            )
            self.assertEqual(
                key_path.read_text(encoding="utf-8").strip(),
                first_env[module.REPLAY_MARKER_KEY_ENV],
            )

    def test_ensure_replay_marker_key_regenerates_blank_file(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            runtime_root = pathlib.Path(temp_dir)
            key_path = runtime_root / module.REPLAY_MARKER_KEY_FILENAME
            key_path.write_text("   \n", encoding="utf-8")

            marker_key = module.ensure_replay_marker_key(runtime_root)

            self.assertEqual(
                key_path.read_text(encoding="utf-8").strip(),
                marker_key,
            )

        self.assertTrue(marker_key)
        self.assertNotEqual(marker_key, "   ")

    def test_stop_proxy_uses_short_fixed_tail_wait_after_kill(self):
        module = load_module()

        class FakeProcess:
            def __init__(self):
                self.wait_timeouts = []

            def poll(self):
                return None

            def terminate(self):
                return None

            def kill(self):
                return None

            def wait(self, timeout=None):
                self.wait_timeouts.append(timeout)
                if len(self.wait_timeouts) == 1:
                    raise subprocess.TimeoutExpired(cmd="proxy", timeout=timeout)
                return 0

        process = FakeProcess()

        module.stop_proxy(process, terminate_grace_secs=15)

        self.assertEqual(
            process.wait_timeouts,
            [15, module.DEFAULT_POST_KILL_WAIT_SECS],
        )

    def test_expected_fail_closed_keeps_claude_openai_family_targets_positive(self):
        module = load_module()
        for target_name, upstream_name, upstream_format in (
            (
                "preset-openai-compatible",
                "PRESET-OPENAI-COMPATIBLE",
                "openai-chat-completions",
            ),
            (
                "preset-openai-responses-compatible",
                "PRESET-OPENAI-RESPONSES-COMPATIBLE",
                "openai-responses",
            ),
        ):
            with self.subTest(target_name=target_name):
                case = make_case(
                    module,
                    client_name="claude",
                    target=make_target(
                        module,
                        name=target_name,
                        upstream_name=upstream_name,
                        upstream_format=upstream_format,
                    ),
                )
                self.assertIsNone(module.expected_fail_closed_for_case(case))

    def test_expected_fail_closed_uses_negative_fixture_semantics(self):
        module = load_module()
        case = make_case(
            module,
            client_name="claude",
            target=make_target(
                module,
                name="preset-openai-compatible",
                upstream_name="PRESET-OPENAI-COMPATIBLE",
                upstream_format="openai-chat-completions",
            ),
            fixture=make_expected_fail_closed_fixture(module),
        )

        expectation = module.expected_fail_closed_for_case(case)

        self.assertIsNotNone(expectation)
        self.assertEqual(expectation.category, "provider_owned_state_resource")
        self.assertTrue(
            module.expected_fail_closed_error_matches(
                expectation,
                "provider-owned state conversation cannot be expanded",
            )
        )
        self.assertFalse(
            module.expected_fail_closed_error_matches(
                expectation,
                "Anthropic request controls thinking, context_management rejected",
            )
        )

    def test_classify_target_health_skips_optional_qwen_probe_failures(self):
        module = load_module()
        optional_target = module.MatrixTarget(
            name="qwen-local",
            required=False,
            enabled=True,
            proxy_model="qwen-local",
            upstream_name="LOCAL-QWEN",
            skip_reason=None,
        )
        required_target = module.MatrixTarget(
            name="minimax-anth",
            required=True,
            enabled=True,
            proxy_model="minimax-anth",
            upstream_name="MINIMAX-ANTHROPIC",
            skip_reason=None,
        )

        self.assertEqual(
            module.classify_target_health(optional_target, "connection refused")[0], "skipped"
        )
        self.assertEqual(
            module.classify_target_health(required_target, "connection refused")[0], "failed"
        )

    def test_probe_target_accepts_valid_responses_shape_without_exact_probe_text(self):
        module = load_module()
        target = make_target(
            module,
            name="qwen-local",
            required=False,
            proxy_model="qwen-local",
            upstream_name="LOCAL-QWEN",
        )

        with mock.patch.object(
            module,
            "http_json",
            return_value=(
                200,
                json.dumps(
                    {
                        "id": "resp_qwen",
                        "object": "response",
                        "status": "completed",
                        "output": [
                            {
                                "type": "message",
                                "content": [
                                    {
                                        "type": "output_text",
                                        "text": "Sure, here is a semantic match.",
                                    }
                                ],
                            }
                        ],
                    }
                ),
            ),
        ) as http_json:
            self.assertIsNone(
                module.probe_target(
                    "http://127.0.0.1:18888",
                    target,
                    proxy_key="probe-proxy-key",
                )
            )
        self.assertEqual(
            http_json.call_args.kwargs["bearer_token"],
            "probe-proxy-key",
        )

    def test_probe_target_rejects_http_200_body_without_response_shape(self):
        module = load_module()
        target = make_target(
            module,
            name="qwen-local",
            required=False,
            proxy_model="qwen-local",
            upstream_name="LOCAL-QWEN",
        )

        with mock.patch.object(module, "http_json", return_value=(200, '{"ok":true}')):
            self.assertIn(
                "valid response shape",
                module.probe_target(
                    "http://127.0.0.1:18888",
                    target,
                    proxy_key=module.DEFAULT_PROXY_KEY,
                ),
            )

    def test_verify_fixture_output_rejects_comment_only_python_bugfix_edits(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="python_bugfix",
            verifier={
                "type": "python_source_and_output",
                "source": {
                    "path": "calc.py",
                    "function": "add",
                    "args": ["a", "b"],
                    "returns": {
                        "kind": "binary_op",
                        "operator": "+",
                        "left": "a",
                        "right": "b",
                    },
                },
                "entrypoint": {
                    "path": "main.py",
                    "expect_stdout_contains": [
                        "2 + 3 = 5",
                        "-1 + 5 = 4",
                        "0 + 0 = 0",
                        "4 * 5 = 20",
                    ],
                },
            },
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            (workspace_dir / "calc.py").write_text(
                textwrap.dedent(
                    """
                    def add(a, b):
                        return a - b

                    # return a + b

                    def multiply(a, b):
                        return a * b
                    """
                ).strip()
                + "\n",
                encoding="utf-8",
            )
            (workspace_dir / "main.py").write_text(
                textwrap.dedent(
                    """
                    from calc import add, multiply

                    print(f"2 + 3 = {add(2, 3)}")
                    print(f"-1 + 5 = {add(-1, 5)}")
                    print(f"0 + 0 = {add(0, 0)}")
                    print(f"4 * 5 = {multiply(4, 5)}")
                    """
                ).strip()
                + "\n",
                encoding="utf-8",
            )

            ok, message = module.verify_fixture_output(fixture, "", workspace_dir)

        self.assertFalse(ok)
        self.assertIn("calc.py", message)
        self.assertIn("return a + b", message)

    def test_verify_fixture_output_checks_main_py_behavior_for_python_bugfix(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="python_bugfix",
            verifier={
                "type": "python_source_and_output",
                "source": {
                    "path": "calc.py",
                    "function": "add",
                    "args": ["a", "b"],
                    "returns": {
                        "kind": "binary_op",
                        "operator": "+",
                        "left": "a",
                        "right": "b",
                    },
                },
                "entrypoint": {
                    "path": "main.py",
                    "expect_stdout_contains": [
                        "2 + 3 = 5",
                        "-1 + 5 = 4",
                        "0 + 0 = 0",
                        "4 * 5 = 20",
                    ],
                },
            },
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            (workspace_dir / "calc.py").write_text(
                textwrap.dedent(
                    """
                    def add(a, b):
                        return a + b

                    def multiply(a, b):
                        return a * b
                    """
                ).strip()
                + "\n",
                encoding="utf-8",
            )
            (workspace_dir / "main.py").write_text(
                'print("2 + 3 = 6")\nprint("-1 + 5 = 4")\nprint("0 + 0 = 0")\nprint("4 * 5 = 20")\n',
                encoding="utf-8",
            )

            ok, message = module.verify_fixture_output(fixture, "", workspace_dir)

        self.assertFalse(ok)
        self.assertIn("main.py", message)
        self.assertIn("2 + 3 = 5", message)

    def test_verify_fixture_output_accepts_python_bugfix_fix_when_behavior_matches(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="python_bugfix",
            verifier={
                "type": "python_source_and_output",
                "source": {
                    "path": "calc.py",
                    "function": "add",
                    "args": ["a", "b"],
                    "returns": {
                        "kind": "binary_op",
                        "operator": "+",
                        "left": "a",
                        "right": "b",
                    },
                },
                "entrypoint": {
                    "path": "main.py",
                    "expect_stdout_contains": [
                        "2 + 3 = 5",
                        "-1 + 5 = 4",
                        "0 + 0 = 0",
                        "4 * 5 = 20",
                    ],
                },
            },
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            (workspace_dir / "calc.py").write_text(
                textwrap.dedent(
                    """
                    def add(a, b):
                        return a + b

                    def multiply(a, b):
                        return a * b
                    """
                ).strip()
                + "\n",
                encoding="utf-8",
            )
            (workspace_dir / "main.py").write_text(
                textwrap.dedent(
                    """
                    from calc import add, multiply

                    print(f"2 + 3 = {add(2, 3)}")
                    print(f"-1 + 5 = {add(-1, 5)}")
                    print(f"0 + 0 = {add(0, 0)}")
                    print(f"4 * 5 = {multiply(4, 5)}")
                    """
                ).strip()
                + "\n",
                encoding="utf-8",
            )

            ok, message = module.verify_fixture_output(fixture, "", workspace_dir)

        self.assertTrue(ok, message)

    def test_verify_fixture_output_accepts_successful_workspace_command(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="command_success",
            verifier={
                "type": "command_success",
                "command": [
                    sys.executable,
                    "-c",
                    "from pathlib import Path; print(Path('answer.txt').read_text().strip())",
                ],
                "expect_stdout_contains": ["forty-two"],
            },
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            (workspace_dir / "answer.txt").write_text("forty-two\n", encoding="utf-8")

            ok, message = module.verify_fixture_output(fixture, "", workspace_dir)

        self.assertTrue(ok, message)

    def test_verify_fixture_output_rejects_failed_workspace_command(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="command_success",
            verifier={
                "type": "command_success",
                "command": [sys.executable, "-c", "import sys; print('bad'); sys.exit(7)"],
            },
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            ok, message = module.verify_fixture_output(
                fixture,
                "",
                pathlib.Path(temp_dir),
            )

        self.assertFalse(ok)
        self.assertIn("exit 0", message)
        self.assertIn("7", message)

    def test_verify_fixture_output_accepts_locked_file_digest(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="file_sha256",
            verifier={
                "type": "file_sha256",
                "path": "contract.txt",
                "sha256": "3dd7131bf1c92dd2a9c9eb03891f3fbaf0321ef9ef019be728d8cc29225d6928",
            },
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            (workspace_dir / "contract.txt").write_text("contract\n", encoding="utf-8")

            ok, message = module.verify_fixture_output(fixture, "", workspace_dir)

        self.assertTrue(ok, message)

    def test_verify_fixture_output_rejects_changed_file_digest(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="file_sha256",
            verifier={
                "type": "file_sha256",
                "path": "contract.txt",
                "sha256": "3dd7131bf1c92dd2a9c9eb03891f3fbaf0321ef9ef019be728d8cc29225d6928",
            },
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            (workspace_dir / "contract.txt").write_text("changed\n", encoding="utf-8")

            ok, message = module.verify_fixture_output(fixture, "", workspace_dir)

        self.assertFalse(ok)
        self.assertIn("sha256", message)

    def test_verify_fixture_output_supports_stdout_contract_without_internal_tool_artifacts(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="tool_identity_public_contract",
            verifier={
                "type": "stdout_contract",
                "contains_any_by_client": {"claude": ["Edit"]},
                "contains_any_by_client_match_mode": "presented_tool_name",
                "not_contains": ["__llmup_custom__"],
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "**Public editing tool used:** `Edit`.",
            workspace_dir=None,
            context=make_context(module, "claude"),
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_verify_fixture_output_accepts_codex_json_completed_agent_message_for_tool_identity_contract(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="tool_identity_public_contract",
            verifier={
                "type": "stdout_contract",
                "contains_any_by_client": {"codex": ["apply_patch"]},
                "contains_any_by_client_match_mode": "presented_tool_name",
                "reject_other_client_contains_any_by_client": True,
            },
        )
        stdout_text = "\n".join(
            [
                json.dumps({"type": "turn.started"}),
                json.dumps(
                    {
                        "type": "item.completed",
                        "item": {
                            "id": "item_1",
                            "type": "agent_message",
                            "text": "apply_patch",
                        },
                    }
                ),
                json.dumps({"type": "turn.completed"}),
            ]
        )

        ok, message = module.verify_fixture_output(
            fixture,
            stdout_text,
            workspace_dir=None,
            context=make_context(module, "codex"),
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_verify_fixture_output_rejects_stdout_contract_internal_tool_artifacts(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="tool_identity_public_contract",
            verifier={
                "type": "stdout_contract",
                "not_contains": ["__llmup_custom__"],
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "Available tool: __llmup_custom__apply_patch",
            workspace_dir=None,
        )

        self.assertFalse(ok)
        self.assertIn("__llmup_custom__", message)

    def test_verify_fixture_output_accepts_claude_openai_multi_turn_trace_contract(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="claude_openai_multi_turn_request_shape_contract",
            verifier={
                "type": "trace_request_contract",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "min_request_count": 2,
                "require_response_for_each_request": True,
                "forbid_response_http_statuses": [400],
                "request_index": -1,
                "required_request_fields": [
                    "client_summary.message_roles",
                    "client_summary.messages_tail",
                    "upstream_summary.message_roles",
                    "upstream_summary.messages_tail",
                ],
                "required_request_objects": [
                    {
                        "field": "client_summary.messages_tail",
                        "contains": {"type": "tool_result"},
                    },
                    {
                        "field": "upstream_summary.messages_tail",
                        "contains": {"role": "tool"},
                    }
                ],
            },
        )
        trace_entries = (
            {
                "request_id": "req-1",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "request": {
                    "client_summary": {
                        "message_roles": ["user"],
                        "messages_tail": [
                            {"role": "user", "content": [{"type": "text"}]}
                        ],
                    },
                    "upstream_summary": {
                        "message_roles": ["user"],
                        "messages_tail": [
                            {"role": "user", "content": "Inspect calc.py"}
                        ],
                    },
                },
            },
            {
                "request_id": "req-1",
                "phase": "response",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "http_status": 200,
            },
            {
                "request_id": "req-2",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "request": {
                    "client_summary": {
                        "message_roles": ["user", "assistant", "user"],
                        "messages_tail": [
                            {
                                "role": "user",
                                "content": [
                                    {
                                        "type": "tool_result",
                                        "tool_use_id": "toolu_123",
                                    }
                                ],
                            }
                        ],
                    },
                    "upstream_summary": {
                        "message_roles": ["user", "assistant", "tool"],
                        "messages_tail": [
                            {
                                "role": "tool",
                                "tool_call_id": "call_123",
                                "content": "2 + 3 = 5",
                            }
                        ],
                    },
                },
            },
            {
                "request_id": "req-2",
                "phase": "response",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "http_status": 200,
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "",
            None,
            context=module.VerifierContext(
                client_name="claude",
                trace_entries=trace_entries,
            ),
        )

        self.assertTrue(ok, message)

    def test_verify_fixture_output_rejects_last_request_missing_bound_tool_result(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="claude_openai_multi_turn_request_shape_contract",
            verifier={
                "type": "trace_request_contract",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "min_request_count": 2,
                "request_index": -1,
                "required_request_objects": [
                    {
                        "field": "client_summary.messages_tail",
                        "contains": {"type": "tool_result"},
                    }
                ],
            },
        )
        trace_entries = (
            {
                "request_id": "req-1",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "request": {
                    "client_summary": {
                        "messages_tail": [
                            {
                                "role": "user",
                                "content": [{"type": "tool_result"}],
                            }
                        ],
                    }
                },
            },
            {
                "request_id": "req-2",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "request": {"client_summary": {"messages_tail": []}},
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "",
            None,
            context=module.VerifierContext(
                client_name="claude",
                trace_entries=trace_entries,
            ),
        )

        self.assertFalse(ok)
        self.assertIn("request_index -1", message)
        self.assertIn("object containing", message)

    def test_verify_fixture_output_accepts_required_on_request_last_binding(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            verifier={
                "type": "trace_request_contract",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "min_request_count": 2,
                "required_on_request": "last",
                "required_request_objects": [
                    {
                        "field": "client_summary.messages_tail",
                        "contains": {"type": "tool_result"},
                    }
                ],
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "",
            None,
            context=module.VerifierContext(
                client_name="claude",
                trace_entries=(
                    {
                        "request_id": "req-1",
                        "phase": "request",
                        "path": "/anthropic/v1/messages",
                        "client_format": "anthropic",
                        "upstream_format": "openai-chat-completions",
                        "request": {"client_summary": {"messages_tail": []}},
                    },
                    {
                        "request_id": "req-2",
                        "phase": "request",
                        "path": "/anthropic/v1/messages",
                        "client_format": "anthropic",
                        "upstream_format": "openai-chat-completions",
                        "request": {
                            "client_summary": {
                                "messages_tail": [
                                    {
                                        "role": "user",
                                        "content": [{"type": "tool_result"}],
                                    }
                                ],
                            }
                        },
                    },
                ),
            ),
        )

        self.assertTrue(ok, message)

    def test_verify_fixture_output_rejects_required_response_per_request_without_request_id(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            verifier={
                "type": "trace_request_contract",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "min_request_count": 2,
                "require_response_for_each_request": True,
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "",
            None,
            context=module.VerifierContext(
                client_name="claude",
                trace_entries=(
                    {
                        "phase": "request",
                        "path": "/anthropic/v1/messages",
                        "client_format": "anthropic",
                        "upstream_format": "openai-chat-completions",
                        "request": {"client_summary": {"messages_tail": []}},
                    },
                    {
                        "request_id": "req-2",
                        "phase": "request",
                        "path": "/anthropic/v1/messages",
                        "client_format": "anthropic",
                        "upstream_format": "openai-chat-completions",
                        "request": {"client_summary": {"messages_tail": []}},
                    },
                    {
                        "request_id": "req-2",
                        "phase": "response",
                        "path": "/anthropic/v1/messages",
                        "client_format": "anthropic",
                        "upstream_format": "openai-chat-completions",
                        "http_status": 200,
                    },
                ),
            ),
        )

        self.assertFalse(ok)
        self.assertIn("non-empty string request_id", message)

    def test_verify_fixture_output_rejects_required_on_request_without_matching_request(
        self,
    ):
        module = load_module()

        for selector in ("first", "last"):
            with self.subTest(selector=selector):
                fixture = make_fixture(
                    module,
                    verifier={
                        "type": "trace_request_contract",
                        "path": "/anthropic/v1/messages",
                        "client_format": "anthropic",
                        "upstream_format": "openai-chat-completions",
                        "min_request_count": 0,
                        "required_on_request": selector,
                    },
                )

                ok, message = module.verify_fixture_output(
                    fixture,
                    "",
                    None,
                    context=module.VerifierContext(
                        client_name="claude",
                        trace_entries=(
                            {
                                "request_id": "req-other",
                                "phase": "request",
                                "path": "/other",
                                "client_format": "anthropic",
                                "upstream_format": "openai-chat-completions",
                                "request": {
                                    "client_summary": {"messages_tail": []}
                                },
                            },
                        ),
                    ),
                )

                self.assertFalse(ok)
                self.assertIn(f"required_on_request '{selector}'", message)
                self.assertIn("0 matching request entries", message)

    def test_verify_fixture_output_rejects_bound_required_request_contains_missing_on_last_request(
        self,
    ):
        module = load_module()
        selector_variants = (
            ("request_index -1", {"request_index": -1}),
            ("required_on_request 'last'", {"required_on_request": "last"}),
        )

        for binding_label, selector in selector_variants:
            with self.subTest(binding_label=binding_label):
                fixture = make_fixture(
                    module,
                    verifier={
                        "type": "trace_request_contract",
                        "path": "/anthropic/v1/messages",
                        "client_format": "anthropic",
                        "upstream_format": "openai-chat-completions",
                        "min_request_count": 2,
                        **selector,
                        "required_request_contains": [
                            {
                                "field": "upstream_summary.message_roles",
                                "value": "tool",
                            }
                        ],
                    },
                )

                ok, message = module.verify_fixture_output(
                    fixture,
                    "",
                    None,
                    context=module.VerifierContext(
                        client_name="claude",
                        trace_entries=(
                            {
                                "request_id": "req-1",
                                "phase": "request",
                                "path": "/anthropic/v1/messages",
                                "client_format": "anthropic",
                                "upstream_format": "openai-chat-completions",
                                "request": {
                                    "upstream_summary": {
                                        "message_roles": [
                                            "user",
                                            "assistant",
                                            "tool",
                                        ]
                                    }
                                },
                            },
                            {
                                "request_id": "req-2",
                                "phase": "request",
                                "path": "/anthropic/v1/messages",
                                "client_format": "anthropic",
                                "upstream_format": "openai-chat-completions",
                                "request": {
                                    "upstream_summary": {
                                        "message_roles": ["user", "assistant"]
                                    }
                                },
                            },
                        ),
                    ),
                )

                self.assertFalse(ok)
                self.assertIn(binding_label, message)
                self.assertIn("to contain 'tool'", message)

    def test_verify_fixture_output_rejects_same_request_split_requirements(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            verifier={
                "type": "trace_request_contract",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "min_request_count": 2,
                "same_request": True,
                "required_request_fields": ["client_summary.message_roles"],
                "required_request_objects": [
                    {
                        "field": "client_summary.messages_tail",
                        "contains": {"type": "tool_result"},
                    }
                ],
            },
        )
        trace_entries = (
            {
                "request_id": "req-1",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "request": {"client_summary": {"message_roles": ["user"]}},
            },
            {
                "request_id": "req-2",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "request": {
                    "client_summary": {
                        "messages_tail": [
                            {
                                "role": "user",
                                "content": [{"type": "tool_result"}],
                            }
                        ],
                    }
                },
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "",
            None,
            context=module.VerifierContext(
                client_name="claude",
                trace_entries=trace_entries,
            ),
        )

        self.assertFalse(ok)
        self.assertIn("same_request", message)
        self.assertIn("object containing", message)

    def test_verify_fixture_output_rejects_claude_openai_trace_contract_with_one_matching_request(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="claude_openai_multi_turn_request_shape_contract",
            verifier={
                "type": "trace_request_contract",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "min_request_count": 2,
                "required_request_objects": [
                    {
                        "field": "client_summary.messages_tail",
                        "contains": {"type": "tool_result"},
                    }
                ],
            },
        )
        trace_entries = (
            {
                "request_id": "req-1",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "request": {
                    "client_summary": {
                        "messages_tail": [
                            {
                                "role": "user",
                                "content": [{"type": "tool_result"}],
                            }
                        ],
                    }
                },
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "",
            None,
            context=module.VerifierContext(
                client_name="claude",
                trace_entries=trace_entries,
            ),
        )

        self.assertFalse(ok)
        self.assertIn("got 1", message)

    def test_verify_fixture_output_rejects_claude_openai_trace_contract_text_only_tool_result(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="claude_openai_multi_turn_request_shape_contract",
            verifier={
                "type": "trace_request_contract",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "min_request_count": 2,
                "required_request_fields": ["client_summary.messages_tail"],
                "required_request_objects": [
                    {
                        "field": "client_summary.messages_tail",
                        "contains": {"type": "tool_result"},
                    }
                ],
            },
        )
        trace_entries = (
            {
                "request_id": "req-1",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "request": {
                    "client_summary": {
                        "messages_tail": [
                            {
                                "role": "user",
                                "content": "tool_result",
                            }
                        ],
                    }
                },
            },
            {
                "request_id": "req-2",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "request": {
                    "client_summary": {
                        "messages_tail": [
                            "tool_result",
                            {
                                "role": "user",
                                "content": [{"type": "text", "text": "tool_result"}],
                            },
                        ],
                    }
                },
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "",
            None,
            context=module.VerifierContext(
                client_name="claude",
                trace_entries=trace_entries,
            ),
        )

        self.assertFalse(ok)
        self.assertIn("object containing", message)

    def test_verify_fixture_output_rejects_claude_openai_trace_contract_local_400(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            verifier={
                "type": "trace_request_contract",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "min_request_count": 2,
                "forbid_response_http_statuses": [400],
                "required_request_objects": [
                    {
                        "field": "client_summary.messages_tail",
                        "contains": {"type": "tool_result"},
                    }
                ],
            },
        )
        trace_entries = (
            {
                "request_id": "req-1",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "request": {
                    "client_summary": {
                        "messages_tail": [
                            {
                                "role": "user",
                                "content": [{"type": "tool_result"}],
                            }
                        ],
                    }
                },
            },
            {
                "request_id": "req-2",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "request": {"client_summary": {"messages_tail": []}},
            },
            {
                "request_id": "req-2",
                "phase": "response",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "http_status": 400,
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "",
            None,
            context=module.VerifierContext(
                client_name="claude",
                trace_entries=trace_entries,
            ),
        )

        self.assertFalse(ok)
        self.assertIn("http_status 400", message)

    def test_verify_fixture_output_rejects_trace_contract_without_required_response(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            verifier={
                "type": "trace_request_contract",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "min_request_count": 1,
                "forbid_response_http_statuses": [400],
                "min_response_count": 1,
            },
        )
        trace_entries = (
            {
                "request_id": "req-1",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "request": {"client_summary": {"messages_tail": []}},
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "",
            None,
            context=module.VerifierContext(
                client_name="claude",
                trace_entries=trace_entries,
            ),
        )

        self.assertFalse(ok)
        self.assertIn("response entries", message)

    def test_verify_fixture_output_rejects_trace_contract_non_integer_forbidden_status(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            verifier={
                "type": "trace_request_contract",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-chat-completions",
                "forbid_response_http_statuses": ["400"],
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "",
            None,
            context=module.VerifierContext(
                client_name="claude",
                trace_entries=(
                    {
                        "request_id": "req-1",
                        "phase": "request",
                        "path": "/anthropic/v1/messages",
                        "client_format": "anthropic",
                        "upstream_format": "openai-chat-completions",
                        "request": {"client_summary": {"messages_tail": []}},
                    },
                    {
                        "request_id": "req-1",
                        "phase": "response",
                        "path": "/anthropic/v1/messages",
                        "client_format": "anthropic",
                        "upstream_format": "openai-chat-completions",
                        "http_status": 200,
                    },
                ),
            ),
        )

        self.assertFalse(ok)
        self.assertIn("integer", message)

    def test_verify_fixture_output_accepts_claude_openai_responses_multi_turn_trace_contract(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="claude_openai_responses_multi_turn_request_shape_contract",
            verifier={
                "type": "trace_request_contract",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-responses",
                "min_request_count": 2,
                "require_response_for_each_request": True,
                "forbid_response_http_statuses": [400],
                "request_index": -1,
                "required_request_fields": [
                    "client_summary.messages_tail",
                    "upstream_summary.input_tail",
                ],
                "required_request_objects": [
                    {
                        "field": "client_summary.messages_tail",
                        "contains": {"type": "tool_result"},
                    },
                    {
                        "field": "upstream_summary.input_tail",
                        "contains": {"type": "function_call_output"},
                    },
                ],
            },
        )
        trace_entries = (
            {
                "request_id": "req-1",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-responses",
                "request": {
                    "client_summary": {
                        "messages_tail": [{"role": "user"}],
                    },
                    "upstream_summary": {
                        "input_tail": [{"type": "message", "role": "user"}],
                    },
                },
            },
            {
                "request_id": "req-1",
                "phase": "response",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-responses",
                "http_status": 200,
            },
            {
                "request_id": "req-2",
                "phase": "request",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-responses",
                "request": {
                    "client_summary": {
                        "messages_tail": [
                            {
                                "role": "user",
                                "content": [{"type": "tool_result"}],
                            }
                        ],
                    },
                    "upstream_summary": {
                        "input_tail": [
                            {
                                "type": "function_call_output",
                                "call_id": "call_123",
                            }
                        ],
                    },
                },
            },
            {
                "request_id": "req-2",
                "phase": "response",
                "path": "/anthropic/v1/messages",
                "client_format": "anthropic",
                "upstream_format": "openai-responses",
                "http_status": 200,
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "",
            None,
            context=module.VerifierContext(
                client_name="claude",
                trace_entries=trace_entries,
            ),
        )

        self.assertTrue(ok, message)

    def test_tool_identity_fixture_prompt_template_renders_client_and_exclusion_constraints(self):
        module = load_module()
        fixture = next(
            fixture
            for fixture in module.load_fixtures(TOOL_IDENTITY_FIXTURE_PATH.parent)
            if fixture.fixture_id == "tool_identity_public_contract"
        )

        rendered = module.render_fixture_prompt(fixture, "claude")

        self.assertEqual(fixture.fixture_id, "tool_identity_public_contract")
        self.assertIn("Current client: claude.", rendered)
        self.assertIn("Reply with exactly one line", rendered)
        self.assertIn("Do not mention any other clients", rendered)
        self.assertIn("do not use any client names as answers", rendered)
        self.assertIn(
            "Do not answer with task IDs, fixture IDs, contract names, workspace/path words, or filenames.",
            rendered,
        )
        self.assertNotIn("{client_name}", rendered)

    def test_run_matrix_case_passes_verifier_context_with_client_name(self):
        module = load_module()
        case = make_case(
            module,
            client_name="claude",
            target=make_target(module),
            fixture=make_fixture(module, prompt="Reply with exactly PONG"),
        )
        observed = {}

        def fake_verify(fixture, stdout_text, workspace_dir, context=None):
            observed["fixture_id"] = fixture.fixture_id
            observed["stdout_text"] = stdout_text
            observed["workspace_dir"] = workspace_dir
            observed["context"] = context
            return True, ""

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir)
            with mock.patch.object(
                module,
                "build_client_launch_plan",
                side_effect=self.fake_launch_plan_side_effect(module),
            ), mock.patch.object(
                module.subprocess,
                "run",
                return_value=subprocess.CompletedProcess(["claude"], 0, stdout="PONG\n", stderr=""),
            ), mock.patch.object(module, "verify_fixture_output", side_effect=fake_verify):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(result["status"], "passed")
        self.assertEqual(observed["fixture_id"], case.fixture.fixture_id)
        self.assertEqual(observed["stdout_text"], "PONG\n")
        self.assertEqual(observed["context"].client_name, "claude")

    def test_run_matrix_case_feeds_claude_prompt_via_stdin(self):
        module = load_module()
        case = make_case(
            module,
            client_name="claude",
            target=make_target(module),
            fixture=make_fixture(module, prompt="Reply with exactly PONG"),
        )
        captured = {}

        def fake_run(command, **kwargs):
            captured["command"] = command
            captured["kwargs"] = kwargs
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir)
            with mock.patch.object(
                module,
                "build_client_launch_plan",
                side_effect=self.fake_launch_plan_side_effect(module),
            ), mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(result["status"], "passed")
        self.assertIn("--add-dir", captured["command"])
        self.assertNotIn(case.fixture.prompt, captured["command"])
        self.assertEqual(captured["kwargs"]["input"], case.fixture.prompt)
        self.assertNotIn("stdin", captured["kwargs"])

    def test_run_matrix_case_records_per_case_trace_window_diagnostics(self):
        module = load_module()
        case = make_case(
            module,
            client_name="claude",
            target=make_target(module, name="minimax-anth", proxy_model="minimax-anth"),
            fixture=make_fixture(module, prompt="Reply with exactly PONG"),
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir)
            trace_path = report_dir / "debug-trace.jsonl"
            trace_path.write_text(
                json.dumps({"request_id": "req_before", "phase": "request"}) + "\n",
                encoding="utf-8",
            )

            def fake_run(command, **kwargs):
                with trace_path.open("a", encoding="utf-8") as handle:
                    handle.write(
                        json.dumps(
                            {
                                "request_id": "req_case",
                                "phase": "request",
                                "path": "/anthropic/v1/messages",
                                "stream": True,
                                "client_format": "anthropic",
                                "upstream_format": "openai-chat-completions",
                                "client_model": "minimax-anth",
                                "upstream_name": "MINIMAX-ANTHROPIC",
                                "upstream_model": "MiniMax-M2.7-highspeed",
                                "request": {
                                    "client_summary": {"tool_names": ["Edit"]},
                                    "upstream_summary": {"tool_names": ["Edit"]},
                                },
                            }
                        )
                        + "\n"
                    )
                    handle.write(
                        json.dumps(
                            {
                                "request_id": "req_case",
                                "phase": "response",
                                "path": "/anthropic/v1/messages",
                                "http_status": 200,
                                "outcome": "completed",
                            }
                        )
                        + "\n"
                    )
                return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

            with mock.patch.object(
                module,
                "build_client_launch_plan",
                side_effect=self.fake_launch_plan_side_effect(module),
            ), mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        diagnostics = result["diagnostics"]
        legacy_key = "la" + "ne"
        self.assertEqual(result["status"], "passed")
        self.assertEqual(result["target"], "minimax-anth")
        self.assertNotIn(legacy_key, result)
        self.assertEqual(diagnostics["request_id"], "req_case")
        self.assertEqual(diagnostics["trace_request_count"], 1)
        self.assertEqual(diagnostics["trace_response_count"], 1)
        self.assertEqual(diagnostics["surface_snapshot"]["target"], "minimax-anth")
        self.assertNotIn(legacy_key, diagnostics["surface_snapshot"])
        self.assertEqual(
            diagnostics["route_summary"][0]["upstream_name"],
            "MINIMAX-ANTHROPIC",
        )
        self.assertEqual(diagnostics["tool_identity"]["client_tool_names"], ["Edit"])
        self.assertNotIn("req_before", diagnostics["request_ids"])

    def test_print_case_list_uses_target_label(self):
        module = load_module()
        case = make_case(
            module,
            client_name="codex",
            target=make_target(module, name="minimax-openai", proxy_model="minimax-openai"),
        )
        stdout = io.StringIO()

        with mock.patch("sys.stdout", stdout):
            module.print_case_list([case])

        output = stdout.getvalue()
        self.assertIn("\ttarget=minimax-openai\t", output)
        self.assertNotIn("\t" + ("la" + "ne") + "=", output)

    def test_run_matrix_case_feeds_claude_rendered_prompt_template_via_stdin(self):
        module = load_module()
        fixture = make_fixture(
            module,
            prompt="Generic fallback prompt",
            prompt_template=(
                "Current client: {client_name}. "
                "Reply with exactly one line containing only that client's public editing tool names."
            ),
        )
        case = make_case(
            module,
            client_name="claude",
            target=make_target(module),
            fixture=fixture,
        )
        captured = {}

        def fake_run(command, **kwargs):
            captured["command"] = command
            captured["kwargs"] = kwargs
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir)
            with mock.patch.object(
                module,
                "build_client_launch_plan",
                side_effect=self.fake_launch_plan_side_effect(module),
            ), mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(result["status"], "passed")
        self.assertEqual(
            captured["kwargs"]["input"],
            "Current client: claude. Reply with exactly one line containing only that client's public editing tool names.",
        )

    def test_run_matrix_case_records_workspace_diff_summary_changed_files(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="python_bugfix",
            verifier={
                "type": "python_source_and_output",
                "source": {
                    "path": "calc.py",
                    "function": "add",
                    "args": ["a", "b"],
                    "returns": {
                        "kind": "binary_op",
                        "operator": "+",
                        "left": "a",
                        "right": "b",
                    },
                },
                "entrypoint": {
                    "path": "main.py",
                    "expect_stdout_contains": [
                        "2 + 3 = 5",
                        "-1 + 5 = 4",
                        "0 + 0 = 0",
                        "4 * 5 = 20",
                    ],
                },
            },
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            temp_root = pathlib.Path(temp_dir)
            workspace_template = temp_root / "template"
            workspace_template.mkdir()
            (workspace_template / "calc.py").write_text(
                textwrap.dedent(
                    """
                    def add(a, b):
                        return a - b

                    def multiply(a, b):
                        return a * b
                    """
                ).strip()
                + "\n",
                encoding="utf-8",
            )
            (workspace_template / "main.py").write_text(
                textwrap.dedent(
                    """
                    from calc import add, multiply

                    print(f"2 + 3 = {add(2, 3)}")
                    print(f"-1 + 5 = {add(-1, 5)}")
                    print(f"0 + 0 = {add(0, 0)}")
                    print(f"4 * 5 = {multiply(4, 5)}")
                    """
                ).strip()
                + "\n",
                encoding="utf-8",
            )
            fixture.workspace_template = workspace_template
            case = make_case(
                module,
                client_name="claude",
                target=make_target(module),
                fixture=fixture,
            )
            report_dir = temp_root / "reports"
            report_dir.mkdir()

            original_run = module.subprocess.run

            def fake_run(command, **kwargs):
                if command and command[0] == sys.executable:
                    return original_run(command, **kwargs)
                workspace_dir = pathlib.Path(kwargs["cwd"])
                (workspace_dir / "calc.py").write_text(
                    "def add(a, b):\n"
                    "    return a + b\n\n"
                    "def multiply(a, b):\n"
                    "    return a * b\n",
                    encoding="utf-8",
                )
                return subprocess.CompletedProcess(command, 0, stdout="fixed\n", stderr="")

            with mock.patch.object(
                module,
                "build_client_launch_plan",
                side_effect=self.fake_launch_plan_side_effect(module),
            ), mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(result["status"], "passed", result["message"])
        workspace_diff = result["diagnostics"]["workspace_diff"]
        self.assertEqual(workspace_diff["changed_files"], ["calc.py"])
        self.assertEqual(workspace_diff["modified_files"], ["calc.py"])

    def test_run_matrix_case_uses_external_runtime_paths_for_claude(self):
        module = load_module()
        case = make_case(
            module,
            client_name="claude",
            target=make_target(module, name="minimax-anth", proxy_model="minimax-anth"),
            fixture=make_fixture(module, fixture_id="tool_identity_public_contract"),
        )
        observed = {}

        def fake_run(command, **kwargs):
            add_dir_index = command.index("--add-dir")
            observed["add_dir"] = pathlib.Path(command[add_dir_index + 1]).resolve()
            observed["cwd"] = pathlib.Path(kwargs["cwd"]).resolve()
            observed["home"] = pathlib.Path(kwargs["env"]["HOME"]).resolve()
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        with tempfile.TemporaryDirectory(dir=REPO_ROOT) as temp_dir:
            report_dir = pathlib.Path(temp_dir) / "reports" / "run-001"
            report_dir.mkdir(parents=True, exist_ok=True)
            with mock.patch.object(
                module,
                "build_client_launch_plan",
                side_effect=self.fake_launch_plan_side_effect(module),
            ), mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(result["status"], "passed", result["message"])
        self.assertEqual(observed["add_dir"], observed["cwd"])
        self.assert_path_not_within(observed["cwd"], report_dir)
        self.assert_path_not_within(observed["home"], report_dir)
        self.assert_path_not_within(observed["cwd"], module.REPO_ROOT)
        self.assert_path_not_within(observed["home"], module.REPO_ROOT)
        self.assert_path_uses_opaque_case_token(observed["cwd"], case)
        self.assert_path_uses_opaque_case_token(observed["home"], case)

    def test_run_matrix_case_uses_external_runtime_paths_for_codex(self):
        module = load_module()
        case = make_case(
            module,
            client_name="codex",
            target=make_target(module, name="minimax-openai", proxy_model="minimax-openai"),
            fixture=make_fixture(module, fixture_id="tool_identity_public_contract"),
        )
        observed = {}

        def fake_run(command, **kwargs):
            workspace_index = command.index("-C")
            observed["workspace"] = pathlib.Path(command[workspace_index + 1]).resolve()
            observed["cwd"] = pathlib.Path(kwargs["cwd"]).resolve()
            observed["home"] = pathlib.Path(kwargs["env"]["HOME"]).resolve()
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        with tempfile.TemporaryDirectory(dir=REPO_ROOT) as temp_dir:
            report_dir = pathlib.Path(temp_dir) / "reports" / "run-001"
            report_dir.mkdir(parents=True, exist_ok=True)
            with mock.patch.object(
                module,
                "build_client_launch_plan",
                side_effect=self.fake_launch_plan_side_effect(module),
            ), mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(result["status"], "passed", result["message"])
        self.assertEqual(observed["workspace"], observed["cwd"])
        self.assert_path_not_within(observed["cwd"], report_dir)
        self.assert_path_not_within(observed["home"], report_dir)
        self.assert_path_not_within(observed["cwd"], module.REPO_ROOT)
        self.assert_path_not_within(observed["home"], module.REPO_ROOT)
        self.assert_path_uses_opaque_case_token(observed["cwd"], case)
        self.assert_path_uses_opaque_case_token(observed["home"], case)

    def test_run_matrix_case_counts_claude_openai_controls_error_as_positive_failure(self):
        module = load_module()
        case = make_case(
            module,
            client_name="claude",
            target=make_target(
                module,
                name="preset-openai-compatible",
                upstream_name="PRESET-OPENAI-COMPATIBLE",
                upstream_format="openai-chat-completions",
            ),
            fixture=make_fixture(module),
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir)
            with mock.patch.object(
                module,
                "build_client_launch_plan",
                side_effect=self.fake_launch_plan_side_effect(module),
            ), mock.patch.object(
                module.subprocess,
                "run",
                return_value=subprocess.CompletedProcess(
                    ["claude"],
                    1,
                    stdout="",
                    stderr=(
                        "Anthropic request controls thinking, context_management "
                        "require native provider semantics and cannot be faithfully translated"
                    ),
                ),
            ):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(result["status"], "failed")
        self.assertNotIn("expected_fail_closed", result)
        self.assertIn("exit code 1", result["message"])
        self.assertEqual(module.summarize_results([result]), (0, 1, 0, 0))

    def test_run_matrix_case_classifies_negative_fixture_fail_closed_nonzero_without_failure(self):
        module = load_module()
        case = make_case(
            module,
            client_name="claude",
            target=make_target(
                module,
                name="preset-openai-compatible",
                upstream_name="PRESET-OPENAI-COMPATIBLE",
                upstream_format="openai-chat-completions",
            ),
            fixture=make_expected_fail_closed_fixture(module),
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir)
            with mock.patch.object(
                module,
                "build_client_launch_plan",
                side_effect=self.fake_launch_plan_side_effect(module),
            ), mock.patch.object(
                module.subprocess,
                "run",
                return_value=subprocess.CompletedProcess(
                    ["claude"],
                    1,
                    stdout="",
                    stderr=(
                        "provider-owned state conversation cannot be expanded "
                        "and must fail closed"
                    ),
                ),
            ):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(result["status"], "expected_fail_closed")
        self.assertEqual(result["expected_fail_closed"], "provider_owned_state_resource")
        self.assertIn("exit code 1", result["message"])
        self.assertEqual(module.summarize_results([result]), (0, 0, 0, 1))

    def test_run_matrix_case_expected_fail_closed_success_counts_as_failure(self):
        module = load_module()
        case = make_case(
            module,
            client_name="claude",
            target=make_target(
                module,
                name="preset-openai-compatible",
                upstream_name="PRESET-OPENAI-COMPATIBLE",
                upstream_format="openai-chat-completions",
            ),
            fixture=make_expected_fail_closed_fixture(module),
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir)
            with mock.patch.object(
                module,
                "build_client_launch_plan",
                side_effect=self.fake_launch_plan_side_effect(module),
            ), mock.patch.object(
                module.subprocess,
                "run",
                return_value=subprocess.CompletedProcess(
                    ["claude"],
                    0,
                    stdout="PONG\n",
                    stderr="",
                ),
            ):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(result["status"], "unexpected_success")
        self.assertEqual(result["expected_fail_closed"], "provider_owned_state_resource")
        self.assertIn("expected fail-closed", result["message"])
        self.assertEqual(module.summarize_results([result]), (0, 1, 0, 0))

    def test_summarize_results_treats_positive_expected_fail_closed_as_failure(self):
        module = load_module()
        positive_misclassified = {
            "case_id": "claude__preset-openai-compatible__smoke_pong",
            "client": "claude",
            "target": "preset-openai-compatible",
            "fixture": "smoke_pong",
            "status": module.EXPECTED_FAIL_CLOSED_STATUS,
            "message": "legacy blanket fail-closed",
        }
        negative_expected = {
            "case_id": "claude__preset-openai-compatible__negative_provider_state_resource",
            "client": "claude",
            "target": "preset-openai-compatible",
            "fixture": "negative_provider_state_resource",
            "status": module.EXPECTED_FAIL_CLOSED_STATUS,
            "expected_outcome": "negative_fail_closed",
            "message": "fixture-declared fail-closed",
        }

        self.assertEqual(
            module.summarize_results([positive_misclassified, negative_expected]),
            (0, 1, 0, 1),
        )

    def test_wait_for_health_rejects_old_proxy_health_without_owned_listening_proof(self):
        module = load_module()

        class AliveProcess:
            def poll(self):
                return None

        class OldProxyHealthResponse:
            status = 200

            def __enter__(self):
                return self

            def __exit__(self, exc_type, exc, traceback):
                return False

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            stdout_path = root / "proxy.stdout.log"
            stderr_path = root / "proxy.stderr.log"
            stdout_path.write_text("booting proxy but not bound yet\n", encoding="utf-8")
            stderr_path.write_text("", encoding="utf-8")

            with mock.patch.object(
                module.urllib.request,
                "urlopen",
                return_value=OldProxyHealthResponse(),
            ), mock.patch.object(
                module.time,
                "time",
                side_effect=[100.0, 100.1, 101.2, 101.3],
            ), mock.patch.object(
                module.time,
                "sleep",
            ):
                with self.assertRaisesRegex(
                    RuntimeError,
                    "owned listening proof|did not become healthy",
                ):
                    module.wait_for_health(
                        "http://127.0.0.1:18888",
                        timeout_secs=1,
                        process=AliveProcess(),
                        stdout_path=stdout_path,
                        stderr_path=stderr_path,
                    )

    def test_wait_for_health_accepts_health_after_owned_listening_proof(self):
        module = load_module()

        class AliveProcess:
            def poll(self):
                return None

        class OwnedProxyHealthResponse:
            status = 200

            def __enter__(self):
                return self

            def __exit__(self, exc_type, exc, traceback):
                return False

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            stdout_path = root / "proxy.stdout.log"
            stderr_path = root / "proxy.stderr.log"
            stdout_path.write_text(
                "2026-04-24T00:00:00Z INFO listening on 127.0.0.1:18888\n",
                encoding="utf-8",
            )
            stderr_path.write_text("", encoding="utf-8")

            with mock.patch.object(
                module.urllib.request,
                "urlopen",
                return_value=OwnedProxyHealthResponse(),
            ) as urlopen:
                module.wait_for_health(
                    "http://127.0.0.1:18888",
                    timeout_secs=1,
                    process=AliveProcess(),
                    stdout_path=stdout_path,
                    stderr_path=stderr_path,
                )

        urlopen.assert_called_once_with("http://127.0.0.1:18888/health", timeout=2)

    def test_run_fails_fast_when_old_proxy_is_healthy_but_owned_process_exited(self):
        module = load_module()

        class ExitedProcess:
            def poll(self):
                return 1

            def wait(self, timeout=None):
                return 1

        class OldProxyHealthResponse:
            status = 200

            def __enter__(self):
                return self

            def __exit__(self, exc_type, exc, traceback):
                return False

        with tempfile.TemporaryDirectory() as temp_dir:
            temp_root = pathlib.Path(temp_dir)
            reports_root = temp_root / "reports"
            binary_path = temp_root / "fake-proxy"
            env_file = write_preset_endpoint_env_file(temp_root / ".env.test")
            proxy_stderr_path = temp_root / "proxy.stderr.log"
            proxy_stderr_path.write_text(
                "error: failed to bind 127.0.0.1:18888: Address already in use\n",
                encoding="utf-8",
            )
            with mock.patch.object(
                module, "ensure_required_binaries"
            ), mock.patch.object(
                module,
                "start_proxy",
                return_value=(
                    ExitedProcess(),
                    pathlib.Path(temp_dir) / "runtime-config.yaml",
                    pathlib.Path(temp_dir) / "proxy.stdout.log",
                    proxy_stderr_path,
                ),
            ), mock.patch.object(
                module.urllib.request,
                "urlopen",
                return_value=OldProxyHealthResponse(),
            ), mock.patch.object(
                module, "refresh_target_model_profiles"
            ), mock.patch.object(
                module, "probe_target", return_value=None
            ), mock.patch.object(
                module,
                "run_matrix_case",
                return_value={
                    "case_id": "unexpected",
                    "client": "codex",
                    "target": "minimax-openai",
                    "fixture": "smoke_pong",
                    "status": "passed",
                    "message": "",
                },
            ) as run_matrix_case:
                with self.assertRaisesRegex(
                    RuntimeError,
                    "proxy process exited.*Address already in use",
                ):
                    module.run(
                        [
                            "--config-source",
                            str(DEFAULT_CONFIG_PATH),
                            "--env-file",
                            str(env_file),
                            "--fixtures-root",
                            str(REPO_ROOT / "scripts" / "fixtures" / "cli_matrix"),
                            "--reports-root",
                            str(reports_root),
                            "--binary",
                            str(binary_path),
                            "--proxy-port",
                            "18888",
                            "--proxy-health-timeout-secs",
                            "1",
                        ]
                    )

        run_matrix_case.assert_not_called()

    def test_run_defaults_to_auto_proxy_port_and_writes_explicit_port_to_runtime_config(self):
        module = load_module()

        class FakeProcess:
            def poll(self):
                return None

            def wait(self, timeout=None):
                return 0

        def available_port() -> int:
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                return int(sock.getsockname()[1])

        observed_runtime_configs: list[str] = []
        observed_health_urls: list[str] = []
        observed_health_kwargs: list[dict[str, object]] = []

        def fake_start_proxy(proxy_binary, runtime_config_text, report_dir, proxy_env):
            observed_runtime_configs.append(runtime_config_text)
            return (
                FakeProcess(),
                pathlib.Path(report_dir) / "runtime-config.yaml",
                pathlib.Path(report_dir) / "proxy.stdout.log",
                pathlib.Path(report_dir) / "proxy.stderr.log",
            )

        def fake_wait_for_health(base_url, **kwargs):
            observed_health_urls.append(base_url)
            observed_health_kwargs.append(kwargs)

        explicit_port = available_port()
        env_port = available_port()
        with tempfile.TemporaryDirectory() as temp_dir:
            temp_root = pathlib.Path(temp_dir)
            reports_root = temp_root / "reports"
            binary_path = temp_root / "fake-proxy"
            env_file = write_preset_endpoint_env_file(temp_root / ".env.test")
            common_args = [
                "--proxy-only",
                "--config-source",
                str(DEFAULT_CONFIG_PATH),
                "--env-file",
                str(env_file),
                "--fixtures-root",
                str(REPO_ROOT / "scripts" / "fixtures" / "cli_matrix"),
                "--reports-root",
                str(reports_root),
                "--binary",
                str(binary_path),
            ]
            with mock.patch.dict(
                os.environ,
                {"PATH": os.environ.get("PATH", "")},
                clear=True,
            ), mock.patch.object(
                module, "ensure_required_binaries"
            ), mock.patch.object(
                module, "start_proxy", side_effect=fake_start_proxy
            ), mock.patch.object(
                module, "wait_for_health", side_effect=fake_wait_for_health
            ), mock.patch.object(
                module, "stop_proxy"
            ), mock.patch.object(
                module, "free_port", return_value=23456
            ):
                auto_exit_code = module.run(common_args)
                explicit_exit_code = module.run(
                    common_args + ["--proxy-port", str(explicit_port)]
                )
                os.environ["PROXY_PORT"] = str(env_port)
                env_exit_code = module.run(common_args)

        self.assertEqual(auto_exit_code, 0)
        self.assertEqual(explicit_exit_code, 0)
        self.assertEqual(env_exit_code, 0)
        self.assertIn("listen: 127.0.0.1:23456", observed_runtime_configs[0])
        self.assertNotIn("listen: 127.0.0.1:18888", observed_runtime_configs[0])
        self.assertEqual(observed_health_urls[0], "http://127.0.0.1:23456")
        self.assertIn(
            f"listen: 127.0.0.1:{explicit_port}",
            observed_runtime_configs[1],
        )
        self.assertEqual(
            observed_health_urls[1],
            f"http://127.0.0.1:{explicit_port}",
        )
        self.assertIn(
            f"listen: 127.0.0.1:{env_port}",
            observed_runtime_configs[2],
        )
        self.assertEqual(
            observed_health_urls[2],
            f"http://127.0.0.1:{env_port}",
        )
        for health_kwargs in observed_health_kwargs:
            self.assertIsInstance(health_kwargs.get("process"), FakeProcess)
            self.assertTrue(str(health_kwargs.get("stdout_path")).endswith("proxy.stdout.log"))
            self.assertTrue(str(health_kwargs.get("stderr_path")).endswith("proxy.stderr.log"))

    def test_run_proxy_only_skips_client_binary_checks(self):
        module = load_module()

        class FakeProcess:
            def poll(self):
                return 0

            def wait(self, timeout=None):
                return 0

        observed = {}

        def fake_ensure_required_binaries(clients, proxy_binary):
            observed["clients"] = list(clients)
            observed["proxy_binary"] = proxy_binary

        with tempfile.TemporaryDirectory() as temp_dir:
            temp_root = pathlib.Path(temp_dir)
            reports_root = temp_root / "reports"
            binary_path = temp_root / "fake-proxy"
            env_file = write_preset_endpoint_env_file(temp_root / ".env.test")
            runtime_config_path = temp_root / "runtime-config.yaml"
            proxy_stdout_path = temp_root / "proxy.stdout.log"
            proxy_stderr_path = temp_root / "proxy.stderr.log"
            stdout = io.StringIO()
            with mock.patch.object(
                module, "ensure_required_binaries", side_effect=fake_ensure_required_binaries
            ), mock.patch.object(
                module,
                "selected_clients",
                side_effect=AssertionError(
                    "selected_clients should not run in proxy-only mode"
                ),
            ), mock.patch.object(
                module,
                "start_proxy",
                return_value=(
                    FakeProcess(),
                    runtime_config_path,
                    proxy_stdout_path,
                    proxy_stderr_path,
                ),
            ), mock.patch.object(module, "wait_for_health"), mock.patch.object(
                module, "stop_proxy"
            ), mock.patch.object(
                module, "free_port", return_value=23456
            ), mock.patch(
                "sys.stdout", stdout
            ):
                exit_code = module.run(
                    [
                        "--proxy-only",
                        "--config-source",
                        str(DEFAULT_CONFIG_PATH),
                        "--env-file",
                        str(env_file),
                        "--fixtures-root",
                        str(REPO_ROOT / "scripts" / "fixtures" / "cli_matrix"),
                        "--reports-root",
                        str(reports_root),
                        "--binary",
                        str(binary_path),
                    ]
                )

        self.assertEqual(exit_code, 0)
        self.assertEqual(observed["clients"], [])
        self.assertEqual(observed["proxy_binary"], binary_path)
        self.assertIn("Proxy healthy at http://127.0.0.1:23456", stdout.getvalue())

    def test_resolve_cli_args_requires_explicit_dangerous_harness_opt_in(self):
        module = load_module()

        safe_args = module.resolve_cli_args(["--list-matrix"])
        dangerous_args = module.resolve_cli_args(["--list-matrix", "--dangerous-harness"])

        self.assertFalse(safe_args.dangerous_harness)
        self.assertTrue(dangerous_args.dangerous_harness)

    def test_docs_publish_the_locked_tool_identity_contract(self):
        for path in LOCKED_TOOL_CONTRACT_SPEC_PATHS:
            with self.subTest(path=path.name):
                text = path.read_text(encoding="utf-8")
                for line in LOCKED_TOOL_CONTRACT_LINES:
                    self.assertIn(line, text)

    def test_readme_points_to_contract_docs_without_promoting_reserved_prefix_as_public_contract(self):
        text = (REPO_ROOT / "README.md").read_text(encoding="utf-8")

        for snippet in README_DOC_ENTRY_SNIPPETS:
            with self.subTest(required_entry=snippet):
                self.assertIn(snippet, text)

        for pattern in README_FORBIDDEN_RESERVED_PREFIX_PUBLIC_CONTRACT_PATTERNS:
            with self.subTest(forbidden_pattern=pattern):
                self.assertNotRegex(text, pattern)

    def test_docs_do_not_describe_reserved_prefix_bridge_as_current_limitation_or_fallback(self):
        for path, forbidden_snippets in FORBIDDEN_LEGACY_TOOL_IDENTITY_LANGUAGE.items():
            with self.subTest(path=path.name):
                text = path.read_text(encoding="utf-8")
                for snippet in forbidden_snippets:
                    self.assertNotIn(snippet, text)

    def test_maximum_compatibility_docs_publish_stable_name_request_scoped_bridge_direction(self):
        for path, required_snippets in REQUIRED_TOOL_BRIDGE_DIRECTION_LANGUAGE.items():
            with self.subTest(path=path.name):
                text = path.read_text(encoding="utf-8")
                for snippet in required_snippets:
                    self.assertIn(snippet, text)


if __name__ == "__main__":
    unittest.main()
