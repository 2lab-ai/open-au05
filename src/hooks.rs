//! AI coding agent hook integration (Claude Code, Codex CLI, Gemini CLI,
//! Cursor Agent, Copilot CLI) and multi-session LED priority aggregator.

use crate::control;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookFormat {
    Nested,
    Flat,
    Copilot,
}

#[derive(Clone, Debug)]
pub struct AgentDescriptor {
    pub id: &'static str,
    pub label: &'static str,
    pub rel_config_path: &'static str,
    pub events: &'static [&'static str],
    pub format: HookFormat,
    pub has_codex_feature: bool,
}

pub const AGENTS: &[AgentDescriptor] = &[
    AgentDescriptor {
        id: "claude-code",
        label: "Claude Code",
        rel_config_path: ".claude/settings.json",
        events: &[
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "Stop",
            "StopFailure",
            "SubagentStart",
            "SubagentStop",
            "Notification",
            "PermissionRequest",
            "PreCompact",
            "SessionEnd",
        ],
        format: HookFormat::Nested,
        has_codex_feature: false,
    },
    AgentDescriptor {
        id: "codex",
        label: "Codex CLI",
        rel_config_path: ".codex/hooks.json",
        events: &[
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "Stop",
            "PermissionRequest",
        ],
        format: HookFormat::Nested,
        has_codex_feature: true,
    },
    AgentDescriptor {
        id: "gemini-cli",
        label: "Gemini CLI",
        rel_config_path: ".gemini/settings.json",
        events: &[
            "SessionStart",
            "SessionEnd",
            "BeforeAgent",
            "BeforeTool",
            "AfterTool",
            "AfterAgent",
            "Notification",
            "PreCompress",
        ],
        format: HookFormat::Nested,
        has_codex_feature: false,
    },
    AgentDescriptor {
        id: "cursor-agent",
        label: "Cursor Agent",
        rel_config_path: ".cursor/hooks.json",
        events: &[
            "sessionStart",
            "sessionEnd",
            "beforeSubmitPrompt",
            "preToolUse",
            "postToolUse",
            "postToolUseFailure",
            "subagentStart",
            "subagentStop",
            "preCompact",
            "afterAgentThought",
            "stop",
        ],
        format: HookFormat::Flat,
        has_codex_feature: false,
    },
    AgentDescriptor {
        id: "copilot-cli",
        label: "Copilot CLI",
        rel_config_path: ".copilot/hooks/hooks.json",
        events: &[
            "userPromptSubmit",
            "preToolUse",
            "postToolUse",
            "postToolUseFailure",
            "agentStop",
            "agentStopFailure",
            "subagentStart",
            "subagentStop",
        ],
        format: HookFormat::Copilot,
        has_codex_feature: false,
    },
];

pub fn find_agent(id: &str) -> Option<&'static AgentDescriptor> {
    AGENTS.iter().find(|a| a.id.eq_ignore_ascii_case(id))
}

