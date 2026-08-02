//! TDD tests for the `codex-setup` subcommand (Phase 1 MVP).
//!
//! Covers: provider TOML, agent TOML (no fork fields), profile features,
//! state.json (no keys), safe write (backup + atomic + merge), arg parsing,
//! install, status, uninstall.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use llm_universal_proxy::user_tools::codex_setup::{
    agent_file_name, agent_name, build_profile_content, detect_codex_version,
    generate_agent_content, generate_provider_block, generate_state_content, install, parse_args,
    patch_catalog_v1, run_status, run_uninstall, write_config_file_atomic, Action, CliOptions,
    GeneratedPaths, SetupInput,
};

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
            "llmup-codex-setup-{label}-{}-{nanos}",
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

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

// ---------- Provider TOML ----------

#[test]
fn provider_block_has_required_fields() {
    let block = generate_provider_block("https://proxy.example.com");
    assert!(
        block.contains("[model_providers.llmup]"),
        "missing provider table header: {block}"
    );
    assert!(block.contains("name = \"LLMUP\""));
    assert!(block.contains("base_url = \"https://proxy.example.com\""));
    assert!(block.contains("wire_api = \"responses\""));
    assert!(block.contains("requires_openai_auth = false"));
    assert!(block.contains("env_key = \"LLMUP_PROXY_KEY\""));
}

#[test]
fn provider_block_normalizes_trailing_slash() {
    let block = generate_provider_block("https://proxy.example.com/");
    assert!(block.contains("base_url = \"https://proxy.example.com\""));
    assert!(!block.contains("example.com/\""));
}

// ---------- Agent TOML ----------

#[test]
fn agent_content_full_has_fields_and_no_fork_or_multiagent_keys() {
    let content = generate_agent_content("gpt-5", Some("medium"), Some(200_000));
    assert_eq!(agent_name("gpt-5"), "llmup_gpt-5");
    assert_eq!(agent_file_name("gpt-5"), "llmup-gpt-5.toml");
    assert!(content.contains("name = \"llmup_gpt-5\""));
    assert!(content.contains("description = \"LLMUP sub-agent for gpt-5\""));
    assert!(content.contains("model = \"gpt-5\""));
    assert!(content.contains("model_provider = \"llmup\""));
    assert!(
        content.contains("developer_instructions ="),
        "missing developer_instructions: {content}"
    );
    assert!(content.contains("model_reasoning_effort = \"medium\""));
    assert!(content.contains("model_context_window = 200000"));

    let lower = content.to_lowercase();
    assert!(
        !lower.contains("fork_turns"),
        "agent TOML must NOT contain fork_turns: {content}"
    );
    assert!(
        !lower.contains("fork_context"),
        "agent TOML must NOT contain fork_context: {content}"
    );
    assert!(
        !lower.contains("multi_agent"),
        "agent TOML must NOT contain multi_agent fields: {content}"
    );
}

#[test]
fn agent_content_minimal_omits_optional_fields() {
    let content = generate_agent_content("claude-opus", None, None);
    assert!(content.contains("model = \"claude-opus\""));
    assert!(content.contains("model_provider = \"llmup\""));
    assert!(
        !content.contains("model_reasoning_effort"),
        "optional effort leaked into minimal agent TOML: {content}"
    );
    assert!(
        !content.contains("model_context_window"),
        "optional context window leaked into minimal agent TOML: {content}"
    );
}

// ---------- Profile TOML (features) ----------

#[test]
fn profile_content_includes_v1_default_feature_flag() {
    let content = build_profile_content(None, "https://proxy.example.com", None);
    assert!(
        content.contains("[features]"),
        "profile missing [features] table: {content}"
    );
    assert!(
        content.contains("multi_agent_v2 = false"),
        "profile must defensively set multi_agent_v2 = false: {content}"
    );
    assert!(content.contains("[model_providers.llmup]"));
}

