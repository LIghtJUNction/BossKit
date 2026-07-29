# BossKit 使用指南

BossKit 是独立维护的 Rust 2024 招聘求职 CLI。可执行文件名为 `boss`，库名为 `bosskit`。

当前可用的 `campaign screen` 只做本地简历筛选并生成 `manual_review` / `dry_run` 计划。BOSS 直聘登录会话可通过纯命令行 HTTPS 与本地 V8 挑战计算刷新并验证，全程不启动或依赖浏览器。`chat greet` 仅对一个本地缓存职位发送平台默认招呼；`chat send` 仅向同一职位的既有精确会话发送一条明确确认的文本；`chat history` 只读取该精确会话最近的文本。批量编排、自动回复和投递仍未开放。

## 安装

```bash
cargo build --release
./target/release/boss --help
```

普通命令向 stdout 输出 JSON 信封；`--help` 和 `--version` 输出文本。命令帮助已本地化：

```bash
boss --help
boss campaign screen --help
```

## 数据目录与配置

数据根目录按以下顺序选择：

1. `BOSS_DATA_DIR`
2. 操作系统本地数据目录下的 `bosskit`
3. 当前目录下 `.boss`

常用配置：

```bash
boss config ls
boss config get page_size
boss config set platform zhipin
boss config set page_size 30
boss config reset page_size
boss config reset
```

| 键 | 默认值 | 可选值 |
| --- | --- | --- |
| `platform` | `all` | `all`, `zhipin`, `zhilian`, `qiancheng` |
| `request_timeout_secs` | `15` | 正整数 |
| `page_size` | `20` | 正整数 |
| `operating_mode` | `assisted` | 当前仅 `assisted` |
| `log_level` | `error` | `error`, `warn`, `info`, `debug` |

## 登录与本地会话

```bash
boss login
boss login --platform zhipin
boss login --platform zhilian --manual
boss status --platform all
boss logout --platform zhipin --yes
```

`login` 先检查运行时环境 Cookie 和已保存的本地会话。对于 BOSS 直聘，命令会通过 HTTPS 验证会话；遇到平台代码 `37` 时，使用 `uv` 临时加载固定版本的 `iv8`、`requests` 与 `paho-mqtt`，在本地 V8 中计算本次挑战并再次验证。整个流程不启动浏览器、不读取浏览器资料，也不会输出 Cookie。首次执行需要已安装 `uv`，并可能下载 Python 运行依赖。其他平台的 `--manual` 只接受终端隐藏输入。

也可在运行时提供：

```bash
export BOSS_ZHIPIN_COOKIE='...'
export BOSS_ZHILIAN_COOKIE='...'
export BOSS_QIANCHENG_COOKIE='...'
```

BossKit 不输出 Cookie，不读取既有浏览器配置、桌面客户端、SQLite 或系统钥匙串。MCP 不暴露登录、登出、浏览器或凭据入口。

## 本地简历

```bash
boss resume init local --title "Rust Engineer"
boss resume set local summary "Rust 后端工程师"
boss resume set local basics.email person@example.test
boss resume skills local --add Rust --add Tokio --add Linux
boss resume show local
boss resume ls
boss resume clone local tailored
boss resume diff local tailored
boss resume export local --output ./local-resume.json
boss resume import ./local-resume.json
boss resume rm tailored --yes
```

简历保存在 `resumes.json`，不会同步到招聘平台。

## 职位搜索与缓存

支持三个只读搜索适配器：

- `zhipin`：BOSS 直聘
- `zhilian`：智联招聘
- `qiancheng`：前程无忧 / 51job

```bash
boss platforms
boss cities
boss search rust --platform all --city 深圳 --page 1 --limit 20
boss search rust --company 示例 --experience 3年 --education 本科 \
  --job-type 全职 --welfare 双休,五险一金
boss ls --platform zhipin --limit 10
boss show <本地职位 ID>
boss detail <本地职位 ID>
boss detail <本地职位 ID> --refresh
boss history --platform zhipin --limit 20
```

