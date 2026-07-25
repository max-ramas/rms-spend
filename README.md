# RMS Spend

Local **IDE-native** AI token spend tracker. Reads official on-disk artifacts from coding tools — **no network interception**.

SQLite (`~/.rms-memory/spend.db`) is the single source of truth. Collectors normalize raw logs into the ledger. The same crate powers the CLI, the interactive TUI, and the **Spend** tab + tray menu in [rms-memory-gui](../rms-memory-gui).

## Status

**Local + cloud MVP delivered:**
- Local CLI/library ledger, tariffs `0.2.0-builtin`, connectors, period filters, TUI
- GUI: Spend tab (period chips, active consumers, connector management, cloud sync)
- Tray menu (icon only — no menu-bar title text): totals + active sources + Open Spend
- Opt-in cloud sync via [`rms-spend-service`](../rms-spend-service)
- Zed zstd JSON token decode; Antigravity step+gen timestamps; Windsurf cache + opt-in GetUserStatus RPC
- `Accuracy::Quota` for Cursor Quota / Windsurf (excluded from token Totals); Cursor proxy labeled **Cursor (lines)**
- Foreground `watch`, optional USD `budget`, `init` onboarding

Out of scope here: launchd packaging, Windsurf cascade `.pb` decrypt.

## Install / build

```bash
cd ~/dev/rms-spend
cargo build --release
cargo install --path .
```

## Commands

```bash
# First-run setup (dirs, builtin tariffs, default connectors)
rms-spend init

# Ingest all enabled collectors
rms-spend scan

# Opt-in network for Windsurf live credits (also: connector windsurf --allow-network)
rms-spend scan --network
rms-spend scan --only windsurf --force --network

# Foreground poll loop (Ctrl-C to stop)
rms-spend watch --interval 300
rms-spend watch --interval 60 --only claude_code --force

# Single collector
rms-spend scan --only claude_code
rms-spend scan --only rms_memory
rms-spend scan --only cursor_proxy
rms-spend scan --only cursor_quota   # off by default — enable via connector
rms-spend scan --only rms_threads
rms-spend scan --only opencode
rms-spend scan --only codex
rms-spend scan --only qwen_code
rms-spend scan --only zcode
rms-spend scan --only gemini_cli
rms-spend scan --only antigravity
rms-spend scan --only vscode_copilot
rms-spend scan --only zed
rms-spend scan --only windsurf   # off by default — enable via connector

# Override data dirs / custom ledger home
rms-spend scan --claude-projects ~/.claude/projects
RMS_SPEND_HOME=/tmp/spend-test rms-spend scan

# Summaries with period filter
rms-spend summary
rms-spend summary --period today|week|month|all
rms-spend summary --period range --from 2026-07-01T00:00:00+04:00 --to 2026-07-21T23:59:59+04:00
rms-spend summary --json --source antigravity

# Interactive TUI (1/t Today · 2/w Week · 3/m Month · 4/a All · r refresh · q quit)
rms-spend tui

rms-spend export --format json -o spend.json --period week
rms-spend export --format csv -o spend.csv

rms-spend status
rms-spend budget set --usd 50 --period month
rms-spend budget show
rms-spend budget clear
rms-spend init-tariffs              # write builtin prices (force overwrite)
rms-spend refresh-tariffs          # rewrite 0.2.0-builtin + reprice null costs in ledger
rms-spend refresh-tariffs --url https://…   # remote bundle + reprice
rms-spend connector cursor_proxy --disable
rms-spend connector antigravity --enable
rms-spend connector windsurf --enable --allow-network

# Cloud sync (opt-in)
rms-spend init-sync
# Optional HMAC attestation (must match service REGISTER_HMAC_SECRET):
#   export RMS_SPEND_REGISTER_SECRET=…
#   # or set registerSecret in spend-sync.json
rms-spend register --api-base http://localhost:8080/api/v1
rms-spend sync --refresh-tariffs
```

## Collectors

