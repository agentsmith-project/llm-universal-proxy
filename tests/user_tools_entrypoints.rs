use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
mod common;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "llmup-user-tools-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
fn link_or_copy(src: &Path, dst: &Path) {
    std::os::unix::fs::symlink(src, dst).unwrap_or_else(|_| {
        fs::copy(src, dst).expect("copy test binary");
    });
}

#[cfg(not(unix))]
fn link_or_copy(src: &Path, dst: &Path) {
    fs::copy(src, dst).expect("copy test binary");
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = fs::metadata(path)
        .expect("executable metadata")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod executable");
}

#[cfg(unix)]
fn init_llmup_config(
    llmup_config: &Path,
    llmup_home: &Path,
    home: &Path,
    interface: &str,
    model_service_url: &str,
    model_name: &str,
    provider_key: &str,
) {
    let mut config = Command::new(llmup_config)
        .env("LLMUP_HOME", llmup_home)
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn llmup-config");
    config
        .stdin
        .as_mut()
        .expect("config stdin")
        .write_all(
            format!("{interface}\n{model_service_url}\n{model_name}\n{provider_key}\n").as_bytes(),
        )
        .expect("write interactive config input");
    let init_output = config.wait_with_output().expect("wait config");
    assert!(
        init_output.status.success(),
        "config failed stdout={} stderr={}",
        String::from_utf8_lossy(&init_output.stdout),
        String::from_utf8_lossy(&init_output.stderr)
    );
}

#[cfg(unix)]
fn local_proxy_key(llmup_home: &Path) -> String {
    let secrets = fs::read_to_string(llmup_home.join("secrets.env")).expect("read secrets");
    secrets
        .lines()
        .find_map(|line| line.strip_prefix("LLM_UNIVERSAL_PROXY_KEY="))
        .expect("local proxy key should be present")
        .to_string()
}

#[cfg(unix)]
fn runtime_config_paths(llmup_home: &Path) -> Vec<PathBuf> {
    let run_root = llmup_home.join("run");
    fs::read_dir(&run_root)
        .expect("read run dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("config.yaml"))
        .filter(|path| path.exists())
        .collect::<Vec<_>>()
}

#[cfg(unix)]
fn assert_runtime_yaml_preserves_data_auth_and_overrides_listen(llmup_home: &Path) {
    let runtime_configs = runtime_config_paths(llmup_home);
    assert!(
        !runtime_configs.is_empty(),
        "runtime config should be written"
    );
    let runtime_yaml = fs::read_to_string(&runtime_configs[0]).expect("read runtime config");
    assert!(runtime_yaml.contains("data_auth"));
    assert!(runtime_yaml.contains("LLM_UNIVERSAL_PROXY_KEY"));
    assert!(!runtime_yaml.contains("listen: 127.0.0.1:8080"));
}

#[cfg(unix)]
fn path_with_bin_dir_first(bin_dir: &Path) -> std::ffi::OsString {
    let mut paths = vec![bin_dir.to_path_buf()];
    if let Some(parent_path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&parent_path));
    }
    std::env::join_paths(paths).expect("join PATH entries")
}

#[cfg(unix)]
fn write_fake_claude_full_flow(path: &Path) {
    fs::write(
        path,
        r#"#!/usr/bin/env python3
import json
import os
import sys
import urllib.error
import urllib.request

args = sys.argv[1:]
log_path = os.environ["LLMUP_FAKE_LOG"]
with open(log_path, "w", encoding="utf-8") as log:
    log.write(f"ARGV_COUNT={len(args)}\n")
    for arg in args:
        log.write(f"ARG={arg}\n")
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
        "ANTHROPIC_CUSTOM_MODEL_OPTION",
        "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
        "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
        "ANTHROPIC_CUSTOM_MODEL_OPTION_SUPPORTED_CAPABILITIES",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_DESCRIPTION",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_SUPPORTED_CAPABILITIES",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_DESCRIPTION",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_SUPPORTED_CAPABILITIES",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_DESCRIPTION",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES",
        "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
        "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
        "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
        "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB",
        "CLAUDE_CONFIG_DIR",
        "LLMUP_PROVIDER_DEFAULT_API_KEY",
        "LLMUP_PROVIDER_MAIN_API_KEY",
        "UNRELATED_SECRET_COPY",
        "HOME",
    ]:
        log.write(f"{name}={os.environ.get(name, 'unset')}\n")

model = os.environ.get("ANTHROPIC_MODEL")
if not model:
    print("missing ANTHROPIC_MODEL from launcher", file=sys.stderr)
    sys.exit(30)
subagent_model = os.environ.get("CLAUDE_CODE_SUBAGENT_MODEL") or model
with open(log_path, "a", encoding="utf-8") as log:
    log.write(f"SUBAGENT_MODEL={subagent_model}\n")

base_url = os.environ["ANTHROPIC_BASE_URL"].rstrip("/")
api_key = os.environ["ANTHROPIC_API_KEY"]

def post_message(request_model, content, error_base):
    body = {
        "model": request_model,
        "max_tokens": 16,
        "messages": [{"role": "user", "content": content}],
        "stream": False,
    }
    request = urllib.request.Request(
        base_url + "/v1/messages",
        data=json.dumps(body).encode("utf-8"),
        method="POST",
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "anthropic-version": "2023-06-01",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            payload = response.read().decode("utf-8")
            status = response.status
    except urllib.error.HTTPError as error:
        print(error.read().decode("utf-8"), file=sys.stderr)
        sys.exit(error_base)
    except Exception as error:
        print(f"request failed: {error}", file=sys.stderr)
        sys.exit(error_base + 1)

    if status != 200 or "OK" not in payload:
        print(f"unexpected response status={status} payload={payload}", file=sys.stderr)
        sys.exit(error_base + 2)

post_message(model, "claude-main", 31)
post_message(subagent_model, "claude-task-subagent", 34)
"#,
    )
    .expect("write fake claude full-flow client");
    make_executable(path);
}

