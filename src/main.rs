use std::num::{NonZeroU32, NonZeroUsize};
use std::process::ExitCode;

use std::path::PathBuf;

use bosskit::export::{ExportFormat, ExportOptions, ExportSource};
use bosskit::model::ErrorBody;
use bosskit::schema::SchemaFormat;
use bosskit::{BossError, BossService, Envelope, Platform, PlatformSelector, SearchSpecPatch};
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::json;

#[derive(Parser)]
#[command(name = "boss", version, about = "只读多平台招聘搜索 CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 列出已注册平台
    Platforms,
    /// 列出三个平台共同支持的逻辑城市
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
        #[arg(long, value_enum)]
        platform: Option<PlatformArg>,
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
        #[arg(long, value_enum)]
        platform: Option<PlatformArg>,
        #[arg(long, default_value = "20")]
        limit: NonZeroUsize,
    },
    /// 安全导出本地职位或短名单
    Export {
        #[arg(long, value_enum, default_value = "jobs")]
        source: ExportSourceArg,
        #[arg(long, value_enum)]
        platform: Option<PlatformArg>,
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
    /// 本地导入或选择登录 Cookie，不联网验证
    Login {
        #[arg(long, value_enum)]
        platform: Option<PlatformArg>,
        #[arg(long)]
        credential_file: Option<PathBuf>,
        #[arg(long, conflicts_with = "credential_file")]
        manual: bool,
    },
    /// 撤销本地保存的登录会话和导出文件引用
    Logout {
        #[arg(long, value_enum)]
        platform: Option<PlatformArg>,
        #[arg(long)]
        yes: bool,
    },
    /// 检查本地 Cookie 环境状态，不联网
    Status {
        #[arg(long, value_enum)]
        platform: Option<PlatformArg>,
    },
    /// 运行严格本地诊断，不联网
    Doctor {
        #[arg(long, value_enum)]
        platform: Option<PlatformArg>,
    },
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

#[derive(Clone, Args)]
struct SearchOptions {
    query: Option<String>,
    #[arg(long)]
    preset: Option<String>,
    #[arg(long, value_enum)]
    platform: Option<PlatformArg>,
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

#[derive(Clone, Args)]
struct SearchFlags {
    #[arg(long, value_enum)]
    platform: Option<PlatformArg>,
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
enum PlatformArg {
    Zhipin,
    Zhilian,
    Qiancheng,
    All,
}

impl PlatformArg {
    const fn selected(self) -> Option<Platform> {
        match self {
            Self::Zhipin => Some(Platform::Zhipin),
            Self::Zhilian => Some(Platform::Zhilian),
            Self::Qiancheng => Some(Platform::Qiancheng),
            Self::All => None,
        }
    }

    const fn selector(self) -> PlatformSelector {
        match self {
            Self::Zhipin => PlatformSelector::Zhipin,
            Self::Zhilian => PlatformSelector::Zhilian,
            Self::Qiancheng => PlatformSelector::Qiancheng,
            Self::All => PlatformSelector::All,
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
            Self::All => "all",
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
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            print!("{error}");
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
    if let Command::Doctor { platform } = &cli.command {
        print_json(&Envelope::success(BossService::doctor_local(
            platform.and_then(PlatformArg::selected),
        )));
        return Ok(ExitCode::SUCCESS);
    }
    let mut service = BossService::discover()?;
    match cli.command {
        Command::Platforms => print_json(&Envelope::success(service.platforms())),
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
        Command::Stats { days } => {
            print_json(&Envelope::success(service.stats(u64::from(days.get()))?));
        }
        Command::Clean { target, yes } => {
            print_json(&Envelope::success(service.clean(target.as_str(), yes)?));
        }
        Command::Ls { platform, limit } => {
            let defaults = service.effective_config();
            let platform =
                platform.map_or_else(|| defaults.platform.selected(), PlatformArg::selected);
            let limit = limit.map_or(defaults.page_size, NonZeroUsize::get);
            print_json(&Envelope::success(service.list(platform, limit)?));
        }
        Command::Show { id } => match service.show(&id)? {
            Some(job) => print_json(&Envelope::success(job)),
            None => return Err(BossError::InvalidArgument(format!("job not found: {id}"))),
        },
        Command::Detail { id, refresh } => {
            print_json(&Envelope::success(service.detail(&id, refresh).await?));
        }
        Command::History { platform, limit } => print_json(&Envelope::success(
            service.history(platform.and_then(PlatformArg::selected), limit.get())?,
        )),
        Command::Export {
            source,
            platform,
            limit,
            format,
            output,
            include_ids,
            force,
        } => print_json(&Envelope::success(service.export(ExportOptions {
            source: source.into(),
            platform: platform.and_then(PlatformArg::selected),
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
            platform,
            credential_file,
            manual,
        } => print_json(&Envelope::success(service.login(
            platform.and_then(PlatformArg::selected),
            credential_file.as_deref(),
            manual,
        )?)),
        Command::Logout { platform, yes } => print_json(&Envelope::success(
            service.logout(platform.and_then(PlatformArg::selected), yes)?,
        )),
        Command::Status { platform } => print_json(&Envelope::success(
            service.status(platform.and_then(PlatformArg::selected)),
        )),
        Command::Doctor { platform } => print_json(&Envelope::success(
            service.doctor(platform.and_then(PlatformArg::selected)),
        )),
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

fn patch_from_options(options: SearchOptions) -> SearchSpecPatch {
    SearchSpecPatch {
        query: options.query,
        platform: options.platform.map(PlatformArg::selector),
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
        platform: flags.platform.map(PlatformArg::selector),
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
    match serde_json::to_string(value) {
        Ok(output) => println!("{output}"),
        Err(error) => println!(
            "{{\"ok\":false,\"data\":null,\"error\":{{\"code\":\"serialization_error\",\"message\":{},\"recoverable\":false}},\"hints\":[]}}",
            serde_json::to_string(&error.to_string())
                .unwrap_or_else(|_| "\"serialization failed\"".to_owned())
        ),
    }
}
