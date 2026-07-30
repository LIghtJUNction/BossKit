# BossKit — BOSS 直聘 CLI 与 MCP

[![crates.io](https://img.shields.io/crates/v/bosskit?logo=rust&logoColor=white)](https://crates.io/crates/bosskit)
[![Rust 2024](https://img.shields.io/badge/Rust-2024-dea584?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-0b7f55)](LICENSE)
[![Scope: BOSS 直聘](https://img.shields.io/badge/scope-BOSS%20%E7%9B%B4%E8%81%98-16a34a)](#能力边界)
[![Interface: CLI + MCP](https://img.shields.io/badge/interface-CLI%20%2B%20MCP-4f46e5)](#mcp)

![BossKit：BOSS 直聘、CLI + MCP、本地筛选、免费开源](public/readme-hero.svg)

免费开源的 Rust 2024 求职辅助工具，专注 BOSS 直聘。BossKit 在终端或 MCP 客户端中提供职位搜索、详情读取、确定性的本地简历筛选与人工复核工作流。

> 设计原则：职位决策留给你。BossKit 不会自动投递简历、批量打招呼、自动回复，或绕过验证码、短信及风控。

| 你可以做什么 | BossKit 如何约束 |
| --- | --- |
| 搜索、缓存并查看 BOSS 职位 | 纯命令行；不启动浏览器、不读取浏览器资料 |
| 以本地简历和规则筛选职位 | 结果稳定可复现，只生成 `manual_review` / `dry_run` 计划 |
| 使用 CLI 或 MCP 接入工作流 | MCP 不暴露登录、账户、在线简历或凭据入口 |
| 对单个缓存职位打招呼或发送消息 | 必须逐次显式 `--yes`；从不发送简历 |

## 快速开始

```bash
cargo install bosskit
boss --help
boss search rust --city 深圳 --limit 20
boss ls --limit 10
boss mcp
```

普通命令向 stdout 输出 JSON 信封；`--help` 和 `--version` 输出文本。数据目录依次使用 `BOSS_DATA_DIR`、系统本地数据目录的 `bosskit`、当前目录的 `.boss`。

## 登录与安全

```bash
boss account use work --yes
wl-paste | boss login --account work --role geek -c
boss --account work status
boss logout --yes
```

登录只支持 BOSS 直聘。Cookie 不进入命令行参数、shell 历史或 JSON 输出；`-c` 仅从非终端标准输入读取一个 Cookie，`--manual` 仅在终端以隐藏回显输入。会话保存于 `0700` 私有目录和 `0600` 私有文件。旧版其它平台会话在读取时被安全忽略，且不会阻塞启动或被重新写出。

`login` 使用纯命令行 HTTPS 验证 BOSS 会话；遇到需要本地 V8 挑战计算的响应时，会在本机完成计算后再验证。整个过程不启动浏览器、不读取浏览器资料，也不输出 Cookie。

每个本地账户保存安全元数据角色：旧账户默认 `geek`；可用 `boss login --role recruiter -c` 保存招聘者会话。招聘者 CLI 提供有界的 `boss recruiter replies` 状态列表、`boss recruiter inbox --limit 20 --page 1` 会话预览，以及 `boss recruiter resume <UID>` 读取一个候选人的完整在线简历详情（只读、限量、脱敏、不落盘）。预览只保留最近一条文本并脱敏联系方式；返回的会话 UID 仅用于人工确认后的精确回复。发送必须显式执行 `boss recruiter reply <UID> --message '...' --yes`，每次只发一条并通过历史记录核验，不自动发 Offer、不批量群发。

命令默认输出紧凑 Markdown；需要脚本解析时显式加全局 `--json`，例如 `boss --json recruiter resume <UID>`。

## 本地筛选与人工复核

```bash
boss preset add rust-backend rust --city 深圳
boss watch add rust-watch rust --city 深圳
boss campaign policy add rust --include title:rust
boss campaign screen --resume local --policy rust
boss campaign plan ls
```

搜索过滤、预设、监视、短名单、简历、统计、导出和 campaign 均以本地数据为边界。`campaign screen` 只生成 `manual_review` / `dry_run` 计划；它不是投递能力。筛选只使用明确填写的本地简历字段和已缓存职位字段，结果按稳定规则排序。

仓库内置两份可复用的 Agent Skills：[简历筛选](.agents/skills/resume-screening/SKILL.md) 与 [申请流程](.agents/skills/resume-application/SKILL.md)。后者同样以人工批准为门槛，不会代替你进行平台投递。

## 明确确认的单目标消息

```bash
boss chat greet <本地职位ID> --yes
boss chat send <本地职位ID> --message "你好，想进一步了解这个职位" --yes
```

两项操作都只作用于一个精确的已缓存职位；`chat send` 还要求存在同一职位的既有会话。它们不会创建批量任务、自动回复或提交简历。

## MCP

```bash
boss mcp
```

MCP 使用 2025-03-26 stdio JSON-RPC。它提供 BOSS 职位搜索、缓存、筛选和本地工作流工具；不提供账户选择、登录、登出、招聘者、在线简历、聊天或凭据入口。

## 配置

```bash
boss config ls
boss config set page_size 30
boss config reset page_size
```

支持的安全配置键为 `request_timeout_secs`、`page_size`、`operating_mode` 和 `log_level`。旧版 `platform` 配置会被安全忽略。

## 能力边界

- 仅支持 BOSS 直聘；不宣称多平台支持。
- 本地简历筛选是确定性的，输出用于人工复核，而非平台自动投递。
- 登录验证在纯命令行 HTTPS 与本机挑战计算中完成；不会启动或依赖浏览器。
- BossKit 是免费的开源软件，采用 [MIT License](LICENSE)。

完整的命令与能力映射见 [docs/PARITY.md](docs/PARITY.md)。