| Id | Display name | Canonical source | Accuracy |
|----|--------------|------------------|----------|
| `claude_code` | Claude Code | `~/.claude/projects/**/*.jsonl` | `exact` |
| `rms_memory` | RMS Memory | `usage.jsonl` → `manifest.json` → `proposal.json` | `exact` / `estimated` |
| `cursor_proxy` | Cursor (lines) | Cursor tracking DB + `state.vscdb` daily stats | `proxy` (lines, not tokens) |
| `cursor_quota` | Cursor Quota | Cursor quota keys in `state.vscdb` | `quota` (units, not tokens) |
| `rms_threads` | RMS Threads | Threads Assistant `app.db` | `exact` (+ stored cost) |
| `opencode` | OpenCode | `~/.local/share/opencode/opencode.db` | `exact` |
| `codex` | Codex | `~/.codex/sessions/**/rollout-*.jsonl` | `exact` |
| `qwen_code` | Qwen Code | `~/.qwen/projects/**/chats/*.jsonl` + `usage_record.jsonl` | `exact` |
| `zcode` | ZCode | `~/.zcode/cli/rollout/model-io-*.jsonl` | `exact` |
| `gemini_cli` | Gemini CLI | `~/.gemini/**/*.jsonl` with `usageMetadata` | `exact` |
| `antigravity` | Google Antigravity | `~/.gemini/antigravity/conversations/*.db` (`gen_metadata` + `steps` join) | `exact` |
| `vscode_copilot` | VS Code Chat | VS Code `workspaceStorage/*/chatSessions/*.jsonl` | `exact` |
| `zed` | Zed | Zed `threads.db` (zstd JSON `request_token_usage`) | `exact` |
| `windsurf` | Windsurf | `state.vscdb` cache; opt-in GetUserStatus RPC | `quota` (credit units) |

**Display names:** use `rms_spend::source_label` / `known_collectors()` — never surface raw ids in UI.

**Totals vs Activity / Quotas:** summary/TUI Totals count only `exact` + `estimated` tokens. `proxy` (Cursor lines) and `quota` (Cursor Quota / Windsurf) are listed separately and do not inflate `total_tokens`.

**Claude Code:** `stats-cache.json` is **not** ingested (avoids double-counting).

**Cursor proxy:** labeled **Cursor (lines)**; `cost_usd` always `null`. Schema drift → `stale`, ingest skipped.

**Antigravity:** token counts from nested protobuf (field 6==24); `occurred_at` prefers `steps.metadata` timestamp, then `gen_metadata` blob, else conversation DB mtime.

**VS Code Chat:** timestamps from `toolCallRounds` (epoch ms); fallback file mtime — never scan-time `Utc::now()`.

**Zed:** decompresses `data_type=zstd` thread blobs; one event per `request_token_usage` entry (+ cumulative remainder when needed). Per-request message timestamps are **not** in current blobs — events use thread `updated_at`. Usage keys join User message ids without storing content.

**Windsurf:** local `state.vscdb` plan cache by default. Live credits: `rms-spend scan --network` or `connector windsurf --allow-network` (reads `windsurfAuthStatus.apiKey`, tries local LS then `server.codeium.com` GetUserStatus; never logs the key). Off by default (`connector windsurf --enable`).

**Connectors:** `~/.rms-memory/spend-connectors.json`. `cursor_quota` and `windsurf` are **off by default**. Optional `allowNetwork.windsurf`.

**Budget:** `~/.rms-memory/spend-budget.json` via `rms-spend budget set|show|clear`; surfaced in `status` / `summary` / TUI footer.

## Tariffs (`0.2.0-builtin`)

File: `~/.rms-memory/spend-tariffs.json`.

- Catalog: Anthropic, OpenAI, Gemini 2.5/3.x (+ Antigravity aliases), DeepSeek V4, OpenCode Zen (MiniMax, GLM, Qwen, Kimi, free tiers), Codex `openai`→GPT-4.1 estimate, common OSS.
- Free models (`big-pickle`, `*-free`) are explicit **$0**, not “unknown”.
- Unknown / subscription-only / proxy → `cost_usd = null` (never invent a price).
- `refresh-tariffs` also **reprices** ledger rows that still have null cost (`reprice_ledger`).

## Period filters

Presets (local TZ): `today`, `week` (rolling 7 days), `month` (calendar month), `range` (`--from` / `--to` RFC3339), `all`.

Used by: CLI `summary` / `export`, TUI, GUI Spend chips, tray snapshot (today/week/month).

## Privacy

Local-only by default. Sync sends **metadata only**: tokens, model, timestamps, cost — never prompts, code, or transcripts.

## Design notes

- Library + CLI: this repo. GUI adapters: `rms-memory-gui` (`commands/spend.rs`, `spend_tray.rs`, `Spend.tsx`).
- Cloud service: [`rms-spend-service`](../rms-spend-service)
- Public leaderboard: `https://spend.rms-ds.com/api/v1/leaderboard`
- Split runbook: `docs/SPLIT.md`

## 📄 License

MIT License — see [LICENSE](LICENSE) for details.

y---

<div align="center">
<sub>Built by <a href="https://ramzaeff.com">Maksim Ramzaev</a> · <a href="https://rms-ds.com">RMS Digital Services</a></sub>
</div>