#[cfg(unix)]
fn write_fake_codex_full_flow(path: &Path) {
    fs::write(
        path,
        r#"#!/usr/bin/env python3
import json
import os
import sys
import urllib.error
import urllib.request

args = sys.argv[1:]
log_path = os.environ["LLMUP_FAKE_LOG"]
with open(log_path, "w", encoding="utf-8") as log:
    log.write(f"ARGV_COUNT={len(args)}\n")
    for arg in args:
        log.write(f"ARG={arg}\n")
    for name in [
        "OPENAI_API_KEY",
        "CODEX_HOME",
        "LLMUP_PROVIDER_DEFAULT_API_KEY",
        "LLMUP_PROVIDER_MAIN_API_KEY",
        "UNRELATED_SECRET_COPY",
        "HOME",
    ]:
        log.write(f"{name}={os.environ.get(name, 'unset')}\n")

model = None
base_url = None
builtin_openai_base_url = None
for index, arg in enumerate(args):
    if arg == "-m" and index + 1 < len(args):
        model = args[index + 1]
    prefix = 'model_providers.proxy.base_url="'
    if arg.startswith(prefix) and arg.endswith('"'):
        base_url = arg[len(prefix):-1]
    prefix = 'openai_base_url="'
    if arg.startswith(prefix) and arg.endswith('"'):
        builtin_openai_base_url = arg[len(prefix):-1]
if model is None or base_url is None or builtin_openai_base_url is None:
    print("missing model, proxy base_url, or openai_base_url from launcher", file=sys.stderr)
    sys.exit(40)
with open(log_path, "a", encoding="utf-8") as log:
    log.write(f"SUBAGENT_MODEL={model}\n")
    log.write(f"OPENAI_FALLBACK_BASE_URL={builtin_openai_base_url}\n")

api_key = os.environ["OPENAI_API_KEY"]

def post_response(request_base_url, input_text, error_base):
    body = {
        "model": model,
        "input": input_text,
        "stream": False,
    }
    request = urllib.request.Request(
        request_base_url.rstrip("/") + "/responses",
        data=json.dumps(body).encode("utf-8"),
        method="POST",
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            payload = response.read().decode("utf-8")
            status = response.status
    except urllib.error.HTTPError as error:
        print(error.read().decode("utf-8"), file=sys.stderr)
        sys.exit(error_base)
    except Exception as error:
        print(f"request failed: {error}", file=sys.stderr)
        sys.exit(error_base + 1)

    if status != 200 or "OK" not in payload:
        print(f"unexpected response status={status} payload={payload}", file=sys.stderr)
        sys.exit(error_base + 2)

post_response(base_url, "codex-main", 41)
post_response(builtin_openai_base_url, "codex-subagent-openai-fallback", 44)
"#,
    )
    .expect("write fake codex full-flow client");
    make_executable(path);
}

#[cfg(unix)]
fn write_fake_codex_custom_agent_gate(path: &Path) {
    fs::write(
        path,
        r#"#!/usr/bin/env python3
import json
import os
import sys
import urllib.error
import urllib.request

args = sys.argv[1:]
log_path = os.environ["LLMUP_FAKE_LOG"]
with open(log_path, "w", encoding="utf-8") as log:
    log.write(f"ARGV_COUNT={len(args)}\n")
    for arg in args:
        log.write(f"ARG={arg}\n")
    for name in [
        "OPENAI_API_KEY",
        "CODEX_HOME",
        "LLMUP_PROVIDER_DEFAULT_API_KEY",
        "LLMUP_PROVIDER_MAIN_API_KEY",
        "UNRELATED_SECRET_COPY",
        "HOME",
    ]:
        log.write(f"{name}={os.environ.get(name, 'unset')}\n")

launcher_model = None
base_url = None
for index, arg in enumerate(args):
    if arg == "-m" and index + 1 < len(args):
        launcher_model = args[index + 1]
    prefix = 'model_providers.proxy.base_url="'
    if arg.startswith(prefix) and arg.endswith('"'):
        base_url = arg[len(prefix):-1]
if launcher_model is None or base_url is None:
    print("missing model or base_url from launcher", file=sys.stderr)
    sys.exit(50)
with open(log_path, "a", encoding="utf-8") as log:
    log.write(f"LAUNCHER_MODEL={launcher_model}\n")
    log.write(f"BASE_URL={base_url}\n")

api_key = os.environ["OPENAI_API_KEY"]
valid_model = os.environ["LLMUP_FAKE_CODEX_CUSTOM_MODEL"]
unknown_model = os.environ["LLMUP_FAKE_CODEX_UNKNOWN_MODEL"]

def post_response(request_model, input_text):
    body = {
        "model": request_model,
        "input": input_text,
        "stream": False,
    }
    request = urllib.request.Request(
        base_url.rstrip("/") + "/responses",
        data=json.dumps(body).encode("utf-8"),
        method="POST",
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
    )
    with urllib.request.urlopen(request, timeout=5) as response:
        return response.status, response.read().decode("utf-8")

try:
    status, payload = post_response(valid_model, "codex-custom-agent-valid")
except urllib.error.HTTPError as error:
    print(error.read().decode("utf-8"), file=sys.stderr)
    sys.exit(51)
except Exception as error:
    print(f"valid custom-agent request failed: {error}", file=sys.stderr)
    sys.exit(52)

if status != 200 or "OK" not in payload:
    print(f"unexpected valid response status={status} payload={payload}", file=sys.stderr)
    sys.exit(53)

try:
    status, payload = post_response(unknown_model, "codex-custom-agent-unknown")
except urllib.error.HTTPError as error:
    status = error.code
    payload = error.read().decode("utf-8")
    with open(log_path, "a", encoding="utf-8") as log:
        log.write(f"UNKNOWN_STATUS={status}\n")
        log.write(f"UNKNOWN_ERROR={payload.replace(chr(10), ' ')}\n")
    if status < 400 or status >= 500:
        print(f"unknown model returned non-4xx status={status} payload={payload}", file=sys.stderr)
        sys.exit(54)
    if unknown_model not in payload or "mock_assertion_failed" in payload:
        print(f"unknown model error did not come clearly from llmup: {payload}", file=sys.stderr)
        sys.exit(55)
except Exception as error:
    print(f"unknown custom-agent request failed without HTTP error: {error}", file=sys.stderr)
    sys.exit(56)
else:
    print(f"unknown model unexpectedly succeeded status={status} payload={payload}", file=sys.stderr)
    sys.exit(57)
"#,
    )
    .expect("write fake codex custom-agent gate client");
    make_executable(path);
}

#[test]
fn installed_entrypoint_help_and_version_do_not_require_native_clients() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_llm-universal-proxy"));

    let server_help = Command::new(&bin)
        .arg("--help")
        .output()
        .expect("run server help");
    assert!(server_help.status.success());
    let server_help_stdout = String::from_utf8_lossy(&server_help.stdout);
    assert!(server_help_stdout.contains("llm-universal-proxy"));
    assert!(server_help_stdout.contains("--config"));

    let server_version = Command::new(&bin)
        .arg("--version")
        .output()
        .expect("run server version");
    assert!(server_version.status.success());
    assert!(String::from_utf8_lossy(&server_version.stdout).contains(env!("CARGO_PKG_VERSION")));

    let temp = TempDir::new("entrypoint-help");
    let llmup_config = temp.path().join("llmup-config");
    let llmup_codex = temp.path().join("llmup-codex");
    let llmup_claude = temp.path().join("llmup-claude");
    link_or_copy(&bin, &llmup_config);
    link_or_copy(&bin, &llmup_codex);
    link_or_copy(&bin, &llmup_claude);

    let config_help = Command::new(&llmup_config)
        .arg("--help")
        .env("PATH", temp.path())
        .output()
        .expect("run llmup-config help");
    assert!(config_help.status.success());
    let config_help_stdout = String::from_utf8_lossy(&config_help.stdout);
    assert!(config_help_stdout.contains("llmup-config"));
    assert!(!config_help_stdout.contains("llmup <subcommand>"));
    assert!(!config_help_stdout.contains(" init "));

    let config_version = Command::new(&llmup_config)
        .arg("--version")
        .env("PATH", temp.path())
        .output()
        .expect("run llmup-config version");
    assert!(config_version.status.success());
    assert!(String::from_utf8_lossy(&config_version.stdout).contains(env!("CARGO_PKG_VERSION")));

    for launcher in [&llmup_codex, &llmup_claude] {
        let help = Command::new(launcher)
            .arg("--llmup-help")
            .env("PATH", temp.path())
            .output()
            .expect("run launcher help");
        assert!(help.status.success(), "help status for {launcher:?}");
        let stdout = String::from_utf8_lossy(&help.stdout);
        assert!(stdout.contains("--llmup-no-proxy"));
        assert!(!stdout.contains("--llmup-port"));
        assert!(!stdout.contains("llmup-internal"));

        let version = Command::new(launcher)
            .arg("--llmup-version")
            .env("PATH", temp.path())
            .output()
            .expect("run launcher version");
        assert!(version.status.success(), "version status for {launcher:?}");
        assert!(String::from_utf8_lossy(&version.stdout).contains(env!("CARGO_PKG_VERSION")));
    }
}

