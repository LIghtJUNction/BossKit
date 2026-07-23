//! Minimal MCP 2025-03-26 JSON-RPC stdio server.

use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::export::{ExportFormat, ExportOptions, ExportSource};
use crate::schema::{SchemaFormat, tool_registry};
use crate::{BossError, BossService, Platform, PlatformSelector, SearchSpecPatch};

const SEARCH_FIELDS: &[&str] = &[
    "query",
    "preset",
    "platform",
    "city",
    "page",
    "limit",
    "company",
    "salary",
    "experience",
    "education",
    "job_type",
    "welfare",
];
const PRESET_ADD_FIELDS: &[&str] = &[
    "name",
    "query",
    "platform",
    "city",
    "page",
    "limit",
    "company",
    "salary",
    "experience",
    "education",
    "job_type",
    "welfare",
];
const WATCH_ADD_FIELDS: &[&str] = &[
    "name",
    "query",
    "preset",
    "platform",
    "city",
    "page",
    "limit",
    "company",
    "salary",
    "experience",
    "education",
    "job_type",
    "welfare",
];

/// Runs newline-delimited JSON-RPC over stdin/stdout.
pub async fn run_stdio(service: &BossService) -> Result<(), BossError> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| BossError::Network(error.to_string()))?
    {
        let parsed = match serde_json::from_str::<Value>(&line) {
            Ok(value) => value,
            Err(error) => {
                write_response(
                    &mut stdout,
                    &rpc_error(Value::Null, -32700, &error.to_string()),
                )
                .await?;
                continue;
            }
        };
        if let Some(response) = handle_input(service, parsed).await {
            write_response(&mut stdout, &response).await?;
        }
    }
    Ok(())
}

async fn handle_input(service: &BossService, input: Value) -> Option<Value> {
    if let Value::Array(requests) = input {
        if requests.is_empty() {
            return Some(rpc_error(Value::Null, -32600, "invalid request"));
        }
        let mut responses = Vec::new();
        for request in requests {
            if let Some(response) = handle_request(service, request).await {
                responses.push(response);
            }
        }
        return (!responses.is_empty()).then_some(Value::Array(responses));
    }
    handle_request(service, input).await
}

async fn handle_request(service: &BossService, request: Value) -> Option<Value> {
    let Some(object) = request.as_object() else {
        return Some(rpc_error(Value::Null, -32600, "invalid request"));
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(rpc_error(
            usable_id(object.get("id")),
            -32600,
            "invalid request",
        ));
    }
    let id = object.get("id")?.clone();
    let method = object.get("method").and_then(Value::as_str);
    let Some(method) = method else {
        return Some(rpc_error(id, -32600, "invalid request"));
    };
    match method {
        "initialize" => Some(json!({
            "jsonrpc":"2.0","id":id,"result":{
                "protocolVersion":"2025-03-26",
                "capabilities":{"tools":{}},
                "serverInfo":{"name":"bosskit","version":env!("CARGO_PKG_VERSION")}
            }
        })),
        "ping" => Some(rpc_result(id, json!({}))),
        "tools/list" => Some(rpc_result(id, json!({"tools":tool_list()}))),
        "tools/call" => {
            let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
            Some(match call_tool(service, &params).await {
                Ok((value, is_error)) => rpc_result(
                    id,
                    json!({
                        "content":[{"type":"text","text":value.to_string()}],
                        "isError":is_error
                    }),
                ),
                Err(ToolCallError::UnknownTool(name)) => {
                    rpc_error(id, -32601, &format!("unknown tool: {name}"))
                }
                Err(ToolCallError::InvalidArguments(message)) => rpc_error(id, -32602, &message),
                Err(ToolCallError::Execution(error)) => rpc_result(
                    id,
                    json!({
                        "content":[{"type":"text","text":error_envelope(&error).to_string()}],
                        "isError":true
                    }),
                ),
            })
        }
        _ => Some(rpc_error(id, -32601, "method not found")),
    }
}