#[test]
fn profile_content_merge_preserves_unrelated_tables_and_is_idempotent() {
    let existing = "\
# my codex profile
[model_providers.openai]
name = \"OpenAI\"
base_url = \"https://api.openai.com\"

[some_other_setting]
foo = \"bar\"
";
    let first = build_profile_content(Some(existing), "https://proxy.example.com", None);
    // Unrelated content preserved.
    assert!(first.contains("# my codex profile"));
    assert!(first.contains("[model_providers.openai]"));
    assert!(first.contains("name = \"OpenAI\""));
    assert!(first.contains("[some_other_setting]"));
    assert!(first.contains("foo = \"bar\""));
    // Managed block present.
    assert!(first.contains("[model_providers.llmup]"));
    assert!(first.contains("multi_agent_v2 = false"));

    // Idempotent: re-running does not duplicate the managed provider table.
    let second = build_profile_content(Some(&first), "https://proxy.example.com", None);
    let occurrences = second.matches("[model_providers.llmup]").count();
    assert_eq!(
        occurrences, 1,
        "managed provider table duplicated after re-run: {second}"
    );
    let feature_occurrences = second.matches("[features]").count();
    assert_eq!(
        feature_occurrences, 1,
        "features table duplicated: {second}"
    );
    // Unrelated content still preserved after re-run.
    assert!(second.contains("name = \"OpenAI\""));
}

// ---------- State JSON ----------

#[test]
fn state_content_has_shape_and_no_key_material() {
    let managed = vec![
        "/home/user/.codex/llmup.config.toml".to_string(),
        "/home/user/.codex/agents/llmup-gpt-5.toml".to_string(),
    ];
    let content = generate_state_content("0.146.0", &managed, "2026-08-02T00:00:00Z");
    let value: serde_json::Value =
        serde_json::from_str(&content).expect("state content must be valid JSON");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["codex_version"], "0.146.0");
    assert_eq!(value["provider_id"], "llmup");
    assert!(value["managed_files"].is_array());
    assert_eq!(value["managed_files"].as_array().unwrap().len(), 2);
    assert_eq!(value["created_at"], "2026-08-02T00:00:00Z");

    let lower = content.to_lowercase();
    for banned in [
        "key",
        "token",
        "secret",
        "bearer",
        "authorization",
        "api_key",
        "provider_key",
    ] {
        assert!(
            !lower.contains(banned),
            "state.json must not contain key-like term `{banned}`: {content}"
        );
    }
}

// ---------- Safe write ----------

#[test]
fn write_config_file_atomic_creates_backup_and_renames() {
    let dir = TempDir::new("safe-write");
    let target = dir.path().join("llmup.config.toml");
    let backup = dir.path().join("llmup.config.toml.bak");

    write_config_file_atomic(&target, "first").expect("first write");
    assert_eq!(read(&target), "first");
    assert!(!backup.exists(), "no backup before second write");

    write_config_file_atomic(&target, "second").expect("second write");
    assert_eq!(read(&target), "second");
    assert!(backup.exists(), "backup must exist after overwrite");
    assert_eq!(read(&backup), "first", "backup must hold previous content");
}

// ---------- Arg parsing ----------

#[test]
fn parse_args_generate_requires_three_core_flags() {
    let opts = parse_args(&[
        "--base-url".into(),
        "https://proxy.example.com".into(),
        "--model".into(),
        "gpt-5".into(),
        "--provider-key".into(),
        "sk-secret".into(),
    ])
    .expect("valid generate args");
    assert_eq!(opts.action, Action::Generate);
    assert_eq!(opts.base_url.as_deref(), Some("https://proxy.example.com"));
    assert_eq!(opts.model.as_deref(), Some("gpt-5"));
    assert_eq!(opts.provider_key.as_deref(), Some("sk-secret"));
    assert!(opts.reasoning_effort.is_none());
    assert!(opts.context_window.is_none());
}

