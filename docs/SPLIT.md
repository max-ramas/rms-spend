# rms-spend-service (extracted)

Cloud sync now lives in the standalone **Rust** repo [`rms-spend-service`](../../rms-spend-service).

## Status

- **Done:** `rms-spend-service` — separate Rust binary (`rms-spend-service serve`)
- **Done:** spend removed from `rms-license` (backend, frontend, migration `000052_drop_spend_module`)
- **Client:** `rms-spend` points to `https://spend.rms-ds.com/api/v1` via `spend-sync.json`

## API (prefix `/api/v1`)

| Method | Path | Auth |
|--------|------|------|
| POST | `/register` | — |
| GET | `/tariffs` | — |
| GET | `/leaderboard` | — |
| GET/PUT | `/me` | X-API-Key |
| POST | `/sync` | X-API-Key |
| GET | `/me/summary` | X-API-Key |
| DELETE | `/me/events` | X-API-Key |
| POST/GET | `/orgs` | X-API-Key |
| POST | `/orgs/join` | X-API-Key |
| GET | `/orgs/:id/summary` | X-API-Key |

Auth is **fully independent** from rms-license — `POST /register` returns an API key.

## What stays local

- `rms-spend` CLI / GUI collectors and `~/.rms-memory/spend.db` never move to the cloud.
- Only metadata sync crosses the boundary.

## Deploy

```bash
cd rms-spend-service
cargo build --release
rms-spend-service serve --db ./data/spend.db
# or: docker compose up -d
```

Public leaderboard: `https://spend.rms-ds.com/api/v1/leaderboard`