fn usable_id(id: Option<&Value>) -> Value {
    match id {
        Some(value @ (Value::Null | Value::String(_) | Value::Number(_))) => value.clone(),
        _ => Value::Null,
    }
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn error_envelope(error: &BossError) -> Value {
    json!({
        "ok":false,
        "data":null,
        "error":{
            "code":error.code(),
            "message":crate::model::redact_secrets(&error.to_string()),
            "recoverable":error.recoverable()
        },
        "hints":[]
    })
}

fn tool_list() -> Value {
    json!(tool_registry())
}

enum ToolCallError {
    UnknownTool(String),
    InvalidArguments(String),
    Execution(BossError),
}

async fn call_tool(service: &BossService, params: &Value) -> Result<(Value, bool), ToolCallError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolCallError::InvalidArguments("missing tool name".to_owned()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err(ToolCallError::InvalidArguments(
            "arguments must be an object".to_owned(),
        ));
    }
    match name {
        "platforms" => {
            validate_allowed(&arguments, &[])?;
            Ok((
                json!({"ok":true,"data":service.platforms(),"error":null,"hints":[]}),
                false,
            ))
        }
        "cities" => {
            validate_allowed(&arguments, &[])?;
            Ok((
                json!({"ok":true,"data":service.cities(),"error":null,"hints":[]}),
                false,
            ))
        }
        "search_jobs" => {
            validate_allowed(&arguments, SEARCH_FIELDS)?;
            let preset = optional_nonblank_owned_string(&arguments, "preset")?;
            if preset.is_none() && arguments.get("query").is_none() {
                return Err(ToolCallError::InvalidArguments(
                    "search_jobs requires query or preset".to_owned(),
                ));
            }
            let spec = service
                .resolve_search_spec(preset.as_deref(), search_patch(&arguments)?)
                .map_err(ToolCallError::Execution)?;
            let report = service
                .search_spec(spec)
                .await
                .map_err(ToolCallError::Execution)?;
            let success = report.has_success();
            Ok((
                json!({
                    "ok":success,"data":report,
                    "error": if success { Value::Null } else { json!({
                        "code":"all_providers_failed",
                        "message":"all selected providers failed",
                        "recoverable":true
                    }) },
                    "hints": if success {
                        Vec::<String>::new()
                    } else {
                        vec!["平台可能要求 Cookie 或触发风控".to_owned()]
                    }
                }),
                !success,
            ))
        }
        "list_jobs" => {
            validate_allowed(&arguments, &["platform", "limit"])?;
            let defaults = service.effective_config();
            let platform = parse_platform(&arguments, defaults.platform.selected())?;
            let limit = positive_usize(&arguments, "limit", defaults.page_size)?;
            let jobs = service
                .list(platform, limit)
                .map_err(ToolCallError::Execution)?;
            Ok((
                json!({"ok":true,"data":jobs,"error":null,"hints":[]}),
                false,
            ))
        }
        "show_job" => {
            validate_allowed(&arguments, &["id"])?;
            let id = required_string(&arguments, "id")?;
            let job = service
                .show(id)
                .map_err(ToolCallError::Execution)?
                .ok_or_else(|| {
                    ToolCallError::Execution(BossError::InvalidArgument(format!(
                        "job not found: {id}"
                    )))
                })?;
            Ok((json!({"ok":true,"data":job,"error":null,"hints":[]}), false))
        }
        "job_detail" => {
            validate_allowed(&arguments, &["id", "refresh"])?;
            let id = required_string(&arguments, "id")?;
            let refresh = optional_bool(&arguments, "refresh", false)?;
            let job = service
                .detail(id, refresh)
                .await
                .map_err(ToolCallError::Execution)?;
            Ok((json!({"ok":true,"data":job,"error":null,"hints":[]}), false))
        }
        "search_history" => {
            validate_allowed(&arguments, &["platform", "limit"])?;
            let platform = parse_platform(&arguments, None)?;
            let limit = positive_usize(&arguments, "limit", 20)?;
            let history = service
                .history(platform, limit)
                .map_err(ToolCallError::Execution)?;
            Ok((
                json!({"ok":true,"data":history,"error":null,"hints":[]}),
                false,
            ))
        }
        "export_jobs" => {
            validate_allowed(&arguments, &["source", "platform", "limit", "include_ids"])?;
            let source = match optional_string(&arguments, "source")?.unwrap_or("jobs") {
                "jobs" => ExportSource::Jobs,
                "shortlist" => ExportSource::Shortlist,
                other => {
                    return Err(ToolCallError::InvalidArguments(format!(
                        "unknown export source: {other}"
                    )));
                }
            };
            let platform = parse_platform(&arguments, None)?;
            let limit = positive_usize(&arguments, "limit", 20)?;
            let include_ids = optional_bool(&arguments, "include_ids", false)?;
            let export = service
                .export(ExportOptions {
                    source,
                    platform,
                    limit,
                    format: ExportFormat::Json,
                    output: None,
                    include_ids,
                    force: false,
                })
                .map_err(ToolCallError::Execution)?;
            Ok((
                json!({"ok":true,"data":export,"error":null,"hints":[]}),
                false,
            ))
        }
        "status" => {
            validate_allowed(&arguments, &["platform"])?;
            let platform = parse_platform(&arguments, None)?;
            Ok((
                json!({"ok":true,"data":service.status(platform),"error":null,"hints":[]}),
                false,
            ))
        }
        "doctor" => {
            validate_allowed(&arguments, &["platform"])?;
            let platform = parse_platform(&arguments, None)?;
            Ok((
                json!({"ok":true,"data":service.doctor(platform),"error":null,"hints":[]}),
                false,
            ))
        }
        "schema" => {
            validate_allowed(&arguments, &["format"])?;
            let format = match required_string(&arguments, "format")? {
                "native" => SchemaFormat::Native,
                "openai-tools" => SchemaFormat::OpenaiTools,
                "anthropic-tools" => SchemaFormat::AnthropicTools,
                "mcp-tools" => SchemaFormat::McpTools,
                other => {
                    return Err(ToolCallError::InvalidArguments(format!(
                        "unknown schema format: {other}"
                    )));
                }
            };
            let schema = service.schema(format).map_err(ToolCallError::Execution)?;
            Ok((
                json!({"ok":true,"data":schema,"error":null,"hints":[]}),
                false,
            ))
        }
        "shortlist_add" => {
            validate_allowed(&arguments, &["job_id", "tags", "note"])?;
            let job_id = required_string(&arguments, "job_id")?;
            let tags = string_array(&arguments, "tags")?;
            let note = optional_owned_string(&arguments, "note")?;
            let entry = service
                .shortlist_add(job_id, tags, note)
                .map_err(ToolCallError::Execution)?;
            Ok((
                json!({"ok":true,"data":entry,"error":null,"hints":[]}),
                false,
            ))
        }
        "shortlist_list" => {
            validate_allowed(&arguments, &["tag"])?;
            let tag = optional_string(&arguments, "tag")?;
            let entries = service
                .shortlist_list(tag)
                .map_err(ToolCallError::Execution)?;
            Ok((
                json!({"ok":true,"data":entries,"error":null,"hints":[]}),
                false,
            ))
        }
        "shortlist_annotate" => {
            validate_allowed(&arguments, &["job_id", "add_tags", "remove_tags", "note"])?;
            let job_id = required_string(&arguments, "job_id")?;
            let add_tags = string_array(&arguments, "add_tags")?;
            let remove_tags = string_array(&arguments, "remove_tags")?;
            let note = optional_owned_string(&arguments, "note")?;
            let entry = service
                .shortlist_annotate(job_id, add_tags, remove_tags, note)
                .map_err(ToolCallError::Execution)?;
            Ok((
                json!({"ok":true,"data":entry,"error":null,"hints":[]}),
                false,
            ))
        }
        "shortlist_remove" => {
            validate_allowed(&arguments, &["job_id"])?;
            let job_id = required_string(&arguments, "job_id")?;
            let entry = service
                .shortlist_remove(job_id)
                .map_err(ToolCallError::Execution)?;
            Ok((
                json!({"ok":true,"data":entry,"error":null,"hints":[]}),
                false,
            ))
        }
        "shortlist_compare" => {
            validate_allowed(&arguments, &["tag"])?;
            let tag = optional_string(&arguments, "tag")?;
            let comparison = service
                .shortlist_compare(tag)
                .map_err(ToolCallError::Execution)?;
            Ok((
                json!({"ok":true,"data":comparison,"error":null,"hints":[]}),
                false,
            ))
        }
        "preset_add" => {
            validate_allowed(&arguments, PRESET_ADD_FIELDS)?;
            let name = required_string(&arguments, "name")?;
            let _ = required_string(&arguments, "query")?;
            let spec = service
                .resolve_search_spec(None, search_patch(&arguments)?)
                .map_err(ToolCallError::Execution)?;
            let preset = service
                .preset_add(name, spec)
                .map_err(ToolCallError::Execution)?;
            Ok((success_value(preset), false))
        }
        "preset_list" => {
            validate_allowed(&arguments, &[])?;
            Ok((
                success_value(service.preset_list().map_err(ToolCallError::Execution)?),
                false,
            ))
        }
        "preset_show" => {
            validate_allowed(&arguments, &["name"])?;
            let name = required_string(&arguments, "name")?;
            Ok((
                success_value(
                    service
                        .preset_show(name)
                        .map_err(ToolCallError::Execution)?,
                ),
                false,
            ))
        }
        "preset_remove" => {
            validate_allowed(&arguments, &["name"])?;
            let name = required_string(&arguments, "name")?;
            Ok((
                success_value(
                    service
                        .preset_remove(name)
                        .map_err(ToolCallError::Execution)?,
                ),
                false,
            ))
        }
        "watch_add" => {
            validate_allowed(&arguments, WATCH_ADD_FIELDS)?;
            let name = required_string(&arguments, "name")?;
            let preset = optional_nonblank_owned_string(&arguments, "preset")?;
            let query = optional_nonblank_owned_string(&arguments, "query")?;
            if query.is_some() == preset.is_some() {
                return Err(ToolCallError::InvalidArguments(
                    "watch_add requires exactly one of query or preset".to_owned(),
                ));
            }
            let spec = service
                .resolve_search_spec(preset.as_deref(), search_patch(&arguments)?)
                .map_err(ToolCallError::Execution)?;
            Ok((
                success_value(
                    service
                        .watch_add(name, spec)
                        .map_err(ToolCallError::Execution)?,
                ),
                false,
            ))
        }
        "watch_list" => {
            validate_allowed(&arguments, &[])?;
            Ok((
                success_value(service.watch_list().map_err(ToolCallError::Execution)?),
                false,
            ))
        }
        "watch_show" => {
            validate_allowed(&arguments, &["name"])?;
            Ok((
                success_value(
                    service
                        .watch_show(required_string(&arguments, "name")?)
                        .map_err(ToolCallError::Execution)?,
                ),
                false,
            ))
        }
        "watch_run" => {
            validate_allowed(&arguments, &["name", "all"])?;
            let name = optional_nonblank_owned_string(&arguments, "name")?;
            let all = optional_bool(&arguments, "all", false)?;
            let value = match (name, all) {
                (Some(name), false) => service
                    .watch_run(&name)
                    .await
                    .map_err(ToolCallError::Execution)?,
                (None, true) => json!(
                    service
                        .watch_run_all()
                        .await
                        .map_err(ToolCallError::Execution)?
                ),
                _ => {
                    return Err(ToolCallError::InvalidArguments(
                        "watch_run requires name or all=true, but not both".to_owned(),
                    ));
                }
            };
            Ok((success_value(value), false))
        }
        "watch_remove" => {
            validate_allowed(&arguments, &["name"])?;
            Ok((
                success_value(
                    service
                        .watch_remove(required_string(&arguments, "name")?)
                        .map_err(ToolCallError::Execution)?,
                ),
                false,
            ))
        }
        "resume_init" => {
            validate_allowed(&arguments, &["name", "title"])?;
            let document = service
                .resume_init(
                    required_string(&arguments, "name")?,
                    optional_owned_string(&arguments, "title")?,
                )
                .map_err(ToolCallError::Execution)?;
            Ok((success_value(document), false))
        }
        "resume_list" => {
            validate_allowed(&arguments, &[])?;
            Ok((
                success_value(service.resume_list().map_err(ToolCallError::Execution)?),
                false,
            ))
        }
        "resume_show" => {
            validate_allowed(&arguments, &["name"])?;
            Ok((
                success_value(
                    service
                        .resume_show(required_string(&arguments, "name")?)
                        .map_err(ToolCallError::Execution)?,
                ),
                false,
            ))
        }
        "resume_set" => {
            validate_allowed(&arguments, &["name", "field", "value"])?;
            let document = service
                .resume_set(
                    required_string(&arguments, "name")?,
                    required_string(&arguments, "field")?,
                    required_string_allow_empty(&arguments, "value")?.to_owned(),
                )
                .map_err(ToolCallError::Execution)?;
            Ok((success_value(document), false))
        }
        "resume_skills" => {
            validate_allowed(&arguments, &["name", "add", "remove"])?;
            let document = service
                .resume_skills(
                    required_string(&arguments, "name")?,
                    nonblank_string_array(&arguments, "add")?,
                    nonblank_string_array(&arguments, "remove")?,
                )
                .map_err(ToolCallError::Execution)?;
            Ok((success_value(document), false))
        }
        "resume_clone" => {
            validate_allowed(&arguments, &["name", "new_name"])?;
            let document = service
                .resume_clone(
                    required_string(&arguments, "name")?,
                    required_string(&arguments, "new_name")?,
                )
                .map_err(ToolCallError::Execution)?;
            Ok((success_value(document), false))
        }
        "resume_diff" => {
            validate_allowed(&arguments, &["left", "right"])?;
            let diff = service
                .resume_diff(
                    required_string(&arguments, "left")?,
                    required_string(&arguments, "right")?,
                )
                .map_err(ToolCallError::Execution)?;
            Ok((success_value(diff), false))
        }
        "resume_remove" => {
            validate_allowed(&arguments, &["name", "confirm"])?;
            if !optional_bool(&arguments, "confirm", false)? {
                return Err(ToolCallError::InvalidArguments(
                    "resume_remove requires confirm=true".to_owned(),
                ));
            }
            let document = service
                .resume_remove(required_string(&arguments, "name")?, true)
                .map_err(ToolCallError::Execution)?;
            Ok((success_value(document), false))
        }
        "stats" => {
            validate_allowed(&arguments, &["days"])?;
            let days = positive_u32(&arguments, "days", 30)?;
            Ok((
                success_value(
                    service
                        .stats(u64::from(days))
                        .map_err(ToolCallError::Execution)?,
                ),
                false,
            ))
        }
        "clean_preview" => {
            validate_allowed(&arguments, &["target"])?;
            let target = required_string(&arguments, "target")?;
            if ![
                "jobs",
                "history",
                "shortlist",
                "presets",
                "watches",
                "resumes",
                "all",
            ]
            .contains(&target)
            {
                return Err(ToolCallError::InvalidArguments(format!(
                    "unknown clean target: {target}"
                )));
            }
            Ok((
                success_value(
                    service
                        .clean(target, false)
                        .map_err(ToolCallError::Execution)?,
                ),
                false,
            ))
        }
        _ => Err(ToolCallError::UnknownTool(name.to_owned())),
    }
}