pub fn map_event_to_state(agent: &str, event: &str) -> Option<&'static str> {
    match (agent, event) {
        ("claude-code", "SessionStart") => Some("idle"),
        ("claude-code", "UserPromptSubmit") => Some("thinking"),
        ("claude-code", "PreToolUse" | "SubagentStart") => Some("working"),
        ("claude-code", "PostToolUse" | "SubagentStop") => Some("thinking"),
        ("claude-code", "PostToolUseFailure" | "StopFailure") => Some("error"),
        ("claude-code", "Stop" | "Notification") => Some("attention"),
        ("claude-code", "PermissionRequest") => Some("notification"),
        ("claude-code", "PreCompact") => Some("sweeping"),
        ("claude-code", "SessionEnd") => Some("sleeping"),

        ("codex", "SessionStart") => Some("idle"),
        ("codex", "UserPromptSubmit" | "PostToolUse") => Some("thinking"),
        ("codex", "PreToolUse") => Some("working"),
        ("codex", "Stop") => Some("attention"),
        ("codex", "PermissionRequest") => Some("notification"),

        ("gemini-cli", "SessionStart") => Some("idle"),
        ("gemini-cli", "BeforeAgent" | "AfterTool") => Some("thinking"),
        ("gemini-cli", "BeforeTool") => Some("working"),
        ("gemini-cli", "AfterAgent" | "Notification") => Some("attention"),
        ("gemini-cli", "PreCompress") => Some("sweeping"),
        ("gemini-cli", "SessionEnd") => Some("sleeping"),

        ("cursor-agent", "sessionStart") => Some("idle"),
        (
            "cursor-agent",
            "beforeSubmitPrompt" | "postToolUse" | "subagentStop" | "afterAgentThought",
        ) => Some("thinking"),
        ("cursor-agent", "preToolUse" | "subagentStart") => Some("working"),
        ("cursor-agent", "postToolUseFailure") => Some("error"),
        ("cursor-agent", "preCompact") => Some("sweeping"),
        ("cursor-agent", "stop") => Some("attention"),
        ("cursor-agent", "sessionEnd") => Some("sleeping"),

        ("copilot-cli", "userPromptSubmit" | "postToolUse" | "subagentStop") => Some("thinking"),
        ("copilot-cli", "preToolUse" | "subagentStart") => Some("working"),
        ("copilot-cli", "postToolUseFailure" | "agentStopFailure") => Some("error"),
        ("copilot-cli", "agentStop") => Some("attention"),
        _ => None,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HookInstallResult {
    pub agent: String,
    pub path: String,
    pub skipped: Option<String>,
    pub added: usize,
    pub removed: usize,
    pub current: usize,
}

const HOOK_MARKER: &str = " hook --agent ";

pub fn current_binary_path() -> String {
    if let Ok(override_bin) = std::env::var("OPEN_AU05_BIN") {
        return override_bin;
    }
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "open-au05".into())
}

fn hook_command(agent: &str, event: &str, binary: &str) -> String {
    let quoted = if binary
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'))
    {
        binary.to_string()
    } else {
        format!("'{}'", binary.replace('\'', "'\\''"))
    };
    format!("{quoted} hook --agent {agent} --event {event}")
}

fn is_our_hook(val: &Value) -> bool {
    let Some(obj) = val.as_object() else {
        return false;
    };
    for key in ["command", "bash"] {
        if let Some(cmd) = obj.get(key).and_then(|v| v.as_str()) {
            if cmd.contains(HOOK_MARKER) && cmd.contains("open-au05") {
                return true;
            }
        }
    }
    if let Some(arr) = obj.get("hooks").and_then(|v| v.as_array()) {
        return arr.iter().any(is_our_hook);
    }
    false
}

fn build_hook_entry(agent: &AgentDescriptor, event: &str, cmd: &str) -> Value {
    match agent.format {
        HookFormat::Nested => {
            let mut inner = Map::new();
            inner.insert("type".into(), json!("command"));
            inner.insert("command".into(), json!(cmd));
            if agent.id == "codex" {
                let timeout = if event == "PermissionRequest" {
                    600
                } else {
                    30
                };
                inner.insert("timeout".into(), json!(timeout));
            }
            json!({
                "matcher": "*",
                "hooks": [Value::Object(inner)],
            })
        }
        HookFormat::Flat => json!({
            "type": "command",
            "command": cmd,
        }),
        HookFormat::Copilot => json!({
            "type": "command",
            "bash": cmd,
            "command": cmd,
        }),
    }
}

pub fn install_hooks(
    agent: &AgentDescriptor,
    home: &Path,
    binary: &str,
    config_override: Option<&Path>,
    dry_run: bool,
) -> Result<HookInstallResult, String> {
    let path = config_override
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(agent.rel_config_path));
    let mut res = HookInstallResult {
        agent: agent.id.into(),
        path: path.to_string_lossy().into_owned(),
        skipped: None,
        added: 0,
        removed: 0,
        current: 0,
    };
    if config_override.is_none() && !path.parent().is_some_and(Path::exists) {
        res.skipped = Some("config directory not found".into());
        return Ok(res);
    }

    let mut root = read_json_object(&path)?;
    let mut hooks = root
        .get("hooks")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    for &event in agent.events {
        let mut entries = hooks
            .get(event)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let before = entries.len();
        entries.retain(|v| !is_our_hook(v));
        let removed_old = before - entries.len();
        let cmd = hook_command(agent.id, event, binary);
        entries.push(build_hook_entry(agent, event, &cmd));
        if removed_old == 1 {
            res.current += 1;
        } else {
            res.added += 1;
        }
        hooks.insert(event.to_string(), Value::Array(entries));
    }
    root.insert("hooks".into(), Value::Object(hooks));

    if !dry_run {
        write_json_object(&path, &root)?;
        if agent.has_codex_feature {
            let feature_path = path
                .parent()
                .map(|p| p.join("config.toml"))
                .unwrap_or_else(|| home.join(".codex/config.toml"));
            enable_codex_hooks(&feature_path)?;
        }
    }
    Ok(res)
}

