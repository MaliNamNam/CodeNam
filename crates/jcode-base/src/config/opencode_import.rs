//! Load OpenCode-compatible JSON project config as CodeNam.json / opencode.json.
//!
//! Merge order (later wins for scalar keys; permission rules append):
//!   1. ~/.jcode/config.toml (already loaded)
//!   2. discovered JSON layers (global then project, root → cwd)
//!   3. env overrides (applied after this module returns)

use super::{
    Config, NamedProviderAuth, NamedProviderConfig, NamedProviderModelConfig, NamedProviderType,
    PermissionRuleConfig,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Apply OpenCode-style JSON configs found on disk into `config`.
pub(super) fn apply_discovered_json_layers(config: &mut Config) {
    for path in discover_config_paths() {
        match load_json_file(&path) {
            Ok(value) => {
                crate::logging::info(&format!(
                    "Loaded CodeNam/OpenCode config: {}",
                    path.display()
                ));
                apply_opencode_value(config, &value);
            }
            Err(err) => {
                crate::logging::warn(&format!(
                    "Failed to load {}: {}",
                    path.display(),
                    err
                ));
            }
        }
    }
}

/// Fingerprint JSON config files for process cache invalidation.
pub(super) fn json_config_fingerprint() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for path in discover_config_paths() {
        let key = format!("json_cfg:{}", path.display());
        let val = std::fs::metadata(&path)
            .ok()
            .map(|m| {
                format!(
                    "{}:{}",
                    m.len(),
                    m.modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                )
            })
            .unwrap_or_else(|| "missing".to_string());
        out.push((key, val));
    }
    out
}

/// Paths in merge order (earlier = lower priority).
pub(super) fn discover_config_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();

    // Global user layers (lowest priority among JSON)
    if let Ok(home) = crate::storage::jcode_dir() {
        for name in ["CodeNam.json", "CodeNam.jsonc", "opencode.json", "opencode.jsonc"] {
            let p = home.join(name);
            if p.is_file() {
                out.push(p);
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        let opencode_cfg = home.join(".config").join("opencode");
        for name in ["opencode.jsonc", "opencode.json", "config.json"] {
            let p = opencode_cfg.join(name);
            if p.is_file() {
                out.push(p);
            }
        }
    }

    // Project layers: walk from cwd up to root (then reverse so cwd wins)
    let mut project = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = Some(cwd.as_path());
        while let Some(d) = dir {
            for name in [
                "CodeNam.json",
                "CodeNam.jsonc",
                "opencode.json",
                "opencode.jsonc",
            ] {
                let p = d.join(name);
                if p.is_file() {
                    project.push(p);
                }
            }
            let nested = d.join(".jcode");
            for name in ["CodeNam.json", "CodeNam.jsonc", "opencode.json"] {
                let p = nested.join(name);
                if p.is_file() {
                    project.push(p);
                }
            }
            dir = d.parent();
        }
    }
    // root first, cwd last → later wins
    project.reverse();
    out.extend(project);
    out
}

fn load_json_file(path: &Path) -> anyhow::Result<Value> {
    let raw = std::fs::read_to_string(path)?;
    let cleaned = if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jsonc"))
        || raw.contains("//")
        || raw.contains("/*")
    {
        strip_jsonc_comments(&raw)
    } else {
        raw
    };
    let value: Value = serde_json::from_str(&cleaned)
        .map_err(|e| anyhow::anyhow!("invalid JSON in {}: {e}", path.display()))?;
    Ok(value)
}

