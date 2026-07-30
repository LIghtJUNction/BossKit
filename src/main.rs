use std::num::{NonZeroU32, NonZeroUsize};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use std::path::PathBuf;

use bosskit::auth::ZhipinRole;
use bosskit::campaign::{
    ApplicationPlanState, BlacklistKind, CampaignField, CampaignPolicy, CampaignRule,
    DEFAULT_MINIMUM_RESUME_SCORE,
};
use bosskit::export::{ExportFormat, ExportOptions, ExportSource};
use bosskit::model::ErrorBody;
use bosskit::schema::SchemaFormat;
use bosskit::{BossError, BossService, Envelope, SearchSpecPatch};
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::json;

static JSON_OUTPUT: AtomicBool = AtomicBool::new(false);

fn localize_generated_help(help: &str) -> String {
    [
        (
            "Print this message or the help of the given subcommand(s)",
            "显示当前命令或指定子命令的帮助",
        ),
        ("Usage:", "用法:"),
        ("Commands:", "命令:"),
        ("Arguments:", "参数:"),
        ("Options:", "选项:"),
        ("<COMMAND>", "<命令>"),
        ("[possible values:", "[可选值:"),
        ("[default:", "[默认值:"),
        ("[aliases:", "[别名:"),
        ("[alias:", "[别名:"),
        ("Print help", "显示帮助"),
        ("Print version", "显示版本"),
    ]
    .into_iter()
    .fold(help.to_owned(), |localized, (english, chinese)| {
        localized.replace(english, chinese)
    })
}

#[derive(Parser)]
#[command(
    name = "boss",
    version,
    about = "BossKit — BOSS 直聘命令行求职辅助工具",
    arg_required_else_help = true
)]
struct Cli {
    /// 临时选择本次命令使用的本地账户，不更改默认账户
    #[arg(long, global = true, value_name = "ALIAS")]
    account: Option<String>,
    /// 输出机器可读 JSON；默认输出紧凑 Markdown
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 列出 BOSS 直聘支持的逻辑城市
    Cities,
    /// 搜索公开职位
    Search {
        #[command(flatten)]
        options: SearchOptions,
    },
    /// 管理本地搜索预设
    Preset {
        #[command(subcommand)]
        command: PresetCommand,
    },
    /// 管理只在本地建议文本的关键词回复
    Reply {
        #[command(subcommand)]
        command: ReplyCommand,
    },
    /// 管理仅基于本地缓存的人工复核求职计划
    Campaign {
        #[command(subcommand)]
        command: CampaignCommand,
    },
    /// 管理显式前台职位监视
    Watch {
        #[command(subcommand)]
        command: WatchCommand,
    },
    /// 管理严格本地简历
    Resume {
        #[command(subcommand)]
        command: ResumeCommand,
    },
    /// 管理本地会话账户并只读查看 BOSS 直聘资料；纯命令行且不启动浏览器
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    /// 管理无凭据的本地 AI 配置与明确确认的模型调用
    Ai {
        #[command(subcommand)]
        command: AiCommand,
    },
    /// 预览或明确发送最小化通知 Webhook 摘要
    Notify {
        #[command(subcommand)]
        command: NotifyCommand,
    },
    /// 汇总严格本地工作流统计
    Stats {
        #[arg(long, default_value = "30")]
        days: NonZeroU32,
    },
    /// 预览或确认将已知工作数据归档到可恢复事务目录
    Clean {
        #[arg(long, value_enum)]
        target: CleanTargetArg,
        #[arg(long)]
        yes: bool,
    },
    /// 列出本地缓存职位
    Ls {
        #[arg(long)]
        limit: Option<NonZeroUsize>,
    },
    /// 查看本地缓存职位
    Show { id: String },
    /// 获取并缓存只读职位详情
    Detail {
        id: String,
        #[arg(long)]
        refresh: bool,
    },
    /// 列出 BossKit 本地搜索审计历史
    History {
        #[arg(long, default_value = "20")]
        limit: NonZeroUsize,
    },
    /// 安全导出本地职位或短名单
    Export {
        #[arg(long, value_enum, default_value = "jobs")]
        source: ExportSourceArg,
        #[arg(long, default_value = "20")]
        limit: NonZeroUsize,
        #[arg(long, value_enum, default_value = "json")]
        format: ExportFormatArg,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        include_ids: bool,
        #[arg(long)]
        force: bool,
    },
    /// 管理安全的本地配置
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// 保存本地 Cookie；可选用本地浏览器修复会话后再验证
    Login {
        /// 通过 BOSS 可见登录页输入手机号并填写短信验证码
        #[arg(long, conflicts_with_all = ["manual", "cookie_stdin", "repair"])]
        phone: bool,
        #[arg(long)]
        manual: bool,
        /// 从标准输入读取一个 Cookie；不接受命令行参数中的凭据
        #[arg(short = 'c', long, conflicts_with_all = ["manual", "repair"])]
        cookie_stdin: bool,
        /// 连接已登录的本地 Chrome 会话，人工完成校验后再验证并保存新 Cookie
        #[arg(long, conflicts_with_all = ["manual", "cookie_stdin", "phone"])]
        repair: bool,
        /// BOSS account surface to verify and save
        #[arg(long, value_enum, default_value = "geek")]
        role: RoleArg,
    },
    /// Recruiter review, full online-resume read, and explicit one-at-a-time replies; CLI-only
    Recruiter {
        #[command(subcommand)]
        command: RecruiterCommand,
    },
    /// 对一个本地缓存 BOSS 职位发送平台默认招呼
    Chat {
        #[command(subcommand)]
        command: ChatCommand,
    },
    /// 撤销本地保存的登录会话
    Logout {
        #[arg(long)]
        yes: bool,
    },
    /// 检查本地 Cookie 环境状态，不联网
    Status {},
    /// 运行严格本地诊断，不联网
    Doctor {},
    /// 输出共享能力 Schema
    Schema {
        #[arg(long, value_enum)]
        format: SchemaArg,
    },
    /// 管理本地职位短名单
    Shortlist {
        #[command(subcommand)]
        command: ShortlistCommand,
    },
    /// 启动 MCP stdio 服务
    Mcp,
}

