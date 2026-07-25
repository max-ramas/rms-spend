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