fn validate_allowed(value: &Value, allowed: &[&str]) -> Result<(), ToolCallError> {
    let object = value
        .as_object()
        .ok_or_else(|| ToolCallError::InvalidArguments("arguments must be an object".to_owned()))?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(ToolCallError::InvalidArguments(format!(
            "unknown argument: {key}"
        )));
    }
    Ok(())
}

fn success_value(value: impl Serialize) -> Value {
    json!({"ok":true,"data":value,"error":null,"hints":[]})
}

fn search_patch(arguments: &Value) -> Result<SearchSpecPatch, ToolCallError> {
    Ok(SearchSpecPatch {
        query: optional_nonblank_owned_string(arguments, "query")?,
        platform: optional_platform_selector(arguments)?,
        city: optional_nonblank_owned_string(arguments, "city")?,
        page: optional_positive_u32(arguments, "page")?,
        limit: optional_positive_u32(arguments, "limit")?,
        company: optional_nonblank_owned_string(arguments, "company")?,
        salary: optional_nonblank_owned_string(arguments, "salary")?,
        experience: optional_nonblank_owned_string(arguments, "experience")?,
        education: optional_nonblank_owned_string(arguments, "education")?,
        employment_type: optional_nonblank_owned_string(arguments, "job_type")?,
        welfare: arguments
            .get("welfare")
            .map(|_| nonblank_string_array(arguments, "welfare"))
            .transpose()?,
    })
}