#[derive(Subcommand)]
enum ChatCommand {
    /// 建立一个职位会话；不发送自定义消息或简历
    Greet {
        /// 本地缓存职位 ID
        job_id: String,
        /// 明确确认本次平台写操作
        #[arg(long)]
        yes: bool,
    },
    /// 向已建立的精确职位会话发送一条文本；不发送简历
    Send {
        /// 本地缓存职位 ID
        job_id: String,
        /// 单行可打印文本，最多 200 个 Unicode 字符
        #[arg(long)]
        message: String,
        /// 明确确认本次平台写操作
        #[arg(long)]
        yes: bool,
    },
    /// 通过 BOSS 原生聊天界面请求交换微信；不发送手机号、消息或简历
    ExchangeWechat {
        /// 本地缓存职位 ID
        job_id: String,
        /// 明确确认本次平台写操作
        #[arg(long)]
        yes: bool,
    },
    /// 读取既有精确职位会话的最近文本；不发送消息或简历
    History {
        /// 本地缓存职位 ID
        job_id: String,
        /// 最多返回的最近文本数
        #[arg(long, default_value = "20")]
        limit: NonZeroUsize,
    },
    /// 批量读取既有精确职位会话的最新文本；不轮询、回复或投递简历
    Inbox {
        /// 可选的本地缓存职位 ID；省略时只扫描最近 3 个缓存职位的既有会话
        #[arg(num_args = 0..=5, value_name = "LOCAL_JOB_ID")]
        job_ids: Vec<String>,
    },
}

#[derive(Subcommand)]
enum RecruiterCommand {
    /// List bounded redacted candidate reply states from the recruiter friend list
    Replies {
        #[arg(long, default_value_t = 20, value_parser = parse_recruiter_limit)]
        limit: usize,
        #[arg(long, default_value_t = 1, value_parser = parse_recruiter_page)]
        page: usize,
    },
    /// Read exact recruiter conversations and the latest safe text
    Inbox {
        #[arg(long, default_value_t = 20, value_parser = parse_recruiter_limit)]
        limit: usize,
        #[arg(long, value_parser = parse_recruiter_page, conflicts_with = "all")]
        page: Option<usize>,
        /// Scan all recruiter pages in one native CLI operation
        #[arg(long, conflicts_with = "page")]
        all: bool,
        /// Keep only conversations whose latest message is from the candidate
        #[arg(long)]
        pending: bool,
        /// Keep jobs whose title contains this text
        #[arg(long, value_name = "TEXT")]
        job: Option<String>,
        /// Return only name, UID, and the latest safe message for quick candidate review
        #[arg(long)]
        brief: bool,
    },
    /// Read one exact candidate's full recruiter-side online resume
    Resume {
        /// Numeric candidate uid from `boss recruiter inbox`
        uid: String,
    },
    /// Read several exact candidate resumes serially; no messages or writes
    Resumes {
        /// Numeric candidate UIDs from `boss recruiter inbox` (maximum 10)
        #[arg(required = true, num_args = 1..=10, value_name = "UID")]
        uids: Vec<String>,
        /// Return only uid, name, expected positions, summary, and projects
        #[arg(long)]
        brief: bool,
    },
    /// Send one explicitly confirmed recruiter follow-up to an exact candidate
    Reply {
        /// Numeric candidate uid from `boss recruiter inbox`
        uid: String,
        /// One printable single-line follow-up, at most 200 characters
        #[arg(long)]
        message: String,
        /// Confirm this external write
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum RoleArg {
    Geek,
    Recruiter,
}

impl From<RoleArg> for ZhipinRole {
    fn from(value: RoleArg) -> Self {
        match value {
            RoleArg::Geek => Self::Geek,
            RoleArg::Recruiter => Self::Recruiter,
        }
    }
}

fn parse_recruiter_limit(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| "limit must be an integer".to_owned())?;
    if (1..=20).contains(&parsed) {
        Ok(parsed)
    } else {
        Err("limit must be between 1 and 20".to_owned())
    }
}

fn parse_recruiter_page(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| "page must be an integer".to_owned())?;
    if (1..=50).contains(&parsed) {
        Ok(parsed)
    } else {
        Err("page must be between 1 and 50".to_owned())
    }
}

#[derive(Subcommand)]
enum AccountCommand {
    /// 列出安全的本地账户元数据；不显示凭据或路径
    #[command(alias = "ls")]
    List,
    /// 创建或选择后续命令使用的默认本地账户
    Use {
        /// 账户别名
        alias: String,
        /// 明确确认本次本地账户写操作
        #[arg(long)]
        yes: bool,
    },
    /// 只读查看 BOSS 直聘在线简历；不修改或投递
    Resume {
        #[command(subcommand)]
        command: AccountResumeCommand,
    },
}

#[derive(Subcommand)]
enum AccountResumeCommand {
    /// 通过纯命令行只读获取在线简历快照；不启动浏览器、不修改或投递
    Show,
}

#[derive(Clone, Args)]
struct SearchOptions {
    query: Option<String>,
    #[arg(long)]
    preset: Option<String>,
    #[arg(long)]
    city: Option<String>,
    #[arg(long)]
    page: Option<NonZeroU32>,
    #[arg(long)]
    limit: Option<NonZeroU32>,
    #[arg(long)]
    company: Option<String>,
    #[arg(long)]
    salary: Option<String>,
    #[arg(long)]
    experience: Option<String>,
    #[arg(long)]
    education: Option<String>,
    #[arg(long = "job-type")]
    employment_type: Option<String>,
    #[arg(long, value_delimiter = ',')]
    welfare: Vec<String>,
}

