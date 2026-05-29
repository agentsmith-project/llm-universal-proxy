use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::config::{ApplyPatchTransport, ModelLimits, ModelModality, ModelSurface};
use crate::formats::UpstreamFormat;
use crate::Config;

const AUTO_COMPACT_NUMERATOR: u128 = 85;
const AUTO_COMPACT_DENOMINATOR: u128 = 100;
const CODEX_TRUNCATION_LIMIT_BYTES: u64 = 10_000;
const PUBLIC_APPLY_PATCH_TOOL_TYPE: &str = "freeform";
const CODEX_BASE_INSTRUCTIONS: &str = "You are Codex, a coding agent based on GPT-5. You and the user share the same workspace and collaborate to achieve the user's goals.";
pub const DEFAULT_AGENT_MODEL_ALIAS: &str = "main";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentModelProfile {
    pub alias: String,
    pub upstream_format: Option<UpstreamFormat>,
    pub limits: Option<ModelLimits>,
    pub surface: ModelSurface,
    pub codex_auto_compact_token_limit: Option<u64>,
}

impl AgentModelProfile {
    pub fn from_config(config: &Config, alias: &str) -> Result<Self, String> {
        let model_alias = config
            .model_aliases
            .get(alias)
            .ok_or_else(|| format!("unknown model alias `{alias}`"))?;
        let upstream_format = config
            .upstream(&model_alias.upstream_name)
            .and_then(|upstream| upstream.fixed_upstream_format);
        let surface = config.effective_model_surface(model_alias);
        let limits = surface.limits.clone();
        let codex_auto_compact_token_limit =
            codex_auto_compact_token_limit_for_profile(limits.as_ref());

        Ok(Self {
            alias: alias.to_string(),
            upstream_format,
            limits,
            surface,
            codex_auto_compact_token_limit,
        })
    }

    pub fn claude_auto_compact_window(&self) -> Option<u64> {
        self.limits
            .as_ref()
            .and_then(|limits| limits.context_window)
    }