pub fn uninstall_hooks(
    agent: &AgentDescriptor,
    home: &Path,
    config_override: Option<&Path>,
    dry_run: bool,
) -> Result<HookInstallResult, String> {
    let path = config_override
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(agent.rel_config_path));
    let mut res = HookInstallResult {
        agent: agent.id.into(),
        path: path.to_string_lossy().into_owned(),
        skipped: None,
        added: 0,
        removed: 0,
        current: 0,
    };
    if !path.exists() {
        res.skipped = Some("no config file".into());
        return Ok(res);
    }
    let mut root = read_json_object(&path)?;
    let Some(mut hooks) = root.get("hooks").and_then(|v| v.as_object()).cloned() else {
        return Ok(res);
    };
    let keys: Vec<String> = hooks.keys().cloned().collect();
    for event in keys {
        if let Some(arr) = hooks.get(&event).and_then(|v| v.as_array()).cloned() {
            let before = arr.len();
            let filtered: Vec<Value> = arr.into_iter().filter(|v| !is_our_hook(v)).collect();
            res.removed += before - filtered.len();
            if filtered.is_empty() {
                hooks.remove(&event);
            } else {
                hooks.insert(event, Value::Array(filtered));
            }
        }
    }
    root.insert("hooks".into(), Value::Object(hooks));
    if !dry_run && res.removed > 0 {
        write_json_object(&path, &root)?;
    }
    Ok(res)
}

pub fn hook_status(agent: &AgentDescriptor, home: &Path) -> (usize, usize, String) {
    let path = home.join(agent.rel_config_path);
    let path_s = path.to_string_lossy().into_owned();
    let Ok(root) = read_json_object(&path) else {
        return (0, agent.events.len(), path_s);
    };
    let Some(hooks) = root.get("hooks").and_then(|v| v.as_object()) else {
        return (0, agent.events.len(), path_s);
    };
    let installed = agent
        .events
        .iter()
        .filter(|&&e| {
            hooks
                .get(e)
                .and_then(|v| v.as_array())
                .is_some_and(|arr| arr.iter().any(is_our_hook))
        })
        .count();
    (installed, agent.events.len(), path_s)
}

fn read_json_object(path: &Path) -> Result<Map<String, Value>, String> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let text =
        fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(&text)
        .map_err(|e| format!("invalid JSON in {}: {e}", path.display()))?
    {
        Value::Object(map) => Ok(map),
        _ => Err(format!("{} is not a JSON object", path.display())),
    }
}