fn optional_platform_selector(
    arguments: &Value,
) -> Result<Option<PlatformSelector>, ToolCallError> {
    let Some(value) = optional_string(arguments, "platform")? else {
        return Ok(None);
    };
    match value {
        "all" => Ok(Some(PlatformSelector::All)),
        "zhipin" => Ok(Some(PlatformSelector::Zhipin)),
        "zhilian" => Ok(Some(PlatformSelector::Zhilian)),
        "qiancheng" => Ok(Some(PlatformSelector::Qiancheng)),
        other => Err(ToolCallError::InvalidArguments(format!(
            "unknown platform: {other}"
        ))),
    }
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, ToolCallError> {
    match value.get(field) {
        None => Err(ToolCallError::InvalidArguments(format!("missing {field}"))),
        Some(Value::String(text)) if !text.is_empty() => Ok(text),
        Some(Value::String(_)) => Err(ToolCallError::InvalidArguments(format!(
            "{field} must not be empty"
        ))),
        Some(_) => Err(ToolCallError::InvalidArguments(format!(
            "{field} must be a string"
        ))),
    }
}

fn required_string_allow_empty<'a>(
    value: &'a Value,
    field: &str,
) -> Result<&'a str, ToolCallError> {
    match value.get(field) {
        None => Err(ToolCallError::InvalidArguments(format!("missing {field}"))),
        Some(Value::String(text)) => Ok(text),
        Some(_) => Err(ToolCallError::InvalidArguments(format!(
            "{field} must be a string"
        ))),
    }
}