#[test]
fn installed_config_entrypoint_without_args_runs_interactive_setup() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_llm-universal-proxy"));
    let temp = TempDir::new("entrypoint-interactive-config");
    let llmup_config = temp.path().join("llmup-config");
    let llmup_home = temp.path().join(".llmup");
    link_or_copy(&bin, &llmup_config);

    let mut child = Command::new(&llmup_config)
        .env("HOME", temp.path())
        .env("LLMUP_HOME", &llmup_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn llmup-config interactive");
    child
        .stdin
        .as_mut()
        .expect("interactive stdin")
        .write_all(b"\nhttps://api.deepseek.com\ndeepseek-v4-flash\nprovider-secret-from-prompt\n")
        .expect("write interactive answers");
    let output = child.wait_with_output().expect("wait interactive config");
    assert!(
        output.status.success(),
        "interactive config failed stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Wrote llmup config"));
    assert!(!stdout.contains("Usage:"));
    assert!(!stdout.contains("provider-secret-from-prompt"));
    assert!(llmup_home.join("config.yaml").exists());
    assert!(llmup_home.join("secrets.env").exists());
}

#[cfg(unix)]
#[test]
fn no_proxy_entrypoints_run_native_clients_without_managed_dirs_or_env_injection() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_llm-universal-proxy"));
    let temp = TempDir::new("entrypoint-no-proxy");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");

    let llmup_codex = bin_dir.join("llmup-codex");
    let llmup_claude = bin_dir.join("llmup-claude");
    link_or_copy(&bin, &llmup_codex);
    link_or_copy(&bin, &llmup_claude);

    let fake_codex_log = temp.path().join("fake-codex-no-proxy.log");
    let fake_codex = bin_dir.join("codex");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
{
  printf 'ARGV_COUNT=%s\n' "$#"
  for arg in "$@"; do
    printf 'ARG=%s\n' "$arg"
  done
  printf 'CODEX_HOME=%s\n' "${CODEX_HOME-unset}"
  printf 'OPENAI_API_KEY=%s\n' "${OPENAI_API_KEY-unset}"
  printf 'OPENAI_BASE_URL=%s\n' "${OPENAI_BASE_URL-unset}"
} > "$LLMUP_FAKE_LOG"
exit 17
"#,
    )
    .expect("write fake codex");
    make_executable(&fake_codex);

    let fake_claude_log = temp.path().join("fake-claude-no-proxy.log");
    let fake_claude = bin_dir.join("claude");
    fs::write(
        &fake_claude,
        r#"#!/bin/sh
{
  printf 'ARGV_COUNT=%s\n' "$#"
  for arg in "$@"; do
    printf 'ARG=%s\n' "$arg"
  done
  printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR-unset}"
  printf 'ANTHROPIC_API_KEY=%s\n' "${ANTHROPIC_API_KEY-unset}"
  printf 'ANTHROPIC_BASE_URL=%s\n' "${ANTHROPIC_BASE_URL-unset}"
  printf 'CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=%s\n' "${CLAUDE_CODE_SUBPROCESS_ENV_SCRUB-unset}"
} > "$LLMUP_FAKE_LOG"
exit 19
"#,
    )
    .expect("write fake claude");
    make_executable(&fake_claude);

    let llmup_home = temp.path().join(".llmup");
    let llmup_codex_home = temp.path().join(".llmup-codex");
    let llmup_claude_config_dir = temp.path().join(".llmup-claude");

    let codex = Command::new(&llmup_codex)
        .args(["--llmup-no-proxy", "--", "resume", "--last"])
        .env("PATH", &bin_dir)
        .env("HOME", temp.path())
        .env("LLMUP_HOME", &llmup_home)
        .env("LLMUP_CODEX_HOME", &llmup_codex_home)
        .env("LLMUP_CLAUDE_CONFIG_DIR", &llmup_claude_config_dir)
        .env("LLMUP_FAKE_LOG", &fake_codex_log)
        .env("OPENAI_API_KEY", "native-openai-key")
        .env("OPENAI_BASE_URL", "https://native-openai.example/v1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run no-proxy codex");
    assert_eq!(
        codex.status.code(),
        Some(17),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&codex.stdout),
        String::from_utf8_lossy(&codex.stderr)
    );
    let codex_log = fs::read_to_string(&fake_codex_log).expect("read codex log");
    assert!(codex_log.contains("ARG=resume"));
    assert!(codex_log.contains("ARG=--last"));
    assert!(codex_log.contains("CODEX_HOME=unset"));
    assert!(codex_log.contains("OPENAI_API_KEY=native-openai-key"));
    assert!(codex_log.contains("OPENAI_BASE_URL=https://native-openai.example/v1"));

    let claude = Command::new(&llmup_claude)
        .args(["--llmup-no-proxy", "--", "auth", "status"])
        .env("PATH", &bin_dir)
        .env("HOME", temp.path())
        .env("LLMUP_HOME", &llmup_home)
        .env("LLMUP_CODEX_HOME", &llmup_codex_home)
        .env("LLMUP_CLAUDE_CONFIG_DIR", &llmup_claude_config_dir)
        .env("LLMUP_FAKE_LOG", &fake_claude_log)
        .env("ANTHROPIC_API_KEY", "native-anthropic-key")
        .env("ANTHROPIC_BASE_URL", "https://native-anthropic.example")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run no-proxy claude");
    assert_eq!(
        claude.status.code(),
        Some(19),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&claude.stdout),
        String::from_utf8_lossy(&claude.stderr)
    );
    let claude_log = fs::read_to_string(&fake_claude_log).expect("read claude log");
    assert!(claude_log.contains("ARG=auth"));
    assert!(claude_log.contains("ARG=status"));
    assert!(claude_log.contains("CLAUDE_CONFIG_DIR=unset"));
    assert!(claude_log.contains("ANTHROPIC_API_KEY=native-anthropic-key"));
    assert!(claude_log.contains("ANTHROPIC_BASE_URL=https://native-anthropic.example"));
    assert!(claude_log.contains("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=unset"));

    assert!(!llmup_home.exists());
    assert!(!llmup_codex_home.exists());
    assert!(!llmup_claude_config_dir.exists());
}

