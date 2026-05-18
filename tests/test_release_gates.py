import json
import os
import importlib.util
import pathlib
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
INSTALLER_SCRIPT = REPO_ROOT / "install.sh"
RELEASE_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "release.yml"
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
GOVERNANCE_SCRIPT = REPO_ROOT / "scripts" / "check-governance.sh"
SUPPLY_CHAIN_AUDIT_SCRIPT = REPO_ROOT / "scripts" / "supply_chain_audit.sh"
CHECKED_IN_CONTAINER_IMAGE_MANIFEST = (
    REPO_ROOT / "docs" / "release-artifacts" / "container-image.json"
)
SUPPLY_CHAIN_AUDIT_COMMAND = "bash scripts/supply_chain_audit.sh"
LOCKFILE_INTEGRITY_COMMAND = "cargo metadata --locked --format-version 1 --no-deps"
ENDPOINT_MATRIX_SCRIPT = REPO_ROOT / "scripts" / "real_endpoint_matrix.py"
PYTHON_CONTRACT_TEST_COMMAND = (
    "PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p 'test*.py'"
)
CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND = (
    "PYTHONDONTWRITEBYTECODE=1 python3 -m unittest "
    "tests.test_interactive_cli.InteractiveCliTests."
    "test_codex_wrapper_executes_scripted_interactive_two_turns_hermetically"
)
REQUIRED_RELEASE_GATE_NEEDS = (
    "mock-endpoint-matrix",
    "cli-wrapper-matrix",
    "installer-smoke",
    "perf-gate",
    "compatible-provider-smoke",
    "supply-chain",
)
OFFICIAL_PROVIDER_SECRET_ENVS = (
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "MINIMAX_API_KEY",
)
COMPAT_PROVIDER_SECRET_ENVS = (
    "COMPAT_PROVIDER_API_KEY",
    "COMPAT_OPENAI_API_KEY",
    "COMPAT_ANTHROPIC_API_KEY",
)
COMPAT_PROVIDER_VAR_ENVS = (
    "COMPAT_OPENAI_BASE_URL",
    "COMPAT_OPENAI_MODEL",
    "COMPAT_ANTHROPIC_BASE_URL",
    "COMPAT_ANTHROPIC_MODEL",
    "COMPAT_PROVIDER_LABEL",
)
COMPAT_PROVIDER_SMOKE_JSON = "artifacts/compatible-provider-smoke.json"
PUSHED_CONTAINER_IMAGE_MANIFEST_JSON = "artifacts/container-image.json"
RELEASE_PUBLISH_JOB_MARKERS = (
    "push: true",
    "packages: write",
    "action-gh-release",
)
INSTALLER_SMOKE_COMMANDS = (
    "llm-universal-proxy --help",
    "llm-universal-proxy --version",
    "llmup-config --help",
    "llmup-config --version",
    "llmup-codex --llmup-help",
    "llmup-codex --llmup-version",
    "llmup-claude --llmup-help",
    "llmup-claude --llmup-version",
)


