# BossKit

BossKit 是受 [boss-agent-cli](https://github.com/can4hou6joeng4/boss-agent-cli) 启发的 Rust 2024 **进行中重实现**，并非上游项目的完整等价移植。可执行文件名为 `boss`，库名为 `bosskit`；函数和模块使用 Rust/Linux 惯用的 `snake_case` 命名。

当前注册三个真实的只读搜索适配器：

- `zhipin`：BOSS 直聘
- `zhilian`：智联招聘
- `qiancheng`：前程无忧 / 51job

选择 `all` 时会依次搜索三个平台并保留各平台独立结果。实时搜索可能要求登录 Cookie，且会受到频率限制、反爬和页面/API 变化影响；能否成功取决于平台当前控制策略。部分平台失败不会隐藏其他平台成功。

## 命令

```bash
cargo build --release

boss platforms
boss cities
boss search rust --platform all --city 深圳 --page 1 --limit 20
boss search rust --company 示例 --experience 3年 --education 本科 \
  --job-type 全职 --welfare 双休,五险一金
boss preset add rust-backend rust --platform all --city 深圳
boss search --preset rust-backend --limit 10
boss preset ls
boss reply add "面试" "感谢您的联系，我会尽快回复。"
boss reply ls                    # 可见别名：list
boss reply match "您方便参加面试吗？"
boss reply rm "面试"             # 可见别名：remove
boss watch add daily --preset rust-backend
boss watch run daily
boss watch run --all
boss resume init local --title "Rust Engineer"
boss resume set local basics.email person@example.test
boss resume skills local --add Rust --add Linux
boss resume diff local tailored
boss stats --days 30
boss clean --target all             # 仅预览
boss clean --target history --yes   # Linux：将已知 history.json 移入可恢复归档事务
boss ls --platform zhipin --limit 10
boss show <本地职位 ID>
boss detail <本地职位 ID>
boss detail <本地职位 ID> --refresh
boss history --platform zhipin --limit 20
boss export --source jobs --format csv --output ./jobs.csv
boss export --source shortlist --format html --include-ids

boss config ls                 # 可见别名：list
boss config get page_size
boss config set page_size 30
boss config reset page_size
boss config reset

boss login --platform zhipin --credential-file ./zhipin-cookies.txt
boss login                         # 自动尝试全部平台的本地来源；无来源时在 TTY 先显示平台二维码，再打开隔离浏览器
boss login --platform zhilian --manual
boss logout --platform zhipin --yes

boss status --platform all
boss doctor --platform all
boss schema --format native
boss schema --format openai-tools
boss schema --format anthropic-tools
boss schema --format mcp-tools

boss shortlist add <职位 ID> --tags rust,remote --note "优先"
boss shortlist ls --tag rust   # 可见别名：list
boss shortlist annotate <职位 ID> --add-tag priority --remove-tag remote --note "更新"
boss shortlist compare --tag rust
boss shortlist rm <职位 ID>    # 可见别名：remove

boss mcp
```

普通 CLI 结果只向 stdout 写 JSON 信封；`--help` 和 `--version` 使用标准文本输出。`status` 和 `doctor` 都严格只检查本机状态，不访问招聘平台。

搜索的 `--company`、`--salary`、`--experience`、`--education`、`--job-type`
和 `--welfare` 是对平台列表响应中已有字段进行的**本地过滤**。它们不会自动请求每个职位详情，
因此平台列表未提供的福利或描述不能被这些过滤器补齐；福利条件使用 AND 语义。

`boss detail` 使用缓存稳定 ID，按平台读取真实详情并将丰富字段原子更新回缓存。
若缓存已有描述，默认直接返回；`--refresh` 强制重新读取。实时详情仍受登录、风控和页面变化影响，
前程无忧详情只读取公开 HTML 与 JSON-LD，不执行 JavaScript。

## 数据与配置

所有持久化 JSON 共用一个数据根目录，按以下顺序解析：

1. `BOSS_DATA_DIR`
2. 操作系统本地数据目录下的 `bosskit`
3. 当前目录下 `.boss`

文件包括：

- `jobs.json`：搜索得到的规范化职位缓存
- `config.json`：仅保存用户覆盖项
- `shortlist.json`：完整职位快照、去重标签、备注和首次添加时间
- `history.json`：BossKit 本地搜索尝试审计，最多保留最新 200 条
- `presets.json`：完整、已验证的命名搜索规范
- `reply_rules.json`：关键词到建议回复的严格本地规则，按添加顺序保存
- `watches.json`：显式前台监视、完整去重的已见稳定 ID 和最后成功运行时间；不会截断后遗忘旧 ID
- `resumes.json`：单一严格类型的本地简历集合
- `.auth/sessions.json`：仅本机私有的登录 Cookie 和已登记导出文件引用；Unix 下目录为 `0700`、文件为 `0600`
- `.auth/zhipin.cookie`、`.auth/zhilian.cookie`、`.auth/qiancheng.cookie`：可选的、用户放置的默认 Cookie 导出文件；只在 `boss login` 自动尝试中读取
- `.bosskit-clean-archive/<事务>/`：Linux 确认 clean 后保留的同文件系统可恢复归档；不会被后续 clean 或 stats 当作活动数据

写入使用数据根目录内的临时文件和原子替换。配置仅接受以下键，不接受 Cookie、Token、API key 或未知键：

| 键 | 默认值 | 可选值 |
| --- | --- | --- |
| `platform` | `all` | `all`, `zhipin`, `zhilian`, `qiancheng` |
| `request_timeout_secs` | `15` | 正整数 |
| `page_size` | `20` | 正整数 |
| `operating_mode` | `assisted` | 当前仅 `assisted` |
| `log_level` | `error` | `error`, `warn`, `info`, `debug` |

`search` / `ls` 未显式传入 `--platform` 或 `--limit` 时使用配置值；显式参数优先。配置变更在下次进程调用生效。

`boss history` 不是招聘平台的远端浏览历史；它只记录 BossKit 自己完成的搜索尝试、
本地过滤条件以及每个平台的数量或错误码。

`boss reply` 只在本地保存规则并对传入文本进行确定性关键词匹配：ASCII 大小写不敏感的字面子串，
多个规则命中时选择关键词最长的规则，同长度时选择最早保存的规则。`match` 只返回建议文本，
绝不会获取会话、调用平台消息接口或发送平台消息。

`boss export` 完全本地运行。无 `--output` 时，即使请求 CSV/HTML，也只在 JSON 信封中返回
结构化脱敏职位和格式元数据；指定路径时才原子写入对应 JSON、CSV 或 HTML 文件。
现有文件默认拒绝覆盖，必须显式 `--force`。默认保留本地稳定 ID 并省略远端 ID，
`--include-ids` 才同时包含两者。

## 城市与认证

`boss cities` 会诚实列出当前三个适配器共同映射的 10 个逻辑城市：北京、上海、广州、深圳、杭州、成都、武汉、南京、苏州、西安。单平台搜索还可传该平台原生纯数字城市代码。

`boss login` 会先做本地 Cookie 导入和保存；无参数调用会依次尝试全部三个平台，每个平台的自动顺序是：显式 `--credential-file`、环境变量、默认导出文件、已登记的导出文件、已保存会话。`--credential-file` 只允许一个具体平台，并会立即导入且登记该文件，供之后的 `boss login` 再次自动尝试。

若没有来源成功且 stdin 与 stderr 都是 TTY，默认 `boss login` 会先为该平台执行受限的、用户驱动的二维码登录：BOSS 直聘需要手机扫描和第二次确认，智联招聘使用微信二维码并只在获得 `at`、`rt` 会话 Cookie 后成功，前程无忧使用页面 GUID、二维码轮询和受限登录跳转。二维码只以 Unicode 图案写入 stderr；结构化 stdout 仍只输出 JSON，绝不会输出二维码载荷、Cookie、会话令牌、GUID 或响应内容。过期、取消、拒绝、风险提示、验证码/SMS 要求、协议不完整或超时都会停止二维码流程，随后才进入浏览器兜底；BossKit 不会绕过这些检查。`--manual` 保留原有的隐藏 Cookie 粘贴输入，且不会启动二维码或浏览器流程。任一流不是 TTY 时不会创建认证状态、启动浏览器或访问二维码端点，而是返回 `manual_login_required`。

自动导入完全本地；交互式二维码和浏览器兜底会产生用户可见的平台流量，但 BossKit 不会发起额外的 Provider 验证请求。二维码成功结果仍是 `stored_unverified`（顶层标记为 `qr_interactive_provider_unverified`）；浏览器成功时仍标记为 `browser_interactive_provider_unverified`；后续正常的只读搜索请求才会体现 Cookie 是否仍有效。

当前交互式浏览器兜底仅支持 Linux：它依赖 Unix 私有目录权限，并只在 `PATH` 中按固定顺序尝试 `google-chrome`、`google-chrome-stable`、`chromium`、`chromium-browser`、`microsoft-edge`、`microsoft-edge-stable`。可通过 `BOSS_BROWSER=/绝对或可执行文件名` 显式指定兼容 Chromium DevTools Protocol 的浏览器；BossKit 不会打印该值，也不会自动搜索 macOS 或 Windows 的浏览器路径。

默认导出文件固定在最终数据根目录（下文以 `<BOSS_DATA_DIR>` 表示）中：

- `<BOSS_DATA_DIR>/.auth/zhipin.cookie`
- `<BOSS_DATA_DIR>/.auth/zhilian.cookie`
- `<BOSS_DATA_DIR>/.auth/qiancheng.cookie`

未设置 `BOSS_DATA_DIR` 时，`<BOSS_DATA_DIR>` 指前述规则解析出的实际数据根目录。默认文件不会被登记为自定义来源；缺失、权限不安全或格式无效时会静默跳过并继续后续自动来源。

```bash
chmod 600 ./zhipin-cookies.txt
boss login --platform zhipin --credential-file ./zhipin-cookies.txt
boss login --platform zhipin --manual     # 仅 TTY，隐藏粘贴 Cookie；不会打开浏览器
boss login                                # 依次尝试全部三平台；无本地来源时先显示二维码、失败后由用户在隔离浏览器完成登录
boss logout --platform zhipin --yes       # 只撤销本地会话和文件引用，不删除原导出文件
```

可直接导入的仅是用户**明确导出并指定**的普通文件，且 Unix 下必须是当前用户拥有、非符号链接、常规文件、最大 64 KiB，并且不对组或其他用户开放。支持的格式是：原始 `Cookie:` 头/纯 Cookie 文本、按当前平台域名筛选的 Netscape Cookie 导出，以及受限 JSON（`{"cookie":"..."}` 或包含 `domain`、`name`、`value` 的 `cookies` 数组）。

BossKit **不会**扫描、遍历、读取、解密或破解桌面客户端、既有浏览器配置、SQLite、系统钥匙串或其他私有凭据库；它也不会输出 Cookie 值、导出文件路径、浏览器可执行路径或 DevTools 地址，亦不会将这些操作暴露给 MCP。交互式兜底只启动 BossKit 在私有 `.auth` 目录下创建的全新临时浏览器配置，不自动填充凭据、不绕过扫码或验证码，并在浏览器退出或被终止后清理。

仍可使用环境变量；环境 Cookie 在正常 Provider 请求和 `boss login` 自动尝试中优先于本地保存会话：

```bash
export BOSS_ZHIPIN_COOKIE='...'
export BOSS_ZHILIAN_COOKIE='...'
export BOSS_QIANCHENG_COOKIE='...'
```

`status` 和 `doctor` 只输出环境变量名、会话/导出引用是否存在以及安全状态类别，从不输出 Cookie、文件路径或内容。输出错误也会脱敏。

## MCP

MCP 客户端命令为 `boss mcp`。stdio 服务支持 MCP `2025-03-26` 的 `initialize`、通知、`ping`、批处理、`tools/list` 和 `tools/call`。stdout 仅输出协议帧。

工具注册表与 `boss schema --format mcp-tools` 共用同一来源，当前工具为：

- `platforms`, `cities`, `search_jobs`, `list_jobs`, `show_job`, `job_detail`
- `search_history`, `export_jobs`
- `status`, `doctor`, `schema`
- `shortlist_add`, `shortlist_list`, `shortlist_annotate`, `shortlist_remove`, `shortlist_compare`
- `preset_add`, `preset_list`, `preset_show`, `preset_remove`
- `watch_add`, `watch_list`, `watch_show`, `watch_run`, `watch_remove`
- `resume_init`, `resume_list`, `resume_show`, `resume_set`, `resume_skills`, `resume_clone`, `resume_diff`, `resume_remove`
- `keyword_reply_add`, `keyword_reply_list`, `keyword_reply_remove`, `keyword_reply_match`
- `stats`, `clean_preview`

`export_jobs` 只返回结构化数据，不接受输出路径，也不会通过 MCP 写文件。
`login` 与 `logout` 特意仅保留在本机 CLI，MCP 没有对应工具、参数或凭据文件入口。
`clean_preview` 永远只预览，MCP 不会移动或移除文件。CLI 的确认 clean 仅在 Linux 可用：
它把七个已知活动 JSON 文件原子移动到 `.bosskit-clean-archive/<事务>/`，返回每个恢复路径，
不执行 unlink。若并发写入阻止错误回滚，文件会移动到数据根目录下经过验证的私有
`.bosskit-clean-recovery-<事务>/`，错误仅报告验证成功的恢复路径；其他平台仅支持预览。
监视仅在显式调用 `watch run` 时顺序执行只读搜索，
没有后台调度。简历功能只管理 `resumes.json` 中的本地类型化文档，不与招聘平台同步。关键词回复
只管理 `reply_rules.json` 并返回本地建议，永不发送平台消息。

参数采用严格 JSON Schema；畸形参数返回 JSON-RPC `-32602`，有效调用中的执行错误才返回 `isError: true`。

## 安全边界与进度

远端能力只读。本项目不实现自动打招呼、投递、聊天、平台简历同步、招聘者个人数据采集或其他招聘平台写操作。当前与上游的证据化差异见 [docs/PARITY.md](docs/PARITY.md)。

## 致谢与许可

设计参考并归因于 [can4hou6joeng4/boss-agent-cli](https://github.com/can4hou6joeng4/boss-agent-cli)。BossKit 自身以 [MIT License](LICENSE) 发布；上游项目仍受其自身许可约束。
