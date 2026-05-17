import pathlib
import re
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]


def read_doc(relative_path: str) -> str:
    return (REPO_ROOT / relative_path).read_text(encoding="utf-8")


def active_public_markdown_docs() -> list[pathlib.Path]:
    docs = [REPO_ROOT / "README.md", REPO_ROOT / "README_CN.md"]
    for path in sorted((REPO_ROOT / "docs").rglob("*.md")):
        relative_parts = path.relative_to(REPO_ROOT).parts
        if relative_parts[:2] == ("docs", "engineering"):
            continue
        if relative_parts[:3] in (
            ("docs", "protocol-baselines", "audits"),
            ("docs", "protocol-baselines", "snapshots"),
        ):
            continue
        docs.append(path)
    return docs


def table_row(text: str, first_cell: str) -> str:
    pattern = re.compile(rf"^\|\s*{re.escape(first_cell)}\s*\|.*$", re.MULTILINE)
    match = pattern.search(text)
    if match is None:
        raise AssertionError(f"missing table row for {first_cell!r}")
    return match.group(0)


class ProtocolDocsContractTests(unittest.TestCase):
    def test_protocol_matrix_names_compatible_provider_ga_routes_precisely(self):
        text = read_doc("docs/protocol-compatibility-matrix.md")

        for snippet in (
            "OpenAI-compatible chat-completions route `/openai/v1/chat/completions`",
            "Anthropic-compatible messages route `/anthropic/v1/messages`",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)
        for forbidden in (
            "OpenAI-compatible completions/chat-completions",
            "OpenAI-compatible completions surface",
            "OpenAI-compatible completions and",
            "Gemini `generateContent` | Where to read more",
            "OpenAI-to-Gemini tool-call translation",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, text)
        self.assertIn("Google OpenAI-compatible upstream", text)
        self.assertIn("`format: openai-completion`", text)

    def test_prd_uses_single_maximum_safe_compatibility_boundary(self):
        text = read_doc("docs/PRD.md")

        self.assertNotIn("same-protocol paths stay native", text)
        for snippet in (
            "Same-wire-protocol native-byte/native-field preservation is an internal request-processing optimization",
            "maximum safe compatibility",
            "provider-native request-control preservation or documented explicit mapping plus usage telemetry",
            "MUST NOT introduce a `llmup` cache store",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_prd_translation_pipeline_records_internal_forwarding_boundary(self):
        text = read_doc("docs/PRD.md")
        pipeline = text.split("### 4.2 Translation Pipeline", 1)[1].split(
            "\n### 4.3", 1
        )[0]

        for snippet in (
            "unified routing, capability, and hard-boundary checks",
            "no body mutation or response normalization is required",
            "construct the upstream request to preserve the maximum safe portable representation",
            "Apply hard portability boundaries before upstream",
            "internal request-processing optimization",
            "not user configurable and is not a product metric",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, pipeline)

    def test_prompt_cache_docs_do_not_plan_cache_affinity_routing(self):
        paths = (
            "docs/engineering/pre-ga-request-processing-prompt-cache-support-plan.md",
            "docs/engineering/pre-ga-conversation-state-bridge-plan.md",
            "docs/engineering/pre-ga-remove-native-gemini-format-plan.md",
            "docs/PRD.md",
            "docs/protocol-compatibility-matrix.md",
            "docs/README.md",
            "README.md",
            "README_CN.md",
        )
        forbidden = (
            "provider-cache affinity",
            "cache warmth",
            "cache-warm",
            "cache_warm",
            "sticky routing",
            "routing-affinity",
            "policy-enabled translated route",
        )

        for path in paths:
            text = read_doc(path)
            for snippet in forbidden:
                with self.subTest(path=path, snippet=snippet):
                    self.assertNotIn(snippet, text)

        plan = read_doc(
            "docs/engineering/pre-ga-request-processing-prompt-cache-support-plan.md"
        )
        self.assertIn("Cache usage telemetry must not influence routing", plan)
        self.assertIn(
            "explicit provider-native request controls and read-only usage telemetry",
            plan,
        )
        self.assertIn(
            "Automatic provider cache key, marker, or breakpoint insertion",
            plan,
        )

    def test_prompt_cache_docs_do_not_plan_automatic_controls(self):
        docs = (
            "docs/engineering/pre-ga-request-processing-prompt-cache-support-plan.md",
            "docs/engineering/pre-ga-conversation-state-bridge-plan.md",
            "docs/max-compat-design.md",
            "docs/protocol-baselines/capabilities/cache.md",
        )
        forbidden = (
            "implementation_reviewed_" + "breakpoints",
            "target-provider request-control " + "synthe" + "sis",
            "preserved or " + "synthesi" + "zed",
            "synthe" + "size only " + "explicit",
            "prompt-cache key or breakpoint " + "synthe" + "sis",
            "Provider-cache auto-injection must be " + "policy-driven",
            "reviewed implementation " + "contract",
        )

        for path in docs:
            text = read_doc(path)
            for snippet in forbidden:
                with self.subTest(path=path, snippet=snippet):
                    self.assertNotIn(snippet, text)

        state_bridge = read_doc(
            "docs/engineering/pre-ga-conversation-state-bridge-plan.md"
        )
        self.assertIsNone(
            re.search(
                r"stable\s+prefix.{0,160}(?:prompt_cache_key|breakpoint)",
                state_bridge,
                re.IGNORECASE | re.DOTALL,
            )
        )

        cache_baseline = read_doc("docs/protocol-baselines/capabilities/cache.md")
        self.assertIn(
            "explicit target-provider request-control mapping",
            cache_baseline,
        )
        self.assertIn(
            "Provider-cache auto-injection is out of scope; explicit mapping must be trace-visible.",
            cache_baseline,
        )

    def test_constitution_records_single_goal_and_narrow_state_exception(self):
        text = read_doc("docs/CONSTITUTION.md")

        for snippet in (
            "product behavior is maximum safe compatibility",
            "Native Preservation Is Internal",
            "not a separate product behavior",
            "conversation_state_bridge.mode=memory",
            "explicit transcript expansion adapter",
            "memory-only",
            "resp_llmup_*",
            "process restart",
            "external provider IDs fail closed",
            "not persistent conversation state",
            "not a response cache",
            "not another compatibility goal",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_public_product_docs_and_python_contracts_reject_old_product_strategy_language(self):
        paths = [
            *active_public_markdown_docs(),
            REPO_ROOT / "tests" / "test_protocol_docs_contract.py",
            REPO_ROOT / "tests" / "test_docs_homepage_contract.py",
            REPO_ROOT / "tests" / "test_ga_docs_contract.py",
            REPO_ROOT / "tests" / "test_release_gates.py",
        ]
        forbidden = (
            "execution " + "lane",
            "translation " + "lane",
            "primary execution " + "path",
            "Raw" + "Provider" + "Passthrough",
            "Maximum" + "Compatibility" + "Translation",
            "Provider" + "Prompt" + "Cache" + "Optimized",
            "compatibility " + "tier",
            "compatibility " + "level",
            "product " + "tier",
            "route " + "mode",
            "operator " + "mode",
            "product " + "lane",
            "primary " + "lane",
            "intended " + "lane",
            "proxy " + "lanes",
            "OpenAI-compatible " + "lane",
            "Anthropic-compatible " + "lane",
            "MiniMax is an OpenAI-compatible " + "lane",
            "raw provider " + "passthrough",
            "raw " + "passthrough",
            "native " + "passthrough",
            "Native Responses " + "passthrough",
            "native Responses " + "passthrough",
            "Native OpenAI Responses " + "passthrough",
            "native OpenAI Responses " + "passthrough",
            "raw/native " + "passthrough",
            "raw same-protocol " + "passthrough",
            "passthrough " + "lane",
            "passthrough " + "lanes",
            "passthrough " + "only",
            "passthrough " + "path",
            "passthrough " + "paths",
            "raw/native " + "lane",
            "lane " + "is available",
            "strict" + "/balanced",
            "default" + "/" + "max" + "_compat",
            "compatible " + "same-protocol " + "lane",
            "same" + "-provider/native " + "passthrough",
            "same" + "-provider native " + "passthrough",
            "compatibility" + "_mode",
            "provider prompt-cache optimized " + "lane",
            "raw " + "passthrough, provider prompt-cache optimization, "
            + "or maximum-compatible translation",
        )

        for path in paths:
            relative_path = path.relative_to(REPO_ROOT)
            text = path.read_text(encoding="utf-8")
            for snippet in forbidden:
                with self.subTest(path=str(relative_path), snippet=snippet):
                    self.assertNotIn(snippet, text)

    def test_active_protocol_baseline_docs_reject_old_forwarding_path_language(self):
        paths = (
            "docs/protocol-baselines/capabilities/reasoning.md",
            "docs/protocol-baselines/capabilities/state-continuity.md",
            "docs/protocol-baselines/capabilities/cache.md",
            "docs/protocol-baselines/capabilities/tools.md",
            "docs/protocol-baselines/matrices/field-mapping-matrix.md",
            "docs/protocol-baselines/matrices/provider-capability-matrix.md",
            "docs/admin-dynamic-config.md",
        )
        forbidden = (
            "raw/native " + "forwarding",
            "raw same-protocol " + "forwarding",
            "raw " + "forwarding",
            "byte-preserving " + "forwarding",
            "passthrough " + "only",
            "passthrough " + "path",
            "passthrough " + "lane",
            "route " + "mode",
            "operator " + "mode",
            "product " + "lane",
            "compatibility " + "level",
            "compatibility " + "tier",
            "forwarding " + "only",
        )

        for path in paths:
            text = read_doc(path).casefold()
            for snippet in forbidden:
                with self.subTest(path=path, snippet=snippet):
                    self.assertNotIn(snippet.casefold(), text)

    def test_active_docs_do_not_promise_native_gemini_wire_format(self):
        active_docs = (
            "docs/README.md",
            "docs/PRD.md",
            "docs/CONSTITUTION.md",
            "docs/DESIGN.md",
            "docs/PROJECT.md",
            "docs/ga-readiness-review.md",
            "docs/max-compat-design.md",
            "docs/container.md",
        )
        forbidden = (
            "Gemini wrapper setup plus common client notes",
            "Google Gemini should be able to route",
            "all four protocols",
            "| 4 | OpenAI Chat Completions | Google Gemini |",
            "| 13 | Google Gemini | OpenAI Chat Completions |",
            "All 16 combinations",
            "**Google Gemini**: `candidates` chunks",
            "`systemInstruction` (Gemini)",
            "`functionCall` parts (Gemini)",
            "`functionResponse` parts (Gemini)",
            "`promptTokenCount/candidatesTokenCount`",
            "`thought` parts (Gemini)",
            "Data plane HTTP API for OpenAI, Anthropic, and Google/Gemini-compatible clients",
            "Main request execution "
            + "path for OpenAI, Anthropic, and Gemini surfaces",
            "Gemini GenerateContent unary",
            "official Gemini live smoke",
            "Gemini `replace`",
            "Gemini request shapes",
            "Gemini to OpenAI Chat/Responses",
            "OpenAI Chat/Responses to Gemini",
        )

        combined = "\n".join(read_doc(path) for path in active_docs)
        for snippet in forbidden:
            with self.subTest(snippet=snippet):
                self.assertNotIn(snippet, combined)

        self.assertIn("Google OpenAI-compatible Gemini", combined)
        self.assertIn("`format: openai-completion`", combined)

    def test_active_protocol_baselines_do_not_model_native_gemini_as_active_surface(self):
        active_baselines = (
            "docs/protocol-baselines/capabilities/cache.md",
            "docs/protocol-baselines/capabilities/reasoning.md",
            "docs/protocol-baselines/capabilities/state-continuity.md",
            "docs/protocol-baselines/capabilities/streaming.md",
            "docs/protocol-baselines/capabilities/tools.md",
            "docs/protocol-baselines/matrices/field-mapping-matrix.md",
            "docs/protocol-baselines/matrices/provider-capability-matrix.md",
        )
        forbidden = (
            "Gemini `generateContent`",
            "Gemini `streamGenerateContent`",
            "`functionDeclarations`",
            "`functionCall`",
            "`functionResponse`",
            "`thinkingConfig`",
            "`cachedContent`",
            "`inlineData`",
            "`fileData`",
            "`mcpServers`",
            "`toolConfig`",
            "`functionCallingConfig`",
            "Gemini video",
            "Gemini-routed",
            "Gemini-native",
            "Gemini built-in",
        )

        combined = "\n".join(read_doc(path) for path in active_baselines)
        for snippet in forbidden:
            with self.subTest(snippet=snippet):
                self.assertNotIn(snippet, combined)

        self.assertIn("Google OpenAI-compatible", combined)
        self.assertIn("retired historical", combined)

    def test_root_protocol_baselines_mark_native_gemini_support_or_routes_retired(self):
        native_gemini_signals = (
            "Gemini `generateContent`",
            "`streamGenerateContent`",
            "/google/v1beta",
            "Gemini request parsing",
            "OpenAI/Anthropic/Gemini request and response translation",
        )
        retired_or_non_active_markers = (
            "retired historical",
            "not active",
            "no longer an active",
            "removed",
            "non-active",
        )

        baseline_docs = sorted(
            (REPO_ROOT / "docs" / "protocol-baselines").glob("*.md")
        )
        for path in baseline_docs:
            text = path.read_text(encoding="utf-8")
            if not any(signal in text for signal in native_gemini_signals):
                continue

            relative_path = path.relative_to(REPO_ROOT)
            with self.subTest(path=str(relative_path)):
                lower_text = text.casefold()
                self.assertTrue(
                    any(
                        marker in lower_text
                        for marker in retired_or_non_active_markers
                    ),
                    (
                        f"{relative_path} contains native Gemini support/route "
                        "language without an explicit retired/non-active marker"
                    ),
                )

    def test_latest_protocol_audit_is_historical_without_stale_strategy_terms(self):
        matrix = read_doc("docs/protocol-compatibility-matrix.md")
        audit_path = "docs/protocol-baselines/audits/2026-05-16-online-recheck.md"
        audit = read_doc(audit_path)

        self.assertIn(
            "Latest refresh audit and historical implementation-risk notes",
            matrix,
        )
        self.assertIn(f"({audit_path.removeprefix('docs/')})", matrix)
        for snippet in (
            "historical audit",
            "not an active implementation plan",
            "internal forwarding optimization",
            "native Gemini removal plan",
            "no longer a planned follow-up",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, audit)

        forbidden = (
            "active online " + "recheck",
            "raw/native " + "passthrough " + "lanes",
            "passthrough " + "lanes",
            "Gemini cache resource " + "adapter",
            "Proxy Gemini `" + "cachedContents`",
        )
        for snippet in forbidden:
            with self.subTest(snippet=snippet):
                self.assertNotIn(snippet, audit)

    def test_tools_baseline_preserves_hosted_tools_only_on_native_same_wire_or_shimmed_handling(self):
        text = read_doc("docs/protocol-baselines/capabilities/tools.md")
        row = table_row(text, "Hosted / server tools")

        self.assertNotIn("Preserve same-protocol hosted tools only", row)
        for snippet in (
            "provider-native same-wire handling",
            "no body mutation or response normalization is required",
            "explicit compatibility shims",
            "Cross-provider translation should warn/drop or fail closed",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, row)
        self.assertIn(
            "Allow hosted/server tools only through provider-native same-wire handling",
            text,
        )

    def test_maximum_compatibility_design_uses_single_goal_and_native_preservation_boundary(self):
        text = read_doc("docs/max-compat-design.md")

        self.assertNotIn("same-protocol paths: native " + "passthrough", text)
        for snippet in (
            "one product goal: maximum safe compatibility",
            "Single Product Behavior",
            "same-wire native preservation",
            "same-wire native preservation is an internal optimization",
            "runtime and config surface expose no compatibility switch",
            "hard portability boundary",
            "provider prompt-cache support is provider-native request-control support",
            "not a separate product behavior",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_maximum_compatibility_records_request_construction_and_native_preservation_facts(self):
        text = read_doc("docs/max-compat-design.md")

        for snippet in (
            "Request construction always uses maximum safe compatibility",
            "Same-format zero-transform handling",
            "Native Responses same-wire handling preserves `context_management`",
            "`include` values such as `reasoning.encrypted_content`",
            "input reasoning and compaction items with `encrypted_content`",
            "exactly one native OpenAI Responses upstream",
            "does not reconstruct provider state",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_maximum_compatibility_records_reasoning_and_compaction_degrade_rules(self):
        text = read_doc("docs/max-compat-design.md")

        self.assertNotIn("safe reasoning carriers", text)
        for snippet in (
            "request-side reasoning encrypted_content",
            "include `reasoning.encrypted_content`",
            "request-side compaction input",
            "maximum safe compatibility",
            "visible summary",
            "visible transcript/history",
            "opaque-only reasoning",
            "opaque-only compaction",
            "one summarized compaction item does not permit another opaque-only compaction item",
            "Native Responses same-wire handling",
            "response-side reasoning encrypted_content",
            "Anthropic carrier recovery path",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_field_mapping_drop_statuses_are_not_ambiguous(self):
        text = read_doc("docs/protocol-baselines/matrices/field-mapping-matrix.md")

        for snippet in (
            "Fail-closed",
            "Warn/drop opaque carrier",
            "Native-only",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

        for first_cell in ("Reasoning opaque state", "Compaction"):
            row = table_row(text, first_cell)
            with self.subTest(row=first_cell):
                self.assertIn("Warn/drop opaque carrier", row)
                self.assertNotRegex(row, r"\|\s*Drop\s*\|")

    def test_baseline_readme_separates_vendor_contract_from_proxy_policy(self):
        readme = read_doc("docs/protocol-baselines/README.md")

        for snippet in (
            "vendor contract",
            "proxy posture",
            "snapshot/source facts",
            "proxy policy",
            "vendor snapshot/captured date",
            "proxy posture updated date",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet.casefold(), readme.casefold())

        responses = read_doc("docs/protocol-baselines/openai-responses.md")
        self.assertIn("`captured_at_utc`: `2026-04-17T06:59:44Z`", responses)
        self.assertIn("`snapshot_bucket`: `2026-04-16`", responses)
        self.assertIn("`proxy_posture_updated`: `2026-04-26`", responses)

    def test_ga_source_spot_check_is_recorded_without_full_snapshot_refresh(self):
        audit_path = (
            REPO_ROOT
            / "docs"
            / "protocol-baselines"
            / "audits"
            / "2026-04-27-ga-source-spot-check.md"
        )
        self.assertTrue(audit_path.exists(), "GA source spot-check audit is missing")
        audit = audit_path.read_text(encoding="utf-8")
        readme = read_doc("docs/protocol-baselines/README.md")
        overview = read_doc("docs/protocol-baselines/overview.md")

        self.assertIn(
            "audits/2026-04-27-ga-source-spot-check.md",
            readme + overview,
        )
        for snippet in (
            "not a full recertification",
            "not a full snapshot refresh",
            "2026-04-16 snapshot bucket",
            "captured baseline",
            "2026-04-27 spot-check",
            "GA portability contract",
            "OpenAI Responses",
            "Conversations",
            "compact",
            "Gemini generateContent",
            "Anthropic 2026-04-23/24 release notes",
            "Rate Limits API",
            "Managed Agents memory",
            "portable-core contract",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, audit)

    def test_development_plan_records_interactive_codex_wrapper_gate(self):
        text = read_doc("docs/engineering/max-compat-development-plan.md")

        self.assertIn("hermetic scripted interactive Codex wrapper gate", text)

    def test_development_plan_records_single_maximum_compatibility_runtime(self):
        text = read_doc("docs/engineering/max-compat-development-plan.md")

        for snippet in (
            "single maximum compatibility runtime",
            "runtime translated paths always use maximum safe compatibility",
            "single maximum safe compatibility strategy",
            "runtime and config surfaces expose no compatibility switch",
            "raw same-protocol " + "forwarding is an internal optimization",
            "fail-closed behavior is a hard portability boundary",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)
        self.assertNotIn("compatibility-" + "policy plumbing", text)
        self.assertNotIn("tier model", text)

    def test_active_maximum_compat_docs_do_not_name_removed_runtime_compatibility_field(self):
        old_field = "compatibility" + "_mode"
        for relative_path in (
            "docs/max-compat-design.md",
            "docs/engineering/max-compat-development-plan.md",
        ):
            text = read_doc(relative_path)
            with self.subTest(relative_path=relative_path):
                self.assertNotIn(old_field, text)

    def test_reasoning_docs_do_not_overstate_opaque_continuity_rules(self):
        for relative_path in (
            "docs/max-compat-design.md",
            "docs/protocol-baselines/capabilities/reasoning.md",
        ):
            text = read_doc(relative_path)
            with self.subTest(relative_path=relative_path):
                self.assertNotIn("safe reasoning carriers", text)
                self.assertNotIn(
                    "provider-specific effort controls across protocol boundaries",
                    text,
                )


if __name__ == "__main__":
    unittest.main()
