# BossKit capability ledger

BossKit is a BOSS 直聘-only Rust 2024 CLI and MCP product. It provides browserless, read-only BOSS job search/detail access plus local cache, filters, presets, watches, shortlist, resumes, exports, statistics, and manual-review campaign plans.

## Implemented

| Category | Evidence | Boundary |
| --- | --- | --- |
| BOSS search and detail | `boss search`, `boss detail`, MCP `search_jobs`, `job_detail` | One BOSS 直聘 read-only adapter; live access remains subject to session and risk controls |
| Local workflow | `boss ls`, `history`, `export`, `preset`, `watch`, `shortlist`, `resume`, `campaign`, MCP equivalents | Strictly local files and deterministic filtering |
| Authentication | `boss --account <alias> login --role geek\|recruiter [-c\|--cookie-stdin]`, `status`, `logout` | Cookie-only login; hidden TTY input by default or one Cookie from non-terminal stdin with `-c`; verifies the requested role before atomic persistence; never uses a stored/environment Cookie as login input; no credential MCP tool or CAPTCHA/risk-control bypass |
| Account resume | `boss account resume show` | Bounded, privacy-filtered read only; no edit or submission |
| Explicit chat | `boss chat greet/send ... --yes`; `boss chat inbox [LOCAL_JOB_ID...]` | Plain-text messages only (no real URL, structural Markdown link, or rich-message reference); send success requires exact outgoing sender/target/text in authoritative history and distinguishes already-sent, verified, rejected, and unverified; no resume or automated conversation; no-ID inbox scans at most 3 newest cached jobs, explicit inbox queries at most 5 exact conversations |
| Native WeChat exchange | `boss chat exchange-wechat ... --yes` | One exact cached BOSS job through local ChromeDriver; no phone, text, resume, or WeChat ID output |
| Recruiter review/greet/reply/resume | CLI-only `boss --account <recruiter> recruiter candidates <keywords> --job-id <encrypted-job-id>`, `boss --account <recruiter> recruiter replies`, `boss --account <recruiter> recruiter inbox [--all --pending --job <text>]`, `boss --account <recruiter> recruiter inbox --brief`, `boss --account <recruiter> recruiter resume <uid>`, `boss --account <recruiter> recruiter resumes <uid>... --brief`, `boss --account <recruiter> recruiter greet --encrypt-geek-id <id> --security-id <id> --encrypt-job-id <id> --expect-id <id> --lid <id> --message ... --yes`, `boss --account <recruiter> recruiter reply <uid> --message ... --yes` | Explicit recruiter account required; only job-scoped recommendation cards with explicit `haveChatted: false` expose greeting context, while missing, malformed, and existing-conversation states are skipped; greeting re-reads the same recommendation list immediately before one exact candidate/job write and rejects a missing, ineligible, or metadata-mismatched card; native pagination, pending/job filtering, brief inbox projection, and bounded serial multi-resume reads are handled inside the CLI; resume detail is read-only, contact-redacted, ephemeral, and never persisted; greeting reports API acceptance without claiming independent delivery verification; no bulk Offer or batch send |
| CLI output | Markdown by default; `--json` opt-in | Compact human-readable output by default; machine-readable JSON only when explicitly requested |
| MCP | `boss mcp` | MCP 2025-03-26 stdio with strict schemas; no credentials or account controls |

## Migration

The runtime accepts legacy local config and account documents so users can start normally. Legacy non-BOSS platform settings and stored sessions are discarded during migration, never selected, exposed, or written back.

## Not implemented

Automatic application submission, resume mutation/synchronization, bulk greeting, automated reply, background scheduling, CAPTCHA/risk-control bypasses, phone/SMS/browser-repair login, and any non-BOSS provider are intentionally absent. Recruiter candidate search and greeting are bounded and CLI-only.
