# BossKit capability ledger

BossKit is a BOSS 直聘-only Rust 2024 CLI and MCP product. It provides browserless, read-only BOSS job search/detail access plus local cache, filters, presets, watches, shortlist, resumes, exports, statistics, and manual-review campaign plans.

## Implemented

| Category | Evidence | Boundary |
| --- | --- | --- |
| BOSS search and detail | `boss search`, `boss detail`, MCP `search_jobs`, `job_detail` | One BOSS 直聘 read-only adapter; live access remains subject to session and risk controls |
| Local workflow | `boss ls`, `history`, `export`, `preset`, `watch`, `shortlist`, `resume`, `campaign`, MCP equivalents | Strictly local files and deterministic filtering |
| Authentication | `boss login --role geek\|recruiter`, `boss login --phone`, `boss login --repair`, `status`, `logout` | Cookie login plus visible ChromeDriver phone/SMS flow and attached-Chrome session repair; phone, code, and refreshed tokens are transient, no credential MCP tool, no CAPTCHA/risk-control bypass |
| Account resume | `boss account resume show` | Bounded, privacy-filtered read only; no edit or submission |
| Explicit chat | `boss chat greet/send ... --yes`; `boss chat inbox [LOCAL_JOB_ID...]` | Plain-text messages only (no URL/Markdown/rich-message references), no resume or automated conversation; no-ID inbox scans at most 3 newest cached jobs, explicit inbox queries at most 5 exact conversations |
| Native WeChat exchange | `boss chat exchange-wechat ... --yes` | One exact cached BOSS job through local ChromeDriver; no phone, text, resume, or WeChat ID output |
| Recruiter review/reply/resume | CLI-only `boss --account <recruiter> recruiter replies`, `boss --account <recruiter> recruiter inbox [--all --pending --job <text>]`, `boss --account <recruiter> recruiter inbox --brief`, `boss --account <recruiter> recruiter resume <uid>`, `boss --account <recruiter> recruiter resumes <uid>... --brief`, `boss --account <recruiter> recruiter reply <uid> --message ... --yes` | Explicit recruiter account required; native pagination, pending/job filtering, brief inbox projection, and bounded serial multi-resume reads are handled inside the CLI; resume detail is read-only, contact-redacted, ephemeral, and never persisted; each write is confirmed and history-verified; no bulk Offer or batch send |
| CLI output | Markdown by default; `--json` opt-in | Compact human-readable output by default; machine-readable JSON only when explicitly requested |
| MCP | `boss mcp` | MCP 2025-03-26 stdio with strict schemas; no credentials or account controls |

## Migration

The runtime accepts legacy local config and account documents so users can start normally. Legacy non-BOSS platform settings and stored sessions are discarded during migration, never selected, exposed, or written back.

## Not implemented

Automatic application submission, resume mutation/synchronization, bulk greeting, automated reply, background scheduling, recruiter candidate search/profile collection, CAPTCHA/risk-control bypasses, and any non-BOSS provider are intentionally absent. Phone/SMS login is supported only through the visible BOSS page and requires the user to complete any interactive security check.
