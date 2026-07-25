# RMS Spend

Local CLI/library for IDE-native AI token spend tracking. Also the path dependency for the **Spend** tab and tray menu in `rms-memory-gui`.

## Layout

- Ledger: `~/.rms-memory/spend.db` (SQLite = source of truth)
- Tariffs: `~/.rms-memory/spend-tariffs.json` (`0.2.0-builtin` after `refresh-tariffs`)
- Connectors: `~/.rms-memory/spend-connectors.json`
- Entry points: `rms-spend` CLI, `rms-spend tui`, library APIs used by Tauri

## Agent rules

- Prefer `source_label` / `known_collectors()` for any user-facing name — no raw ids in tray/TUI/GUI.
- Tray: **icon only** — never `set_title` with token counts; stats belong in the click menu.
- Spend UI: **Totals** first, then **active consumers only** (events > 0).
- Unknown model price → `cost_usd = null`; free tiers are explicit `0.0`.
- After expanding tariffs, run `rms-spend refresh-tariffs` so the ledger is repriced.

## Project key

When writing to RMS Memory for this repo, use project key `rms-spend` if registered; otherwise document under `rms-memory-gui` vault paths that already hold spend docs (`docs/spend-*.md`).

<!-- RMS-MEMORY-START -->
# RMS Memory MCP Guide

You are currently connected to the **RMS Memory MCP Server**. This is your persistent memory for the repository. 

**Core Directives:**
0. **Project Routing:** This repository's registered key is `rms-spend`. If the MCP client did not initialize a workspace root, pass `project: "rms-spend"` to RMS Memory tools. Use `rms_projects` to list valid keys. Never guess another project's vault.
1. **Search First:** Use the `rms-memory_rms_search` tool to find past decisions, architecture records, or rules before making substantial changes to the code.
2. **Read Context:** Use the `rms-memory_rms_read` tool to pull in full context for any documents found during your search.
3. **Repository moves:** Never run `git init`, delete `.git`, or recreate the repository to fix a folder-name mismatch. If the repository moved or was renamed, tell the user to run `rms-memory projects migrate` (GUI: Settings → Migrate project).
4. **Persist Knowledge (MANDATORY TASK-END ROUTINE):** Before completing ANY request, you MUST ask yourself: "Did I discover a new project convention, fix a tricky bug, or learn a user preference?" If YES, you MUST proactively save this knowledge. DO NOT ask for permission.
   - **CRITICAL:** You MUST use the `rms-memory_rms_write` tool. DO NOT use your standard file writing tools (like `write_to_file`, `bash`, etc.) because the memory vault is stored externally.
   - **PATH:** Provide only the relative folder and filename (e.g., `architecture/decision.md`, `rules/api.md`, `decisions/001.md`, `artifacts/walkthrough.md`, `docs/setup.md`). The MCP server will automatically route it to the correct external vault. Do NOT prepend `.agents` or the project root.
<!-- RMS-MEMORY-END -->
