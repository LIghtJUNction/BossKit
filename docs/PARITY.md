# BossKit capability ledger

BossKit is a BOSS 直聘-only Rust 2024 CLI and MCP product. It provides browserless, read-only BOSS job search/detail access plus local cache, filters, presets, watches, shortlist, resumes, exports, statistics, and manual-review campaign plans.

## Implemented

| Category | Evidence | Boundary |
| --- | --- | --- |
| BOSS search and detail | `boss search`, `boss detail`, MCP `search_jobs`, `job_detail` | One BOSS 直聘 read-only adapter; live access remains subject to session and risk controls |
| Local workflow | `boss ls`, `history`, `export`, `preset`, `watch`, `shortlist`, `resume`, `campaign`, MCP equivalents | Strictly local files and deterministic filtering |
| Authentication | `boss login --role geek\|recruiter`, `status`, `logout` | Per-account safe role metadata; legacy accounts default to `geek`; no credential MCP tool |
| Account resume | `boss account resume show` | Bounded, privacy-filtered read only; no edit or submission |
| Explicit chat | `boss chat greet/send ... --yes` | One exact cached BOSS job, no resume, no batch or automated conversation |
| Recruiter review/reply/resume | CLI-only `boss recruiter replies`, `boss recruiter inbox`, `boss recruiter resume <uid>`, `boss recruiter reply <uid> --message ... --yes` | Bounded recruiter friend-list/history/detail reads; resume detail is read-only, bounded, contact-redacted, and ephemeral; each write is confirmed and history-verified; no bulk Offer or batch send |
| CLI output | Markdown by default; `--json` opt-in | Compact human-readable output by default; machine-readable JSON only when explicitly requested |
| MCP | `boss mcp` | MCP 2025-03-26 stdio with strict schemas; no credentials or account controls |

## Migration

The runtime accepts legacy local config and account documents so users can start normally. Legacy non-BOSS platform settings and stored sessions are discarded during migration, never selected, exposed, or written back.

## Not implemented

Automatic application submission, resume mutation/synchronization, bulk greeting, automated reply, background scheduling, recruiter candidate search/profile collection, CAPTCHA/SMS/risk-control bypasses, and any non-BOSS provider are intentionally absent.
