# BossKit capability ledger

BossKit is an independently developed Rust 2024 product with its own command protocol, data model, safety boundary, and release lifecycle. It is not a branch, compatibility layer, or parity promise for another project. This ledger records BossKit's own shipped boundaries; a green build alone is not evidence that a live platform workflow works.

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
| Resume screening | `boss campaign screen`, MCP `campaign_screen` | Deterministic local-only title and skill matching over cached job title, skills, and description; policy and blacklist gates run first; creates only deduplicated `manual_review` / `dry_run` plans |
| Campaign review plans | `boss campaign plan ...`, matching MCP tools | Local policy, blacklist, bounded ephemeral greeting previews, and human-recorded state transitions; never applies or chats |
| Default Zhipin greeting | `boss chat greet <LOCAL_JOB_ID> --yes` | CLI-only browserless initial contact for one exact cached job; verifies the exact encrypted job ID, sends no custom text or resume, and is unavailable through MCP |
| Existing Zhipin chat | `boss chat send <LOCAL_JOB_ID> --message <TEXT> --yes` | CLI-only browserless single text to an existing exact conversation; bounded printable text, one QoS 1 publish with no automatic retry, exact outgoing-history verification, no resume/application, and no MCP surface |
| Zhipin chat history | `boss chat history <LOCAL_JOB_ID> --limit 20` | CLI-only browserless bounded text read from one existing exact conversation; chronological direction/text/timestamp output, no platform message or resume/application, and no MCP surface |
| Statistics | `boss stats`, MCP `stats` | Exact local counts, time-window history outcomes, and known-file sizes |
| Recoverable cleanup | `boss clean`, MCP `clean_preview` | Preview by default; Linux confirmation atomically archives only six known mutable JSON files and returns verified recovery paths, using a private root-level rescue transaction if rollback is blocked; no unlink; non-Linux and MCP are preview-only |
| MCP transport | `boss mcp` | MCP 2025-03-26 stdio, strict arguments, batch requests |

## Partial

| Category | Current boundary |
| --- | --- |
| Authentication | CLI stores sessions from explicit local sources. BOSS 直聘 additionally refreshes and verifies its session through browserless HTTPS plus local V8 challenge computation; MCP has no login surface |
| Cities | 10 common logical mappings, not the broader upstream city catalog |
| Search filters | Richer company/salary/experience/education/job-type/welfare filters exist, but operate only on fields present in provider list responses |
| Detail compatibility | Implemented for all three adapters, but live endpoints/pages remain subject to login and risk controls |
| Platform compatibility | Live results remain subject to current login, risk-control, endpoint, and response-shape changes |

## Pending

Platform resume synchronization, background scheduling, crawling, batch greeting or message orchestration, reply polling, automatic conversations, and platform application workflows are not implemented.

## Policy-blocked / intentionally unimplemented

Automatic BOSS application submission and autonomous chat are not implemented. Recruiter personal-data collection, platform resume mutation, and bypasses for CAPTCHA, SMS, or risk controls also remain absent. Campaign screening remains strictly local and must not be interpreted as submission capability.