/// Minimal JSONC: strip // line comments and /* block comments */ outside strings.
fn strip_jsonc_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    let mut escape = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_string {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < bytes.len() {
            let n = bytes[i + 1] as char;
            if n == '/' {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if n == '*' {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Map OpenCode-style JSON object fields into jcode `Config`.
pub(super) fn apply_opencode_value(config: &mut Config, value: &Value) {
    let Some(obj) = value.as_object() else {
        return;
    };

    if let Some(model) = obj.get("model").and_then(|v| v.as_str()) {
        apply_model_string(config, model);
    }
    if let Some(agent) = obj
        .get("default_agent")
        .or_else(|| obj.get("defaultAgent"))
        .and_then(|v| v.as_str())
    {
        config.agents.default_agent = agent.to_string();
    }
    if let Some(depth) = obj
        .get("subagent_depth")
        .or_else(|| obj.get("subagentDepth"))
        .and_then(|v| v.as_u64())
    {
        config.agents.subagent_depth = depth as usize;
    }
    // max steps: agent.build.steps or top-level steps
    if let Some(steps) = obj.get("steps").and_then(|v| v.as_u64()) {
        config.agents.max_steps = Some(steps as u32);
    }
    if let Some(agent_map) = obj.get("agent").or_else(|| obj.get("agents")) {
        if let Some(build) = agent_map.get("build") {
            if let Some(steps) = build.get("steps").and_then(|v| v.as_u64()) {
                config.agents.max_steps = Some(steps as u32);
            }
            if let Some(model) = build.get("model").and_then(|v| v.as_str()) {
                apply_model_string(config, model);
            }
        }
        if let Some(plan) = agent_map.get("plan") {
            // If default_agent not set but plan is present and user wants plan-first,
            // only set model from plan when default is plan.
            if config.agents.default_agent == "plan" {
                if let Some(model) = plan.get("model").and_then(|v| v.as_str()) {
                    apply_model_string(config, model);
                }
            }
        }
    }

    if let Some(perm) = obj.get("permission").or_else(|| obj.get("permissions")) {
        merge_permission_config(config, perm);
    }

    if let Some(tools) = obj.get("tools") {
        merge_tools_config(config, tools);
    }

    // OpenCode custom providers → jcode [providers.<name>] (shows in model picker).
    if let Some(providers) = obj.get("provider").or_else(|| obj.get("providers")) {
        merge_providers_config(config, providers);
    }

    if let Some(instructions) = obj.get("instructions").and_then(|v| v.as_array()) {
        // Store as preferred-tools-style hint via env is awkward; append as comment in logging only.
        // Real AGENTS.md loading is separate; instructions paths are best-effort note.
        let paths: Vec<&str> = instructions
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        if !paths.is_empty() {
            crate::logging::info(&format!(
                "CodeNam.json instructions (use AGENTS.md / prompt-overlay for full support): {}",
                paths.join(", ")
            ));
        }
    }
}

fn apply_model_string(config: &mut Config, model: &str) {
    let model = model.trim();
    if model.is_empty() {
        return;
    }
    // OpenCode: "provider/model-id"
    if let Some((provider, rest)) = model.split_once('/') {
        if !provider.is_empty() && !rest.is_empty() {
            config.provider.default_provider = Some(provider.to_string());
            config.provider.default_model = Some(rest.to_string());
            return;
        }
    }
    config.provider.default_model = Some(model.to_string());
}

fn merge_permission_config(config: &mut Config, perm: &Value) {
    // Top-level string → { "*": action }
    if let Some(action) = perm.as_str() {
        config.permission.rules.push(PermissionRuleConfig {
            permission: "*".to_string(),
            pattern: "*".to_string(),
            action: action.to_string(),
        });
        return;
    }
    let Some(map) = perm.as_object() else {
        return;
    };
    for (key, value) in map {
        // OpenCode interactive flags (non-standard but useful)
        if key == "interactive_ask" {
            if let Some(b) = value.as_bool() {
                config.permission.interactive_ask = b;
            }
            continue;
        }
        if key == "ask_timeout_secs" {
            if let Some(n) = value.as_u64() {
                config.permission.ask_timeout_secs = n;
            }
            continue;
        }
        if let Some(action) = value.as_str() {
            config.permission.rules.push(PermissionRuleConfig {
                permission: key.clone(),
                pattern: "*".to_string(),
                action: action.to_string(),
            });
            continue;
        }
        if let Some(patterns) = value.as_object() {
            for (pattern, action_v) in patterns {
                if let Some(action) = action_v.as_str() {
                    config.permission.rules.push(PermissionRuleConfig {
                        permission: key.clone(),
                        pattern: pattern.clone(),
                        action: action.to_string(),
                    });
                }
            }
        }
    }
}

/// Map OpenCode `provider` map into jcode named provider profiles.
///
/// Example OpenCode shape:
/// ```json
/// "provider": {
///   "grok-cli": {
///     "options": { "baseURL": "http://localhost:8787/v1", "apiKey": "local-proxy" },
///     "models": { "grok-4.5": { "id": "gcli/grok-4.5", "limit": { "context": 500000 } } }
///   }
/// }
/// ```
fn merge_providers_config(config: &mut Config, providers: &Value) {
    let Some(map) = providers.as_object() else {
        return;
    };

    let mut first_profile: Option<(String, String)> = None; // (profile, first_model_id)
    let mut imported_profiles: Vec<String> = Vec::new();

    for (name, entry) in map {
        let Some(entry_obj) = entry.as_object() else {
            continue;
        };

        let options = entry_obj.get("options").and_then(|v| v.as_object());
        let base_url = options
            .and_then(|o| {
                o.get("baseURL")
                    .or_else(|| o.get("baseUrl"))
                    .or_else(|| o.get("base_url"))
                    .and_then(|v| v.as_str())
            })
            .or_else(|| entry_obj.get("baseURL").and_then(|v| v.as_str()))
            .unwrap_or("")
            .trim()
            .to_string();

        if base_url.is_empty() {
            crate::logging::warn(&format!(
                "CodeNam.json provider '{name}' has no baseURL; skipped"
            ));
            continue;
        }

        let api_key = options
            .and_then(|o| {
                o.get("apiKey")
                    .or_else(|| o.get("api_key"))
                    .and_then(|v| v.as_str())
            })
            .map(|s| s.to_string());

        let api_key_env = options
            .and_then(|o| o.get("apiKeyEnv").or_else(|| o.get("api_key_env")))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mut models: Vec<NamedProviderModelConfig> = Vec::new();
        if let Some(models_obj) = entry_obj.get("models").and_then(|v| v.as_object()) {
            for (key, model_val) in models_obj {
                let id = model_val
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(key)
                    .trim()
                    .to_string();
                if id.is_empty() {
                    continue;
                }
                let context_window = model_val
                    .get("limit")
                    .and_then(|l| l.get("context"))
                    .and_then(|v| v.as_u64())
                    .or_else(|| {
                        model_val
                            .get("context")
                            .or_else(|| model_val.get("contextWindow"))
                            .and_then(|v| v.as_u64())
                    })
                    .map(|n| n as usize);
                let display_name = model_val
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        // Fall back to the map key when it's human-friendly.
                        let key = key.trim();
                        if key.contains('/') {
                            None
                        } else {
                            Some(key.to_string())
                        }
                    });
                models.push(NamedProviderModelConfig {
                    id,
                    name: display_name,
                    context_window,
                    input: Vec::new(),
                });
            }
        }

        // OpenCode sometimes lists models as an array of strings
        if models.is_empty() {
            if let Some(arr) = entry_obj.get("models").and_then(|v| v.as_array()) {
                for item in arr {
                    if let Some(id) = item.as_str() {
                        models.push(NamedProviderModelConfig {
                            id: id.to_string(),
                            name: None,
                            context_window: None,
                            input: Vec::new(),
                        });
                    } else if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                        models.push(NamedProviderModelConfig {
                            id: id.to_string(),
                            name: item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            context_window: item
                                .get("limit")
                                .and_then(|l| l.get("context"))
                                .and_then(|v| v.as_u64())
                                .map(|n| n as usize),
                            input: Vec::new(),
                        });
                    }
                }
            }
        }

        let default_model = models.first().map(|m| m.id.clone()).or_else(|| {
            entry_obj
                .get("default_model")
                .or_else(|| entry_obj.get("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

        let npm = entry_obj
            .get("npm")
            .and_then(|v| v.as_str())
            .unwrap_or("@ai-sdk/openai-compatible");
        let provider_type = if npm.contains("openrouter") {
            NamedProviderType::OpenRouter
        } else {
            NamedProviderType::OpenAiCompatible
        };

        let requires_api_key = if api_key.is_some() || api_key_env.is_some() {
            Some(true)
        } else {
            Some(false)
        };

        let profile = NamedProviderConfig {
            provider_type,
            base_url,
            api: None,
            auth: if api_key.is_some() || api_key_env.is_some() {
                NamedProviderAuth::Bearer
            } else {
                NamedProviderAuth::None
            },
            auth_header: None,
            api_key_env,
            api_key,
            env_file: None,
            default_model: default_model.clone(),
            requires_api_key,
            provider_routing: false,
            model_catalog: false,
            allow_provider_pinning: false,
            models,
            extra_body: None,
            supports_reasoning_effort: None,
        };

        if first_profile.is_none() {
            if let Some(m) = default_model.clone() {
                first_profile = Some((name.clone(), m));
            }
        }

        crate::logging::info(&format!(
            "CodeNam.json: registered provider profile '{name}' ({} models, base={})",
            profile.models.len(),
            profile.base_url
        ));
        imported_profiles.push(name.clone());
        config.providers.insert(name.clone(), profile);
    }

    // If user didn't set a top-level model, default to first imported profile/model.
    if config.provider.default_model.is_none() {
        if let Some((profile, model)) = first_profile {
            config.provider.default_provider = Some(profile);
            config.provider.default_model = Some(model);
        }
    }

    // Slim /model list: CodeNam.json profiles + first-party OAuth/API routes only.
    // Avoids flooding the picker with OpenRouter/Bedrock mega-catalogs.
    if config.provider.model_picker_providers.is_none() && !imported_profiles.is_empty() {
        let mut allow = imported_profiles;
        for method in [
            "claude-oauth",
            "claude-api",
            "openai-oauth",
            "openai-api",
            "copilot",
            "cursor",
            "gemini",
            "antigravity",
            "bedrock",
            "jcode",
        ] {
            allow.push(method.to_string());
        }
        config.provider.model_picker_providers = Some(allow);
    }
}

fn merge_tools_config(config: &mut Config, tools: &Value) {
    // OpenCode legacy: { "write": false, "bash": true }
    if let Some(map) = tools.as_object() {
        // Skip if it looks like our ToolConfig shape
        if map.contains_key("profile") || map.contains_key("enabled") || map.contains_key("disabled")
        {
            if let Some(profile) = map.get("profile").and_then(|v| v.as_str()) {
                config.tools.profile = profile.to_string();
            }
            if let Some(enabled) = map.get("enabled").and_then(|v| v.as_array()) {
                config.tools.enabled = enabled
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
            }
            if let Some(disabled) = map.get("disabled").and_then(|v| v.as_array()) {
                config.tools.disabled = disabled
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
            }
            return;
        }
        for (name, enabled) in map {
            if let Some(on) = enabled.as_bool() {
                let canonical = match name.as_str() {
                    "write" | "edit" | "patch" => "edit",
                    other => other,
                };
                if on {
                    if !config.tools.enabled.iter().any(|e| e == canonical) {
                        // Only push to enabled if user is building an allowlist;
                        // prefer disabled list for false flags.
                    }
                } else {
                    let tool = if canonical == "edit" {
                        // disable common write tools
                        for t in ["write", "edit", "multiedit", "patch", "apply_patch"] {
                            if !config.tools.disabled.iter().any(|d| d == t) {
                                config.tools.disabled.push(t.to_string());
                            }
                        }
                        continue;
                    } else {
                        canonical.to_string()
                    };
                    if !config.tools.disabled.iter().any(|d| d == &tool) {
                        config.tools.disabled.push(tool);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_model_provider_slash() {
        let mut cfg = Config::default();
        apply_opencode_value(
            &mut cfg,
            &json!({ "model": "anthropic/claude-sonnet-4-5" }),
        );
        assert_eq!(
            cfg.provider.default_provider.as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            cfg.provider.default_model.as_deref(),
            Some("claude-sonnet-4-5")
        );
    }

    #[test]
    fn maps_permission_nested_patterns() {
        let mut cfg = Config::default();
        apply_opencode_value(
            &mut cfg,
            &json!({
                "permission": {
                    "bash": { "*": "ask", "git *": "allow" },
                    "edit": "deny"
                }
            }),
        );
        assert!(cfg.permission.rules.iter().any(|r| {
            r.permission == "bash" && r.pattern == "git *" && r.action == "allow"
        }));
        assert!(cfg
            .permission
            .rules
            .iter()
            .any(|r| r.permission == "edit" && r.action == "deny"));
    }

    #[test]
    fn maps_default_agent_and_depth() {
        let mut cfg = Config::default();
        apply_opencode_value(
            &mut cfg,
            &json!({
                "default_agent": "plan",
                "subagent_depth": 2
            }),
        );
        assert_eq!(cfg.agents.default_agent, "plan");
        assert_eq!(cfg.agents.subagent_depth, 2);
    }

    #[test]
    fn strip_jsonc_line_comments() {
        let raw = r#"{
  // comment
  "model": "openai/gpt-4.1"
}"#;
        let cleaned = strip_jsonc_comments(raw);
        let v: Value = serde_json::from_str(&cleaned).unwrap();
        assert_eq!(v["model"], "openai/gpt-4.1");
    }

    #[test]
    fn maps_opencode_custom_provider_models() {
        let mut cfg = Config::default();
        apply_opencode_value(
            &mut cfg,
            &json!({
                "provider": {
                    "grok-cli": {
                        "options": {
                            "baseURL": "http://localhost:8787/v1",
                            "apiKey": "local-proxy"
                        },
                        "models": {
                            "grok-4.5": {
                                "id": "gcli/grok-4.5",
                                "limit": { "context": 500000 }
                            },
                            "fast": {
                                "id": "gcli/grok-composer-2.5-fast"
                            }
                        }
                    }
                }
            }),
        );
        let profile = cfg.providers.get("grok-cli").expect("profile");
        assert_eq!(profile.base_url, "http://localhost:8787/v1");
        assert_eq!(profile.api_key.as_deref(), Some("local-proxy"));
        assert_eq!(profile.models.len(), 2);
        assert!(profile.models.iter().any(|m| m.id == "gcli/grok-4.5"));
        assert_eq!(cfg.provider.default_provider.as_deref(), Some("grok-cli"));
        assert!(cfg.provider.default_model.is_some());
        let allow = cfg
            .provider
            .model_picker_providers
            .expect("picker allowlist");
        assert!(allow.iter().any(|e| e == "grok-cli"));
        assert!(allow.iter().any(|e| e == "claude-oauth"));
    }
}