#[test]
fn parse_args_accepts_inline_equals_form() {
    let opts = parse_args(&[
        "--base-url=https://proxy.example.com".into(),
        "--model=gpt-5".into(),
        "--provider-key=sk-secret".into(),
        "--reasoning-effort=low".into(),
        "--context-window=128000".into(),
    ])
    .expect("inline args");
    assert_eq!(opts.base_url.as_deref(), Some("https://proxy.example.com"));
    assert_eq!(opts.model.as_deref(), Some("gpt-5"));
    assert_eq!(opts.provider_key.as_deref(), Some("sk-secret"));
    assert_eq!(opts.reasoning_effort.as_deref(), Some("low"));
    assert_eq!(opts.context_window, Some(128_000));
}

#[test]
fn parse_args_status_and_uninstall_actions() {
    let status = parse_args(&["--status".into()]).expect("status");
    assert_eq!(status.action, Action::Status);
    let uninstall = parse_args(&["--uninstall".into()]).expect("uninstall");
    assert_eq!(uninstall.action, Action::Uninstall);
}

#[test]
fn parse_args_status_and_uninstall_are_mutually_exclusive() {
    let err = parse_args(&["--status".into(), "--uninstall".into()]).unwrap_err();
    assert!(
        err.contains("mutually exclusive"),
        "expected mutual-exclusion error: {err}"
    );
}

#[test]
fn parse_args_generate_missing_required_flags_errors() {
    let err = parse_args(&["--base-url".into(), "https://proxy.example.com".into()]).unwrap_err();
    assert!(
        err.contains("--model") || err.contains("--provider-key"),
        "expected missing-required error: {err}"
    );
}

#[test]
fn parse_args_rejects_unknown_flag() {
    let err = parse_args(&[
        "--base-url".into(),
        "u".into(),
        "--model".into(),
        "m".into(),
        "--provider-key".into(),
        "k".into(),
        "--bogus".into(),
    ])
    .unwrap_err();
    assert!(err.contains("unknown"), "expected unknown-arg error: {err}");
}

// ---------- install / status / uninstall (filesystem) ----------

fn sample_input() -> SetupInput {
    SetupInput {
        base_url: "https://proxy.example.com".to_string(),
        model: "gpt-5".to_string(),
        reasoning_effort: Some("medium".to_string()),
        context_window: Some(200_000),
    }
}

#[test]
fn install_writes_all_files_with_correct_shape() {
    let home = TempDir::new("install");
    let paths: GeneratedPaths =
        install(&sample_input(), home.path(), None).expect("install succeeds");

    assert!(paths.profile_path.ends_with("llmup.config.toml"));
    assert!(
        paths.agent_path.ends_with(agent_file_name("gpt-5")),
        "agent path mismatch: {}",
        paths.agent_path.display()
    );
    assert!(paths.state_path.ends_with("state.json"));

    let profile = read(&paths.profile_path);
    assert!(profile.contains("[model_providers.llmup]"));
    assert!(profile.contains("multi_agent_v2 = false"));

    let agent = read(&paths.agent_path);
    assert!(agent.contains("model_provider = \"llmup\""));
    assert!(agent.contains("model = \"gpt-5\""));
    let agent_lower = agent.to_lowercase();
    assert!(!agent_lower.contains("fork_turns"));
    assert!(!agent_lower.contains("fork_context"));

    let state = read(&paths.state_path);
    let value: serde_json::Value = serde_json::from_str(&state).expect("state json");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["provider_id"], "llmup");
    assert!(value["managed_files"].is_array());
    let state_lower = state.to_lowercase();
    assert!(!state_lower.contains("key") && !state_lower.contains("token"));
}

