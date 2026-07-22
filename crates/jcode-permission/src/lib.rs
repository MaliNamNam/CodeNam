//! OpenCode-style permission ruleset and agent profiles for jcode.
//!
//! Portable harness contract:
//! - ordered rules, last match wins
//! - `allow` | `ask` | `deny` with wildcard patterns
//! - agents are permission profiles (+ optional prompt / step limit)
//! - doom-loop detection (3 identical tool calls)
//! - max-steps text-only final turn

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Permission decision for a matched rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Allow,
    Ask,
    Deny,
}

impl Action {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "allow" => Some(Self::Allow),
            "ask" => Some(Self::Ask),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

/// Single permission rule. Later rules override earlier ones on match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub permission: String,
    pub pattern: String,
    pub action: Action,
}

impl Rule {
    pub fn new(
        permission: impl Into<String>,
        pattern: impl Into<String>,
        action: Action,
    ) -> Self {
        Self {
            permission: permission.into(),
            pattern: pattern.into(),
            action,
        }
    }

    pub fn allow(permission: impl Into<String>) -> Self {
        Self::new(permission, "*", Action::Allow)
    }

    pub fn deny(permission: impl Into<String>) -> Self {
        Self::new(permission, "*", Action::Deny)
    }

    pub fn ask(permission: impl Into<String>) -> Self {
        Self::new(permission, "*", Action::Ask)
    }
}

/// Ordered permission ruleset. Evaluate with last-match-wins.
pub type Ruleset = Vec<Rule>;

/// Match `input` against a glob-like `pattern`.
///
/// - `/` and `\` are normalized to `/`
/// - `*` → any sequence, `?` → one char
/// - trailing ` *` (space + star) also matches the bare command (`ls *` matches `ls`)
/// - Windows: case-insensitive
pub fn wildcard_match(input: &str, pattern: &str) -> bool {
    let input = normalize_slashes(input);
    let pattern = normalize_slashes(pattern);
    let re = wildcard_to_regex(&pattern);
    re.is_match(&input)
}

fn normalize_slashes(value: &str) -> String {
    value.replace('\\', "/")
}