#[derive(Subcommand)]
enum PresetCommand {
    /// 添加或更新预设
    Add {
        name: String,
        query: String,
        #[arg(long)]
        city: Option<String>,
        #[command(flatten)]
        flags: Box<SearchFlags>,
    },
    /// 列出预设
    #[command(visible_alias = "list")]
    Ls,
    /// 查看预设
    Show { name: String },
    /// 删除预设
    #[command(visible_alias = "remove")]
    Rm { name: String },
}

#[derive(Subcommand)]
enum ReplyCommand {
    /// 添加或更新本地关键词回复
    Add { keyword: String, reply: String },
    /// 列出本地关键词回复
    #[command(visible_alias = "list")]
    Ls,
    /// 移除本地关键词回复
    #[command(visible_alias = "remove")]
    Rm { keyword: String },
    /// 按本地消息文本返回建议，不发送平台消息
    Match { message: String },
}

#[derive(Subcommand)]
enum CampaignCommand {
    /// 管理可复用的本地筛选策略
    Policy {
        #[command(subcommand)]
        command: CampaignPolicyCommand,
    },
    /// 管理本地公司、缓存职位描述或职位黑名单
    Blacklist {
        #[command(subcommand)]
        command: CampaignBlacklistCommand,
    },
    /// 管理只渲染不发送的问候模板
    Template {
        #[command(subcommand)]
        command: CampaignTemplateCommand,
    },
    /// 生成或查看人工复核 dry-run 计划
    Plan {
        #[command(subcommand)]
        command: CampaignPlanCommand,
    },
    /// 按本地简历和策略筛选缓存职位，仅生成按分数排序的人工复核计划
    Screen {
        /// 现有本地类型化简历名称
        #[arg(long)]
        resume: String,
        /// 现有本地筛选策略名称
        #[arg(long)]
        policy: String,
        /// 可选的本地问候预览模板
        #[arg(long)]
        template: Option<String>,
        /// 本次最多创建的去重计划数
        #[arg(long, default_value = "20")]
        limit: NonZeroUsize,
        /// 简历标题与技能的最低本地匹配分数
        #[arg(
            long,
            default_value_t = DEFAULT_MINIMUM_RESUME_SCORE,
            value_parser = clap::value_parser!(u8).range(0..=100)
        )]
        minimum_resume_score: u8,
    },
    /// 汇总本地活动数据
    Stats,
}

#[derive(Subcommand)]
enum CampaignPolicyCommand {
    /// 添加或更新策略；规则格式为 field:value
    Add {
        name: String,
        #[arg(long = "include")]
        include: Vec<String>,
        #[arg(long = "exclude")]
        exclude: Vec<String>,
        #[arg(long = "welfare", value_delimiter = ',')]
        required_welfare: Vec<String>,
        #[arg(long = "monthly-salary-min")]
        monthly_salary_min: Option<NonZeroU32>,
        #[arg(long = "monthly-salary-max")]
        monthly_salary_max: Option<NonZeroU32>,
        #[arg(long = "minimum-score")]
        minimum_score: Option<u8>,
    },
    /// 列出策略
    #[command(visible_alias = "list")]
    Ls,
    /// 查看策略
    Show { name: String },
    /// 移除策略
    #[command(visible_alias = "remove")]
    Rm { name: String },
}

#[derive(Subcommand)]
enum CampaignBlacklistCommand {
    /// 添加本地黑名单规则
    Add {
        kind: BlacklistKindArg,
        value: String,
    },
    /// 列出本地黑名单规则
    #[command(visible_alias = "list")]
    Ls,
    /// 移除本地黑名单规则
    #[command(visible_alias = "remove")]
    Rm {
        kind: BlacklistKindArg,
        value: String,
    },
}

#[derive(Subcommand)]
enum CampaignTemplateCommand {
    /// 添加或更新允许占位符的本地模板
    Add { name: String, body: String },
    /// 列出本地模板
    #[command(visible_alias = "list")]
    Ls,
    /// 查看模板
    Show { name: String },
    /// 移除模板
    #[command(visible_alias = "remove")]
    Rm { name: String },
    /// 使用一条缓存职位本地渲染模板，不发送
    Render { name: String, job_id: String },
}

#[derive(Subcommand)]
enum CampaignPlanCommand {
    /// 从缓存职位建立人工复核 dry-run 计划
    Create {
        policy: String,
        #[arg(long)]
        template: Option<String>,
        /// 绑定一份现有的本地类型化简历，不将其内容复制到计划中
        #[arg(long)]
        resume_name: Option<String>,
        #[arg(long, default_value = "20")]
        limit: NonZeroUsize,
    },
    /// 列出现有人工复核计划
    #[command(visible_alias = "list")]
    Ls,
    /// 记录一项已确认的本地人工工作流状态变化，不会提交到平台
    Transition {
        job_id: String,
        state: CampaignPlanStateArg,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Clone, Args)]
struct SearchFlags {
    #[arg(long)]
    page: Option<NonZeroU32>,
    #[arg(long)]
    limit: Option<NonZeroU32>,
    #[arg(long)]
    company: Option<String>,
    #[arg(long)]
    salary: Option<String>,
    #[arg(long)]
    experience: Option<String>,
    #[arg(long)]
    education: Option<String>,
    #[arg(long = "job-type")]
    employment_type: Option<String>,
    #[arg(long, value_delimiter = ',')]
    welfare: Vec<String>,
}

