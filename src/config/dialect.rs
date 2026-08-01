//! Per-upstream reasoning dialect types (Steps 1 + 2 of the reasoning/dialect plan).
//!
//! These are pure type definitions plus a preset lookup table. They are parsed and validated at
//! config load time but NOT acted upon yet — runtime emit logic lands in a later step. When a
//! config carries no `dialect` block, every behavior remains identical to today.
//!
//! Lives under `config` so the config module can reach it directly; the `translate::internal`
//! module tree is private to `translate` and cannot be referenced from here.

use serde::{Deserialize, Serialize};

/// Unified reasoning-effort vocabulary (low → high).
///
/// Variants are declared in ascending union order so that the derived `Ord` matches the
/// vocabulary ladder defined in the plan: `none` → `minimal` → `low` → `medium` → `high` →
/// `xhigh` → `max` → `ultra`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ReasoningLevel {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "minimal")]
    Minimal,
    #[serde(rename = "low")]
    Low,
    #[serde(rename = "medium")]
    Medium,
    #[serde(rename = "high")]
    High,
    #[serde(rename = "xhigh")]
    Xhigh,
    #[serde(rename = "max")]
    Max,
    #[serde(rename = "ultra")]
    Ultra,
}

impl ReasoningLevel {
    /// Parse a level from its kebab-case wire name.
    pub(crate) fn parse(name: &str) -> Result<Self, String> {
        match name {
            "none" => Ok(Self::None),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            "ultra" => Ok(Self::Ultra),
            other => Err(format!("unknown reasoning level `{other}`")),
        }
    }
}

impl std::str::FromStr for ReasoningLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl std::fmt::Display for ReasoningLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
        .fmt(f)
    }
}

/// How the proxy emits unified reasoning effort to one upstream's native wire shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReasoningMechanism {
    #[serde(rename = "openai-effort")]
    OpenAiEffort,
    #[serde(rename = "anthropic-effort")]
    AnthropicEffort,
    #[serde(rename = "anthropic-thinking")]
    AnthropicThinking,
    #[serde(rename = "auto-only")]
    AutoOnly,
    #[serde(rename = "none")]
    None,
}

/// A resolved per-upstream dialect declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialectBlock {
    /// Required: selects the upstream emit shape.
    pub reasoning: ReasoningMechanism,
    /// Whether the upstream echoes reasoning output. Default is per-mechanism when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_echo: Option<bool>,
    /// Ordered subset of the union declaring the upstream's supported ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_levels: Option<Vec<ReasoningLevel>>,
}

impl DialectBlock {
    /// Validate the block: `reasoning_levels`, when present, must be a non-empty, strictly
    /// increasing subset of the union (no duplicates, no out-of-order entries).
    pub(crate) fn validate(&self) -> Result<(), String> {
        if let Some(levels) = &self.reasoning_levels {
            if levels.is_empty() {
                return Err("dialect.reasoning_levels must not be empty".to_string());
            }
            for window in levels.windows(2) {
                if window[0] >= window[1] {
                    return Err(format!(
                        "dialect.reasoning_levels must be strictly increasing in union order; `{}` before `{}`",
                        window[0], window[1]
                    ));
                }
            }
        }
        Ok(())
    }
}

/// A known preset name. Any string parses here; resolution against the registry (and rejection of
/// unknown names) happens at config validation time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetName(String);

/// Optional per-upstream dialect. Accepts either a preset string shorthand or a detailed block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UpstreamDialect {
    /// A known preset name from the §4.1 registry, expanded at config-parse time.
    Preset(PresetName),
    /// A fully specified block for any custom/unknown provider.
    Detailed(DialectBlock),
}

impl UpstreamDialect {
    /// Expand a preset or validate a detailed block into a resolved [`DialectBlock`].
    ///
    /// Unknown preset names and malformed `reasoning_levels` are reported as errors here, at
    /// config validation time. The returned block is not yet acted upon (Step 3 does that).
    pub(crate) fn resolve(&self) -> Result<DialectBlock, String> {
        match self {
            UpstreamDialect::Preset(name) => {
                resolve_dialect_preset(name.0.as_str())
                    .ok_or_else(|| format!("unknown dialect preset `{}`", name.0))
            }
            UpstreamDialect::Detailed(block) => {
                block.validate()?;
                Ok(block.clone())
            }
        }
    }
}

