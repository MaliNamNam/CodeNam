//! Lightweight OpenCode-style `task` tool: spawn a child session, run one
//! prompt to completion, return the final assistant text.
//!
//! Prefer this for single-shot subagent work. Use `swarm` for multi-agent
//! coordination, plans, and long-running worker fleets.

use super::{Registry, Tool, ToolContext, ToolOutput};
use crate::agent::Agent;
use crate::session::Session;
use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;

pub struct TaskTool {
    registry: Registry,
}

impl TaskTool {
    pub fn new(registry: Registry) -> Self {
        Self { registry }
    }
}

#[derive(Debug, Deserialize)]
struct TaskInput {
    description: String,
    prompt: String,
    #[serde(default)]
    subagent_type: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "Spawn a child agent session for focused multi-step work. \
Use for parallel exploration or isolated research. \
Launch multiple tasks in one message when work is independent. \
Do not use for simple single-step tool calls. \
For multi-agent plans and coordination, use swarm instead."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["description", "prompt"],
            "properties": {
                "intent": super::intent_schema_property(),
                "description": {
                    "type": "string",
                    "description": "Short 3-5 word label for the task"
                },
                "prompt": {
                    "type": "string",
                    "description": "Full instructions for the subagent"
                },
                "subagent_type": {
                    "type": "string",
                    "description": "Agent profile: explore (read-only search) or general (multi-step). Default: general",
                    "enum": ["explore", "general"]
                },
                "task_id": {
                    "type": "string",
                    "description": "Optional existing child session id to resume"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: TaskInput = serde_json::from_value(input)
            .map_err(|e| anyhow::anyhow!("Invalid task input: {e}"))?;
        if params.prompt.trim().is_empty() {
            bail!("task prompt must not be empty");
        }

        let subagent_type = params
            .subagent_type
            .as_deref()
            .unwrap_or("general")
            .trim()
            .to_ascii_lowercase();
        if subagent_type != "explore" && subagent_type != "general" {
            bail!("subagent_type must be 'explore' or 'general'");
        }

        let max_depth = std::env::var("JCODE_SUBAGENT_DEPTH")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(|| crate::config::config().agents.subagent_depth);
        let depth = jcode_permission::session_depth(&ctx.session_id, |id| {
            Session::load(id)
                .ok()
                .and_then(|s| s.parent_id)
        });
        if depth >= max_depth {
            bail!(
                "Subagent depth limit reached (depth={depth}, max={max_depth}). \
Nested task spawning is disabled for this session."
            );
        }

        let parent = Session::load(&ctx.session_id)
            .map_err(|e| anyhow::anyhow!("Failed to load parent session: {e}"))?;

        let provider = crate::provider::active_provider_fork()
            .ok_or_else(|| anyhow::anyhow!("No active provider available for task spawn"))?;

        let registry = self.registry.clone();
        let description = params.description.trim();
        let title = format!("{description} (@{subagent_type} subagent)");

        let mut session = if let Some(task_id) = params.task_id.as_ref() {
            let existing = Session::load(task_id)
                .map_err(|e| anyhow::anyhow!("Failed to resume task session '{task_id}': {e}"))?;
            if existing.parent_id.as_deref() != Some(ctx.session_id.as_str()) {
                bail!(
                    "task_id '{task_id}' is not a child of the current session"
                );
            }
            existing
        } else {
            let mut session = Session::create(Some(ctx.session_id.clone()), Some(title));
            session.model = parent.model.clone();
            session.provider_key = parent.provider_key.clone();
            session.route_api_method = parent.route_api_method.clone();
            session.working_dir = ctx
                .working_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .or(parent.working_dir.clone());
            session
                .save()
                .map_err(|e| anyhow::anyhow!("Failed to save child session: {e}"))?;
            session
        };

        if session.working_dir.is_none() {
            session.working_dir = ctx
                .working_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .or(parent.working_dir.clone());
        }

        let mut allowed: HashSet<String> = registry.tool_names().await.into_iter().collect();
        for blocked in [
            "task",
            "subagent",
            "swarm",
            "todo",
            "todowrite",
            "todoread",
            "batch",
        ] {
            allowed.remove(blocked);
        }
        if subagent_type == "explore" {
            allowed = jcode_permission::AgentProfile::explore_tool_allowlist();
        }
        crate::config::config()
            .tools
            .apply_to_allowed_set(&mut allowed);

        let mut worker =
            Agent::new_with_session(provider, registry, session, Some(allowed));
        worker.apply_subagent_profile(&subagent_type);

        let child_id = worker.session_id().to_string();
        match worker.run_once_capture(&params.prompt).await {
            Ok(text) => {
                let output = jcode_permission::render_task_result(
                    &child_id,
                    "completed",
                    Some(description),
                    &text,
                    false,
                );
                Ok(ToolOutput::new(output)
                    .with_title(description.to_string())
                    .with_metadata(json!({
                        "parentSessionId": ctx.session_id,
                        "sessionId": child_id,
                        "subagent_type": subagent_type,
                    })))
            }
            Err(e) => {
                let output = jcode_permission::render_task_result(
                    &child_id,
                    "error",
                    Some(description),
                    &e.to_string(),
                    true,
                );
                Ok(ToolOutput::new(output)
                    .with_title(format!("{description} (error)"))
                    .with_metadata(json!({
                        "parentSessionId": ctx.session_id,
                        "sessionId": child_id,
                        "subagent_type": subagent_type,
                        "error": true,
                    })))
            }
        }
    }
}