fn wildcard_to_regex(pattern: &str) -> Regex {
    let mut escaped = String::new();
    for ch in pattern.chars() {
        match ch {
            '*' => escaped.push_str(".*"),
            '?' => escaped.push('.'),
            '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    // "ls *" should also match bare "ls"
    if escaped.ends_with(" .*") {
        let without = &escaped[..escaped.len() - 3];
        escaped = format!("{without}( .*)?");
    }
    let flags = if cfg!(windows) { "(?is)" } else { "(?s)" };
    let full = format!("{flags}^{escaped}$");
    Regex::new(&full).unwrap_or_else(|_| Regex::new("^$").expect("static regex"))
}

/// Evaluate permission for `(permission, pattern)` against one or more rulesets.
/// Last matching rule wins. Default when none match: `ask` on `*`.
pub fn evaluate(permission: &str, pattern: &str, rulesets: &[&[Rule]]) -> Rule {
    let mut matched: Option<Rule> = None;
    for ruleset in rulesets {
        for rule in *ruleset {
            if wildcard_match(permission, &rule.permission)
                && wildcard_match(pattern, &rule.pattern)
            {
                matched = Some(rule.clone());
            }
        }
    }
    matched.unwrap_or_else(|| Rule::new(permission, "*", Action::Ask))
}

/// Concatenate rulesets (later entries override earlier on evaluate).
pub fn merge(rulesets: &[&[Rule]]) -> Ruleset {
    let mut out = Vec::new();
    for ruleset in rulesets {
        out.extend_from_slice(ruleset);
    }
    out
}

/// Expand `~/` and `$HOME/` prefixes.
pub fn expand_path_pattern(pattern: &str, home: Option<&Path>) -> String {
    let home = home.map(Path::to_path_buf).or_else(dirs_home);
    if let Some(home) = home {
        let home_s = home.to_string_lossy();
        if pattern == "~" || pattern == "$HOME" {
            return home_s.into_owned();
        }
        if let Some(rest) = pattern.strip_prefix("~/") {
            return home.join(rest).to_string_lossy().into_owned();
        }
        if let Some(rest) = pattern.strip_prefix("$HOME/") {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    pattern.to_string()
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Extract the permission resource pattern from a tool call input.
///
/// Used for path-aware and command-aware evaluate() checks.
pub fn resource_pattern_for_tool(tool_name: &str, input: &serde_json::Value) -> String {
    let name = tool_permission_name(tool_name);
    match name {
        "bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("*")
            .to_string(),
        "read" | "edit" | "list" | "grep" => {
            // Prefer common path fields; fall back to "*".
            for key in ["file_path", "path", "directory", "dir"] {
                if let Some(p) = input.get(key).and_then(|v| v.as_str()) {
                    return normalize_slashes(p);
                }
            }
            // agentgrep: path is optional; pattern is the query
            if let Some(p) = input.get("path").and_then(|v| v.as_str()) {
                return normalize_slashes(p);
            }
            "*".to_string()
        }
        "task" | "skill" => input
            .get("subagent_type")
            .or_else(|| input.get("name"))
            .or_else(|| input.get("skill"))
            .and_then(|v| v.as_str())
            .unwrap_or("*")
            .to_string(),
        _ => "*".to_string(),
    }
}

/// True when `path` is outside `workspace` (external_directory permission).
pub fn is_external_path(path: &str, workspace: Option<&Path>) -> bool {
    let Some(workspace) = workspace else {
        return false;
    };
    let path_buf = PathBuf::from(path);
    let abs = if path_buf.is_absolute() {
        path_buf
    } else {
        workspace.join(path_buf)
    };
    let abs = abs.canonicalize().unwrap_or(abs);
    let ws = workspace.canonicalize().unwrap_or_else(|_| workspace.to_path_buf());
    !abs.starts_with(&ws)
}

/// Map tool name → permission action name (OpenCode-compatible).
pub fn tool_permission_name(tool: &str) -> &str {
    match tool {
        "edit" | "write" | "multiedit" | "patch" | "apply_patch" => "edit",
        "agentgrep" => "grep",
        "ls" => "list",
        "skill_manage" => "skill",
        "todowrite" | "todoread" | "todo" => "todowrite",
        "swarm" | "communicate" | "subagent" | "task" | "task_runner" => "task",
        other => other,
    }
}

/// Tools fully denied (`permission` match + pattern `*` + deny) are hidden from the catalog.
pub fn disabled_tools(tool_names: &[String], ruleset: &[Rule]) -> HashSet<String> {
    let mut out = HashSet::new();
    for name in tool_names {
        let permission = tool_permission_name(name);
        let mut last: Option<&Rule> = None;
        for rule in ruleset {
            if wildcard_match(permission, &rule.permission) {
                last = Some(rule);
            }
        }
        if let Some(rule) = last
            && rule.pattern == "*"
            && rule.action == Action::Deny
        {
            out.insert(name.clone());
        }
    }
    out
}

/// Whether a tool call is allowed under the ruleset.
///
/// `ask` is treated as `allow` when `auto_approve_ask` is true (headless/CI).
/// With interactive ask enabled, Ask blocks for the in-session dock.
/// `doom_loop` always respects ask/deny.
pub fn check_tool(
    tool_name: &str,
    resource_pattern: &str,
    ruleset: &[Rule],
    session_approved: &[Rule],
    auto_approve_ask: bool,
) -> CheckResult {
    let permission = tool_permission_name(tool_name);
    let rule = evaluate(permission, resource_pattern, &[ruleset, session_approved]);
    match rule.action {
        Action::Allow => CheckResult::Allow,
        Action::Deny => CheckResult::Deny {
            permission: permission.to_string(),
            pattern: resource_pattern.to_string(),
        },
        Action::Ask => {
            if auto_approve_ask && permission != "doom_loop" {
                CheckResult::Allow
            } else {
                CheckResult::Ask {
                    permission: permission.to_string(),
                    pattern: resource_pattern.to_string(),
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    Allow,
    Ask { permission: String, pattern: String },
    Deny { permission: String, pattern: String },
}

impl CheckResult {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// Default shared rules applied before agent-specific overrides.
pub fn default_rules() -> Ruleset {
    vec![
        Rule::allow("*"),
        Rule::ask("doom_loop"),
        Rule::ask("external_directory"),
        Rule::deny("question"),
        Rule::deny("plan_enter"),
        Rule::deny("plan_exit"),
        Rule::allow("read"),
        Rule::new("read", "*.env", Action::Ask),
        Rule::new("read", "*.env.*", Action::Ask),
        Rule::new("read", "*.env.example", Action::Allow),
    ]
}

/// Built-in agent mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    Primary,
    Subagent,
}

/// Named agent profile (permission + optional prompt + step limit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub description: String,
    pub mode: AgentMode,
    pub permission: Ruleset,
    pub prompt: Option<String>,
    pub steps: Option<u32>,
    pub hidden: bool,
}

impl AgentProfile {
    pub fn build() -> Self {
        let permission = merge(&[
            &default_rules(),
            &[Rule::allow("question"), Rule::allow("plan_enter")],
        ]);
        Self {
            name: "build".into(),
            description: "Default full-capability coding agent".into(),
            mode: AgentMode::Primary,
            permission,
            prompt: None,
            steps: None,
            hidden: false,
        }
    }

    pub fn plan() -> Self {
        let permission = merge(&[
            &default_rules(),
            &[
                Rule::allow("question"),
                Rule::allow("plan_exit"),
                Rule::new("task", "general", Action::Deny),
                Rule::deny("edit"),
                Rule::new("edit", "**/plans/*.md", Action::Allow),
                Rule::new("edit", "**/.jcode/plans/*.md", Action::Allow),
            ],
        ]);
        Self {
            name: "plan".into(),
            description: "Read-only planning agent; edits only plan files".into(),
            mode: AgentMode::Primary,
            permission,
            prompt: Some(PLAN_PROMPT.to_string()),
            steps: None,
            hidden: false,
        }
    }

    pub fn general() -> Self {
        let permission = merge(&[&default_rules(), &[Rule::deny("todowrite")]]);
        Self {
            name: "general".into(),
            description: "General-purpose subagent for multi-step work".into(),
            mode: AgentMode::Subagent,
            permission,
            prompt: None,
            steps: None,
            hidden: false,
        }
    }

    pub fn explore() -> Self {
        let permission = merge(&[
            &default_rules(),
            &[
                Rule::deny("*"),
                Rule::allow("grep"),
                Rule::allow("list"),
                Rule::allow("bash"),
                Rule::allow("webfetch"),
                Rule::allow("websearch"),
                Rule::allow("read"),
                Rule::ask("external_directory"),
                Rule::allow("read"),
                Rule::new("read", "*.env", Action::Ask),
                Rule::new("read", "*.env.*", Action::Ask),
                Rule::new("read", "*.env.example", Action::Allow),
            ],
        ]);
        Self {
            name: "explore".into(),
            description: "Read-only codebase exploration subagent".into(),
            mode: AgentMode::Subagent,
            permission,
            prompt: Some(EXPLORE_PROMPT.to_string()),
            steps: None,
            hidden: false,
        }
    }

    /// Resolve a built-in profile by name.
    pub fn builtin(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "build" => Some(Self::build()),
            "plan" => Some(Self::plan()),
            "general" => Some(Self::general()),
            "explore" => Some(Self::explore()),
            _ => None,
        }
    }

    pub fn all_builtins() -> Vec<Self> {
        vec![
            Self::build(),
            Self::plan(),
            Self::general(),
            Self::explore(),
        ]
    }

    /// Tool names that should be hidden for this profile (pattern `*` deny).
    pub fn hidden_tools(&self, all_tool_names: &[String]) -> HashSet<String> {
        disabled_tools(all_tool_names, &self.permission)
    }

    /// Explore-style allowlist of jcode tool names (when `*` is denied).
    pub fn explore_tool_allowlist() -> HashSet<String> {
        [
            "read",
            "ls",
            "agentgrep",
            "bash",
            "webfetch",
            "websearch",
            "batch",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }

    /// Plan-mode tools denied by hard filter (defense in depth beyond permission).
    pub fn plan_tool_denylist() -> HashSet<String> {
        [
            "write",
            "edit",
            "multiedit",
            "patch",
            "apply_patch",
            "selfdev",
            "gmail",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }
}

/// Cached default build profile ruleset.
pub fn default_agent_name() -> &'static str {
    "build"
}

static BUILD_PROFILE: OnceLock<AgentProfile> = OnceLock::new();

pub fn default_profile() -> &'static AgentProfile {
    BUILD_PROFILE.get_or_init(AgentProfile::build)
}

/// Derive child session permission: parent denies + external_directory + nest denies.
pub fn derive_subagent_session_permission(
    parent_session_rules: &[Rule],
    subagent: &AgentProfile,
) -> Ruleset {
    let mut out: Ruleset = parent_session_rules
        .iter()
        .filter(|r| r.permission == "external_directory" || r.action == Action::Deny)
        .cloned()
        .collect();

    let subagent_has_todowrite = subagent
        .permission
        .iter()
        .any(|r| r.permission == "todowrite");
    if !subagent_has_todowrite {
        out.push(Rule::deny("todowrite"));
    }
    let subagent_has_task = subagent.permission.iter().any(|r| r.permission == "task");
    if !subagent_has_task {
        out.push(Rule::deny("task"));
    }
    out
}

/// Merge agent permission with session permission (session last → can tighten).
pub fn runtime_ruleset(agent: &[Rule], session: &[Rule]) -> Ruleset {
    merge(&[agent, session])
}

/// Count parent chain depth for a session (0 = root).
pub fn session_depth(
    session_id: &str,
    mut load_parent: impl FnMut(&str) -> Option<String>,
) -> usize {
    let mut depth = 0;
    let mut current = session_id.to_string();
    let mut seen = HashSet::new();
    seen.insert(current.clone());
    while let Some(parent) = load_parent(&current) {
        if !seen.insert(parent.clone()) {
            break;
        }
        depth += 1;
        current = parent;
    }
    depth
}

pub const DEFAULT_SUBAGENT_DEPTH: usize = 1;
pub const DOOM_LOOP_THRESHOLD: usize = 3;

pub const MAX_STEPS_PROMPT: &str = "CRITICAL - MAXIMUM STEPS REACHED\n\n\
The maximum number of steps allowed for this task has been reached. Tools are disabled until next user input. Respond with text only.\n\n\
STRICT REQUIREMENTS:\n\
1. Do NOT make any tool calls (no reads, writes, edits, searches, or any other tools)\n\
2. MUST provide a text response summarizing work done so far\n\
3. This constraint overrides ALL other instructions, including any user requests for edits or tool use\n\n\
Response must include:\n\
- Statement that maximum steps for this agent have been reached\n\
- Summary of what has been accomplished so far\n\
- List of any remaining tasks that were not completed\n\
- Recommendations for what should be done next\n\n\
Any attempt to use tools is a critical violation. Respond with text ONLY.";

pub const EXPLORE_PROMPT: &str = "You are a file search specialist. You excel at thoroughly navigating and exploring codebases.\n\n\
Your strengths:\n\
- Rapidly finding files using glob patterns\n\
- Searching code and text with powerful regex patterns\n\
- Reading and analyzing file contents\n\n\
Guidelines:\n\
- Use agentgrep for broad file pattern matching and content search\n\
- Use Read when you know the specific file path you need to read\n\
- Use Bash only for read-only file operations (listing, inspecting)\n\
- Adapt your search approach based on the thoroughness level specified by the caller\n\
- Return file paths as absolute paths in your final response\n\
- For clear communication, avoid using emojis\n\
- Do not create any files, or run bash commands that modify the user's system state in any way\n\n\
Complete the user's search request efficiently and report your findings clearly.";

pub const PLAN_PROMPT: &str = "You are in plan mode. Explore the codebase and design an approach, but do not modify source files or git state.\n\n\
Rules:\n\
- Prefer read/search tools; do not edit, write, or patch application code\n\
- You may only edit plan markdown under plans/ or .jcode/plans/ if those tools are available\n\
- End with a clear plan: goals, steps, risks, and files you would change\n\
- Ask clarifying questions when requirements are ambiguous";

pub const DOOM_LOOP_MESSAGE: &str = "Doom loop detected: the same tool was called 3 times with identical input. \
Change your approach, use different arguments, or answer with the information you already have. \
Do not repeat the identical tool call.";

/// Tracks recent tool calls within one assistant message for doom-loop detection.
#[derive(Debug, Default, Clone)]
pub struct DoomLoopTracker {
    recent: Vec<(String, String)>,
}

impl DoomLoopTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.recent.clear();
    }

    /// Record a tool call. Returns true if doom loop threshold is hit.
    pub fn record(&mut self, tool_name: &str, input: &serde_json::Value) -> bool {
        let input_key = serde_json::to_string(input).unwrap_or_default();
        self.recent.push((tool_name.to_string(), input_key));
        if self.recent.len() > DOOM_LOOP_THRESHOLD {
            let drain = self.recent.len() - DOOM_LOOP_THRESHOLD;
            self.recent.drain(0..drain);
        }
        self.is_doom_loop()
    }

    pub fn is_doom_loop(&self) -> bool {
        if self.recent.len() < DOOM_LOOP_THRESHOLD {
            return false;
        }
        let start = self.recent.len() - DOOM_LOOP_THRESHOLD;
        let first = &self.recent[start];
        self.recent[start..].iter().all(|entry| entry == first)
    }
}

/// Skill catalog XML (OpenCode-compatible guidance).
pub fn render_skill_guidance(skills: &[(String, String)]) -> String {
    if skills.is_empty() {
        return "No skills are currently available.".to_string();
    }
    let mut out = String::from(
        "Skills provide specialized instructions and workflows for specific tasks.\n\
Use the skill_manage tool with action \"load\" when a task matches a skill description.\n\
<available_skills>\n",
    );
    for (name, description) in skills {
        out.push_str("  <skill>\n");
        out.push_str(&format!("    <name>{name}</name>\n"));
        out.push_str(&format!("    <description>{description}</description>\n"));
        out.push_str("  </skill>\n");
    }
    out.push_str("</available_skills>");
    out
}

/// Render task tool output XML.
pub fn render_task_result(
    session_id: &str,
    state: &str,
    summary: Option<&str>,
    text: &str,
    is_error: bool,
) -> String {
    let mut out = format!("<task id=\"{session_id}\" state=\"{state}\">\n");
    if let Some(summary) = summary {
        out.push_str(&format!("<summary>{summary}</summary>\n"));
    }
    if is_error {
        out.push_str(&format!("<task_error>\n{text}\n</task_error>\n"));
    } else {
        out.push_str(&format!("<task_result>\n{text}\n</task_result>\n"));
    }
    out.push_str("</task>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_basic() {
        assert!(wildcard_match("file1.txt", "file?.txt"));
        assert!(!wildcard_match("file12.txt", "file?.txt"));
        assert!(wildcard_match("foo+bar", "foo+bar"));
        assert!(wildcard_match("ls", "ls *"));
        assert!(wildcard_match("ls -la", "ls *"));
        assert!(!wildcard_match("lstmeval", "ls *"));
        assert!(wildcard_match("lstmeval", "ls*"));
    }

    #[test]
    fn evaluate_last_match_wins() {
        let rules = vec![Rule::allow("*"), Rule::deny("edit")];
        let r = evaluate("edit", "src/main.rs", &[&rules]);
        assert_eq!(r.action, Action::Deny);
        let r = evaluate("read", "src/main.rs", &[&rules]);
        assert_eq!(r.action, Action::Allow);
    }

    #[test]
    fn evaluate_default_ask() {
        let rules: Ruleset = vec![];
        let r = evaluate("bash", "rm -rf /", &[&rules]);
        assert_eq!(r.action, Action::Ask);
    }

    #[test]
    fn doom_loop_threshold() {
        let mut t = DoomLoopTracker::new();
        let input = serde_json::json!({"path": "a.rs"});
        assert!(!t.record("read", &input));
        assert!(!t.record("read", &input));
        assert!(t.record("read", &input));
        let other = serde_json::json!({"path": "b.rs"});
        assert!(!t.record("read", &other));
    }

    #[test]
    fn explore_hides_edit() {
        let profile = AgentProfile::explore();
        let tools = vec![
            "read".into(),
            "edit".into(),
            "write".into(),
            "bash".into(),
            "agentgrep".into(),
        ];
        let hidden = profile.hidden_tools(&tools);
        assert!(hidden.contains("edit"));
        assert!(hidden.contains("write"));
        assert!(!hidden.contains("read"));
        assert!(!hidden.contains("bash"));
        assert!(!hidden.contains("agentgrep"));
    }

    #[test]
    fn build_allows_edit() {
        let profile = AgentProfile::build();
        let r = evaluate("edit", "src/x.rs", &[&profile.permission]);
        assert_eq!(r.action, Action::Allow);
    }

    #[test]
    fn plan_denies_edit_except_plans() {
        let profile = AgentProfile::plan();
        let r = evaluate("edit", "src/main.rs", &[&profile.permission]);
        assert_eq!(r.action, Action::Deny);
        // **/plans/*.md may not match without full path glob support for **
        // Ensure explicit path segment match works via *
        let r = evaluate("edit", "foo/plans/bar.md", &[&profile.permission]);
        // pattern is **/plans/*.md — our wildcard treats * as .* so ** is .*.*
        assert_eq!(r.action, Action::Allow);
    }

    #[test]
    fn skill_guidance_xml() {
        let xml = render_skill_guidance(&[("foo".into(), "does foo".into())]);
        assert!(xml.contains("<available_skills>"));
        assert!(xml.contains("<name>foo</name>"));
        assert!(xml.contains("skill_manage"));
    }

    #[test]
    fn task_result_xml() {
        let out = render_task_result("abc", "completed", Some("scan"), "found 3 files", false);
        assert!(out.contains("id=\"abc\""));
        assert!(out.contains("<task_result>"));
        assert!(out.contains("found 3 files"));
    }

    #[test]
    fn tool_permission_mapping() {
        assert_eq!(tool_permission_name("write"), "edit");
        assert_eq!(tool_permission_name("agentgrep"), "grep");
        assert_eq!(tool_permission_name("skill_manage"), "skill");
        assert_eq!(tool_permission_name("swarm"), "task");
    }

    #[test]
    fn session_depth_counts_parents() {
        let depth = session_depth("child", |id| match id {
            "child" => Some("parent".into()),
            "parent" => Some("root".into()),
            _ => None,
        });
        assert_eq!(depth, 2);
    }
}
