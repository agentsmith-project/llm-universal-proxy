use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

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
            b"https://api.minimaxi.com/v1\nMiniMax-M2.7-highspeed\nprovider-secret-from-prompt\n",
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