#[test]
fn status_reports_installed_files_from_state() {
    let home = TempDir::new("status");
    install(&sample_input(), home.path(), None).expect("install");

    let mut buffer = Vec::new();
    let code = run_status(home.path(), &mut Cursor::new(&mut buffer)).expect("status ok");
    assert_eq!(code, 0);
    let output = String::from_utf8(buffer).expect("utf8");
    assert!(
        output.contains("codex-setup"),
        "status header missing: {output}"
    );
    assert!(
        output.contains("llmup-gpt-5.toml") || output.contains("llmup.config.toml"),
        "status should list managed files: {output}"
    );
}

#[test]
fn status_reports_not_installed_when_state_absent() {
    let home = TempDir::new("status-empty");
    let mut buffer = Vec::new();
    let code = run_status(home.path(), &mut Cursor::new(&mut buffer)).expect("status ok");
    assert_eq!(code, 0);
    let output = String::from_utf8(buffer).expect("utf8");
    assert!(
        output.contains("not installed") || output.contains("nothing"),
        "expected not-installed message: {output}"
    );
}

#[test]
fn uninstall_removes_managed_files_and_state() {
    let home = TempDir::new("uninstall");
    let paths = install(&sample_input(), home.path(), None).expect("install");
    assert!(paths.profile_path.exists());
    assert!(paths.agent_path.exists());
    assert!(paths.state_path.exists());

    let mut buffer = Vec::new();
    let code = run_uninstall(home.path(), &mut Cursor::new(&mut buffer)).expect("uninstall ok");
    assert_eq!(code, 0);
    assert!(!paths.profile_path.exists(), "profile still present");
    assert!(!paths.agent_path.exists(), "agent still present");
    assert!(!paths.state_path.exists(), "state still present");
}

// ---------- detect codex version (graceful) ----------

#[test]
fn detect_codex_version_returns_string_without_panicking() {
    let _version = detect_codex_version();
    // Just ensure it does not panic when codex is absent from PATH.
}

// ---------- silent drop of unused CliOptions field check ----------

#[test]
fn parse_args_help_action() {
    let opts = parse_args(&["--help".into()]).expect("help parses");
    assert_eq!(opts.action, Action::Help);
    let opts = parse_args(&["-h".into()]).expect("-h parses");
    assert_eq!(opts.action, Action::Help);
}

// ---------- --force-v1 catalog derivation ----------

/// Fixture: 3 models — one already on "v2", one explicitly null, one field absent.
const FIXTURE_CATALOG: &str = r#"{"models":[
  {"id":"gpt-5","multi_agent_version":"v2","display_name":"GPT-5"},
  {"id":"claude-opus","multi_agent_version":null,"display_name":"Opus"},
  {"id":"gemini-pro","display_name":"Gemini Pro"}
]}"#;

#[test]
fn patch_catalog_v1_sets_every_model_to_v1_and_preserves_other_fields() {
    let catalog: serde_json::Value = serde_json::from_str(FIXTURE_CATALOG).expect("fixture json");
    let patched = patch_catalog_v1(&catalog);
    let models = patched["models"]
        .as_array()
        .expect("patched catalog keeps a models array");
    assert_eq!(models.len(), 3);
    // ALL three models — regardless of prior value — must end up on "v1".
    for model in models {
        assert_eq!(model["multi_agent_version"], "v1");
    }
    // Unrelated fields are preserved verbatim.
    assert_eq!(patched["models"][0]["id"], "gpt-5");
    assert_eq!(patched["models"][0]["display_name"], "GPT-5");
    assert_eq!(patched["models"][1]["id"], "claude-opus");
    assert_eq!(patched["models"][2]["id"], "gemini-pro");
    // The result must still be valid JSON shaped {models: [...]}.
    let reparsed: serde_json::Value =
        serde_json::from_str(&patched.to_string()).expect("patched output is valid JSON");
    assert!(reparsed["models"].is_array());
}

#[test]
fn parse_args_force_v1_flag_is_opt_in() {
    let opts = parse_args(&[
        "--base-url".into(),
        "https://proxy.example.com".into(),
        "--model".into(),
        "gpt-5".into(),
        "--provider-key".into(),
        "sk-secret".into(),
        "--force-v1".into(),
    ])
    .expect("valid args with --force-v1");
    assert!(opts.force_v1, "force_v1 must be true when flag is passed");
}