#[derive(Subcommand)]
enum WatchCommand {
    /// 添加或更新前台监视
    Add {
        name: String,
        query: Option<String>,
        #[arg(long)]
        preset: Option<String>,
        #[arg(long)]
        city: Option<String>,
        #[command(flatten)]
        flags: Box<SearchFlags>,
    },
    /// 列出监视
    #[command(visible_alias = "list")]
    Ls,
    /// 查看监视
    Show { name: String },
    /// 删除监视
    #[command(visible_alias = "remove")]
    Rm { name: String },
    /// 显式运行一个或全部监视
    Run {
        name: Option<String>,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum ResumeCommand {
    /// 初始化本地简历
    Init {
        name: String,
        #[arg(long)]
        title: Option<String>,
    },
    /// 列出本地简历
    #[command(visible_alias = "list")]
    Ls,
    /// 查看本地简历
    Show { name: String },
    /// 设置允许的本地简历字段
    Set {
        name: String,
        field: String,
        value: String,
    },
    /// 管理本地简历技能
    Skills {
        name: String,
        #[arg(long = "add")]
        add: Vec<String>,
        #[arg(long = "remove")]
        remove: Vec<String>,
    },
    /// 克隆本地简历
    Clone { name: String, new_name: String },
    /// 对比本地简历
    Diff { left: String, right: String },
    /// 导入严格 JSON 简历
    Import {
        path: PathBuf,
        #[arg(long)]
        force: bool,
    },
    /// 导出严格 JSON 简历
    Export {
        name: String,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
    /// 删除本地简历
    #[command(visible_alias = "remove")]
    Rm {
        name: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum AiCommand {
    /// 添加或更新无凭据的 HTTPS OpenAI 兼容配置
    Profile {
        #[command(subcommand)]
        command: AiProfileCommand,
    },
    /// 使用一个缓存职位和一份本地类型化简历生成 AI 文稿
    Draft {
        profile: String,
        job_id: String,
        resume_name: String,
        #[arg(long)]
        yes: bool,
    },
    /// 根据一份本地类型化简历评估一个缓存职位
    Score {
        profile: String,
        job_id: String,
        resume_name: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum AiProfileCommand {
    /// 仅添加或更新配置元数据；不接受或存储密钥
    Add {
        name: String,
        base_url: String,
        model: String,
    },
    /// 列出本地配置
    #[command(visible_alias = "list")]
    Ls,
    /// 查看一个本地配置
    Show { name: String },
    /// 移除一个本地配置
    #[command(visible_alias = "remove")]
    Rm { name: String },
}

#[derive(Subcommand)]
enum NotifyCommand {
    /// 仅渲染受限的本地载荷；不会读取 Webhook 或访问网络
    Preview { event: String },
    /// 明确确认后，将受限载荷发送到仅运行时提供的 Webhook
    Send {
        event: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// 列出所有安全配置
    #[command(visible_alias = "list")]
    Ls,
    /// 读取一个配置
    Get { key: String },
    /// 设置一个用户覆盖值
    Set { key: String, value: String },
    /// 重置一个配置或全部配置
    Reset { key: Option<String> },
}

#[derive(Subcommand)]
enum ShortlistCommand {
    /// 添加缓存职位
    Add {
        job_id: String,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// 列出短名单
    #[command(visible_alias = "list")]
    Ls {
        #[arg(long)]
        tag: Option<String>,
    },
    /// 更新标签或备注
    Annotate {
        job_id: String,
        #[arg(long = "add-tag")]
        add_tags: Vec<String>,
        #[arg(long = "remove-tag")]
        remove_tags: Vec<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// 移除短名单项目
    #[command(visible_alias = "remove")]
    Rm { job_id: String },
    /// 比较短名单项目
    Compare {
        #[arg(long)]
        tag: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum BlacklistKindArg {
    Company,
    Description,
    Job,
}

impl From<BlacklistKindArg> for BlacklistKind {
    fn from(value: BlacklistKindArg) -> Self {
        match value {
            BlacklistKindArg::Company => Self::Company,
            BlacklistKindArg::Description => Self::Description,
            BlacklistKindArg::Job => Self::Job,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum CleanTargetArg {
    Jobs,
    History,
    Shortlist,
    Presets,
    #[value(name = "reply_rules", alias = "reply-rules")]
    ReplyRules,
    Watches,
    Resumes,
    #[value(name = "campaign_policies", alias = "campaign-policies")]
    CampaignPolicies,
    #[value(name = "campaign_blacklist", alias = "campaign-blacklist")]
    CampaignBlacklist,
    #[value(name = "greeting_templates", alias = "greeting-templates")]
    GreetingTemplates,
    #[value(name = "application_plans", alias = "application-plans")]
    ApplicationPlans,
    #[value(name = "ai_profiles", alias = "ai-profiles")]
    AiProfiles,
    #[value(name = "notification_audit", alias = "notification-audit")]
    NotificationAudit,
    All,
}

impl CleanTargetArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Jobs => "jobs",
            Self::History => "history",
            Self::Shortlist => "shortlist",
            Self::Presets => "presets",
            Self::ReplyRules => "reply_rules",
            Self::Watches => "watches",
            Self::Resumes => "resumes",
            Self::CampaignPolicies => "campaign_policies",
            Self::CampaignBlacklist => "campaign_blacklist",
            Self::GreetingTemplates => "greeting_templates",
            Self::ApplicationPlans => "application_plans",
            Self::AiProfiles => "ai_profiles",
            Self::NotificationAudit => "notification_audit",
            Self::All => "all",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum CampaignPlanStateArg {
    Approved,
    Rejected,
    #[value(name = "recorded_submitted", alias = "recorded-submitted")]
    RecordedSubmitted,
}

impl From<CampaignPlanStateArg> for ApplicationPlanState {
    fn from(value: CampaignPlanStateArg) -> Self {
        match value {
            CampaignPlanStateArg::Approved => Self::Approved,
            CampaignPlanStateArg::Rejected => Self::Rejected,
            CampaignPlanStateArg::RecordedSubmitted => Self::RecordedSubmitted,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum SchemaArg {
    Native,
    OpenaiTools,
    AnthropicTools,
    McpTools,
}

#[derive(Clone, Copy, ValueEnum)]
enum ExportSourceArg {
    Jobs,
    Shortlist,
}

impl From<ExportSourceArg> for ExportSource {
    fn from(value: ExportSourceArg) -> Self {
        match value {
            ExportSourceArg::Jobs => Self::Jobs,
            ExportSourceArg::Shortlist => Self::Shortlist,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ExportFormatArg {
    Json,
    Csv,
    Html,
}

impl From<ExportFormatArg> for ExportFormat {
    fn from(value: ExportFormatArg) -> Self {
        match value {
            ExportFormatArg::Json => Self::Json,
            ExportFormatArg::Csv => Self::Csv,
            ExportFormatArg::Html => Self::Html,
        }
    }
}

impl From<SchemaArg> for SchemaFormat {
    fn from(value: SchemaArg) -> Self {
        match value {
            SchemaArg::Native => Self::Native,
            SchemaArg::OpenaiTools => Self::OpenaiTools,
            SchemaArg::AnthropicTools => Self::AnthropicTools,
            SchemaArg::McpTools => Self::McpTools,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    JSON_OUTPUT.store(
        std::env::args().any(|argument| argument == "--json"),
        Ordering::Relaxed,
    );
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp
                    | ErrorKind::DisplayVersion
                    | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            ) =>
        {
            print!("{}", localize_generated_help(&error.to_string()));
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            print_json(&Envelope::<serde_json::Value>::failure(
                &BossError::InvalidArgument(error.to_string()),
                None,
                Vec::new(),
            ));
            return ExitCode::FAILURE;
        }
    };
    match run(cli).await {
        Ok(code) => code,
        Err(error) => {
            print_json(&Envelope::<serde_json::Value>::failure(
                &error,
                None,
                Vec::new(),
            ));
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode, BossError> {
    JSON_OUTPUT.store(cli.json, Ordering::Relaxed);
    if matches!(&cli.command, Command::Doctor {}) {
        print_json(&Envelope::success(BossService::doctor_local()));
        return Ok(ExitCode::SUCCESS);
    }
    if matches!(&cli.command, Command::Recruiter { .. }) && cli.account.is_none() {
        return Err(BossError::InvalidArgument(
            "recruiter commands require explicit --account <recruiter alias>".to_owned(),
        ));
    }
    if let Command::Chat { command } = &cli.command {
        match command {
            ChatCommand::Greet { yes: false, .. } => {
                return Err(BossError::InvalidArgument(
                    "chat greet requires --yes".to_owned(),
                ));
            }
            ChatCommand::Send { yes: false, .. } => {
                return Err(BossError::InvalidArgument(
                    "chat send requires --yes".to_owned(),
                ));
            }
            ChatCommand::ExchangeWechat { yes: false, .. } => {
                return Err(BossError::InvalidArgument(
                    "chat exchange-wechat requires --yes".to_owned(),
                ));
            }
            ChatCommand::Greet { yes: true, .. }
            | ChatCommand::Send { yes: true, .. }
            | ChatCommand::ExchangeWechat { yes: true, .. }
            | ChatCommand::History { .. }
            | ChatCommand::Inbox { .. } => {}
        }
    }
    let mut service = BossService::discover_for_account(cli.account.as_deref())?;
    match cli.command {
        Command::Cities => print_json(&Envelope::success(service.cities())),
        Command::Search { options } => {
            let preset = options.preset.clone();
            let spec =
                service.resolve_search_spec(preset.as_deref(), patch_from_options(options))?;
            let report = service.search_spec(spec).await?;
            if !print_search_report(report) {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Preset { command } => match command {
            PresetCommand::Add {
                name,
                query,
                city,
                flags,
            } => {
                let spec = service
                    .resolve_search_spec(None, patch_from_flags(Some(query), city, *flags))?;
                print_json(&Envelope::success(service.preset_add(&name, spec)?));
            }
            PresetCommand::Ls => print_json(&Envelope::success(service.preset_list()?)),
            PresetCommand::Show { name } => {
                print_json(&Envelope::success(service.preset_show(&name)?));
            }
            PresetCommand::Rm { name } => {
                print_json(&Envelope::success(service.preset_remove(&name)?));
            }
        },
        Command::Reply { command } => match command {
            ReplyCommand::Add { keyword, reply } => {
                print_json(&Envelope::success(service.reply_add(&keyword, &reply)?));
            }
            ReplyCommand::Ls => print_json(&Envelope::success(service.reply_list()?)),
            ReplyCommand::Rm { keyword } => {
                print_json(&Envelope::success(service.reply_remove(&keyword)?));
            }
            ReplyCommand::Match { message } => {
                print_json(&Envelope::success(service.reply_match(&message)?));
            }
        },
        Command::Campaign { command } => match command {
            CampaignCommand::Policy { command } => match command {
                CampaignPolicyCommand::Add {
                    name,
                    include,
                    exclude,
                    required_welfare,
                    monthly_salary_min,
                    monthly_salary_max,
                    minimum_score,
                } => {
                    let policy = CampaignPolicy {
                        name,
                        include: parse_campaign_rules(include, "include")?,
                        exclude: parse_campaign_rules(exclude, "exclude")?,
                        required_welfare,
                        monthly_salary_min: monthly_salary_min.map(NonZeroU32::get),
                        monthly_salary_max: monthly_salary_max.map(NonZeroU32::get),
                        minimum_score,
                    };
                    print_json(&Envelope::success(service.campaign_policy_add(policy)?));
                }
                CampaignPolicyCommand::Ls => {
                    print_json(&Envelope::success(service.campaign_policy_list()?));
                }
                CampaignPolicyCommand::Show { name } => {
                    print_json(&Envelope::success(service.campaign_policy_show(&name)?));
                }
                CampaignPolicyCommand::Rm { name } => {
                    print_json(&Envelope::success(service.campaign_policy_remove(&name)?));
                }
            },
            CampaignCommand::Blacklist { command } => match command {
                CampaignBlacklistCommand::Add { kind, value } => {
                    print_json(&Envelope::success(
                        service.campaign_blacklist_add(kind.into(), &value)?,
                    ));
                }
                CampaignBlacklistCommand::Ls => {
                    print_json(&Envelope::success(service.campaign_blacklist_list()?));
                }
                CampaignBlacklistCommand::Rm { kind, value } => {
                    print_json(&Envelope::success(
                        service.campaign_blacklist_remove(kind.into(), &value)?,
                    ));
                }
            },
            CampaignCommand::Template { command } => match command {
                CampaignTemplateCommand::Add { name, body } => {
                    print_json(&Envelope::success(
                        service.campaign_template_add(&name, &body)?,
                    ));
                }
                CampaignTemplateCommand::Ls => {
                    print_json(&Envelope::success(service.campaign_template_list()?));
                }
                CampaignTemplateCommand::Show { name } => {
                    print_json(&Envelope::success(service.campaign_template_show(&name)?));
                }
                CampaignTemplateCommand::Rm { name } => {
                    print_json(&Envelope::success(service.campaign_template_remove(&name)?));
                }
                CampaignTemplateCommand::Render { name, job_id } => {
                    print_json(&Envelope::success(json!({
                        "mode":"local_render_only",
                        "sent":false,
                        "text":service.campaign_template_render(&name, &job_id)?
                    })));
                }
            },
            CampaignCommand::Plan { command } => match command {
                CampaignPlanCommand::Create {
                    policy,
                    template,
                    resume_name,
                    limit,
                } => {
                    print_json(&Envelope::success(service.campaign_plan_create(
                        &policy,
                        template.as_deref(),
                        resume_name.as_deref(),
                        limit.get(),
                    )?));
                }
                CampaignPlanCommand::Ls => {
                    print_json(&Envelope::success(service.campaign_plan_list()?));
                }
                CampaignPlanCommand::Transition {
                    job_id,
                    state,
                    yes,
                    note,
                } => {
                    if !yes {
                        return Err(BossError::InvalidArgument(
                            "campaign plan transition requires --yes".to_owned(),
                        ));
                    }
                    print_json(&Envelope::success(service.campaign_plan_transition(
                        &job_id,
                        state.into(),
                        note,
                    )?));
                }
            },
            CampaignCommand::Screen {
                resume,
                policy,
                template,
                limit,
                minimum_resume_score,
            } => {
                print_json(&Envelope::success(service.campaign_screen(
                    &resume,
                    &policy,
                    template.as_deref(),
                    limit.get(),
                    minimum_resume_score,
                )?));
            }
            CampaignCommand::Stats => {
                print_json(&Envelope::success(service.campaign_stats()?));
            }
        },
        Command::Watch { command } => match command {
            WatchCommand::Add {
                name,
                query,
                preset,
                city,
                flags,
            } => {
                if query.is_some() == preset.is_some() {
                    return Err(BossError::InvalidArgument(
                        "watch add requires exactly one of query or --preset".to_owned(),
                    ));
                }
                let spec = service.resolve_search_spec(
                    preset.as_deref(),
                    patch_from_flags(query, city, *flags),
                )?;
                print_json(&Envelope::success(service.watch_add(&name, spec)?));
            }
            WatchCommand::Ls => print_json(&Envelope::success(service.watch_list()?)),
            WatchCommand::Show { name } => {
                print_json(&Envelope::success(service.watch_show(&name)?));
            }
            WatchCommand::Rm { name } => {
                print_json(&Envelope::success(service.watch_remove(&name)?));
            }
            WatchCommand::Run { name, all } => match (name, all) {
                (Some(name), false) => {
                    print_json(&Envelope::success(service.watch_run(&name).await?));
                }
                (None, true) => {
                    print_json(&Envelope::success(service.watch_run_all().await?));
                }
                _ => {
                    return Err(BossError::InvalidArgument(
                        "watch run requires a name or --all, but not both".to_owned(),
                    ));
                }
            },
        },
        Command::Resume { command } => match command {
            ResumeCommand::Init { name, title } => {
                print_json(&Envelope::success(service.resume_init(&name, title)?));
            }
            ResumeCommand::Ls => print_json(&Envelope::success(service.resume_list()?)),
            ResumeCommand::Show { name } => {
                print_json(&Envelope::success(service.resume_show(&name)?));
            }
            ResumeCommand::Set { name, field, value } => print_json(&Envelope::success(
                service.resume_set(&name, &field, value)?,
            )),
            ResumeCommand::Skills { name, add, remove } => print_json(&Envelope::success(
                service.resume_skills(&name, add, remove)?,
            )),
            ResumeCommand::Clone { name, new_name } => {
                print_json(&Envelope::success(service.resume_clone(&name, &new_name)?));
            }
            ResumeCommand::Diff { left, right } => {
                print_json(&Envelope::success(service.resume_diff(&left, &right)?));
            }
            ResumeCommand::Import { path, force } => {
                print_json(&Envelope::success(service.resume_import(&path, force)?));
            }
            ResumeCommand::Export {
                name,
                output,
                force,
            } => print_json(&Envelope::success(service.resume_export(
                &name,
                output.as_deref(),
                force,
            )?)),
            ResumeCommand::Rm { name, yes } => {
                print_json(&Envelope::success(service.resume_remove(&name, yes)?));
            }
        },
        Command::Account { command } => match command {
            AccountCommand::List => {
                print_json(&Envelope::success(service.account_list()));
            }
            AccountCommand::Use { alias, yes } => {
                print_json(&Envelope::success(service.account_use(&alias, yes)?));
            }
            AccountCommand::Resume { command } => match command {
                AccountResumeCommand::Show => {
                    print_json(&Envelope::success(service.account_resume_show()?));
                }
            },
        },
        Command::Ai { command } => match command {
            AiCommand::Profile { command } => match command {
                AiProfileCommand::Add {
                    name,
                    base_url,
                    model,
                } => print_json(&Envelope::success(
                    service.ai_profile_add(&name, &base_url, &model)?,
                )),
                AiProfileCommand::Ls => {
                    print_json(&Envelope::success(service.ai_profile_list()?));
                }
                AiProfileCommand::Show { name } => {
                    print_json(&Envelope::success(service.ai_profile_show(&name)?));
                }
                AiProfileCommand::Rm { name } => {
                    print_json(&Envelope::success(service.ai_profile_remove(&name)?));
                }
            },
            AiCommand::Draft {
                profile,
                job_id,
                resume_name,
                yes,
            } => print_json(&Envelope::success(json!({
                "mode":"confirmed_ai_draft",
                "text":service.ai_draft(&profile, &job_id, &resume_name, yes).await?
            }))),
            AiCommand::Score {
                profile,
                job_id,
                resume_name,
                yes,
            } => print_json(&Envelope::success(json!({
                "mode":"confirmed_ai_score",
                "score":service.ai_score(&profile, &job_id, &resume_name, yes).await?
            }))),
        },
        Command::Notify { command } => match command {
            NotifyCommand::Preview { event } => {
                print_json(&Envelope::success(json!({
                    "mode":"local_notification_preview",
                    "sent":false,
                    "payload":service.notification_preview(&event)?
                })));
            }
            NotifyCommand::Send { event, yes } => {
                print_json(&Envelope::success(
                    service.notification_send(&event, yes).await?,
                ));
            }
        },
        Command::Stats { days } => {
            print_json(&Envelope::success(service.stats(u64::from(days.get()))?));
        }
        Command::Clean { target, yes } => {
            print_json(&Envelope::success(service.clean(target.as_str(), yes)?));
        }
        Command::Ls { limit } => {
            let defaults = service.effective_config();
            let limit = limit.map_or(defaults.page_size, NonZeroUsize::get);
            print_json(&Envelope::success(service.list(None, limit)?));
        }
        Command::Show { id } => match service.show(&id)? {
            Some(job) => print_json(&Envelope::success(job)),
            None => return Err(BossError::InvalidArgument(format!("job not found: {id}"))),
        },
        Command::Detail { id, refresh } => {
            print_json(&Envelope::success(service.detail(&id, refresh).await?));
        }
        Command::History { limit } => {
            print_json(&Envelope::success(service.history(None, limit.get())?))
        }
        Command::Export {
            source,
            limit,
            format,
            output,
            include_ids,
            force,
        } => print_json(&Envelope::success(service.export(ExportOptions {
            source: source.into(),
            platform: None,
            limit: limit.get(),
            format: format.into(),
            output,
            include_ids,
            force,
        })?)),
        Command::Config { command } => match command {
            ConfigCommand::Ls => print_json(&Envelope::success(service.config_list()?)),
            ConfigCommand::Get { key } => {
                print_json(&Envelope::success(service.config_get(&key)?));
            }
            ConfigCommand::Set { key, value } => {
                print_json(&Envelope::success(service.config_set(&key, &value)?));
            }
            ConfigCommand::Reset { key } => {
                print_json(&Envelope::success(service.config_reset(key.as_deref())?));
            }
        },
        Command::Login {
            phone,
            manual,
            cookie_stdin,
            repair,
            role,
        } => {
            if phone {
                print_json(&Envelope::success(service.login_phone(role.into())?));
                return Ok(ExitCode::SUCCESS);
            }
            if repair {
                print_json(&Envelope::success(service.repair_login(role.into())?));
                return Ok(ExitCode::SUCCESS);
            }
            let cookie = if cookie_stdin {
                Some(BossService::read_login_cookie_stdin()?)
            } else {
                None
            };
            print_json(&Envelope::success(
                service.login(manual, cookie, role.into()).await?,
            ));
        }
        Command::Recruiter { command } => match command {
            RecruiterCommand::Replies { limit, page } => {
                print_json(&Envelope::success(service.recruiter_replies(limit, page)?));
            }
            RecruiterCommand::Inbox {
                limit,
                page,
                all,
                pending,
                job,
                brief,
            } => {
                print_json(&Envelope::success(service.recruiter_inbox(
                    limit,
                    page.unwrap_or(1),
                    all,
                    pending,
                    job.as_deref(),
                    brief,
                )?));
            }
            RecruiterCommand::Resume { uid } => {
                print_json(&Envelope::success(service.recruiter_resume(&uid)?));
            }
            RecruiterCommand::Resumes { uids, brief } => {
                print_json(&Envelope::success(service.recruiter_resumes(&uids, brief)?));
            }
            RecruiterCommand::Reply { uid, message, yes } => {
                print_json(&Envelope::success(
                    service.recruiter_reply(&uid, &message, yes)?,
                ));
            }
        },
        Command::Chat { command } => match command {
            ChatCommand::Greet { job_id, yes } => {
                print_json(&Envelope::success(service.chat_greet(&job_id, yes)?));
            }
            ChatCommand::Send {
                job_id,
                message,
                yes,
            } => {
                print_json(&Envelope::success(
                    service.chat_send(&job_id, &message, yes)?,
                ));
            }
            ChatCommand::ExchangeWechat { job_id, yes } => {
                print_json(&Envelope::success(
                    service.chat_exchange_wechat(&job_id, yes)?,
                ));
            }
            ChatCommand::History { job_id, limit } => {
                print_json(&Envelope::success(
                    service.chat_history(&job_id, limit.get())?,
                ));
            }
            ChatCommand::Inbox { job_ids } => {
                print_json(&Envelope::success(service.chat_inbox(&job_ids)?));
            }
        },
        Command::Logout { yes } => print_json(&Envelope::success(service.logout(yes)?)),
        Command::Status {} => print_json(&Envelope::success(service.status())),
        Command::Doctor {} => print_json(&Envelope::success(service.doctor())),
        Command::Schema { format } => {
            print_json(&Envelope::success(service.schema(format.into())?));
        }
        Command::Shortlist { command } => match command {
            ShortlistCommand::Add { job_id, tags, note } => print_json(&Envelope::success(
                service.shortlist_add(&job_id, tags, note)?,
            )),
            ShortlistCommand::Ls { tag } => {
                print_json(&Envelope::success(service.shortlist_list(tag.as_deref())?));
            }
            ShortlistCommand::Annotate {
                job_id,
                add_tags,
                remove_tags,
                note,
            } => print_json(&Envelope::success(service.shortlist_annotate(
                &job_id,
                add_tags,
                remove_tags,
                note,
            )?)),
            ShortlistCommand::Rm { job_id } => {
                print_json(&Envelope::success(service.shortlist_remove(&job_id)?));
            }
            ShortlistCommand::Compare { tag } => print_json(&Envelope::success(
                service.shortlist_compare(tag.as_deref())?,
            )),
        },
        Command::Mcp => bosskit::mcp::run_stdio(&service).await?,
    }
    Ok(ExitCode::SUCCESS)
}

fn parse_campaign_rules(
    values: Vec<String>,
    category: &str,
) -> Result<Vec<CampaignRule>, BossError> {
    values
        .into_iter()
        .map(|value| {
            let (field, value) = value.split_once(':').ok_or_else(|| {
                BossError::InvalidArgument(format!("{category} rules must use field:value syntax"))
            })?;
            Ok(CampaignRule {
                field: parse_campaign_field(field)?,
                value: value.to_owned(),
            })
        })
        .collect()
}

fn parse_campaign_field(value: &str) -> Result<CampaignField, BossError> {
    match value.trim() {
        "title" => Ok(CampaignField::Title),
        "company" => Ok(CampaignField::Company),
        "city" => Ok(CampaignField::City),
        "district" => Ok(CampaignField::District),
        "salary" => Ok(CampaignField::Salary),
        "experience" => Ok(CampaignField::Experience),
        "education" => Ok(CampaignField::Education),
        "employment_type" => Ok(CampaignField::EmploymentType),
        "skills" => Ok(CampaignField::Skills),
        "welfare" => Ok(CampaignField::Welfare),
        "description" => Ok(CampaignField::Description),
        "address" => Ok(CampaignField::Address),
        other => Err(BossError::InvalidArgument(format!(
            "unknown campaign field: {other}"
        ))),
    }
}

fn patch_from_options(options: SearchOptions) -> SearchSpecPatch {
    SearchSpecPatch {
        query: options.query,
        city: options.city,
        page: options.page.map(NonZeroU32::get),
        limit: options.limit.map(NonZeroU32::get),
        company: options.company,
        salary: options.salary,
        experience: options.experience,
        education: options.education,
        employment_type: options.employment_type,
        welfare: (!options.welfare.is_empty()).then_some(options.welfare),
    }
}

fn patch_from_flags(
    query: Option<String>,
    city: Option<String>,
    flags: SearchFlags,
) -> SearchSpecPatch {
    SearchSpecPatch {
        query,
        city,
        page: flags.page.map(NonZeroU32::get),
        limit: flags.limit.map(NonZeroU32::get),
        company: flags.company,
        salary: flags.salary,
        experience: flags.experience,
        education: flags.education,
        employment_type: flags.employment_type,
        welfare: (!flags.welfare.is_empty()).then_some(flags.welfare),
    }
}

fn print_search_report(report: bosskit::SearchReport) -> bool {
    if report.has_success() {
        print_json(&Envelope::success(report));
        true
    } else {
        print_json(&json!({
            "ok":false,"data":report,
            "error":ErrorBody {
                code:"all_providers_failed".to_owned(),
                message:"all selected providers failed".to_owned(),
                recoverable:true
            },
            "hints":["平台可能要求 Cookie 或触发风控"]
        }));
        false
    }
}

fn print_json(value: &impl Serialize) {
    match serde_json::to_value(value) {
        Ok(value) if JSON_OUTPUT.load(Ordering::Relaxed) => match serde_json::to_string(&value) {
            Ok(output) => println!("{output}"),
            Err(error) => println!(
                "{{\"ok\":false,\"data\":null,\"error\":{{\"code\":\"serialization_error\",\"message\":{},\"recoverable\":false}},\"hints\":[]}}",
                serde_json::to_string(&error.to_string())
                    .unwrap_or_else(|_| "\"serialization failed\"".to_owned())
            ),
        },
        Ok(value) => println!("{}", render_markdown(&value)),
        Err(error) => println!(
            "- **ok**: false\n- **error**: serialization_error\n- **message**: {}",
            markdown_scalar(&serde_json::Value::String(error.to_string()))
        ),
    }
}

fn render_markdown(value: &serde_json::Value) -> String {
    render_markdown_value(value, 0)
}

fn render_markdown_value(value: &serde_json::Value, indent: usize) -> String {
    let prefix = " ".repeat(indent);
    match value {
        serde_json::Value::Object(object) => object
            .iter()
            .map(|(key, value)| match value {
                serde_json::Value::Object(_) | serde_json::Value::Array(_) => format!(
                    "{prefix}- **{key}**:\n{}",
                    render_markdown_value(value, indent + 2)
                ),
                _ => format!("{prefix}- **{key}**: {}", markdown_scalar(value)),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Array(array) => array
            .iter()
            .map(|value| match value {
                serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                    format!("{prefix}-\n{}", render_markdown_value(value, indent + 2))
                }
                _ => format!("{prefix}- {}", markdown_scalar(value)),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => format!("{prefix}{}", markdown_scalar(value)),
    }
}

fn markdown_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.replace('\n', "<br>").replace('\r', ""),
        serde_json::Value::Null => "-".to_owned(),
        _ => value.to_string(),
    }
}
