<p align="center">
  <img src="public/readme-hero.svg" alt="BossKit — BOSS 直聘 CLI + MCP, local screening, free and open source" width="100%">
</p>

<div align="center">
  <p><strong>把 BOSS 直聘职位搜索、人工复核和有界沟通带到终端与 MCP。</strong></p>
  <p>Rust 2024 · CLI + stdio MCP · 本地确定性筛选 · 单目标显式确认</p>

  [![Crates.io](https://img.shields.io/crates/v/bosskit?style=flat-square&logo=rust)](https://crates.io/crates/bosskit)
  [![Source](https://img.shields.io/badge/source-0.8.1-111111?style=flat-square)](Cargo.toml)
  [![Release](https://img.shields.io/github/v/release/LIghtJUNction/BossKit?style=flat-square)](https://github.com/LIghtJUNction/BossKit/releases/latest)
  [![License](https://img.shields.io/badge/license-MIT-111111?style=flat-square)](LICENSE)

  [快速开始](#快速开始) · [求职者](#求职者工作流) · [招聘者](#招聘者工作流) · [MCP](#mcp) · [安全边界](#明确边界)
</div>

> [!IMPORTANT]
> BossKit 只支持 BOSS 直聘。它不会自动投递简历、批量打招呼、自动回复、自动发 Offer，或绕过验证码、短信和平台风控。职位判断与每次写操作都留给人。

## 本质上是什么

BossKit 把工作流拆成三层：

```text
BOSS 只读数据
搜索 · 职位详情 · 会话/候选人读取
            ↓
本地确定性工作流
缓存 · 筛选 · 预设 · 监视 · 简历 · campaign 计划
            ↓
少量平台写操作
一次一人 / 一次一职位 / 显式 --yes / 结果核验
```

它不是浏览器自动化框架，也不是“让 Agent 自己找工作或招人”的机器人。默认输出紧凑 Markdown；只有显式使用全局 `--json` 才输出机器可读 JSON。

## 选择你的入口

| 使用者 | 可以做什么 | 明确不会做什么 |
| --- | --- | --- |
| **求职者** | 搜索和缓存职位、本地简历筛选、查看既有会话、对单个职位发送纯文本消息 | 自动投递、批量打招呼、上传简历、自动追问 |
| **招聘者** | 查看岗位推荐、会话和脱敏简历；对一个候选人执行一次招呼或回复 | 批量群发、按性别筛选、读取联系方式、自动 Offer |
| **MCP 客户端** | 搜索、详情、本地缓存和筛选工具 | 登录、Cookie、账户切换、招聘者命令、聊天和在线简历 |

完整能力账本与边界见 [docs/PARITY.md](docs/PARITY.md)。

## 版本状态

当前源码声明版本为 `0.8.1`。GitHub 最新正式 Release 标签仍为 `v0.7.0`。因此：

- `cargo install bosskit` 获取的是 crates.io 当前已发布版本；
- 从 `main` 构建可获得源码树当前能力；
- GitHub `v0.7.0` 发布说明不能证明后续 0.8.x 源码功能已发布或完成现场平台验证。

平台接口和风控会变化。编译通过、单元测试通过与真实 BOSS 会话写入成功是不同证据层级。

## 快速开始

```bash
cargo install bosskit
boss --help
```

搜索并缓存职位：

```bash
boss search rust --city 深圳 --limit 20
boss ls --limit 10
boss detail <本地职位ID>
```

启动 MCP：

```bash
boss mcp
```

数据目录按以下顺序选择：

```text
BOSS_DATA_DIR
    ↓ 未设置
系统本地数据目录 / bosskit
    ↓ 不可用
当前目录 / .boss
```

## 登录模型

BossKit 只接受 BOSS 直聘 Cookie，不提供手机号、短信或验证码登录。

### 交互式隐藏输入

```bash
boss account use work --yes
boss --account work login --role geek
```

### 从非终端 stdin 读取一次新 Cookie

```bash
printf '%s' "$NEW_BOSS_COOKIE" \
  | boss --account recruiter login --role recruiter --cookie-stdin
```

```bash
boss --account work status
boss --account work logout --yes
```

登录流程的关键点：

```text
读取一个新 Cookie
      ↓
按指定 geek / recruiter 入口验证
      ↓ 成功
原子替换旧会话
      ↓ 失败
保留旧会话
```

- Cookie 不进入命令行参数、shell 历史或 JSON 输出。
- 默认从交互终端隐藏读取；`--cookie-stdin` 只接受非终端 stdin。
- `login` 不会把已保存 Cookie 或 `BOSS_ZHIPIN_COOKIE` 偷偷当作本次输入。
- 会话目录使用 `0700`，文件使用 `0600`。
- 需要本地 V8 计算的平台挑战可在本机完成，但 BossKit 不启动浏览器、不读取浏览器资料，也不绕过风险控制。
- API code 7、验证页、限流或身份不匹配会明确停止。

`boss status` 是离线检查：它只报告会话文件和关键 Cookie 字段是否存在，不发平台请求，也不输出值。

## 求职者工作流

### 搜索、缓存与本地筛选

```bash
boss preset add rust-backend rust --city 深圳
boss watch add rust-watch rust --city 深圳
boss shortlist add <本地职位ID>

boss campaign policy add rust --include title:rust
boss campaign screen --resume local --policy rust
boss campaign plan ls
```

搜索过滤、预设、监视、短名单、简历、统计、导出和 campaign 都以本地数据为边界。`campaign screen` 只生成 `manual_review` / `dry_run` 计划，不提交简历。

筛选使用明确填写的本地简历字段和已缓存职位字段，并按稳定规则排序。它不会调用通用模型替用户做不可解释的“合适/不合适”判断。

仓库包含两个 Agent Skills：

- [简历筛选](.agents/skills/resume-screening/SKILL.md)
- [申请流程](.agents/skills/resume-application/SKILL.md)

二者都要求人工复核，不获得平台投递权限。

### 单目标会话

```bash
boss chat greet <本地职位ID> --yes
boss chat send <本地职位ID> \
  --message "你好，想进一步了解这个职位" \
  --yes

boss --json chat inbox
boss --json chat inbox <本地职位ID>
```

约束：

- `greet` 与 `send` 只作用于一个已缓存职位；
- `send` 要求已存在同一职位的会话；
- 只接受纯文本，不接受真实 URL、结构化 Markdown 链接或富消息引用；
- 无 ID 的 inbox 只检查最近 3 个缓存职位；
- 显式 ID 查询最多 5 个精确会话；
- 不创建后台任务，不自动回复，不附带简历。

发送结果不会把 MQTT PUBACK 当作交付证明。BossKit 根据权威历史中的精确发送方、目标和文本区分：

```text
already_sent
message_verified
rejected
unverified
```

### 平台原生微信交换

```bash
boss chat exchange-wechat <本地职位ID> --yes
```

这条命令通过本机 ChromeDriver 点击 BOSS 页面原生“换微信”动作。它不发送手机号、文本或简历，也不输出微信号。

默认 ChromeDriver 地址是 `http://127.0.0.1:9515`。需要附加到已登录 Chrome 时，可设置：

```bash
export BOSS_CHROMEDRIVER_DEBUGGER_ADDRESS=127.0.0.1:<端口>
```

只允许本机地址。这个单独的页面动作不改变普通登录仍是纯 CLI Cookie 验证的事实。

## 招聘者工作流

招聘者命令必须显式指定一个保存为 `recruiter` 角色的账户：

```bash
boss --account lty login --role recruiter --cookie-stdin
boss --account lty recruiter replies
boss --account lty recruiter inbox --limit 20 --page 1
```

### 会话和在线简历读取

```bash
boss --account lty recruiter inbox \
  --all \
  --pending \
  --job 'AI应用提效官'

boss --account lty recruiter inbox \
  --job 'AI应用提效官' \
  --page 3 \
  --brief

boss --account lty recruiter resume <UID>
boss --account lty recruiter resumes <UID>... --brief
```

`--all` 最多扫描 3 页，总记录数仍受 `--limit` 限制。`--brief` 输出姓名、UID、期望职位和有限摘要。完整在线简历读取仍是只读、限量、联系方式脱敏、临时返回且不落盘。

### 有界候选人初筛

```bash
boss --account lty recruiter candidates '视频剪辑' \
  --job-id <加密职位ID> \
  --city 101230100 \
  --limit 5
```

当前规则：

- 只查询第 1 页，最多 5 人；
- 本科及以上是本地硬筛；
- 95–00 年只用于软排序；
- 作品、项目经历和学习成长信号可加分；
- 不按性别筛选；
- 不读取联系方式；
- 已有会话者不作为主动招呼目标；
- `--detail` 最多增加一份结构化简历摘要供人工复核。

### 一次招呼一个新候选人

先读取岗位限定的推荐卡：

```bash
boss --account lty recruiter candidates '视频剪辑' \
  --job-id <加密职位ID> \
  --limit 5
```

只有平台明确返回 `haveChatted: false` 的卡才会包含 `greeting` 上下文。然后使用同一张新鲜卡的全部不透明字段：

```bash
boss --account lty recruiter greet \
  --encrypt-geek-id <ID> \
  --security-id <ID> \
  --encrypt-job-id <ID> \
  --expect-id <ID> \
  --lid <ID> \
  --message '你好，想进一步了解你的经历' \
  --yes
```

写入前，BossKit 会重新读取同一岗位推荐列表，并要求：

```text
同一候选人仍存在
AND haveChatted == false
AND 所有不透明字段完全一致
```

任一条件变化都会停止。命令只调用一次平台原生开始聊天动作，不再额外重复发送 MQTT 消息。`api_accepted` 只表示平台接口接受请求，不代表已经独立核验聊天历史交付。

已有会话的单人回复：

```bash
boss --account lty recruiter reply <UID> \
  --message '谢谢，已收到' \
  --yes
```

招聘者路径同样没有批量发送、自动 Offer、定时跟进或自动回复。

## MCP

```bash
boss mcp
```

MCP 使用 `2025-03-26` stdio JSON-RPC，提供严格 schema 的：

- BOSS 职位搜索与详情；
- 本地缓存、筛选、预设、监视与短名单；
- 本地简历和 campaign 人工复核工作流。

MCP 不暴露：

```text
账户选择
登录 / 登出
Cookie 或凭据
招聘者工具
在线简历
聊天写入
微信交换
```

这使 Agent 可以协助整理信息，但不能通过 MCP 获得平台身份或发送权限。

## 输出与配置

默认 Markdown：

```bash
boss search rust --city 深圳
```

显式 JSON：

```bash
boss --json search rust --city 深圳
boss --json --account lty recruiter resume <UID>
```

配置：

```bash
boss config ls
boss config set page_size 30
boss config reset page_size
```

允许持久化的配置键只有：

```text
request_timeout_secs
page_size
```

旧 `platform`、`operating_mode` 和 `log_level` 会被忽略，并在后续写入时移除。旧版非 BOSS 会话不会被选择、暴露或重新写出。

## 明确边界

BossKit 有意不实现：

- 自动申请、简历提交或在线简历修改；
- 批量招呼、群发、自动回复、定时调度；
- 通用模型调用和任意 Webhook；
- 本地关键词自动回复规则；
- 手机号/SMS/浏览器修复登录；
- CAPTCHA 或风控绕过；
- BOSS 直聘之外的平台。

本地文件权限减少意外泄露，但 Cookie 仍是高价值凭据。不要把 Cookie 放入 Issue、日志、截图、命令参数或 Agent 提示词。

平台页面、接口和字段可随时变化。遇到身份验证、限流或未知状态时，正确行为是停止并报告，不是扩大自动化范围。

## 开发

```bash
git clone https://github.com/LIghtJUNction/BossKit.git
cd BossKit
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

代码使用 Rust 2024。命令、行为和边界的完整映射见 [docs/PARITY.md](docs/PARITY.md)。

## License

[MIT](LICENSE)
