# Upstream parity ledger

BossKit is an in-progress Rust reimplementation of `boss-agent-cli`, not a claim of full command parity. This ledger compares capability categories rather than treating a green build as proof that all upstream workflows exist.

## Implemented now

| Category | Rust command/tool evidence | Scope |
| --- | --- | --- |
| Provider inventory | `boss platforms`, MCP `platforms` | BOSS 直聘, 智联招聘, 前程无忧 / 51job registered |
| Search | `boss search`, MCP `search_jobs` | Three-provider normalized read-only search with per-provider outcomes |
| Local jobs | `boss ls`, `boss show`, MCP `list_jobs`, `show_job` | Atomic normalized JSON cache |
| Job detail | `boss detail`, MCP `job_detail` | Three read-only provider adapters; cached overlays; subject to live controls |
| Local filters | `boss search` and MCP `search_jobs` filter arguments | Local-only matching over list response fields; no automatic detail fetch |
| Search history | `boss history`, MCP `search_history` | BossKit local search-attempt audit, not remote platform browsing history |
| Export | `boss export`, MCP `export_jobs` | Local JSON/CSV/HTML files in CLI; MCP structured data only |
| Cities | `boss cities`, MCP `cities` | Exactly 10 logical cities mapped across all three adapters |
| Configuration | `boss config ls/get/set/reset` | Five typed safe keys; defaults merged with user overrides |
| Local auth status | `boss status`, MCP `status` | Cookie environment presence only; no network or secret values |
| Local diagnostics | `boss doctor`, MCP `doctor` | Data/config/cache/shortlist/registration/cookie checks; no network |
| Capability schema | `boss schema`, MCP `schema` | Native, OpenAI, Anthropic, and MCP wrapper formats |
| Shortlist | `boss shortlist add/ls/annotate/rm/compare` and matching MCP tools | Local cached snapshots, tags, notes, timestamps |
| Presets | `boss preset add/ls/show/rm`, matching MCP tools | Complete validated local search specifications with override-aware search |
| Watches | `boss watch add/ls/show/rm/run`, matching MCP tools | Explicit foreground read-only searches with an exact deduplicated union of every seen stable ID; no scheduler |
| Local resumes | `boss resume ...`, matching structured MCP tools | Strict typed documents in one local file; no platform resume synchronization |
| Statistics | `boss stats`, MCP `stats` | Exact local counts, time-window history outcomes, and known-file sizes |
| Recoverable cleanup | `boss clean`, MCP `clean_preview` | Preview by default; Linux confirmation atomically archives only six known mutable JSON files and returns verified recovery paths, using a private root-level rescue transaction if rollback is blocked; no unlink; non-Linux and MCP are preview-only |
| MCP transport | `boss mcp` | MCP 2025-03-26 stdio, strict arguments, batch requests |

## Partial

| Category | Current boundary |
| --- | --- |
| Authentication | Status only detects the three Cookie environment variables; no login/session acquisition or validity probe |
| Cities | 10 common logical mappings, not the broader upstream city catalog |
| Search filters | Richer company/salary/experience/education/job-type/welfare filters exist, but operate only on fields present in provider list responses |
| Detail compatibility | Implemented for all three adapters, but live endpoints/pages remain subject to login and risk controls |
| Platform compatibility | Live results remain subject to current login, risk-control, endpoint, and response-shape changes |

## Pending

Remote platform browsing history, platform resume synchronization, background scheduling, AI analysis, crawling, and other upstream operator categories are not implemented in this slice.

## Policy-blocked / intentionally unimplemented

Remote write or sensitive-person workflows remain absent: greeting, applying, chat, resume mutation, recruiter workflows, and recruiter personal-data collection. The current Rust service performs remote reads and clearly identified local JSON writes only.