fn write_json_object(path: &Path, map: &Map<String, Value>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    if path.exists() {
        let backup = path.with_extension("json.open-au05-backup");
        let _ = fs::copy(path, &backup);
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(map.clone()))
        .map_err(|e| format!("JSON serialize failed: {e}"))?;
    text.push('\n');
    fs::write(path, text).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn enable_codex_hooks(path: &Path) -> Result<(), String> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = if existing.is_empty() {
        Vec::new()
    } else {
        existing.lines().map(String::from).collect()
    };
    if let Some(start) = lines.iter().position(|l| l.trim() == "[features]") {
        let end = ((start + 1)..lines.len())
            .find(|&i| lines[i].trim().starts_with('['))
            .unwrap_or(lines.len());
        if let Some(idx) = ((start + 1)..end).find(|&i| {
            let t = lines[i].trim();
            t.starts_with("hooks") && t[5..].trim_start().starts_with('=')
        }) {
            if lines[idx].contains("true") {
                return Ok(());
            }
            lines[idx] = "hooks = true".into();
        } else {
            lines.insert(start + 1, "hooks = true".into());
        }
    } else {
        if lines.last().is_some_and(|l| !l.is_empty()) {
            lines.push(String::new());
        }
        lines.push("[features]".into());
        lines.push("hooks = true".into());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut out = lines.join("\n");
    out.push('\n');
    fs::write(path, out).map_err(|e| e.to_string())
}

/// Fast entry point invoked by AI coding agents: `open-au05 hook --agent <id> --event <name>`.
/// Never leaks prompts or transcripts; finishes cleanly even if the daemon is stopped.
pub fn run_hook_client(agent: &str, event_arg: Option<&str>) -> i32 {
    let mut stdin_buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin_buf);
    let payload: Value = serde_json::from_str(&stdin_buf).unwrap_or(Value::Null);
    let event = payload
        .get("hook_event_name")
        .or_else(|| payload.get("event"))
        .or_else(|| payload.get("eventName"))
        .or_else(|| payload.get("hookEventName"))
        .and_then(|v| v.as_str())
        .or(event_arg)
        .unwrap_or("");

    let mut state = map_event_to_state(agent, event);
    if agent == "gemini-cli"
        && event == "AfterTool"
        && payload
            .get("tool_response")
            .and_then(|r| r.get("error"))
            .is_some()
    {
        state = Some("error");
    }

    if let Some(st) = state {
        let session = [
            "session_id",
            "sessionId",
            "conversation_id",
            "thread_id",
            "turn_id",
        ]
        .iter()
        .find_map(|k| payload.get(*k).and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty())
        .unwrap_or("default");
        let _ = control::send_command(&json!({
            "cmd": "agent-state",
            "agent": agent,
            "state": st,
            "session": session,
            "event": event,
        }));
    }
    println!("{{}}");
    0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrackedSession {
    pub agent: String,
    pub session_id: String,
    pub state: String,
    pub age_secs: u64,
}

/// Tracks multiple active coding agent sessions and resolves the aggregate LED state.
pub struct AgentTracker {
    sessions: BTreeMap<String, (String, String, Instant)>,
    ttl: Duration,
}

impl Default for AgentTracker {
    fn default() -> Self {
        Self {
            sessions: BTreeMap::new(),
            ttl: Duration::from_secs(30 * 60),
        }
    }
}

impl AgentTracker {
    pub fn update(&mut self, agent: &str, session_id: &str, state: &str) {
        let key = format!("{agent}:{session_id}");
        if state == "sleeping" {
            self.sessions.remove(&key);
        } else {
            self.sessions
                .insert(key, (agent.to_string(), state.to_string(), Instant::now()));
        }
        self.prune();
    }

    pub fn prune(&mut self) {
        let ttl = self.ttl;
        self.sessions.retain(|_, (_, _, ts)| ts.elapsed() < ttl);
    }

    /// Returns `(most_urgent_state, any_session_busy)`.
    pub fn aggregate(&mut self) -> (&'static str, bool) {
        self.prune();
        if self.sessions.is_empty() {
            return ("idle", false);
        }
        let rank = |s: &str| -> u8 {
            match s {
                "notification" => 7,
                "error" => 6,
                "attention" => 5,
                "working" => 4,
                "thinking" => 3,
                "sweeping" => 2,
                "idle" => 1,
                _ => 0,
            }
        };
        let mut best: &'static str = "idle";
        let mut best_rank = 0u8;
        let mut any_busy = false;
        for (_, state, _) in self.sessions.values() {
            if matches!(state.as_str(), "working" | "thinking" | "sweeping") {
                any_busy = true;
            }
            let r = rank(state);
            if r > best_rank {
                best_rank = r;
                best = match state.as_str() {
                    "notification" => "notification",
                    "error" => "error",
                    "attention" => "attention",
                    "working" => "working",
                    "thinking" => "thinking",
                    "sweeping" => "sweeping",
                    _ => "idle",
                };
            }
        }
        (best, any_busy)
    }

    pub fn snapshot(&mut self) -> Vec<TrackedSession> {
        self.prune();
        self.sessions
            .iter()
            .map(|(k, (agent, state, ts))| {
                let session_id = k
                    .split_once(':')
                    .map(|(_, s)| s.to_string())
                    .unwrap_or_else(|| k.clone());
                TrackedSession {
                    agent: agent.clone(),
                    session_id,
                    state: state.clone(),
                    age_secs: ts.elapsed().as_secs(),
                }
            })
            .collect()
    }
}