搜索结果规范化后写入 `jobs.json`。`--company`、`--salary`、`--experience`、`--education`、`--job-type` 和 `--welfare` 只过滤平台列表响应中已有的字段，不会为过滤自动请求职位详情；福利条件使用 AND 语义。

实时搜索和详情仍受登录状态、频率限制、风险控制、接口及页面变化影响。选择 `all` 时，单个平台失败不会隐藏其他平台的成功结果。

可保存完整搜索参数：

```bash
boss preset add rust-backend rust --platform all --city 深圳
boss search --preset rust-backend --limit 10
boss preset ls
boss preset show rust-backend
boss preset rm rust-backend
```

## 本地简历筛选

先建立本地策略；规则格式为 `field:value`：

```bash
boss campaign policy add rust-remote \
  --include title:rust \
  --include skills:rust \
  --welfare 远程 \
  --monthly-salary-min 20000 \
  --minimum-score 50

boss campaign blacklist add company "不考虑的公司"
boss campaign blacklist add description "外包驻场"
boss campaign template add brief "您好，我关注到 {{company}} 的 {{title}} 职位。"
```

筛选缓存职位：

```bash
boss campaign screen \
  --resume local \
  --policy rust-remote \
  --template brief \
  --limit 20 \
  --minimum-resume-score 40
```

筛选顺序和分数是确定性的：

1. 先应用黑名单。
2. 再应用策略的排除规则、福利、月薪和策略最低分门槛。
3. 只使用简历显式填写的 `title` 与 `skills`，对缓存职位的 `title`、`skills`、`description` 做规范化字面匹配；不从摘要、经历、教育或项目推断能力。
4. 简历分数由标题匹配 50 分和技能覆盖率 50 分组成，默认最低分为 40。
5. 最终分数为简历分数 70% 加策略分数 30%；同分按稳定职位 ID 升序。

结果只写入去重后的本地人工复核计划。计划记录绑定简历名称、更新时间、策略分、简历分、最终分、标题命中状态和命中技能；不会保存简历摘要、经历、教育或项目。问候预览长度受限，明确返回 `sent: false`，只存在于本次响应，不写入 `application_plans.json`。

查看或记录人工状态：

```bash
boss campaign plan ls
boss campaign plan transition <本地职位 ID> approved --yes --note "已人工复核"
boss campaign plan transition <本地职位 ID> recorded_submitted --yes
boss campaign stats
```

`recorded_submitted` 只是用户对外部手动操作的本地记录，不会提交职位或发送消息。

不需要简历评分时，仍可直接按策略生成本地计划：

```bash
boss campaign plan create rust-remote --template brief --resume-name local --limit 20
```

## 文本与消息预览

本地关键词回复只返回建议：

```bash
boss reply add "面试" "感谢您的联系，我会尽快回复。"
boss reply match "您方便参加面试吗？"
boss reply ls
boss reply rm "面试"
```

模板渲染只读取一条缓存职位，不发送：

```bash
boss campaign template render brief <本地职位 ID>
```

## BOSS 直聘纯命令行会话

```bash
boss login --platform zhipin
boss search "AI Agent" --platform zhipin --limit 10
boss chat greet <本地职位 ID> --yes
boss chat send <本地职位 ID> --message "你好，想进一步了解这个职位" --yes
boss chat history <本地职位 ID> --limit 20
```

`login` 只刷新并验证已由环境变量、隐藏手工输入或 BossKit 私有存储提供的 Cookie；它不代替手机号、短信验证码或平台安全验证，也不绕过这些流程。搜索仍是只读操作。

`chat greet` 必须使用 `boss search` 已缓存的精确职位 ID，并逐次提供 `--yes`。它先按职位的 `encryptJobId` 检查既有会话；尚未建立时，才解析同一职位并调用平台默认招呼接口，随后再次按精确职位 ID 验证。该命令不接受自定义消息。

