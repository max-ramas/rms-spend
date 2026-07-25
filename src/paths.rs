use std::path::PathBuf;

/// Well-known paths for the local spend agent.
#[derive(Debug, Clone)]
pub struct SpendPaths {
    pub base_dir: PathBuf,
    pub ledger_db: PathBuf,
    pub tariffs_json: PathBuf,
    pub connectors_json: PathBuf,
    pub budget_json: PathBuf,
    pub raw_audit_dir: PathBuf,
}

impl SpendPaths {
    /// Default: `~/.rms-memory/` (shared with RMS Memory ecosystem).
    pub fn default_home() -> Self {
        let base = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".rms-memory");
        Self::from_base(base)
    }

    pub fn from_base(base_dir: PathBuf) -> Self {
        let ledger_db = base_dir.join("spend.db");
        let tariffs_json = base_dir.join("spend-tariffs.json");
        let connectors_json = base_dir.join("spend-connectors.json");
        let budget_json = base_dir.join("spend-budget.json");
        let raw_audit_dir = base_dir.join("spend-raw");
        Self {
            base_dir,
            ledger_db,
            tariffs_json,
            connectors_json,
            budget_json,
            raw_audit_dir,
        }
    }

    pub fn ensure_base(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.base_dir)?;
        std::fs::create_dir_all(&self.raw_audit_dir)?;
        Ok(())
    }
}

/// Default Claude Code projects root.
pub fn claude_projects_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(custom).join("projects");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join("projects")
}

/// Default RMS Memory GUI ai-runs directory (macOS Application Support).
pub fn rms_memory_ai_runs_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Library")
                .join("Application Support")
        })
        .join("rms-memory-gui")
        .join("ai-runs")
}

/// Cursor AI code tracking SQLite (`~/.cursor/ai-tracking/ai-code-tracking.db`).
pub fn cursor_tracking_db() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cursor")
        .join("ai-tracking")
        .join("ai-code-tracking.db")
}

/// Cursor VS Code state DB with daily line stats.
pub fn cursor_state_vscdb() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Library")
                .join("Application Support")
        })
        .join("Cursor")
        .join("User")
        .join("globalStorage")
        .join("state.vscdb")
}

/// RMS Threads Assistant SQLite (`~/Library/Application Support/com.rms.threadsassistant/app.db`).
pub fn rms_threads_app_db() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Library")
                .join("Application Support")
        })
        .join("com.rms.threadsassistant")
        .join("app.db")
}

/// OpenCode local DB (`~/.local/share/opencode/opencode.db`).
pub fn opencode_db() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db")
}

/// OpenAI Codex session rollouts (`~/.codex/sessions`).
pub fn codex_sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
        .join("sessions")
}

/// Qwen Code home (`~/.qwen`).
pub fn qwen_home_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".qwen")
}

/// ZCode CLI rollout logs (`~/.zcode/cli/rollout`).
pub fn zcode_rollout_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".zcode")
        .join("cli")
        .join("rollout")
}

/// Google Gemini CLI / Antigravity home (`~/.gemini`).
pub fn gemini_home_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".gemini")
}

/// Google Antigravity conversations (`~/.gemini/antigravity/conversations`).
pub fn antigravity_conversations_dir() -> PathBuf {
    gemini_home_dir().join("antigravity").join("conversations")
}

/// VS Code workspace storage root (Chat / Copilot chatSessions).
pub fn vscode_workspace_storage_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Library")
                .join("Application Support")
        })
        .join("Code")
        .join("User")
        .join("workspaceStorage")
}

/// Zed assistant threads DB.
pub fn zed_threads_db() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Library")
                .join("Application Support")
        })
        .join("Zed")
        .join("threads")
        .join("threads.db")
}

fn app_support_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library")
            .join("Application Support")
    })
}

/// Windsurf VS Code state DB with cached plan / quota.
pub fn windsurf_state_vscdb() -> PathBuf {
    app_support_dir()
        .join("Windsurf")
        .join("User")
        .join("globalStorage")
        .join("state.vscdb")
}

/// Windsurf Next variant state DB.
pub fn windsurf_next_state_vscdb() -> PathBuf {
    app_support_dir()
        .join("Windsurf - Next")
        .join("User")
        .join("globalStorage")
        .join("state.vscdb")
}
