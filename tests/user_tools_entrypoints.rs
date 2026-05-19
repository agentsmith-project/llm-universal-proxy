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
    let mut init = Command::new(llmup_config)
        .args([
            "init",
            "--non-interactive",
            "--interface",
            interface,
            "--model-service-url",
            model_service_url,
            "--model-name",
            model_name,
            "--model-alias",
            "default",
            "--api-key-stdin",
        ])
        .env("LLMUP_HOME", llmup_home)
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn llmup-config init");
    init.stdin
        .as_mut()
        .expect("init stdin")
        .write_all(format!("{provider_key}\n").as_bytes())
        .expect("write api key");
    let init_output = init.wait_with_output().expect("wait init");
    assert!(
        init_output.status.success(),
        "init failed stdout={} stderr={}",
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
        "CLAUDE_CONFIG_DIR",
        "LLMUP_PROVIDER_DEFAULT_API_KEY",
        "UNRELATED_SECRET_COPY",
        "HOME",
    ]:
        log.write(f"{name}={os.environ.get(name, 'unset')}\n")

model = None
for index, arg in enumerate(args):
    if arg == "--model" and index + 1 < len(args):
        model = args[index + 1]
        break
if model is None:
    print("missing --model from launcher", file=sys.stderr)
    sys.exit(30)

base_url = os.environ["ANTHROPIC_BASE_URL"].rstrip("/")
api_key = os.environ["ANTHROPIC_API_KEY"]
body = {
    "model": model,
    "max_tokens": 16,
    "messages": [{"role": "user", "content": "ping"}],
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
    sys.exit(31)
except Exception as error:
    print(f"request failed: {error}", file=sys.stderr)
    sys.exit(32)

if status != 200 or "OK" not in payload:
    print(f"unexpected response status={status} payload={payload}", file=sys.stderr)
    sys.exit(33)
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
        "UNRELATED_SECRET_COPY",
        "HOME",
    ]:
        log.write(f"{name}={os.environ.get(name, 'unset')}\n")

model = None
base_url = None
for index, arg in enumerate(args):
    if arg == "-m" and index + 1 < len(args):
        model = args[index + 1]
    prefix = 'model_providers.proxy.base_url="'
    if arg.startswith(prefix) and arg.endswith('"'):
        base_url = arg[len(prefix):-1]
if model is None or base_url is None:
    print("missing model or base_url from launcher", file=sys.stderr)
    sys.exit(40)