/// Look up a shipped preset by name. Returns `None` for unknown names.
///
/// The registry is `const`-equivalent data shipped with the proxy (based on the plan's provider
/// research), not runtime provider branching.
pub(crate) fn resolve_dialect_preset(name: &str) -> Option<DialectBlock> {
    let block = match name {
        "deepseek-openai" => DialectBlock {
            reasoning: ReasoningMechanism::OpenAiEffort,
            reasoning_echo: Some(true),
            reasoning_levels: Some(vec![
                ReasoningLevel::Low,
                ReasoningLevel::High,
                ReasoningLevel::Max,
            ]),
        },
        "glm-openai" => DialectBlock {
            reasoning: ReasoningMechanism::OpenAiEffort,
            reasoning_echo: Some(true),
            reasoning_levels: Some(vec![
                ReasoningLevel::None,
                ReasoningLevel::Minimal,
                ReasoningLevel::High,
                ReasoningLevel::Max,
            ]),
        },
        "glm-anthropic" => DialectBlock {
            reasoning: ReasoningMechanism::AutoOnly,
            reasoning_echo: Some(false),
            reasoning_levels: None,
        },
        "qwen-openai" => DialectBlock {
            reasoning: ReasoningMechanism::AutoOnly,
            reasoning_echo: Some(true),
            reasoning_levels: None,
        },
        _ => return None,
    };
    Some(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    // --- Step 1: ReasoningLevel enum ---

    #[test]
    fn reasoning_level_round_trips_through_serde() {
        for (name, level) in [
            ("none", ReasoningLevel::None),
            ("minimal", ReasoningLevel::Minimal),
            ("low", ReasoningLevel::Low),
            ("medium", ReasoningLevel::Medium),
            ("high", ReasoningLevel::High),
            ("xhigh", ReasoningLevel::Xhigh),
            ("max", ReasoningLevel::Max),
            ("ultra", ReasoningLevel::Ultra),
        ] {
            let serialized = serde_json::to_string(&level).unwrap();
            assert_eq!(serialized, format!("\"{name}\""), "serialize {name}");
            let parsed: ReasoningLevel = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed, level, "parse {name}");
            // Display + FromStr agree with serde.
            assert_eq!(level.to_string(), name, "display {name}");
            assert_eq!(ReasoningLevel::from_str(name).unwrap(), level, "from_str {name}");
        }
    }

    #[test]
    fn reasoning_level_ordering_matches_union_ladder() {
        assert!(ReasoningLevel::Low < ReasoningLevel::Medium);
        assert!(ReasoningLevel::Medium < ReasoningLevel::High);
        assert!(ReasoningLevel::None < ReasoningLevel::Ultra);
        // Full ladder is strictly increasing in declaration order.
        let ladder = [
            ReasoningLevel::None,
            ReasoningLevel::Minimal,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
            ReasoningLevel::Max,
            ReasoningLevel::Ultra,
        ];
        for window in ladder.windows(2) {
            assert!(window[0] < window[1], "{} < {}", window[0], window[1]);
        }
    }

    #[test]
    fn reasoning_level_rejects_unknown_strings() {
        assert!(ReasoningLevel::from_str("extreme").is_err());
        assert!(serde_json::from_str::<ReasoningLevel>("\"EXTREME\"").is_err());
        assert!(serde_json::from_str::<ReasoningLevel>("\"\"").is_err());
    }

    // --- Step 1: ReasoningMechanism enum ---

    #[test]
    fn reasoning_mechanism_round_trips_through_serde() {
        for (name, mechanism) in [
            ("openai-effort", ReasoningMechanism::OpenAiEffort),
            ("anthropic-effort", ReasoningMechanism::AnthropicEffort),
            ("anthropic-thinking", ReasoningMechanism::AnthropicThinking),
            ("auto-only", ReasoningMechanism::AutoOnly),
            ("none", ReasoningMechanism::None),
        ] {
            let serialized = serde_json::to_string(&mechanism).unwrap();
            assert_eq!(serialized, format!("\"{name}\""), "serialize {name}");
            let parsed: ReasoningMechanism = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed, mechanism, "parse {name}");
        }
    }

    #[test]
    fn reasoning_mechanism_rejects_unknown_strings() {
        assert!(serde_json::from_str::<ReasoningMechanism>("\"openai\"").is_err());
        assert!(serde_json::from_str::<ReasoningMechanism>("\"OpenAiEffort\"").is_err());
    }

    // --- Step 2: DialectBlock validation ---

    #[test]
    fn dialect_block_accepts_valid_strictly_increasing_levels() {
        let block = DialectBlock {
            reasoning: ReasoningMechanism::OpenAiEffort,
            reasoning_echo: Some(true),
            reasoning_levels: Some(vec![
                ReasoningLevel::Low,
                ReasoningLevel::High,
                ReasoningLevel::Max,
            ]),
        };
        assert!(block.validate().is_ok());
    }

    #[test]
    fn dialect_block_rejects_empty_levels() {
        let block = DialectBlock {
            reasoning: ReasoningMechanism::OpenAiEffort,
            reasoning_echo: None,
            reasoning_levels: Some(Vec::new()),
        };
        let error = block.validate().expect_err("empty levels must fail");
        assert!(error.contains("must not be empty"), "{error}");
    }

    #[test]
    fn dialect_block_rejects_duplicate_levels() {
        let block = DialectBlock {
            reasoning: ReasoningMechanism::OpenAiEffort,
            reasoning_echo: None,
            reasoning_levels: Some(vec![ReasoningLevel::Low, ReasoningLevel::Low, ReasoningLevel::High]),
        };
        let error = block.validate().expect_err("duplicate levels must fail");
        assert!(error.contains("strictly increasing"), "{error}");
    }

    #[test]
    fn dialect_block_rejects_out_of_order_levels() {
        let block = DialectBlock {
            reasoning: ReasoningMechanism::OpenAiEffort,
            reasoning_echo: None,
            reasoning_levels: Some(vec![ReasoningLevel::High, ReasoningLevel::Low]),
        };
        assert!(block.validate().is_err());
    }

    // --- Step 2: Preset registry ---

    #[test]
    fn preset_registry_resolves_all_four_known_presets() {
        let deepseek = resolve_dialect_preset("deepseek-openai").expect("deepseek-openai");
        assert_eq!(deepseek.reasoning, ReasoningMechanism::OpenAiEffort);
        assert_eq!(deepseek.reasoning_echo, Some(true));
        assert_eq!(
            deepseek.reasoning_levels,
            Some(vec![
                ReasoningLevel::Low,
                ReasoningLevel::High,
                ReasoningLevel::Max,
            ])
        );

        let glm_openai = resolve_dialect_preset("glm-openai").expect("glm-openai");
        assert_eq!(glm_openai.reasoning, ReasoningMechanism::OpenAiEffort);
        assert_eq!(glm_openai.reasoning_echo, Some(true));
        assert_eq!(
            glm_openai.reasoning_levels,
            Some(vec![
                ReasoningLevel::None,
                ReasoningLevel::Minimal,
                ReasoningLevel::High,
                ReasoningLevel::Max,
            ])
        );

        let glm_anthropic = resolve_dialect_preset("glm-anthropic").expect("glm-anthropic");
        assert_eq!(glm_anthropic.reasoning, ReasoningMechanism::AutoOnly);
        assert_eq!(glm_anthropic.reasoning_echo, Some(false));
        assert_eq!(glm_anthropic.reasoning_levels, None);

        let qwen = resolve_dialect_preset("qwen-openai").expect("qwen-openai");
        assert_eq!(qwen.reasoning, ReasoningMechanism::AutoOnly);
        assert_eq!(qwen.reasoning_echo, Some(true));
        assert_eq!(qwen.reasoning_levels, None);
    }

    #[test]
    fn preset_registry_rejects_unknown_preset_name() {
        assert!(resolve_dialect_preset("deepseek-anthropic").is_none());
        assert!(resolve_dialect_preset("").is_none());
    }

    // --- Step 2: UpstreamDialect parse + resolve ---

    #[test]
    fn upstream_dialect_parses_preset_string() {
        let dialect: UpstreamDialect = serde_json::from_str("\"deepseek-openai\"").unwrap();
        match &dialect {
            UpstreamDialect::Preset(name) => assert_eq!(name.0, "deepseek-openai"),
            other => panic!("expected Preset, got {other:?}"),
        }
        let resolved = dialect.resolve().expect("known preset resolves");
        assert_eq!(resolved.reasoning, ReasoningMechanism::OpenAiEffort);
        assert_eq!(resolved.reasoning_echo, Some(true));
    }

    #[test]
    fn upstream_dialect_parses_detailed_block() {
        let dialect: UpstreamDialect = serde_json::from_str(
            r#"{"reasoning":"anthropic-effort","reasoning_echo":false,"reasoning_levels":["low","high"]}"#,
        )
        .unwrap();
        let resolved = dialect.resolve().expect("valid detailed block resolves");
        assert_eq!(resolved.reasoning, ReasoningMechanism::AnthropicEffort);
        assert_eq!(resolved.reasoning_echo, Some(false));
        assert_eq!(
            resolved.reasoning_levels,
            Some(vec![ReasoningLevel::Low, ReasoningLevel::High])
        );
    }

    #[test]
    fn upstream_dialect_resolve_rejects_unknown_preset() {
        let dialect: UpstreamDialect = serde_json::from_str("\"no-such-preset\"").unwrap();
        let error = dialect.resolve().expect_err("unknown preset must fail");
        assert!(error.contains("unknown dialect preset"), "{error}");
        assert!(error.contains("no-such-preset"), "{error}");
    }

    #[test]
    fn upstream_dialect_detailed_resolve_validates_levels() {
        let dialect: UpstreamDialect = serde_json::from_str(
            r#"{"reasoning":"openai-effort","reasoning_levels":["high","low"]}"#,
        )
        .unwrap();
        assert!(dialect.resolve().is_err());
    }

    #[test]
    fn upstream_dialect_skips_absent_optional_fields_when_serializing() {
        let block = DialectBlock {
            reasoning: ReasoningMechanism::AutoOnly,
            reasoning_echo: None,
            reasoning_levels: None,
        };
        let serialized = serde_json::to_string(&block).unwrap();
        assert_eq!(serialized, r#"{"reasoning":"auto-only"}"#);
    }
}
