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
| Recruiter reply state | CLI-only `boss recruiter replies --limit 1..20 --page 1..50` | Recruiter-only fixed friend-list GET; bounded redacted records, no identifiers/contact URLs, unknown direction is never guessed |
| MCP | `boss mcp` | MCP 2025-03-26 stdio with strict schemas; no credentials or account controls |

## Migration

The runtime accepts legacy local config and account documents so users can start normally. Legacy non-BOSS platform settings and stored sessions are discarded during migration, never selected, exposed, or written back.

## Not implemented

Automatic application submission, resume mutation/synchronization, bulk greeting, automated reply, background scheduling, recruiter candidate search/profile collection, CAPTCHA/SMS/risk-control bypasses, and any non-BOSS provider are intentionally absent.