#[cfg(unix)]
#[test]
fn codex_managed_launcher_runs_fake_client_with_injection_isolation_and_proxy_lifecycle() {
    use std::os::unix::fs::PermissionsExt;

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_llm-universal-proxy"));
    let temp = TempDir::new("managed-codex");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");

    let llmup_config = bin_dir.join("llmup-config");
    let llmup_codex = bin_dir.join("llmup-codex");
    link_or_copy(&bin, &llmup_config);
    link_or_copy(&bin, &llmup_codex);

    let fake_log = temp.path().join("fake-codex.log");
    let fake_codex = bin_dir.join("codex");
    fs::write(
        &fake_codex,
        format!(
            r#"#!/bin/sh
{{
  printf 'ARGV_COUNT=%s\n' "$#"
  for arg in "$@"; do
    printf 'ARG=%s\n' "$arg"
  done
  printf 'OPENAI_API_KEY=%s\n' "$OPENAI_API_KEY"
  printf 'CODEX_HOME=%s\n' "$CODEX_HOME"
  printf 'LLMUP_PROVIDER_DEFAULT_API_KEY=%s\n' "${{LLMUP_PROVIDER_DEFAULT_API_KEY-unset}}"
  printf 'LLMUP_PROVIDER_MAIN_API_KEY=%s\n' "${{LLMUP_PROVIDER_MAIN_API_KEY-unset}}"
  printf 'HOME=%s\n' "$HOME"
}} > "{}"
exit 7
"#,
            fake_log.display()
        ),
    )
    .expect("write fake codex");
    let mut perms = fs::metadata(&fake_codex)
        .expect("fake codex metadata")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake_codex, perms).expect("chmod fake codex");

    let llmup_home = temp.path().join(".llmup");
    init_llmup_config(
        &llmup_config,
        &llmup_home,
        temp.path(),
        "openai-chat-completions",
        "https://api.example.com/v1",
        "test-upstream-model",
        "provider-secret-from-stdin",
    );

    let output = Command::new(&llmup_codex)
        .arg("--help")
        .env("PATH", &bin_dir)
        .env("LLMUP_HOME", &llmup_home)
        .env("LLMUP_FAKE_LOG", &fake_log)
        .env("LLMUP_PROVIDER_MAIN_API_KEY", "parent-provider-key")
        .env("OPENAI_API_KEY", "parent-openai-key")
        .env("HOME", temp.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run llmup-codex");
    assert_eq!(
        output.status.code(),
        Some(7),
        "launcher should return fake client exit code, stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let fake = fs::read_to_string(&fake_log).expect("read fake client log");
    assert!(fake.contains("ARG=-c"));
    assert!(fake.contains("ARG=model_provider=\"proxy\""));
    assert!(fake.contains("ARG=-m"));
    assert!(fake.contains("ARG=main"));
    assert!(fake.contains("ARG=--help"));
    assert!(fake.contains(&format!(
        "CODEX_HOME={}",
        temp.path().join(".llmup-codex").display()
    )));
    assert!(!fake.contains("parent-provider-key"));
    assert!(!fake.contains("provider-secret-from-stdin"));
    assert!(!fake.contains("OPENAI_API_KEY=parent-openai-key"));
    assert!(fake.contains("LLMUP_PROVIDER_DEFAULT_API_KEY=unset"));
    assert!(fake.contains("LLMUP_PROVIDER_MAIN_API_KEY=unset"));

    let secrets = fs::read_to_string(llmup_home.join("secrets.env")).expect("read secrets");
    let local_proxy_key = secrets
        .lines()
        .find_map(|line| line.strip_prefix("LLM_UNIVERSAL_PROXY_KEY="))
        .expect("local proxy key should be present");
    assert!(fake.contains(&format!("OPENAI_API_KEY={local_proxy_key}")));

    let user_config = fs::read_to_string(llmup_home.join("config.yaml")).expect("read user config");
    assert!(user_config.contains("listen: 127.0.0.1:8080"));

    let run_root = llmup_home.join("run");
    let runtime_configs = fs::read_dir(&run_root)
        .expect("read run dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("config.yaml"))
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    assert!(
        !runtime_configs.is_empty(),
        "runtime config should be written"
    );
    let runtime_yaml = fs::read_to_string(&runtime_configs[0]).expect("read runtime config");
    assert!(runtime_yaml.contains("data_auth"));
    assert!(runtime_yaml.contains("LLM_UNIVERSAL_PROXY_KEY"));
    assert!(!runtime_yaml.contains("listen: 127.0.0.1:8080"));
}

#[cfg(unix)]
#[test]
fn claude_managed_launcher_runs_fake_client_with_injection_isolation_and_proxy_lifecycle() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_llm-universal-proxy"));
    let temp = TempDir::new("managed-claude");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");

    let llmup_config = bin_dir.join("llmup-config");
    let llmup_claude = bin_dir.join("llmup-claude");
    link_or_copy(&bin, &llmup_config);
    link_or_copy(&bin, &llmup_claude);

    let fake_log = temp.path().join("fake-claude.log");
    let fake_claude = bin_dir.join("claude");
    fs::write(
        &fake_claude,
        r#"#!/bin/sh
{
  printf 'ARGV_COUNT=%s\n' "$#"
  for arg in "$@"; do
    printf 'ARG=%s\n' "$arg"
  done
  printf 'ANTHROPIC_API_KEY=%s\n' "$ANTHROPIC_API_KEY"
  printf 'ANTHROPIC_BASE_URL=%s\n' "$ANTHROPIC_BASE_URL"
  printf 'ANTHROPIC_MODEL=%s\n' "${ANTHROPIC_MODEL-unset}"
  printf 'ANTHROPIC_CUSTOM_MODEL_OPTION=%s\n' "${ANTHROPIC_CUSTOM_MODEL_OPTION-unset}"
  printf 'ANTHROPIC_CUSTOM_MODEL_OPTION_NAME=%s\n' "${ANTHROPIC_CUSTOM_MODEL_OPTION_NAME-unset}"
  printf 'ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION=%s\n' "${ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION-unset}"
  printf 'ANTHROPIC_CUSTOM_MODEL_OPTION_SUPPORTED_CAPABILITIES=%s\n' "${ANTHROPIC_CUSTOM_MODEL_OPTION_SUPPORTED_CAPABILITIES-unset}"
  printf 'CLAUDE_CODE_MAX_OUTPUT_TOKENS=%s\n' "${CLAUDE_CODE_MAX_OUTPUT_TOKENS-unset}"
  printf 'CLAUDE_CODE_AUTO_COMPACT_WINDOW=%s\n' "${CLAUDE_CODE_AUTO_COMPACT_WINDOW-unset}"
  printf 'CLAUDE_CONFIG_DIR=%s\n' "$CLAUDE_CONFIG_DIR"
  printf 'CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=%s\n' "${CLAUDE_CODE_SUBPROCESS_ENV_SCRUB-unset}"
  printf 'LLMUP_PROVIDER_DEFAULT_API_KEY=%s\n' "${LLMUP_PROVIDER_DEFAULT_API_KEY-unset}"
  printf 'LLMUP_PROVIDER_MAIN_API_KEY=%s\n' "${LLMUP_PROVIDER_MAIN_API_KEY-unset}"
  printf 'UNRELATED_SECRET_COPY=%s\n' "${UNRELATED_SECRET_COPY-unset}"
  printf 'ANTHROPIC_AUTH_TOKEN=%s\n' "${ANTHROPIC_AUTH_TOKEN-unset}"
  printf 'ANTHROPIC_BEDROCK_TOKEN=%s\n' "${ANTHROPIC_BEDROCK_TOKEN-unset}"
  printf 'AWS_ACCESS_KEY_ID=%s\n' "${AWS_ACCESS_KEY_ID-unset}"
  printf 'HOME=%s\n' "$HOME"
} > "$LLMUP_FAKE_LOG"
exit 9
"#,
    )
    .expect("write fake claude");
    make_executable(&fake_claude);

    let llmup_home = temp.path().join(".llmup");
    init_llmup_config(
        &llmup_config,
        &llmup_home,
        temp.path(),
        "anthropic-messages",
        "https://api.example.com/v1",
        "test-upstream-model",
        "provider-secret-from-stdin",
    );

    let output = Command::new(&llmup_claude)
        .args([
            "--resume",
            "session with spaces",
            "--permission-mode",
            "bypassPermissions",
            "mcp",
        ])
        .env("PATH", &bin_dir)
        .env("LLMUP_HOME", &llmup_home)
        .env("LLMUP_FAKE_LOG", &fake_log)
        .env("LLMUP_PROVIDER_MAIN_API_KEY", "parent-provider-key")
        .env("UNRELATED_SECRET_COPY", "provider-secret-from-stdin")
        .env("ANTHROPIC_API_KEY", "parent-anthropic-key")
        .env("ANTHROPIC_MODEL", "parent-model")
        .env("ANTHROPIC_CUSTOM_MODEL_OPTION", "parent-option")
        .env(
            "ANTHROPIC_CUSTOM_MODEL_OPTION_SUPPORTED_CAPABILITIES",
            "parent-capability",
        )
        .env("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "42")
        .env("ANTHROPIC_AUTH_TOKEN", "parent-auth-token")
        .env("ANTHROPIC_BEDROCK_TOKEN", "parent-bedrock-token")
        .env("AWS_ACCESS_KEY_ID", "parent-aws-key")
        .env("HOME", temp.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run llmup-claude");
    assert_eq!(
        output.status.code(),
        Some(9),
        "launcher should return fake client exit code, stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let fake = fs::read_to_string(&fake_log).expect("read fake client log");
    assert!(!fake.contains("ARG=--model"));
    assert!(!fake.contains("ARG=main"));
    assert!(fake.contains("ARG=--resume"));
    assert!(fake.contains("ARG=session with spaces"));
    assert!(fake.contains("ARG=--permission-mode"));
    assert!(fake.contains("ARG=bypassPermissions"));
    assert!(fake.contains("ARG=mcp"));
    assert!(fake.contains("ANTHROPIC_MODEL=main"));
    assert!(fake.contains("ANTHROPIC_CUSTOM_MODEL_OPTION=main"));
    assert!(fake.contains("ANTHROPIC_CUSTOM_MODEL_OPTION_NAME=main"));
    assert!(fake.contains("ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION=llmup proxy model main"));
    assert!(fake.contains("ANTHROPIC_CUSTOM_MODEL_OPTION_SUPPORTED_CAPABILITIES=unset"));
    assert!(fake.contains("CLAUDE_CODE_MAX_OUTPUT_TOKENS=unset"));
    assert!(fake.contains(&format!(
        "CLAUDE_CONFIG_DIR={}",
        temp.path().join(".llmup-claude").display()
    )));
    assert!(fake.contains("ANTHROPIC_BASE_URL=http://127.0.0.1:"));
    assert!(fake.contains("/anthropic"));
    assert!(fake.contains("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=unset"));
    assert!(!fake.contains("parent-provider-key"));
    assert!(!fake.contains("provider-secret-from-stdin"));
    assert!(!fake.contains("ANTHROPIC_API_KEY=parent-anthropic-key"));
    assert!(!fake.contains("parent-model"));
    assert!(!fake.contains("parent-option"));
    assert!(!fake.contains("parent-capability"));
    assert!(fake.contains("LLMUP_PROVIDER_DEFAULT_API_KEY=unset"));
    assert!(fake.contains("LLMUP_PROVIDER_MAIN_API_KEY=unset"));
    assert!(fake.contains("UNRELATED_SECRET_COPY=unset"));
    assert!(fake.contains("ANTHROPIC_AUTH_TOKEN=unset"));
    assert!(fake.contains("ANTHROPIC_BEDROCK_TOKEN=unset"));
    assert!(fake.contains("AWS_ACCESS_KEY_ID=unset"));

    let local_proxy_key = local_proxy_key(&llmup_home);
    assert!(fake.contains(&format!("ANTHROPIC_API_KEY={local_proxy_key}")));

    let user_config = fs::read_to_string(llmup_home.join("config.yaml")).expect("read user config");
    assert!(user_config.contains("listen: 127.0.0.1:8080"));
    assert_runtime_yaml_preserves_data_auth_and_overrides_listen(&llmup_home);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_full_flow_fake_client_reaches_proxy_and_mock_upstream() {
    const PROVIDER_KEY: &str = "provider-secret-full-flow-claude";
    const UPSTREAM_MODEL: &str = "real-upstream-claude-model";

    let (mock_base, _mock, captured) =
        common::mock_upstream::spawn_asserting_openai_responses_mock(|request| {
            if request.method != "POST" {
                return Err(format!("unexpected method {}", request.method));
            }
            if request.path != "/v1/responses" {
                return Err(format!("unexpected path {}", request.path));
            }
            if request.headers.get("authorization").map(String::as_str)
                != Some("Bearer provider-secret-full-flow-claude")
            {
                return Err(format!(
                    "unexpected authorization {:?}",
                    request.headers.get("authorization")
                ));
            }
            if request
                .body
                .get("model")
                .and_then(serde_json::Value::as_str)
                != Some(UPSTREAM_MODEL)
            {
                return Err(format!("unexpected model body {}", request.body));
            }
            Ok(())
        })
        .await;

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_llm-universal-proxy"));
    let temp = TempDir::new("full-flow-claude");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");

    let llmup_config = bin_dir.join("llmup-config");
    let llmup_claude = bin_dir.join("llmup-claude");
    link_or_copy(&bin, &llmup_config);
    link_or_copy(&bin, &llmup_claude);
    let fake_log = temp.path().join("fake-claude-full-flow.log");
    let fake_claude = bin_dir.join("claude");
    write_fake_claude_full_flow(&fake_claude);

    let llmup_home = temp.path().join(".llmup");
    init_llmup_config(
        &llmup_config,
        &llmup_home,
        temp.path(),
        "openai-responses",
        &format!("{mock_base}/v1"),
        UPSTREAM_MODEL,
        PROVIDER_KEY,
    );

    let output = Command::new(&llmup_claude)
        .arg("--dangerously-skip-permissions")
        .env("PATH", path_with_bin_dir_first(&bin_dir))
        .env("LLMUP_HOME", &llmup_home)
        .env("LLMUP_FAKE_LOG", &fake_log)
        .env("LLMUP_PROVIDER_MAIN_API_KEY", "parent-provider-key")
        .env("UNRELATED_SECRET_COPY", PROVIDER_KEY)
        .env("ANTHROPIC_API_KEY", "parent-anthropic-key")
        .env("ANTHROPIC_MODEL", "parent-model")
        .env("ANTHROPIC_CUSTOM_MODEL_OPTION", "parent-option")
        .env("HOME", temp.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run llmup-claude full flow");
    assert!(
        output.status.success(),
        "full flow failed stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = captured
        .wait_for_count(2, std::time::Duration::from_secs(2))
        .await;
    assert_eq!(
        requests.len(),
        2,
        "main and subagent upstream requests should be captured"
    );
    let request_bodies = requests
        .iter()
        .map(|request| serde_json::to_string(&request.body).expect("serialize captured body"))
        .collect::<Vec<_>>();
    assert!(
        request_bodies
            .iter()
            .any(|body| body.contains("claude-main")),
        "main request body should reach upstream: {request_bodies:?}"
    );
    assert!(
        request_bodies
            .iter()
            .any(|body| body.contains("claude-task-subagent")),
        "subagent request body should reach upstream: {request_bodies:?}"
    );

    let fake = fs::read_to_string(&fake_log).expect("read fake client log");
    let local_proxy_key = local_proxy_key(&llmup_home);
    assert!(fake.contains(&format!("ANTHROPIC_API_KEY={local_proxy_key}")));
    assert!(!fake.contains(PROVIDER_KEY));
    assert!(!fake.contains("parent-provider-key"));
    assert!(!fake.contains("parent-model"));
    assert!(!fake.contains("parent-option"));
    assert!(!fake.contains("ARG=--model"));
    assert!(!fake.contains("ARG=main"));
    assert!(fake.contains("ANTHROPIC_MODEL=main"));
    assert!(fake.contains("ANTHROPIC_CUSTOM_MODEL_OPTION=main"));
    assert!(fake.contains("SUBAGENT_MODEL=main"));
    assert!(fake.contains("ARG=--dangerously-skip-permissions"));
    assert!(fake.contains("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=unset"));

    let user_config = fs::read_to_string(llmup_home.join("config.yaml")).expect("read user config");
    assert!(user_config.contains("listen: 127.0.0.1:8080"));
    assert_runtime_yaml_preserves_data_auth_and_overrides_listen(&llmup_home);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_family_alias_full_flow_routes_each_alias_to_configured_upstream_model() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_llm-universal-proxy"));
    let temp = TempDir::new("full-flow-claude-family");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");

    let llmup_claude = bin_dir.join("llmup-claude");
    link_or_copy(&bin, &llmup_claude);
    let fake_claude = bin_dir.join("claude");
    write_fake_claude_full_flow(&fake_claude);

    for alias in ["haiku", "sonnet", "opus"] {
        let provider_key = format!("provider-secret-full-flow-claude-{alias}");
        let expected_upstream_model = format!("real-upstream-{alias}-model");
        let expected_authorization = format!("Bearer {provider_key}");
        let expected_model_for_mock = expected_upstream_model.clone();
        let expected_authorization_for_mock = expected_authorization.clone();
        let (mock_base, _mock, captured) =
            common::mock_upstream::spawn_asserting_openai_responses_mock(move |request| {
                if request.method != "POST" {
                    return Err(format!("unexpected method {}", request.method));
                }
                if request.path != "/v1/responses" {
                    return Err(format!("unexpected path {}", request.path));
                }
                if request.headers.get("authorization").map(String::as_str)
                    != Some(expected_authorization_for_mock.as_str())
                {
                    return Err(format!(
                        "unexpected authorization {:?}",
                        request.headers.get("authorization")
                    ));
                }
                if request
                    .body
                    .get("model")
                    .and_then(serde_json::Value::as_str)
                    != Some(expected_model_for_mock.as_str())
                {
                    return Err(format!("unexpected model body {}", request.body));
                }
                Ok(())
            })
            .await;

        let llmup_home = temp.path().join(format!(".llmup-{alias}"));
        fs::create_dir_all(&llmup_home).expect("create llmup home");
        fs::write(
            llmup_home.join("config.yaml"),
            format!(
                "\
listen: 127.0.0.1:8080
upstream_timeout_secs: 120

data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY

upstreams:
  DEFAULT:
    api_root: {mock_base}/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_FAMILY_API_KEY

model_aliases:
  main: DEFAULT:real-upstream-main-model
  haiku: DEFAULT:real-upstream-haiku-model
  sonnet: DEFAULT:real-upstream-sonnet-model
  opus: DEFAULT:real-upstream-opus-model
"
            ),
        )
        .expect("write family alias config");
        fs::write(
            llmup_home.join("secrets.env"),
            format!(
                "LLM_UNIVERSAL_PROXY_KEY=local-proxy-key-family-{alias}\nLLMUP_PROVIDER_FAMILY_API_KEY={provider_key}\n"
            ),
        )
        .expect("write family alias secrets");

        let fake_log = temp.path().join(format!("fake-claude-family-{alias}.log"));
        let output = Command::new(&llmup_claude)
            .args(["--llmup-model", alias, "--dangerously-skip-permissions"])
            .env("PATH", path_with_bin_dir_first(&bin_dir))
            .env("LLMUP_HOME", &llmup_home)
            .env("LLMUP_FAKE_LOG", &fake_log)
            .env("LLMUP_PROVIDER_FAMILY_API_KEY", "parent-provider-key")
            .env("UNRELATED_SECRET_COPY", &provider_key)
            .env("ANTHROPIC_API_KEY", "parent-anthropic-key")
            .env("ANTHROPIC_MODEL", "parent-model")
            .env("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY", "1")
            .env(
                "ANTHROPIC_DEFAULT_HAIKU_MODEL_SUPPORTED_CAPABILITIES",
                "thinking",
            )
            .env(
                "ANTHROPIC_DEFAULT_SONNET_MODEL_SUPPORTED_CAPABILITIES",
                "thinking",
            )
            .env(
                "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES",
                "thinking",
            )
            .env("HOME", temp.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run llmup-claude family alias full flow");
        assert!(
            output.status.success(),
            "family alias {alias} full flow failed stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let requests = captured
            .wait_for_count(2, std::time::Duration::from_secs(2))
            .await;
        assert_eq!(
            requests.len(),
            2,
            "main and subagent requests for {alias} should reach upstream"
        );
        for request in &requests {
            assert_eq!(
                request
                    .body
                    .get("model")
                    .and_then(serde_json::Value::as_str),
                Some(expected_upstream_model.as_str())
            );
        }

        let fake = fs::read_to_string(&fake_log).expect("read fake client log");
        assert!(fake.contains(&format!("ANTHROPIC_API_KEY=local-proxy-key-family-{alias}")));
        assert!(!fake.contains(&provider_key));
        assert!(!fake.contains("parent-provider-key"));
        assert!(!fake.contains("parent-model"));
        assert!(fake.contains(&format!("ANTHROPIC_MODEL={alias}")));
        assert!(fake.contains(&format!("ANTHROPIC_CUSTOM_MODEL_OPTION={alias}")));
        assert!(fake.contains(&format!("SUBAGENT_MODEL={alias}")));
        for family_alias in ["haiku", "sonnet", "opus"] {
            let family = family_alias.to_ascii_uppercase();
            assert!(fake.contains(&format!("ANTHROPIC_DEFAULT_{family}_MODEL={family_alias}")));
            assert!(fake.contains(&format!(
                "ANTHROPIC_DEFAULT_{family}_MODEL_SUPPORTED_CAPABILITIES=unset"
            )));
        }
        assert!(fake.contains("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=unset"));
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_full_flow_fake_client_reaches_proxy_and_mock_upstream() {
    const PROVIDER_KEY: &str = "provider-secret-full-flow-codex";
    const UPSTREAM_MODEL: &str = "real-upstream-codex-model";

    let (mock_base, _mock, captured) =
        common::mock_upstream::spawn_asserting_openai_responses_mock(|request| {
            if request.method != "POST" {
                return Err(format!("unexpected method {}", request.method));
            }
            if request.path != "/v1/responses" {
                return Err(format!("unexpected path {}", request.path));
            }
            if request.headers.get("authorization").map(String::as_str)
                != Some("Bearer provider-secret-full-flow-codex")
            {
                return Err(format!(
                    "unexpected authorization {:?}",
                    request.headers.get("authorization")
                ));
            }
            if request
                .body
                .get("model")
                .and_then(serde_json::Value::as_str)
                != Some(UPSTREAM_MODEL)
            {
                return Err(format!("unexpected model body {}", request.body));
            }
            Ok(())
        })
        .await;

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_llm-universal-proxy"));
    let temp = TempDir::new("full-flow-codex");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");

    let llmup_config = bin_dir.join("llmup-config");
    let llmup_codex = bin_dir.join("llmup-codex");
    link_or_copy(&bin, &llmup_config);
    link_or_copy(&bin, &llmup_codex);
    let fake_log = temp.path().join("fake-codex-full-flow.log");
    let fake_codex = bin_dir.join("codex");
    write_fake_codex_full_flow(&fake_codex);

    let llmup_home = temp.path().join(".llmup");
    init_llmup_config(
        &llmup_config,
        &llmup_home,
        temp.path(),
        "openai-responses",
        &format!("{mock_base}/v1"),
        UPSTREAM_MODEL,
        PROVIDER_KEY,
    );

    let output = Command::new(&llmup_codex)
        .args(["resume", "--last"])
        .env("PATH", path_with_bin_dir_first(&bin_dir))
        .env("LLMUP_HOME", &llmup_home)
        .env("LLMUP_FAKE_LOG", &fake_log)
        .env("LLMUP_PROVIDER_MAIN_API_KEY", "parent-provider-key")
        .env("UNRELATED_SECRET_COPY", PROVIDER_KEY)
        .env("OPENAI_API_KEY", "parent-openai-key")
        .env("HOME", temp.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run llmup-codex full flow");
    assert!(
        output.status.success(),
        "full flow failed stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = captured
        .wait_for_count(2, std::time::Duration::from_secs(2))
        .await;
    assert_eq!(
        requests.len(),
        2,
        "main and subagent upstream requests should be captured"
    );
    let request_bodies = requests
        .iter()
        .map(|request| serde_json::to_string(&request.body).expect("serialize captured body"))
        .collect::<Vec<_>>();
    assert!(
        request_bodies
            .iter()
            .any(|body| body.contains("codex-main")),
        "main request body should reach upstream: {request_bodies:?}"
    );
    assert!(
        request_bodies
            .iter()
            .any(|body| body.contains("codex-subagent")),
        "subagent request body should reach upstream: {request_bodies:?}"
    );

    let fake = fs::read_to_string(&fake_log).expect("read fake client log");
    let local_proxy_key = local_proxy_key(&llmup_home);
    assert!(fake.contains(&format!("OPENAI_API_KEY={local_proxy_key}")));
    assert!(!fake.contains(PROVIDER_KEY));
    assert!(!fake.contains("parent-provider-key"));
    assert!(fake.contains("ARG=-m"));
    assert!(fake.contains("ARG=main"));
    assert!(fake.contains("SUBAGENT_MODEL=main"));
    assert!(fake.contains("OPENAI_FALLBACK_BASE_URL=http://127.0.0.1:"));
    assert!(fake.contains("/openai/v1"));
    assert!(fake.contains("ARG=resume"));
    assert!(fake.contains("ARG=--last"));

    let user_config = fs::read_to_string(llmup_home.join("config.yaml")).expect("read user config");
    assert!(user_config.contains("listen: 127.0.0.1:8080"));
    assert_runtime_yaml_preserves_data_auth_and_overrides_listen(&llmup_home);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_custom_agent_explicit_model_routes_alias_and_rejects_unknown_before_upstream() {
    const PROVIDER_KEY: &str = "provider-secret-custom-agent-codex";
    const LOCAL_PROXY_KEY: &str = "local-proxy-key-custom-agent-codex";
    const CUSTOM_ALIAS: &str = "custom-agent";
    const CUSTOM_UPSTREAM_MODEL: &str = "routed-custom-agent-upstream-model";
    const UNKNOWN_MODEL: &str = "unconfigured-custom-agent-model";

    let (mock_base, _mock, captured) =
        common::mock_upstream::spawn_asserting_openai_responses_mock(|request| {
            if request.method != "POST" {
                return Err(format!("unexpected method {}", request.method));
            }
            if request.path != "/v1/responses" {
                return Err(format!("unexpected path {}", request.path));
            }
            if request.headers.get("authorization").map(String::as_str)
                != Some("Bearer provider-secret-custom-agent-codex")
            {
                return Err(format!(
                    "unexpected authorization {:?}",
                    request.headers.get("authorization")
                ));
            }
            if request
                .body
                .get("model")
                .and_then(serde_json::Value::as_str)
                != Some(CUSTOM_UPSTREAM_MODEL)
            {
                return Err(format!("unexpected model body {}", request.body));
            }
            if !serde_json::to_string(&request.body)
                .expect("serialize captured body")
                .contains("codex-custom-agent-valid")
            {
                return Err(format!("missing custom-agent marker {}", request.body));
            }
            Ok(())
        })
        .await;

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_llm-universal-proxy"));
    let temp = TempDir::new("custom-agent-codex");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");

    let llmup_codex = bin_dir.join("llmup-codex");
    link_or_copy(&bin, &llmup_codex);
    let fake_log = temp.path().join("fake-codex-custom-agent.log");
    let fake_codex = bin_dir.join("codex");
    write_fake_codex_custom_agent_gate(&fake_codex);

    let llmup_home = temp.path().join(".llmup");
    fs::create_dir_all(&llmup_home).expect("create llmup home");
    fs::write(
        llmup_home.join("config.yaml"),
        format!(
            "\
listen: 127.0.0.1:8080
upstream_timeout_secs: 120

data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY

upstreams:
  DEFAULT:
    api_root: {mock_base}/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
    surface_defaults:
      modalities:
        input: [\"text\"]
        output: [\"text\"]
      tools:
        supports_search: false
        supports_view_image: false
        apply_patch_transport: freeform
        supports_parallel_calls: false
  CUSTOM:
    api_root: {mock_base}/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
    surface_defaults:
      modalities:
        input: [\"text\"]
        output: [\"text\"]
      tools:
        supports_search: false
        supports_view_image: false
        apply_patch_transport: freeform
        supports_parallel_calls: false

model_aliases:
  main: DEFAULT:default-upstream-codex-model
  {CUSTOM_ALIAS}: CUSTOM:{CUSTOM_UPSTREAM_MODEL}
"
        ),
    )
    .expect("write custom agent config");
    fs::write(
        llmup_home.join("secrets.env"),
        format!(
            "LLM_UNIVERSAL_PROXY_KEY={LOCAL_PROXY_KEY}\nLLMUP_PROVIDER_DEFAULT_API_KEY={PROVIDER_KEY}\n"
        ),
    )
    .expect("write custom agent secrets");

    let output = Command::new(&llmup_codex)
        .args(["resume", "--last"])
        .env("PATH", path_with_bin_dir_first(&bin_dir))
        .env("LLMUP_HOME", &llmup_home)
        .env("LLMUP_FAKE_LOG", &fake_log)
        .env("LLMUP_FAKE_CODEX_CUSTOM_MODEL", CUSTOM_ALIAS)
        .env("LLMUP_FAKE_CODEX_UNKNOWN_MODEL", UNKNOWN_MODEL)
        .env("LLMUP_PROVIDER_DEFAULT_API_KEY", "parent-provider-key")
        .env("UNRELATED_SECRET_COPY", PROVIDER_KEY)
        .env("OPENAI_API_KEY", "parent-openai-key")
        .env("HOME", temp.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run llmup-codex custom-agent gate");
    assert!(
        output.status.success(),
        "custom-agent gate failed stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = captured.snapshot();
    assert_eq!(
        requests.len(),
        1,
        "unknown custom-agent model must be rejected by llmup before upstream: {requests:?}"
    );
    let request_body =
        serde_json::to_string(&requests[0].body).expect("serialize captured custom-agent body");
    assert!(
        request_body.contains("codex-custom-agent-valid"),
        "valid custom-agent body should reach upstream: {request_body}"
    );
    assert!(
        !request_body.contains(UNKNOWN_MODEL),
        "unknown custom-agent model should not reach upstream: {request_body}"
    );

    let fake = fs::read_to_string(&fake_log).expect("read fake client log");
    assert!(fake.contains("LAUNCHER_MODEL=main"));
    assert!(fake.contains(&format!("OPENAI_API_KEY={LOCAL_PROXY_KEY}")));
    assert!(fake.contains("BASE_URL=http://127.0.0.1:"));
    assert!(fake.contains("/openai/v1"));
    assert!(fake.contains("UNKNOWN_STATUS=400"));
    assert!(fake.contains(UNKNOWN_MODEL));
    assert!(fake.contains("ambiguous") || fake.contains("routable"));
    assert!(!fake.contains("mock_assertion_failed"));
    assert!(!fake.contains(PROVIDER_KEY));
    assert!(!fake.contains("parent-provider-key"));
    assert!(fake.contains("ARG=-m"));
    assert!(fake.contains("ARG=main"));
    assert!(fake.contains("ARG=resume"));
    assert!(fake.contains("ARG=--last"));

    let user_config = fs::read_to_string(llmup_home.join("config.yaml")).expect("read user config");
    assert!(user_config.contains("listen: 127.0.0.1:8080"));
    assert_runtime_yaml_preserves_data_auth_and_overrides_listen(&llmup_home);
}

#[cfg(unix)]
#[test]
fn explicit_launcher_port_collision_fails_with_requested_port_in_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind occupied port");
    let port = listener.local_addr().expect("occupied local addr").port();

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_llm-universal-proxy"));
    let temp = TempDir::new("port-collision");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");

    let llmup_config = bin_dir.join("llmup-config");
    let llmup_claude = bin_dir.join("llmup-claude");
    link_or_copy(&bin, &llmup_config);
    link_or_copy(&bin, &llmup_claude);

    let llmup_home = temp.path().join(".llmup");
    init_llmup_config(
        &llmup_config,
        &llmup_home,
        temp.path(),
        "anthropic-messages",
        "http://127.0.0.1:9/v1",
        "unused-upstream-model",
        "unused-provider-key",
    );

    let output = Command::new(&llmup_claude)
        .args(["--llmup-port", &port.to_string()])
        .env("PATH", &bin_dir)
        .env("LLMUP_HOME", &llmup_home)
        .env("HOME", temp.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run llmup-claude on occupied port");
    assert_eq!(
        output.status.code(),
        Some(2),
        "launcher should fail before client startup, stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("requested port {port}")),
        "stderr should name the occupied explicit port, got {stderr}"
    );
}
