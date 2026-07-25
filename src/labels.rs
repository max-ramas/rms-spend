//! Human-readable labels for collector / source ids.

/// English display name for a source or collector id.
pub fn source_label(id: &str) -> String {
    match id {
        "claude_code" => "Claude Code".into(),
        "rms_memory" => "RMS Memory".into(),
        "cursor_proxy" => "Cursor (lines)".into(),
        "cursor_quota" => "Cursor Quota".into(),
        "rms_threads" => "RMS Threads".into(),
        "opencode" => "OpenCode".into(),
        "codex" => "Codex".into(),
        "qwen_code" => "Qwen Code".into(),
        "zcode" => "ZCode".into(),
        "gemini_cli" => "Gemini CLI".into(),
        "antigravity" => "Google Antigravity".into(),
        "vscode_copilot" => "VS Code Chat".into(),
        "zed" => "Zed".into(),
        "windsurf" => "Windsurf".into(),
        other => other.to_string(),
    }
}

/// Localized display name (`ru` or English fallback).
pub fn source_label_locale(id: &str, russian: bool) -> String {
    if russian {
        // Product names stay Latin; keep a few Russian clarifiers if needed later.
        return source_label(id);
    }
    source_label(id)
}

/// Known collectors with display labels (stable order for UI).
pub fn known_collectors() -> &'static [(&'static str, &'static str)] {
    &[
        ("claude_code", "Claude Code"),
        ("rms_memory", "RMS Memory"),
        ("cursor_proxy", "Cursor (lines)"),
        ("cursor_quota", "Cursor Quota"),
        ("rms_threads", "RMS Threads"),
        ("opencode", "OpenCode"),
        ("codex", "Codex"),
        ("qwen_code", "Qwen Code"),
        ("zcode", "ZCode"),
        ("gemini_cli", "Gemini CLI"),
        ("antigravity", "Google Antigravity"),
        ("vscode_copilot", "VS Code Chat"),
        ("zed", "Zed"),
        ("windsurf", "Windsurf"),
    ]
}