fn optional_string<'a>(value: &'a Value, field: &str) -> Result<Option<&'a str>, ToolCallError> {
    match value.get(field) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(ToolCallError::InvalidArguments(format!(
            "{field} must be a string"
        ))),
    }
}

fn optional_owned_string(value: &Value, field: &str) -> Result<Option<String>, ToolCallError> {
    optional_string(value, field).map(|item| item.map(ToOwned::to_owned))
}

fn optional_nonblank_owned_string(
    value: &Value,
    field: &str,
) -> Result<Option<String>, ToolCallError> {
    optional_string(value, field)?
        .map(|text| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Err(ToolCallError::InvalidArguments(format!(
                    "{field} must not be blank"
                )))
            } else {
                Ok(trimmed.to_owned())
            }
        })
        .transpose()
}

fn string_array(value: &Value, field: &str) -> Result<Vec<String>, ToolCallError> {
    match value.get(field) {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                    ToolCallError::InvalidArguments(format!("{field} must contain only strings"))
                })
            })
            .collect(),
        Some(_) => Err(ToolCallError::InvalidArguments(format!(
            "{field} must be an array"
        ))),
    }
}

fn nonblank_string_array(value: &Value, field: &str) -> Result<Vec<String>, ToolCallError> {
    let items = string_array(value, field)?;
    items
        .into_iter()
        .map(|item| {
            let trimmed = item.trim();
            if trimmed.is_empty() {
                Err(ToolCallError::InvalidArguments(format!(
                    "{field} must not contain blank strings"
                )))
            } else {
                Ok(trimmed.to_owned())
            }
        })
        .collect()
}