    pub fn claude_max_output_tokens(&self) -> Option<u64> {
        self.limits
            .as_ref()
            .and_then(|limits| limits.max_output_tokens)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentModelCatalog {
    pub selected: AgentModelProfile,
    pub profiles: Vec<AgentModelProfile>,
}

impl AgentModelCatalog {
    pub fn from_config(config: &Config, selected_alias: &str) -> Result<Self, String> {
        let selected = AgentModelProfile::from_config(config, selected_alias)
            .map_err(|_| selected_alias_error(config, selected_alias))?;
        let profiles = config
            .model_aliases
            .keys()
            .map(|alias| AgentModelProfile::from_config(config, alias))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { selected, profiles })
    }

    pub fn has_alias(&self, alias: &str) -> bool {
        self.profiles.iter().any(|profile| profile.alias == alias)
    }
}

pub fn codex_auto_compact_token_limit(
    alias: &str,
    limits: Option<&ModelLimits>,
) -> Result<Option<u64>, String> {
    let Some(limits) = limits else {
        return Ok(None);
    };
    let Some(context_window) = limits.context_window else {
        return Ok(None);
    };
    let input_budget = match limits.max_output_tokens {
        Some(max_output_tokens) => {
            if max_output_tokens >= context_window {
                return Err(format!(
                    "model alias `{alias}` max_output_tokens ({max_output_tokens}) must be less than context_window ({context_window}) for Codex auto compact"
                ));
            }
            context_window - max_output_tokens
        }
        None => context_window,
    };
    Ok(Some(calculate_codex_auto_compact_token_limit(input_budget)))
}

pub fn build_codex_model_catalog(profile: &AgentModelProfile) -> Result<Value, String> {
    build_codex_model_catalog_for_profiles(std::slice::from_ref(profile))
}

pub fn build_codex_model_catalog_for_profiles(
    profiles: &[AgentModelProfile],
) -> Result<Value, String> {
    if profiles.is_empty() {
        return Err("codex model catalog requires at least one model alias".to_string());
    }
    let models = profiles
        .iter()
        .map(|profile| codex_catalog_entry(profile).map(Value::Object))
        .collect::<Result<Vec<_>, _>>()?;
    let payload = json!({ "models": models });
    reject_internal_tool_artifacts(&payload, "codex model catalog")?;
    Ok(payload)
}

fn codex_catalog_entry(profile: &AgentModelProfile) -> Result<Map<String, Value>, String> {
    let mut entry = default_codex_catalog_entry(&profile.alias);
    let codex_auto_compact_token_limit =
        codex_auto_compact_token_limit(&profile.alias, profile.limits.as_ref())?;

    if let Some(limits) = &profile.limits {
        if let Some(context_window) = limits.context_window {
            entry.insert("context_window".to_string(), json!(context_window));
            if let Some(auto_compact) = codex_auto_compact_token_limit {
                entry.insert("auto_compact_token_limit".to_string(), json!(auto_compact));
            }
        }
    }

    if let Some(input) = codex_input_modalities(profile) {
        entry.insert(
            "input_modalities".to_string(),
            Value::Array(
                input
                    .iter()
                    .map(|item| json!(modality_name(*item)))
                    .collect(),
            ),
        );
    }

    if let Some(tools) = &profile.surface.tools {
        if let Some(supports_search) = tools.supports_search {
            entry.insert("supports_search_tool".to_string(), json!(supports_search));
        }
        if let Some(apply_patch_transport) = tools.apply_patch_transport {
            entry.insert(
                "apply_patch_tool_type".to_string(),
                json!(codex_apply_patch_tool_type(apply_patch_transport)),
            );
        }
        if let Some(supports_parallel_calls) = tools.supports_parallel_calls {
            entry.insert(
                "supports_parallel_tool_calls".to_string(),
                json!(supports_parallel_calls),
            );
        }
    }

    Ok(entry)
}

fn codex_auto_compact_token_limit_for_profile(limits: Option<&ModelLimits>) -> Option<u64> {
    let limits = limits?;
    let context_window = limits.context_window?;
    let input_budget = match limits.max_output_tokens {
        Some(max_output_tokens) if max_output_tokens >= context_window => return None,
        Some(max_output_tokens) => context_window - max_output_tokens,
        None => context_window,
    };
    Some(calculate_codex_auto_compact_token_limit(input_budget))
}

fn calculate_codex_auto_compact_token_limit(input_budget: u64) -> u64 {
    ((input_budget as u128 * AUTO_COMPACT_NUMERATOR) / AUTO_COMPACT_DENOMINATOR) as u64
}

fn codex_input_modalities(profile: &AgentModelProfile) -> Option<Vec<ModelModality>> {
    if let Some(input) = profile
        .surface
        .modalities
        .as_ref()
        .and_then(|modalities| modalities.input.clone())
    {
        return Some(input);
    }
    if profile
        .surface
        .tools
        .as_ref()
        .and_then(|tools| tools.supports_view_image)
        == Some(false)
    {
        return Some(vec![ModelModality::Text]);
    }
    None
}

pub fn write_codex_model_catalog(
    profile: &AgentModelProfile,
    run_dir: impl AsRef<Path>,
) -> Result<PathBuf, String> {
    let catalog = build_codex_model_catalog(profile)?;
    write_codex_model_catalog_value(&catalog, run_dir)
}

pub fn write_codex_model_catalog_for_profiles(
    profiles: &[AgentModelProfile],
    run_dir: impl AsRef<Path>,
) -> Result<PathBuf, String> {
    let catalog = build_codex_model_catalog_for_profiles(profiles)?;
    write_codex_model_catalog_value(&catalog, run_dir)
}

fn write_codex_model_catalog_value(
    catalog: &Value,
    run_dir: impl AsRef<Path>,
) -> Result<PathBuf, String> {
    let catalog_dir = run_dir.as_ref().join("codex");
    fs::create_dir_all(&catalog_dir).map_err(|error| {
        format!(
            "failed to create Codex catalog dir {}: {error}",
            catalog_dir.display()
        )
    })?;
    let catalog_path = catalog_dir.join("model-catalog.json");
    let json = serde_json::to_string_pretty(&catalog)
        .map_err(|error| format!("failed to serialize Codex model catalog: {error}"))?;
    fs::write(&catalog_path, format!("{json}\n")).map_err(|error| {
        format!(
            "failed to write Codex model catalog {}: {error}",
            catalog_path.display()
        )
    })?;
    Ok(catalog_path)
}

fn selected_alias_error(config: &Config, selected_alias: &str) -> String {
    if selected_alias == DEFAULT_AGENT_MODEL_ALIAS
        && !config.model_aliases.contains_key(DEFAULT_AGENT_MODEL_ALIAS)
        && config.model_aliases.contains_key("default")
    {
        return "default llmup model alias `main` was not found, and this config still has the legacy alias `default`; run `llmup-config` to rename `default` to `main`, or run `llmup-config list` to view configured aliases. Pass `--llmup-model default` only when you intentionally want the legacy alias.".to_string();
    }
    format!(
        "unknown llmup model alias `{selected_alias}`; run `llmup-config list` to view configured aliases"
    )
}

fn default_codex_catalog_entry(alias: &str) -> Map<String, Value> {
    let mut entry = Map::new();
    entry.insert("slug".to_string(), json!(alias));
    entry.insert("display_name".to_string(), json!(alias));
    entry.insert(
        "description".to_string(),
        json!(format!("llmup proxy model {alias}")),
    );
    entry.insert(
        "supported_reasoning_levels".to_string(),
        json!([
            {
                "effort": "low",
                "description": "Fast responses with lighter reasoning"
            },
            {
                "effort": "medium",
                "description": "Balanced reasoning depth for everyday work"
            },
            {
                "effort": "high",
                "description": "Greater reasoning depth for harder problems"
            },
            {
                "effort": "xhigh",
                "description": "Maximum reasoning depth for complex problems"
            }
        ]),
    );
    entry.insert("shell_type".to_string(), json!("shell_command"));
    entry.insert("visibility".to_string(), json!("list"));
    entry.insert("supported_in_api".to_string(), json!(true));
    entry.insert("priority".to_string(), json!(0));
    entry.insert(
        "base_instructions".to_string(),
        json!(CODEX_BASE_INSTRUCTIONS),
    );
    entry.insert("supports_reasoning_summaries".to_string(), json!(false));
    entry.insert("support_verbosity".to_string(), json!(false));
    entry.insert(
        "truncation_policy".to_string(),
        json!({
            "mode": "bytes",
            "limit": CODEX_TRUNCATION_LIMIT_BYTES,
        }),
    );
    entry.insert(
        "apply_patch_tool_type".to_string(),
        json!(PUBLIC_APPLY_PATCH_TOOL_TYPE),
    );
    entry.insert("supports_parallel_tool_calls".to_string(), json!(false));
    entry.insert("experimental_supported_tools".to_string(), json!([]));
    entry
}

fn modality_name(modality: ModelModality) -> &'static str {
    match modality {
        ModelModality::Text => "text",
        ModelModality::Image => "image",
        ModelModality::Audio => "audio",
        ModelModality::Pdf => "pdf",
        ModelModality::File => "file",
        ModelModality::Video => "video",
    }
}

fn codex_apply_patch_tool_type(_transport: ApplyPatchTransport) -> &'static str {
    PUBLIC_APPLY_PATCH_TOOL_TYPE
}

fn reject_internal_tool_artifacts(value: &Value, context: &str) -> Result<(), String> {
    let text = value.to_string();
    if let Some(index) = text.find("__llmup_custom__") {
        let artifact = text[index..]
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '-')))
            .next()
            .unwrap_or("__llmup_custom__");
        return Err(format!(
            "{context} must not expose reserved internal tool artifact {artifact}"
        ));
    }
    Ok(())
}