`chat send` 同样要求逐次 `--yes`，并且只接受 1–200 个 Unicode 字符的单行可打印文本。它不会替用户建立新会话：只有好友列表中存在同一 `encryptJobId` 时才准备 MQTT WebSocket；发送前先检查历史中的完全相同自发文本以避免重复，QoS 1 发布一次且不自动重试。Broker 的 PUBACK 只作为传输信号；无论是否及时返回，只有在只读历史中验证到完全相同的本人发出文本才算成功。

`chat history` 不需要 `--yes`，因为它不发送平台消息。它最多返回 20 条按时间升序排列的文本，只包含 `incoming` / `outgoing` 方向、正文和毫秒时间戳；不会返回 Cookie、用户 ID、招聘者身份、加密 Boss ID 或平台授权参数。

`chat greet` 与 `chat send` 的输出不包含消息正文；三个命令都不点击投递入口，也不提交或上传简历。MCP 不暴露任何聊天操作。

本地通知预览不会读取 webhook、联网或创建审计记录：

```bash
boss notify preview campaign.ready
```

`notify send` 是独立的显式确认 webhook 操作，不是招聘平台消息：

```bash
export BOSS_NOTIFY_WEBHOOK_URL='https://notify.example.test/hooks/boss'
boss notify send campaign.ready --yes
```

## 输出、导出与 MCP

```bash
boss export --source jobs --format csv --output ./jobs.csv
boss export --source shortlist --format html --include-ids
boss schema --format native
boss schema --format mcp-tools
boss mcp
```

无 `--output` 时，导出命令只在 JSON 信封中返回结构化数据与格式元数据。已有文件默认拒绝覆盖，必须显式传 `--force`。

MCP stdio 使用共享工具注册表。简历筛选工具为：

```text
campaign_screen {
  resume: string,
  policy: string,
  template?: string,
  limit?: 1..100,
  minimum_resume_score?: 0..100
}
```

它与 CLI 一样只读取本地简历和职位缓存，并写入人工复核 dry-run 计划，不访问招聘平台。

## 其他本地工作流

```bash
boss shortlist add <本地职位 ID> --tags rust,remote --note "优先"
boss shortlist ls --tag rust
boss shortlist compare --tag rust
boss watch add daily --preset rust-backend
boss watch run daily
boss stats --days 30
boss clean --target all
boss clean --target history --yes
```

`watch run` 只在前台显式执行。`clean` 默认预览；Linux 上 `--yes` 将已知活动 JSON 移入数据根目录下的可恢复归档事务，不执行不可恢复删除。

## 技术与授权边界

本地筛选不做反向工程、漏洞利用或反爬绕过，也不产生新的远端交互。它只最小化保存人工复核所需的计划与显式匹配元数据，不保存问候预览或额外远端数据。

任何进一步的生产级招聘平台写操作都必须在平台规则和用户授权范围内实施速率控制与最小化数据留存。当前只有逐次确认、已认证会话下的单目标默认招呼，以及既有精确会话中的单条文本；自动投递、自动或批量消息、回复轮询和自动聊天均未实现。

## 排障

```bash
boss doctor --platform all
boss status --platform all
boss config ls
boss schema --format native
```

常见情况：

- `manual_login_required`：当前不是可交互 TTY，或没有可用的本地 Cookie 来源；在终端重试或使用 `--manual`。
- 搜索或详情失败：检查 `status`，再考虑平台登录、频率限制、风险控制或页面变化。
- `job not found`：先运行搜索或 `boss ls`，确认使用的是本地稳定职位 ID。
- `resume not found` / `policy not found`：分别运行 `boss resume ls`、`boss campaign policy ls`。
- 筛选结果为零：检查黑名单、策略硬门槛、`--minimum-resume-score`，以及 `jobs.json` 是否包含技能或描述字段。
- MCP 参数错误：通过 `boss schema --format mcp-tools` 查看严格 JSON Schema；未知参数会返回 JSON-RPC `-32602`。