fn optional_bool(value: &Value, field: &str, default: bool) -> Result<bool, ToolCallError> {
    match value.get(field) {
        None => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(ToolCallError::InvalidArguments(format!(
            "{field} must be a boolean"
        ))),
    }
}

fn positive_u32(value: &Value, field: &str, default: u32) -> Result<u32, ToolCallError> {
    let number = match value.get(field) {
        None => u64::from(default),
        Some(value) => value.as_u64().ok_or_else(|| {
            ToolCallError::InvalidArguments(format!("{field} must be a positive integer"))
        })?,
    };
    u32::try_from(number)
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| ToolCallError::InvalidArguments(format!("{field} must be positive")))
}

fn optional_positive_u32(value: &Value, field: &str) -> Result<Option<u32>, ToolCallError> {
    value
        .get(field)
        .map(|value| {
            value
                .as_u64()
                .and_then(|number| u32::try_from(number).ok())
                .filter(|number| *number > 0)
                .ok_or_else(|| ToolCallError::InvalidArguments(format!("{field} must be positive")))
        })
        .transpose()
}

fn positive_usize(value: &Value, field: &str, default: usize) -> Result<usize, ToolCallError> {
    let number = match value.get(field) {
        None => default as u64,
        Some(value) => value.as_u64().ok_or_else(|| {
            ToolCallError::InvalidArguments(format!("{field} must be a positive integer"))
        })?,
    };
    usize::try_from(number)
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| ToolCallError::InvalidArguments(format!("{field} must be positive")))
}