#[test]
fn parse_args_force_v1_defaults_false() {
    let opts = parse_args(&[
        "--base-url".into(),
        "https://proxy.example.com".into(),
        "--model".into(),
        "gpt-5".into(),
        "--provider-key".into(),
        "sk-secret".into(),
    ])
    .expect("valid args");
    assert!(!opts.force_v1, "force_v1 must default to false");
}

#[test]
fn profile_content_includes_model_catalog_json_when_path_given() {
    let content = build_profile_content(
        None,
        "https://proxy.example.com",
        Some("/home/user/.codex/llmup/model-catalog.json"),
    );
    assert!(
        content.contains("model_catalog_json = \"/home/user/.codex/llmup/model-catalog.json\""),
        "expected top-level model_catalog_json: {content}"
    );
    // The defensive V1 feature flag is still present.
    assert!(content.contains("[features]"));
    assert!(content.contains("multi_agent_v2 = false"));
}

#[test]
fn profile_content_omits_model_catalog_json_when_no_path() {
    let content = build_profile_content(None, "https://proxy.example.com", None);
    assert!(
        !content.contains("model_catalog_json"),
        "default profile must not set model_catalog_json: {content}"
    );
    assert!(content.contains("multi_agent_v2 = false"));
}

#[test]
fn install_with_force_v1_catalog_writes_derived_catalog_and_state_entry() {
    let home = TempDir::new("force-v1");
    let paths = install(&sample_input(), home.path(), Some(FIXTURE_CATALOG))
        .expect("install with derived catalog");

    // Derived catalog lands at ~/.codex/llmup/model-catalog.json.
    let expected_catalog = home.path().join("llmup").join("model-catalog.json");
    assert_eq!(
        paths.catalog_path.as_ref().expect("catalog path recorded"),
        &expected_catalog,
        "derived catalog path mismatch"
    );
    assert!(expected_catalog.exists(), "derived catalog file missing");
    let catalog_body = read(&expected_catalog);
    let parsed: serde_json::Value =
        serde_json::from_str(&catalog_body).expect("derived catalog is valid JSON");
    for model in parsed["models"].as_array().unwrap() {
        assert_eq!(model["multi_agent_version"], "v1");
    }

    // Profile references the derived catalog at top level.
    let profile = read(&paths.profile_path);
    assert!(
        profile.contains("model_catalog_json = \""),
        "profile must reference the derived catalog: {profile}"
    );

    // State.json lists the derived catalog under managed_files (for uninstall).
    let state = read(&paths.state_path);
    let state_value: serde_json::Value = serde_json::from_str(&state).expect("state json");
    let files = state_value["managed_files"].as_array().unwrap();
    assert!(
        files.iter().any(|f| {
            f.as_str()
                .is_some_and(|s| s.ends_with("model-catalog.json"))
        }),
        "state must track the derived catalog: {state}"
    );
}

#[test]
fn install_without_force_v1_is_feature_flag_only_with_no_catalog() {
    let home = TempDir::new("no-force-v1");
    let paths = install(&sample_input(), home.path(), None).expect("install without catalog");

    let profile = read(&paths.profile_path);
    assert!(profile.contains("multi_agent_v2 = false"));
    assert!(
        !profile.contains("model_catalog_json"),
        "feature-flag-only profile must not set model_catalog_json: {profile}"
    );
    assert!(
        paths.catalog_path.is_none(),
        "no catalog path should be reported"
    );
    let catalog_file = home.path().join("llmup").join("model-catalog.json");
    assert!(
        !catalog_file.exists(),
        "no catalog file should be written without --force-v1"
    );
}

#[allow(dead_code)]
fn _ensure_cli_options_fields_present(_opts: CliOptions) {}