def load_endpoint_matrix_module():
    spec = importlib.util.spec_from_file_location(
        "real_endpoint_matrix_release_gate_contract",
        ENDPOINT_MATRIX_SCRIPT,
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def workflow_jobs(workflow_path: pathlib.Path):
    text = workflow_path.read_text(encoding="utf-8")
    matches = list(re.finditer(r"^  ([A-Za-z0-9_-]+):\n", text, re.MULTILINE))
    jobs = {}
    for index, match in enumerate(matches):
        job_name = match.group(1)
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        jobs[job_name] = text[match.start() : end]
    return jobs


def release_workflow_jobs():
    return workflow_jobs(RELEASE_WORKFLOW)


def job_needs(job_block: str):
    match = re.search(r"^    needs:\s*(?P<value>.*)$", job_block, re.MULTILINE)
    if not match:
        return set()

    value = match.group("value").strip()
    if value.startswith("[") and value.endswith("]"):
        return {
            item.strip().strip("\"'")
            for item in value.removeprefix("[").removesuffix("]").split(",")
            if item.strip()
        }
    if value:
        return {value.strip("\"'")}

    needs = set()
    for line in job_block[match.end() :].splitlines():
        if line.startswith("    ") and not line.startswith("      "):
            break
        item_match = re.match(r"^\s*-\s*([A-Za-z0-9_-]+)\s*$", line)
        if item_match:
            needs.add(item_match.group(1))
    return needs


def compatible_provider_smoke_invocation_lines(text: str):
    return [
        line.strip()
        for line in text.splitlines()
        if "python3 scripts/real_endpoint_matrix.py" in line
        and "--mode compatible-provider-smoke" in line
    ]


def workflow_step_block(text: str, step_name: str) -> str:
    marker = f"      - name: {step_name}"
    start = text.find(marker)
    if start == -1:
        return ""
    next_step = text.find("\n      - name: ", start + len(marker))
    if next_step == -1:
        return text[start:]
    return text[start:next_step]


def workflow_step_inline_python(step_block: str) -> str:
    marker = "          python3 - <<'PY'\n"
    start = step_block.find(marker)
    if start == -1:
        return ""
    start += len(marker)
    end = step_block.find("\n          PY", start)
    if end == -1:
        return ""

    lines = []
    for line in step_block[start:end].splitlines():
        if line.startswith("          "):
            line = line[10:]
        lines.append(line)
    return "\n".join(lines) + "\n"


def published_timestamp_fields(manifest: dict) -> set[str]:
    return {"published_at", "released_at"} & set(manifest["published"])


def sha256_file(path: pathlib.Path) -> str:
    import hashlib

    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


class ReleaseGateWorkflowContractTests(unittest.TestCase):
    def read_text(self, relative_path: str) -> str:
        return (REPO_ROOT / relative_path).read_text(encoding="utf-8")

    def write_fake_release_asset(
        self,
        release_dir: pathlib.Path,
        asset_name: str = "llm-universal-proxy-linux-x86_64.tar.gz",
        *,
        entry_name: str = "llm-universal-proxy",
        executable_text: str | None = None,
        checksum: str | None = None,
    ) -> pathlib.Path:
        payload_dir = release_dir / "payload"
        payload_dir.mkdir(exist_ok=True)
        binary = payload_dir / "llm-universal-proxy"
        binary.write_text(
            executable_text
            or """#!/bin/sh
set -eu
name=${0##*/}
arg=${1:-}
case "$name:$arg" in
  llm-universal-proxy:--help|llm-universal-proxy:--version|llmup-config:--help|llmup-config:--version|llmup-codex:--llmup-help|llmup-codex:--llmup-version|llmup-claude:--llmup-help|llmup-claude:--llmup-version)
    printf '%s %s\\n' "$name" "$arg"
    exit 0
    ;;
esac
printf 'unexpected invocation: %s %s\\n' "$name" "$arg" >&2
exit 64
""",
            encoding="utf-8",
        )
        binary.chmod(0o755)

        archive = release_dir / asset_name
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(binary, arcname=entry_name)
        (release_dir / f"{asset_name}.sha256").write_text(
            f"{checksum or sha256_file(archive)}  {asset_name}\n",
            encoding="utf-8",
        )
        shutil.rmtree(payload_dir)
        return archive

    def run_installer(
        self,
        tmp_path: pathlib.Path,
        release_dir: pathlib.Path,
        *args: str,
        extra_env: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(tmp_path / "home"),
                "LLMUP_INSTALL_BASE_URL": f"file://{release_dir}",
                "LLMUP_INSTALL_UNAME_S": "Linux",
                "LLMUP_INSTALL_UNAME_M": "x86_64",
            }
        )
        if extra_env:
            env.update(extra_env)
        (tmp_path / "home").mkdir(exist_ok=True)
        return subprocess.run(
            ["sh", str(INSTALLER_SCRIPT), *args],
            cwd=tmp_path,
            env=env,
            text=True,
            capture_output=True,
        )

    def assert_has_compatible_provider_smoke_invocation(self, text: str):
        invocation_lines = compatible_provider_smoke_invocation_lines(text)
        self.assertTrue(
            invocation_lines,
            "compatible provider smoke must invoke real_endpoint_matrix.py with "
            "--mode compatible-provider-smoke",
        )
        self.assertTrue(
            any("--json-out" in line and COMPAT_PROVIDER_SMOKE_JSON in line for line in invocation_lines),
            "compatible provider smoke must emit the machine-readable JSON artifact",
        )

    def test_release_workflow_contains_ga_release_gates(self):
        release = RELEASE_WORKFLOW.read_text(encoding="utf-8")

        required_snippets = (
            "Run Rust tests",
            "cargo test --locked --verbose",
            "Run Python contract tests",
            PYTHON_CONTRACT_TEST_COMMAND,
            "Check version, toolchain, and Secret Scan governance",
            "bash scripts/check-governance.sh",
            "Run container smoke",
            "bash scripts/test_container_smoke.sh",
            "Mock Endpoint Matrix",
            "python3 scripts/real_endpoint_matrix.py --mock",
            "CLI Wrapper Matrix",
            "python3 scripts/real_cli_matrix.py --test basic --skip-slow --list-matrix",
            CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND,
            "Installer Smoke",
            "Prepare install.sh release asset",
            "Upload install.sh release asset",
            "Run installer smoke",
            "Perf Gate",
            "python3 scripts/real_endpoint_matrix.py --mock --perf",
            "Supply Chain",
            SUPPLY_CHAIN_AUDIT_COMMAND,
            "anchore/sbom-action",
            "Compatible Provider Smoke",
            "environment: release-compatible-provider",
            COMPAT_PROVIDER_SMOKE_JSON,
        )

        for snippet in required_snippets:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, release)
        self.assert_has_compatible_provider_smoke_invocation(release)

        self.assertRegex(
            release,
            r"release:\n(?:.|\n)*needs: \[[^\]]*mock-endpoint-matrix[^\]]*"
            r"cli-wrapper-matrix[^\]]*installer-smoke[^\]]*perf-gate[^\]]*compatible-provider-smoke[^\]]*"
            r"supply-chain[^\]]*\]",
        )

    def test_installer_script_contract_is_posix_and_fail_closed(self):
        self.assertTrue(INSTALLER_SCRIPT.exists(), "release asset source install.sh must exist")
        installer = INSTALLER_SCRIPT.read_text(encoding="utf-8")

        for snippet in (
            "#!/bin/sh",
            "llm-universal-proxy-${asset_os}-${asset_arch}.tar.gz",
            "LLMUP_INSTALL_BASE_URL",
            "LLMUP_INSTALL_UNAME_S",
            "LLMUP_INSTALL_UNAME_M",
            "--bin-dir",
            "--no-modify-path",
            ".sha256",
            "checksum mismatch",
            "validate_archive",
            "path traversal",
            "absolute path",
            "unexpected archive entry",
            ".llmup-install-manifest",
            "llmup-config",
            "llmup-codex",
            "llmup-claude",
            "reopen your terminal",
            "__LLMUP_RELEASE_TAG__",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, installer)

        self.assertNotIn("sudo", installer)
        subprocess.run(["sh", "-n", str(INSTALLER_SCRIPT)], check=True)

    def test_installer_installs_fake_release_atomically_with_aliases_and_absolute_next_steps(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = pathlib.Path(tmpdir)
            release_dir = tmp_path / "release assets"
            release_dir.mkdir()
            self.write_fake_release_asset(release_dir)
            bin_dir = tmp_path / "bin with spaces"

            result = self.run_installer(
                tmp_path,
                release_dir,
                "--bin-dir",
                str(bin_dir),
                "--no-modify-path",
            )

            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
            self.assertTrue((bin_dir / "llm-universal-proxy").is_file())
            for alias in ("llmup-config", "llmup-codex", "llmup-claude"):
                with self.subTest(alias=alias):
                    self.assertTrue((bin_dir / alias).exists())
                    self.assertIn(str(bin_dir / alias), result.stdout)
            self.assertFalse((bin_dir / "llmup").exists(), "installer must not create a standalone llmup command")
            self.assertTrue((bin_dir / ".llmup-install-manifest").is_file())
            self.assertFalse((tmp_path / "home" / ".profile").exists())

            for command in INSTALLER_SMOKE_COMMANDS:
                with self.subTest(command=command):
                    parts = command.split()
                    installed = bin_dir / parts[0]
                    smoke = subprocess.run(
                        [str(installed), *parts[1:]],
                        text=True,
                        capture_output=True,
                    )
                    self.assertEqual(smoke.returncode, 0, smoke.stderr + smoke.stdout)

    def test_installer_rejects_checksum_mismatch_before_installing(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = pathlib.Path(tmpdir)
            release_dir = tmp_path / "release"
            release_dir.mkdir()
            self.write_fake_release_asset(release_dir, checksum="0" * 64)
            bin_dir = tmp_path / "bin"

            result = self.run_installer(
                tmp_path,
                release_dir,
                "--bin-dir",
                str(bin_dir),
                "--no-modify-path",
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("checksum mismatch", result.stderr + result.stdout)
            self.assertFalse((bin_dir / "llm-universal-proxy").exists())

    def test_installer_rejects_unsafe_archive_entries(self):
        unsafe_entries = (
            "/tmp/llm-universal-proxy",
            "../llm-universal-proxy",
            "nested/llm-universal-proxy",
            "llmup",
        )
        for entry_name in unsafe_entries:
            with self.subTest(entry=entry_name):
                with tempfile.TemporaryDirectory() as tmpdir:
                    tmp_path = pathlib.Path(tmpdir)
                    release_dir = tmp_path / "release"
                    release_dir.mkdir()
                    self.write_fake_release_asset(release_dir, entry_name=entry_name)
                    result = self.run_installer(
                        tmp_path,
                        release_dir,
                        "--bin-dir",
                        str(tmp_path / "bin"),
                        "--no-modify-path",
                    )

                    self.assertNotEqual(result.returncode, 0)
                    output = result.stderr + result.stdout
                    self.assertTrue(
                        any(
                            phrase in output
                            for phrase in (
                                "absolute path",
                                "path traversal",
                                "unexpected archive entry",
                            )
                        ),
                        output,
                    )

    def test_installer_maps_supported_unix_platform_assets_and_rejects_unknown_arch(self):
        cases = (
            ("Linux", "x86_64", "llm-universal-proxy-linux-x86_64.tar.gz"),
            ("Linux", "aarch64", "llm-universal-proxy-linux-aarch64.tar.gz"),
            ("Linux", "arm64", "llm-universal-proxy-linux-aarch64.tar.gz"),
            ("Darwin", "x86_64", "llm-universal-proxy-macos-x86_64.tar.gz"),
            ("Darwin", "arm64", "llm-universal-proxy-macos-aarch64.tar.gz"),
        )
        for uname_s, uname_m, asset_name in cases:
            with self.subTest(uname_s=uname_s, uname_m=uname_m):
                with tempfile.TemporaryDirectory() as tmpdir:
                    tmp_path = pathlib.Path(tmpdir)
                    release_dir = tmp_path / "release"
                    release_dir.mkdir()
                    self.write_fake_release_asset(release_dir, asset_name=asset_name)
                    result = self.run_installer(
                        tmp_path,
                        release_dir,
                        "--bin-dir",
                        str(tmp_path / "bin"),
                        "--no-modify-path",
                        extra_env={
                            "LLMUP_INSTALL_UNAME_S": uname_s,
                            "LLMUP_INSTALL_UNAME_M": uname_m,
                        },
                    )
                    self.assertEqual(result.returncode, 0, result.stderr + result.stdout)

        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = pathlib.Path(tmpdir)
            release_dir = tmp_path / "release"
            release_dir.mkdir()
            result = self.run_installer(
                tmp_path,
                release_dir,
                "--bin-dir",
                str(tmp_path / "bin"),
                "--no-modify-path",
                extra_env={"LLMUP_INSTALL_UNAME_M": "sparc64"},
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("unsupported architecture", result.stderr + result.stdout)

    def test_installer_profile_marker_is_idempotent_unless_no_modify_path(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = pathlib.Path(tmpdir)
            release_dir = tmp_path / "release"
            release_dir.mkdir()
            self.write_fake_release_asset(release_dir)
            bin_dir = tmp_path / "home" / ".local" / "bin"

            first = self.run_installer(tmp_path, release_dir)
            second = self.run_installer(tmp_path, release_dir)

            self.assertEqual(first.returncode, 0, first.stderr + first.stdout)
            self.assertEqual(second.returncode, 0, second.stderr + second.stdout)
            profile = tmp_path / "home" / ".profile"
            text = profile.read_text(encoding="utf-8")
            self.assertEqual(text.count(">>> llmup installer >>>"), 1)
            self.assertIn(str(bin_dir / "llmup-config"), first.stdout)

    def test_release_workflow_uploads_install_sh_asset_and_runs_installer_smoke(self):
        release = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        jobs = release_workflow_jobs()
        installer_smoke = jobs.get("installer-smoke", "")
        release_job = jobs.get("release", "")

        self.assertTrue(installer_smoke, "release workflow must define installer-smoke")
        for snippet in (
            "Prepare install.sh release asset",
            "artifacts/install.sh",
            "__LLMUP_RELEASE_TAG__",
            'replace("__LLMUP_RELEASE_TAG__", release_tag, 1)',
            "${{ github.ref_name }}",
            "Upload install.sh release asset",
            "name: install-sh",
            "path: artifacts/install.sh",
            "Run installer smoke",
            "LLMUP_INSTALL_BASE_URL",
            "--bin-dir",
            "--no-modify-path",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, installer_smoke)
        for command in INSTALLER_SMOKE_COMMANDS:
            with self.subTest(command=command):
                self.assertIn(f'"$BIN_DIR"/{command}', installer_smoke)

        self.assertIn("installer-smoke", job_needs(release_job))
        self.assertIn("Download release artifacts", release_job)
        self.assertIn("pattern: llm-universal-proxy-*", release_job)
        self.assertIn("Download install.sh artifact", release_job)
        self.assertIn("name: install-sh", release_job)
        self.assertIn("path: artifacts/install-sh", release_job)
        self.assertIn("artifacts/install-sh/install.sh", release_job)
        self.assertIn("install.sh", release)

    def test_release_cli_wrapper_matrix_runs_structure_and_hermetic_interactive_gates(self):
        jobs = release_workflow_jobs()
        job = jobs.get("cli-wrapper-matrix", "")
        self.assertTrue(job, "release workflow must define cli-wrapper-matrix")

        run_step = workflow_step_block(job, "Run CLI wrapper matrix")
        self.assertTrue(run_step, "cli-wrapper-matrix must keep a script run step")
        self.assertIn(CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND, run_step)
        self.assertIn(
            "python3 scripts/real_cli_matrix.py --test basic --skip-slow --list-matrix",
            run_step,
        )
        self.assertIn("cli-wrapper-matrix.txt", run_step)
        self.assertNotIn("--mode real-provider-smoke", run_step)
        self.assertNotIn("--test live", run_step)

    def test_governance_checkout_fetches_full_history_for_release_tag_visibility(self):
        for workflow_path in (CI_WORKFLOW, RELEASE_WORKFLOW):
            with self.subTest(workflow=workflow_path.name):
                job = workflow_jobs(workflow_path).get("governance", "")
                self.assertTrue(job, "workflow must define a governance job")
                checkout_step = workflow_step_block(job, "Checkout code")
                self.assertTrue(
                    checkout_step,
                    "governance job must checkout repository code",
                )
                self.assertIn("uses: actions/checkout@v5", checkout_step)
                self.assertIn("        with:", checkout_step)
                self.assertRegex(checkout_step, r"(?m)^          fetch-depth: 0$")

    def test_release_compatible_provider_smoke_delegates_missing_secret_json_to_script(self):
        jobs = release_workflow_jobs()
        job = jobs.get("compatible-provider-smoke", "")
        self.assertTrue(job, "release workflow must define compatible-provider-smoke")

        run_step = workflow_step_block(job, "Run compatible provider smoke")
        self.assertTrue(run_step, "compatible provider smoke must have a script run step")
        for secret_name in COMPAT_PROVIDER_SECRET_ENVS:
            with self.subTest(secret=secret_name):
                self.assertIn(f"{secret_name}: ${{{{ secrets.{secret_name} }}}}", run_step)
        for var_name in COMPAT_PROVIDER_VAR_ENVS:
            with self.subTest(var=var_name):
                self.assertIn(f"{var_name}: ${{{{ vars.{var_name} }}}}", run_step)
        for secret_name in OFFICIAL_PROVIDER_SECRET_ENVS:
            with self.subTest(no_official_secret=secret_name):
                self.assertNotIn(f"{secret_name}: ${{{{ secrets.{secret_name} }}}}", job)
        self.assertNotIn("GLM_APIKEY", run_step)
        self.assertNotIn("secrets.GLM_APIKEY", job)

        invocation_lines = compatible_provider_smoke_invocation_lines(job)
        self.assertTrue(invocation_lines)
        invocation_index = job.find(invocation_lines[0])
        self.assertGreaterEqual(invocation_index, 0)
        before_invocation = job[:invocation_index]

        self.assertNotIn("Validate protected real provider secrets", before_invocation)
        self.assertNotIn("is required in the release-compatible-provider environment", before_invocation)
        self.assertNotIn("exit 1", before_invocation)
        for env_name in (*COMPAT_PROVIDER_SECRET_ENVS, *COMPAT_PROVIDER_VAR_ENVS):
            with self.subTest(no_preflight=env_name):
                self.assertNotIn(f'test -n "${{{env_name}:-}}"', before_invocation)

        self.assert_has_compatible_provider_smoke_invocation(job)

    def test_release_compatible_provider_smoke_uploads_json_artifact_always(self):
        jobs = release_workflow_jobs()
        job = jobs.get("compatible-provider-smoke", "")
        self.assertTrue(job, "release workflow must define compatible-provider-smoke")

        upload_step = workflow_step_block(job, "Upload compatible provider smoke result")
        self.assertTrue(upload_step, "compatible provider smoke JSON artifact must be uploaded")
        self.assertIn("Upload compatible provider smoke result", job)
        self.assertIn('if: ${{ always() }}', upload_step)
        self.assertIn("uses: actions/upload-artifact@v4", upload_step)
        self.assertIn("name: compatible-provider-smoke", upload_step)
        self.assertIn(f"path: {COMPAT_PROVIDER_SMOKE_JSON}", upload_step)
        self.assertIn("if-no-files-found: error", upload_step)

    def test_release_publish_jobs_need_ga_gates_before_publishing(self):
        jobs = release_workflow_jobs()
        publish_jobs = {
            name: block
            for name, block in jobs.items()
            if any(marker in block for marker in RELEASE_PUBLISH_JOB_MARKERS)
        }

        self.assertIn("container", publish_jobs)
        self.assertIn("release", publish_jobs)
        self.assertIn("${{ env.GHCR_IMAGE }}:latest", publish_jobs["container"])

        for job_name, job_block in publish_jobs.items():
            with self.subTest(job=job_name):
                needs = job_needs(job_block)
                missing = set(REQUIRED_RELEASE_GATE_NEEDS) - needs
                self.assertFalse(
                    missing,
                    f"{job_name} publishes release artifacts before GA gates: "
                    f"{', '.join(sorted(missing))}",
                )
                self.assertNotIn(
                    "real-provider-smoke",
                    needs,
                    f"{job_name} must not block GA release on the legacy four-provider smoke",
                )

    def test_release_container_job_publishes_ref_version_and_latest_tags(self):
        jobs = release_workflow_jobs()
        container = jobs.get("container", "")
        self.assertTrue(container, "release workflow must define container job")

        push_step = workflow_step_block(container, "Build and push multi-arch image")
        self.assertTrue(push_step, "container job must keep a multi-arch push step")
        for snippet in (
            "id: push_image",
            "${{ env.GHCR_IMAGE }}:${{ github.ref_name }}",
            "${{ env.GHCR_IMAGE }}:${{ steps.repo_meta.outputs.version }}",
            "${{ env.GHCR_IMAGE }}:latest",
            "VERSION=${{ steps.repo_meta.outputs.version }}",
            "org.opencontainers.image.version=${{ steps.repo_meta.outputs.version }}",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, push_step)

    def test_release_container_job_exports_pushed_digest_manifest_artifact(self):
        jobs = release_workflow_jobs()
        container = jobs.get("container", "")
        self.assertTrue(container, "release workflow must define container job")

        push_step = workflow_step_block(container, "Build and push multi-arch image")
        self.assertTrue(push_step, "container job must keep a multi-arch push step")
        self.assertIn("id: push_image", push_step)

        write_step = workflow_step_block(container, "Write pushed container image manifest")
        self.assertTrue(
            write_step,
            "container job must write the pushed image digest to a machine-readable manifest",
        )
        for snippet in (
            "PUSH_DIGEST: ${{ steps.push_image.outputs.digest }}",
            "RELEASE_TAG: ${{ github.ref_name }}",
            "VERSION: ${{ steps.repo_meta.outputs.version }}",
            PUSHED_CONTAINER_IMAGE_MANIFEST_JSON,
            '"digest": digest',
            '"release_tag": release_tag',
            '"cargo_package_version": version',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, write_step)

        upload_step = workflow_step_block(container, "Upload pushed container image manifest")
        self.assertTrue(
            upload_step,
            "container job must upload the pushed digest manifest for docs refresh",
        )
        for snippet in (
            "uses: actions/upload-artifact@v4",
            "name: container-image",
            f"path: {PUSHED_CONTAINER_IMAGE_MANIFEST_JSON}",
            "if-no-files-found: error",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, upload_step)

    def test_release_container_manifest_writer_emits_drop_in_post_release_schema(self):
        jobs = release_workflow_jobs()
        container = jobs.get("container", "")
        self.assertTrue(container, "release workflow must define container job")

        write_step = workflow_step_block(container, "Write pushed container image manifest")
        script = workflow_step_inline_python(write_step)
        self.assertTrue(script, "manifest writer must be executable Python")

        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = pathlib.Path(tmpdir)
            (tmp_path / "artifacts").mkdir()
            env = os.environ.copy()
            env.update(
                {
                    "GHCR_IMAGE": "ghcr.io/agentsmith-project/llm-universal-proxy",
                    "RELEASE_TAG": "v0.2.23",
                    "VERSION": "0.2.23",
                    "PUSH_DIGEST": f"sha256:{'a' * 64}",
                    "GIT_SHA": "b" * 40,
                    "GITHUB_SERVER_URL": "https://github.com",
                    "GITHUB_REPOSITORY": "agentsmith-project/llm-universal-proxy",
                    "GITHUB_RUN_ID": "123456789",
                }
            )
            subprocess.run(
                [sys.executable, "-c", script],
                cwd=tmp_path,
                env=env,
                check=True,
                text=True,
                capture_output=True,
            )
            manifest = json.loads(
                (tmp_path / PUSHED_CONTAINER_IMAGE_MANIFEST_JSON).read_text(
                    encoding="utf-8"
                )
            )

        self.assertEqual(manifest["schema"], 1)
        self.assertEqual(
            manifest["image"], "ghcr.io/agentsmith-project/llm-universal-proxy"
        )
        self.assertEqual(manifest["published"]["release_tag"], "v0.2.23")
        self.assertEqual(manifest["published"]["version_tag"], "0.2.23")
        self.assertEqual(manifest["published"]["digest"], f"sha256:{'a' * 64}")
        self.assertEqual(manifest["published"]["cargo_package_version"], "0.2.23")
        self.assertEqual(manifest["published"]["git_sha"], "b" * 40)
        self.assertEqual(manifest["published"]["status"], "published")
        checked_in_manifest = json.loads(
            CHECKED_IN_CONTAINER_IMAGE_MANIFEST.read_text(encoding="utf-8")
        )
        self.assertEqual(
            {"published_at"}, published_timestamp_fields(checked_in_manifest)
        )
        self.assertEqual(
            published_timestamp_fields(checked_in_manifest),
            published_timestamp_fields(manifest),
        )
        self.assertRegex(
            manifest["published"]["published_at"],
            r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$",
        )
        self.assertEqual(
            manifest["published"]["run_url"],
            "https://github.com/agentsmith-project/llm-universal-proxy/actions/runs/123456789",
        )

        self.assertEqual(manifest["next_release"]["cargo_package_version"], "0.2.24")
        self.assertEqual(manifest["next_release"]["release_tag"], "v0.2.24")
        self.assertEqual(manifest["next_release"]["status"], "not_published")
        self.assertIn("main", manifest["next_release"]["main_branch_action"])
        self.assertIn("0.2.24", manifest["next_release"]["main_branch_action"])

    def test_ci_workflow_contains_local_mock_perf_and_supply_chain_gates(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")

        for snippet in (
            "Mock Endpoint Matrix",
            "python3 scripts/real_endpoint_matrix.py --mock",
            "Perf Gate",
            "python3 scripts/real_endpoint_matrix.py --mock --perf",
            "Supply Chain",
            SUPPLY_CHAIN_AUDIT_COMMAND,
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, ci)

    def test_supply_chain_audit_gate_has_central_contract(self):
        self.assertTrue(
            SUPPLY_CHAIN_AUDIT_SCRIPT.exists(),
            "supply-chain audit must have one repo-local entrypoint shared by CI and release",
        )
        audit_script = SUPPLY_CHAIN_AUDIT_SCRIPT.read_text(encoding="utf-8")

        for workflow_path in (CI_WORKFLOW, RELEASE_WORKFLOW):
            workflow = workflow_path.read_text(encoding="utf-8")
            jobs = workflow_jobs(workflow_path)
            job = jobs.get("supply-chain", "")
            with self.subTest(workflow=workflow_path.name):
                self.assertTrue(job, "workflow must define a supply-chain job")
                self.assertIn("Install cargo-audit", job)
                self.assertIn(SUPPLY_CHAIN_AUDIT_COMMAND, job)
                self.assertNotIn("cargo audit --locked", workflow)

        release_supply_chain = workflow_jobs(RELEASE_WORKFLOW).get("supply-chain", "")
        self.assertIn("anchore/sbom-action", release_supply_chain)
        self.assertIn("Upload SBOM", release_supply_chain)

        self.assertIn(LOCKFILE_INTEGRITY_COMMAND, audit_script)
        self.assertIn("cargo audit", audit_script)
        self.assertNotIn("cargo audit --locked", audit_script)

    def test_endpoint_mock_matrix_covers_public_surface_minimal_paths(self):
        module = load_endpoint_matrix_module()
        cases = module.build_mock_matrix_cases()

        expected_surfaces = {
            "openai_chat",
            "openai_responses",
            "anthropic_messages",
        }
        expected_modes = {"unary", "stream", "tool", "error"}
        actual_pairs = {(case.surface, case.mode) for case in cases}

        self.assertEqual({case.surface for case in cases}, expected_surfaces)
        self.assertEqual({case.mode for case in cases}, expected_modes)
        for surface in expected_surfaces:
            for mode in expected_modes:
                with self.subTest(surface=surface, mode=mode):
                    self.assertIn((surface, mode), actual_pairs)

        case_ids = [case.case_id for case in cases]
        self.assertEqual(len(case_ids), len(set(case_ids)))

    def test_endpoint_matrix_cli_contract_is_machine_readable_and_secret_free(self):
        script = ENDPOINT_MATRIX_SCRIPT.read_text(encoding="utf-8")

        for snippet in (
            "--mock",
            "--perf",
            "--json-out",
            "--mode",
            "PERF_DEFAULT_P95_MS",
            "PERF_DEFAULT_TOTAL_MS",
            "build_mock_matrix_cases",
            '"status"',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, script)

        self.assertTrue(
            "--mode" in script,
            "real endpoint matrix must expose explicit CLI modes",
        )

        self.assertNotIn("sk-proj-", script)
        self.assertNotIn("sk-ant-", script)
        self.assertNotIn("sk-cp-", script)

    def test_governance_locks_new_release_gate_contracts(self):
        governance = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")

        for snippet in (
            "python3 scripts/real_endpoint_matrix.py --mock",
            "python3 scripts/real_cli_matrix.py --test basic --skip-slow --list-matrix",
            "Installer Smoke",
            "check_installer_release_contract",
            "install.sh",
            "LLMUP_INSTALL_BASE_URL",
            "name: install-sh",
            "artifacts/install-sh/install.sh",
            "python3 scripts/real_endpoint_matrix.py --mock --perf",
            "environment: release-compatible-provider",
            "COMPAT_PROVIDER_SECRET_ENVS",
            "COMPAT_PROVIDER_VAR_ENVS",
            "COMPAT_PROVIDER_SMOKE_JSON",
            "check_compatible_provider_smoke_invocation",
            "if-no-files-found: error",
            "REQUIRED_RELEASE_GATE_NEEDS",
            "check_release_publish_jobs_need_ga_gates",
            "check_release_tag_identity",
            "refs/tags/v${VERSION}",
            "git rev-parse --verify --quiet",
            "check_governance_checkout_fetch_depth",
            "git rev-parse --is-shallow-repository",
            "fetch-depth: 0",
            "tag visibility",
            "CONTAINER_IMAGE_MANIFEST",
            "check_container_image_manifest_contract",
            "PUBLISHED_CONTAINER_DIGEST_REF",
            "NEXT_RELEASE_TAG",
            "published_at",
            "released_at",
            "pushed container image manifest",
            SUPPLY_CHAIN_AUDIT_COMMAND,
            LOCKFILE_INTEGRITY_COMMAND,
            "cargo audit --locked",
            "anchore/sbom-action",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, governance)
        self.assert_has_compatible_provider_smoke_invocation(governance)

    def test_docs_record_local_and_protected_release_gates(self):
        ga_review = self.read_text("docs/ga-readiness-review.md")
        clients = self.read_text("docs/clients.md")
        advanced_usage = self.read_text("docs/advanced-usage.md")
        container = self.read_text("docs/container.md")

        for snippet in (
            "GA release gates",
            "mock endpoint matrix",
            "perf gate",
            "compatible provider smoke",
            "hermetic scripted interactive Codex wrapper gate",
            "not a full live multi-client/provider matrix",
            "release-compatible-provider",
            "portable-core production GA",
            "maximum safe compatibility with hard portability boundaries",
            "may keep provider-native bytes, fields, and lifecycle resources unchanged",
            "compatibility machinery",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, ga_review)

        for snippet in (
            "launcher-managed path",
            "llmup-config",
            "llmup-codex",
            "llmup-claude",
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "real provider secret out of the client process",
            "Advanced Usage](./advanced-usage.md)",
        ):
            with self.subTest(client_doc_snippet=snippet):
                self.assertIn(snippet, clients)
        for snippet in (
            "CLI wrapper matrix",
            "hermetic scripted interactive Codex wrapper gate",
            "not a full live multi-client/provider matrix",
            "MiniMax is only a replaceable OpenAI-compatible example",
            "scripts/run_codex_proxy.sh",
            "scripts/run_claude_proxy.sh",
        ):
            with self.subTest(client_doc_forbidden_snippet=snippet):
                self.assertNotIn(snippet, clients)

        for snippet in (
            "wire a client without the launchers",
            "Manual Proxy Startup",
            "Auth Modes",
            "Manual Codex Wiring",
            "Manual Claude Wiring",
            "provider key belongs to the proxy",
        ):
            with self.subTest(advanced_doc_snippet=snippet):
                self.assertIn(snippet, advanced_usage)
        self.assertIn("CLI wrapper matrix", container)
        self.assertIn("structure gate", container)
        self.assertIn("hermetic scripted interactive Codex wrapper gate", container)
        self.assertIn("not a full live multi-client/provider matrix", container)
        self.assertIn("mock endpoint matrix", container)
        self.assertIn("perf gate", container)
        self.assertIn("compatible-provider-smoke.json", container)
        self.assertNotIn("not yet mandatory release gates", ga_review)
        self.assertNotIn("not mandatory", ga_review)
        self.assertNotIn("not mandatory", container)

    def test_docs_record_opaque_reasoning_and_compaction_degrade_contract(self):
        docs = {
            "compatibility": self.read_text("docs/protocol-compatibility-matrix.md"),
            "reasoning": self.read_text("docs/protocol-baselines/capabilities/reasoning.md"),
            "state": self.read_text("docs/protocol-baselines/capabilities/state-continuity.md"),
            "field_mapping": self.read_text(
                "docs/protocol-baselines/matrices/field-mapping-matrix.md"
            ),
            "responses": self.read_text("docs/protocol-baselines/openai-responses.md"),
            "ga_review": self.read_text("docs/ga-readiness-review.md"),
        }

        required_by_doc = {
            "compatibility": (
                "maximum safe compatibility",
                "visible summary",
                "opaque-only",
                "same-wire-protocol",
            ),
            "reasoning": (
                "reasoning.encrypted_content",
                "maximum safe compatibility",
                "visible summary",
                "opaque-only reasoning fails closed",
                "provider-native same-wire handling",
            ),
            "state": (
                "context_management",
                "request-side compaction input",
                "maximum safe compatibility",
                "opaque-only",
                "Native Responses same-wire handling",
                "native same-wire handling",
            ),
            "field_mapping": (
                "Reasoning opaque state",
                "Portability action",
                "Warn and omit opaque carrier",
                "Compaction",
                "opaque-only compaction",
                "provider-native same-wire handling",
            ),
            "responses": (
                "context_management",
                "request-side compaction",
                "visible portable transcript",
                "Opaque-only compaction input",
                "Native OpenAI Responses same-wire handling",
            ),
            "ga_review": (
                "maximum safe compatibility",
                "visible summary",
                "opaque-only",
                "provider-native same-wire handling",
            ),
        }

        for doc_name, snippets in required_by_doc.items():
            text = docs[doc_name].casefold()
            for snippet in snippets:
                with self.subTest(doc=doc_name, snippet=snippet):
                    self.assertIn(snippet.casefold(), text)


if __name__ == "__main__":
    unittest.main()