fn parse_platform(
    arguments: &Value,
    default: Option<Platform>,
) -> Result<Option<Platform>, ToolCallError> {
    let Some(value) = optional_string(arguments, "platform")? else {
        return Ok(default);
    };
    match value {
        "all" => Ok(None),
        "zhipin" => Ok(Some(Platform::Zhipin)),
        "zhilian" => Ok(Some(Platform::Zhilian)),
        "qiancheng" => Ok(Some(Platform::Qiancheng)),
        other => Err(ToolCallError::InvalidArguments(format!(
            "unknown platform: {other}"
        ))),
    }
}

async fn write_response(stdout: &mut tokio::io::Stdout, response: &Value) -> Result<(), BossError> {
    let mut bytes = serde_json::to_vec(response)?;
    bytes.push(b'\n');
    stdout
        .write_all(&bytes)
        .await
        .map_err(|error| BossError::Network(error.to_string()))?;
    stdout
        .flush()
        .await
        .map_err(|error| BossError::Network(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use tempfile::tempdir;

    use super::*;

    fn service() -> BossService {
        BossService::discover().expect("service")
    }

    #[tokio::test]
    async fn tools_list_returns_all_tools() {
        let response = handle_input(
            &service(),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        )
        .await
        .expect("response");
        assert_eq!(response["result"]["tools"], json!(tool_registry()));
    }

    #[test]
    fn search_workflow_schema_properties_match_runtime_allowlists() {
        let registry = tool_registry();
        for (name, allowed) in [
            ("search_jobs", SEARCH_FIELDS),
            ("preset_add", PRESET_ADD_FIELDS),
            ("watch_add", WATCH_ADD_FIELDS),
        ] {
            let schema = registry
                .iter()
                .find(|tool| tool.name == name)
                .expect("tool");
            let properties: BTreeSet<&str> = schema.input_schema["properties"]
                .as_object()
                .expect("properties")
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(properties, allowed.iter().copied().collect(), "{name}");
        }
    }

    #[tokio::test]
    async fn representative_new_tools_succeed() {
        for (name, arguments) in [
            ("cities", json!({})),
            ("status", json!({})),
            ("doctor", json!({})),
            ("schema", json!({"format":"mcp-tools"})),
            ("search_history", json!({})),
            ("export_jobs", json!({})),
            ("shortlist_list", json!({})),
            ("shortlist_compare", json!({})),
        ] {
            let response = handle_input(
                &service(),
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                    "name":name,"arguments":arguments
                }}),
            )
            .await
            .expect("response");
            assert!(response.get("error").is_none(), "{response}");
        }
    }

    #[tokio::test]
    async fn local_workflow_tools_share_state_and_clean_only_previews() {
        let directory = tempdir().expect("tempdir");
        let service =
            BossService::from_paths(crate::DataPaths::new(directory.path())).expect("service");
        for (name, arguments) in [
            (
                "preset_add",
                json!({"name":"saved","query":"rust","limit":3}),
            ),
            ("preset_list", json!({})),
            ("watch_add", json!({"name":"daily","preset":"saved"})),
            ("watch_show", json!({"name":"daily"})),
            ("resume_init", json!({"name":"base","title":"Engineer"})),
            (
                "resume_set",
                json!({"name":"base","field":"summary","value":"Local"}),
            ),
            ("stats", json!({"days":30})),
            ("clean_preview", json!({"target":"all"})),
        ] {
            let response = handle_input(
                &service,
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                    "name":name,"arguments":arguments
                }}),
            )
            .await
            .expect("response");
            assert_eq!(response["result"]["isError"], false, "{response}");
        }
        assert!(directory.path().join("presets.json").exists());
    }

    #[tokio::test]
    async fn new_tools_reject_unknown_arguments_and_missing_confirmation() {
        for (name, arguments) in [
            ("preset_list", json!({"unexpected":true})),
            (
                "preset_add",
                json!({"name":"bad","query":"rust","preset":"forbidden"}),
            ),
            ("resume_remove", json!({"name":"base","confirm":false})),
            ("clean_preview", json!({"target":"jobs","yes":true})),
        ] {
            let response = handle_input(
                &service(),
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                    "name":name,"arguments":arguments
                }}),
            )
            .await
            .expect("response");
            assert_eq!(response["error"]["code"], -32602, "{response}");
        }
    }

    #[tokio::test]
    async fn unknown_new_tool_argument_is_invalid_params() {
        let response = handle_input(
            &service(),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"cities","arguments":{"unexpected":true}
            }}),
        )
        .await
        .expect("response");
        assert_eq!(response["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn malformed_shortlist_tags_are_invalid_params() {
        let response = handle_input(
            &service(),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"shortlist_add","arguments":{"job_id":"job","tags":[1]}
            }}),
        )
        .await
        .expect("response");
        assert_eq!(response["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn export_tool_rejects_filesystem_path_argument() {
        let response = handle_input(
            &service(),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"export_jobs","arguments":{"output":"/tmp/jobs.json"}
            }}),
        )
        .await
        .expect("response");
        assert_eq!(response["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn detail_missing_cache_id_is_valid_execution_error() {
        let response = handle_input(
            &service(),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"job_detail","arguments":{"id":"definitely-missing"}
            }}),
        )
        .await
        .expect("response");
        assert_eq!(response["result"]["isError"], true);
    }

    #[tokio::test]
    async fn discovery_tools_succeed_with_seeded_local_data() {
        let directory = tempdir().expect("tempdir");
        let paths = crate::DataPaths::new(directory.path());
        let mut job = crate::Job::new(
            "cached",
            Platform::Zhipin,
            "remote",
            "Rust",
            "https://example.test/job",
        );
        job.description = "cached detail".to_owned();
        crate::JobCache::from_paths(&paths)
            .save(&[job])
            .expect("seed cache");
        let service = BossService::from_paths(paths).expect("service");
        for (name, arguments) in [
            ("job_detail", json!({"id":"cached"})),
            ("search_history", json!({})),
            ("export_jobs", json!({"source":"jobs"})),
        ] {
            let response = handle_input(
                &service,
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                    "name":name,"arguments":arguments
                }}),
            )
            .await
            .expect("response");
            assert_eq!(response["result"]["isError"], false, "{response}");
        }
    }

    #[tokio::test]
    async fn batch_omits_notification_responses() {
        let response = handle_input(
            &service(),
            json!([
                {"jsonrpc":"2.0","id":1,"method":"ping"},
                {"jsonrpc":"2.0","method":"notifications/initialized"},
                {"jsonrpc":"2.0","id":2,"method":"tools/list"}
            ]),
        )
        .await
        .expect("response");
        assert_eq!(response.as_array().expect("batch").len(), 2);
    }

    #[tokio::test]
    async fn unknown_tool_returns_method_not_found() {
        let response = handle_input(
            &service(),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"missing","arguments":{}
            }}),
        )
        .await
        .expect("response");
        assert_eq!(response["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn invalid_tool_arguments_return_invalid_params() {
        let response = handle_input(
            &service(),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"search_jobs","arguments":{"limit":0}
            }}),
        )
        .await
        .expect("response");
        assert_eq!(response["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn wrong_argument_types_return_invalid_params() {
        for arguments in [
            json!({"query":"rust","limit":"oops"}),
            json!({"query":"rust","platform":5}),
        ] {
            let response = handle_input(
                &service(),
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                    "name":"search_jobs","arguments":arguments
                }}),
            )
            .await
            .expect("response");
            assert_eq!(response["error"]["code"], -32602);
        }
    }

    #[tokio::test]
    async fn blank_search_filters_return_invalid_params_before_tool_execution() {
        for arguments in [
            json!({"query":"rust","company":" "}),
            json!({"query":"rust","salary":"\t"}),
            json!({"query":"rust","experience":"\n"}),
            json!({"query":"rust","education":""}),
            json!({"query":"rust","job_type":"\r\n"}),
            json!({"query":"rust","welfare":["remote"," "]}),
        ] {
            let response = handle_input(
                &service(),
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                    "name":"search_jobs","arguments":arguments
                }}),
            )
            .await
            .expect("response");
            assert_eq!(response["error"]["code"], -32602, "{response}");
            assert!(response.get("result").is_none(), "{response}");
        }
    }

    #[tokio::test]
    async fn missing_or_invalid_jsonrpc_returns_invalid_request() {
        for request in [
            json!({"id":7,"method":"ping"}),
            json!({"jsonrpc":"1.0","id":"request-id","method":"ping"}),
        ] {
            let expected_id = request["id"].clone();
            let response = handle_input(&service(), request).await.expect("response");
            assert_eq!(response["error"]["code"], -32600);
            assert_eq!(response["id"], expected_id);
        }
    }
}