api_key = os.environ["OPENAI_API_KEY"]
body = {
    "model": model,
    "input": "ping",
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
try:
    with urllib.request.urlopen(request, timeout=5) as response:
        payload = response.read().decode("utf-8")
        status = response.status
except urllib.error.HTTPError as error:
    print(error.read().decode("utf-8"), file=sys.stderr)
    sys.exit(41)
except Exception as error:
    print(f"request failed: {error}", file=sys.stderr)
    sys.exit(42)

if status != 200 or "OK" not in payload:
    print(f"unexpected response status={status} payload={payload}", file=sys.stderr)
    sys.exit(43)
"#,
    )
    .expect("write fake codex full-flow client");
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
        .write_all(
            b"\nhttps://api.minimaxi.com/v1\nMiniMax-M2.7-highspeed\nprovider-secret-from-prompt\n",
        )
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
    let mut init = Command::new(&llmup_config)
        .args([
            "init",
            "--non-interactive",
            "--interface",
            "openai",
            "--model-service-url",
            "https://api.example.com/v1",
            "--model-name",
            "test-upstream-model",
            "--model-alias",
            "default",
            "--api-key-stdin",
        ])
        .env("LLMUP_HOME", &llmup_home)
        .env("HOME", temp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn llmup-config init");
    init.stdin
        .as_mut()
        .expect("init stdin")
        .write_all(b"provider-secret-from-stdin\n")
        .expect("write api key");
    let init_output = init.wait_with_output().expect("wait init");
    assert!(
        init_output.status.success(),
        "init failed stdout={} stderr={}",
        String::from_utf8_lossy(&init_output.stdout),
        String::from_utf8_lossy(&init_output.stderr)
    );

    let output = Command::new(&llmup_codex)
        .arg("--help")
        .env("PATH", &bin_dir)
        .env("LLMUP_HOME", &llmup_home)
        .env("LLMUP_FAKE_LOG", &fake_log)
        .env("LLMUP_PROVIDER_DEFAULT_API_KEY", "parent-provider-key")
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
    assert!(fake.contains("ARG=default"));
    assert!(fake.contains("ARG=--help"));
    assert!(fake.contains(&format!(
        "CODEX_HOME={}",
        temp.path().join(".llmup-codex").display()
    )));
    assert!(!fake.contains("parent-provider-key"));
    assert!(!fake.contains("provider-secret-from-stdin"));
    assert!(!fake.contains("OPENAI_API_KEY=parent-openai-key"));
    assert!(fake.contains("LLMUP_PROVIDER_DEFAULT_API_KEY=unset"));

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
  printf 'CLAUDE_CONFIG_DIR=%s\n' "$CLAUDE_CONFIG_DIR"
  printf 'CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=%s\n' "${CLAUDE_CODE_SUBPROCESS_ENV_SCRUB-unset}"
  printf 'LLMUP_PROVIDER_DEFAULT_API_KEY=%s\n' "${LLMUP_PROVIDER_DEFAULT_API_KEY-unset}"
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
        "anthropic",
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
        .env("LLMUP_PROVIDER_DEFAULT_API_KEY", "parent-provider-key")
        .env("UNRELATED_SECRET_COPY", "provider-secret-from-stdin")
        .env("ANTHROPIC_API_KEY", "parent-anthropic-key")
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
    assert!(fake.contains("ARG=--model"));
    assert!(fake.contains("ARG=default"));
    assert!(fake.contains("ARG=--resume"));
    assert!(fake.contains("ARG=session with spaces"));
    assert!(fake.contains("ARG=--permission-mode"));
    assert!(fake.contains("ARG=bypassPermissions"));
    assert!(fake.contains("ARG=mcp"));
    assert!(fake.contains(&format!(
        "CLAUDE_CONFIG_DIR={}",
        temp.path().join(".llmup-claude").display()
    )));
    assert!(fake.contains("ANTHROPIC_BASE_URL=http://127.0.0.1:"));
    assert!(fake.contains("/anthropic"));
    assert!(fake.contains("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1"));
    assert!(!fake.contains("parent-provider-key"));
    assert!(!fake.contains("provider-secret-from-stdin"));
    assert!(!fake.contains("ANTHROPIC_API_KEY=parent-anthropic-key"));
    assert!(fake.contains("LLMUP_PROVIDER_DEFAULT_API_KEY=unset"));
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
        .env("LLMUP_PROVIDER_DEFAULT_API_KEY", "parent-provider-key")
        .env("UNRELATED_SECRET_COPY", PROVIDER_KEY)
        .env("ANTHROPIC_API_KEY", "parent-anthropic-key")
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
        .wait_for_count(1, std::time::Duration::from_secs(2))
        .await;
    assert_eq!(requests.len(), 1, "upstream request should be captured");

    let fake = fs::read_to_string(&fake_log).expect("read fake client log");
    let local_proxy_key = local_proxy_key(&llmup_home);
    assert!(fake.contains(&format!("ANTHROPIC_API_KEY={local_proxy_key}")));
    assert!(!fake.contains(PROVIDER_KEY));
    assert!(!fake.contains("parent-provider-key"));
    assert!(fake.contains("ARG=--model"));
    assert!(fake.contains("ARG=default"));
    assert!(fake.contains("ARG=--dangerously-skip-permissions"));

    let user_config = fs::read_to_string(llmup_home.join("config.yaml")).expect("read user config");
    assert!(user_config.contains("listen: 127.0.0.1:8080"));
    assert_runtime_yaml_preserves_data_auth_and_overrides_listen(&llmup_home);
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
        .env("LLMUP_PROVIDER_DEFAULT_API_KEY", "parent-provider-key")
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
        .wait_for_count(1, std::time::Duration::from_secs(2))
        .await;
    assert_eq!(requests.len(), 1, "upstream request should be captured");

    let fake = fs::read_to_string(&fake_log).expect("read fake client log");
    let local_proxy_key = local_proxy_key(&llmup_home);
    assert!(fake.contains(&format!("OPENAI_API_KEY={local_proxy_key}")));
    assert!(!fake.contains(PROVIDER_KEY));
    assert!(!fake.contains("parent-provider-key"));
    assert!(fake.contains("ARG=-m"));
    assert!(fake.contains("ARG=default"));
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
        "anthropic",
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
