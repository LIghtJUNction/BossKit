//! Shared command and MCP tool capability registry.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

use crate::BossError;
use crate::reply::{MAX_KEYWORD_CHARS, MAX_MESSAGE_CHARS, MAX_REPLY_CHARS};

/// Supported capability schema wrapper.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SchemaFormat {
    /// BossKit-native command and tool inventory.
    Native,
    /// OpenAI function tool wrappers.
    OpenaiTools,
    /// Anthropic tool wrappers.
    AnthropicTools,
    /// MCP tool definitions.
    McpTools,
}

/// A shared tool definition used by MCP and schema output.
#[derive(Clone, Debug, Serialize)]
pub struct ToolDefinition {
    /// Stable snake_case tool name.
    pub name: &'static str,
    /// Safe operator description.
    pub description: &'static str,
    /// Strict JSON object input schema.
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Returns every MCP tool from one authoritative registry.
#[must_use]
pub fn tool_registry() -> Vec<ToolDefinition> {
    let empty = || json!({"type":"object","properties":{},"additionalProperties":false});
    let platform = json!({"type":"string","enum":["all","zhipin","zhilian","qiancheng"]});
    vec![
        tool("platforms", "List supported job platforms", empty()),
        tool("cities", "List shared logical city mappings", empty()),
        tool(
            "search_jobs",
            "Search jobs and apply local-only list-field filters",
            json!({"type":"object","properties":{
                "query":{"type":"string","minLength":1},"preset":{"type":"string","minLength":1},"platform":platform,
                "city":{"type":"string"},"page":{"type":"integer","minimum":1},
                "limit":{"type":"integer","minimum":1},
                "company":{"type":"string","minLength":1},
                "salary":{"type":"string","minLength":1},
                "experience":{"type":"string","minLength":1},
                "education":{"type":"string","minLength":1},
                "job_type":{"type":"string","minLength":1},
                "welfare":{"type":"array","items":{"type":"string","minLength":1}}
            },"anyOf":[{"required":["query"]},{"required":["preset"]}],
            "additionalProperties":false}),
        ),
        tool(
            "list_jobs",
            "List locally cached jobs",
            json!({"type":"object","properties":{
                "platform":{"type":"string","enum":["all","zhipin","zhilian","qiancheng"]},
                "limit":{"type":"integer","minimum":1}
            },"additionalProperties":false}),
        ),
        tool(
            "show_job",
            "Show one cached job",
            json!({"type":"object","required":["id"],"properties":{
                "id":{"type":"string","minLength":1}
            },"additionalProperties":false}),
        ),
        tool(
            "job_detail",
            "Fetch or reuse read-only detail for one locally cached job",
            json!({"type":"object","required":["id"],"properties":{
                "id":{"type":"string","minLength":1},"refresh":{"type":"boolean"}
            },"additionalProperties":false}),
        ),
        tool(
            "search_history",
            "List BossKit local search-attempt history, not platform browsing history",
            json!({"type":"object","properties":{
                "platform":{"type":"string","enum":["all","zhipin","zhilian","qiancheng"]},
                "limit":{"type":"integer","minimum":1}
            },"additionalProperties":false}),
        ),
        tool(
            "export_jobs",
            "Return structured local jobs or shortlist snapshots without filesystem access",
            json!({"type":"object","properties":{
                "source":{"type":"string","enum":["jobs","shortlist"]},
                "platform":{"type":"string","enum":["all","zhipin","zhilian","qiancheng"]},
                "limit":{"type":"integer","minimum":1},"include_ids":{"type":"boolean"}
            },"additionalProperties":false}),
        ),
        tool(
            "status",
            "Inspect local cookie environment status without network access",
            platform_schema(),
        ),
        tool(
            "doctor",
            "Run local data and registration diagnostics without network access",
            platform_schema(),
        ),
        tool(
            "schema",
            "Describe BossKit command and tool capabilities",
            json!({"type":"object","required":["format"],"properties":{
                "format":{"type":"string","enum":["native","openai-tools","anthropic-tools","mcp-tools"]}
            },"additionalProperties":false}),
        ),
        tool(
            "shortlist_add",
            "Add a cached job snapshot to the local shortlist",
            json!({"type":"object","required":["job_id"],"properties":{
                "job_id":{"type":"string","minLength":1},
                "tags":{"type":"array","items":{"type":"string"}},
                "note":{"type":"string"}
            },"additionalProperties":false}),
        ),
        tool(
            "shortlist_list",
            "List local shortlist entries",
            json!({"type":"object","properties":{"tag":{"type":"string"}},"additionalProperties":false}),
        ),
        tool(
            "shortlist_annotate",
            "Annotate one local shortlist entry",
            json!({"type":"object","required":["job_id"],"properties":{
                "job_id":{"type":"string","minLength":1},
                "add_tags":{"type":"array","items":{"type":"string"}},
                "remove_tags":{"type":"array","items":{"type":"string"}},
                "note":{"type":"string"}
            },"additionalProperties":false}),
        ),
        tool(
            "shortlist_remove",
            "Remove one local shortlist entry",
            json!({"type":"object","required":["job_id"],"properties":{
                "job_id":{"type":"string","minLength":1}
            },"additionalProperties":false}),
        ),
        tool(
            "shortlist_compare",
            "Compare local shortlist entries",
            json!({"type":"object","properties":{"tag":{"type":"string"}},"additionalProperties":false}),
        ),
        tool(
            "preset_add",
            "Add or update a validated local search preset",
            search_named_schema(false),
        ),
        tool("preset_list", "List local search presets", empty()),
        tool(
            "preset_show",
            "Show one local search preset",
            name_schema(false),
        ),
        tool(
            "preset_remove",
            "Remove one local search preset",
            name_schema(false),
        ),
        tool(
            "watch_add",
            "Add a foreground watch with a copied search specification",
            search_named_schema(true),
        ),
        tool("watch_list", "List explicit foreground watches", empty()),
        tool(
            "watch_show",
            "Show one explicit foreground watch",
            name_schema(false),
        ),
        tool(
            "watch_run",
            "Run one or all foreground watches using remote reads and local writes",
            json!({"type":"object","properties":{
                "name":{"type":"string","minLength":1},"all":{"type":"boolean"}
            },"additionalProperties":false}),
        ),
        tool(
            "watch_remove",
            "Remove one explicit foreground watch",
            name_schema(false),
        ),
        tool(
            "resume_init",
            "Initialize one strictly local typed resume",
            json!({"type":"object","required":["name"],"properties":{
                "name":{"type":"string","minLength":1,"maxLength":64},"title":{"type":"string"}
            },"additionalProperties":false}),
        ),
        tool("resume_list", "List strictly local typed resumes", empty()),
        tool(
            "resume_show",
            "Show one strictly local typed resume",
            name_schema(false),
        ),
        tool(
            "resume_set",
            "Set an allow-listed field on a strictly local resume",
            json!({"type":"object","required":["name","field","value"],"properties":{
                "name":{"type":"string","minLength":1},"field":{"type":"string","minLength":1},
                "value":{"type":"string"}
            },"additionalProperties":false}),
        ),
        tool(
            "resume_skills",
            "Add or remove skills on a strictly local resume",
            json!({"type":"object","required":["name"],"properties":{
                "name":{"type":"string","minLength":1},
                "add":{"type":"array","items":{"type":"string","minLength":1}},
                "remove":{"type":"array","items":{"type":"string","minLength":1}}
            },"additionalProperties":false}),
        ),
        tool(
            "resume_clone",
            "Clone a strictly local typed resume",
            json!({"type":"object","required":["name","new_name"],"properties":{
                "name":{"type":"string","minLength":1},"new_name":{"type":"string","minLength":1}
            },"additionalProperties":false}),
        ),
        tool(
            "resume_diff",
            "Compare two strictly local typed resumes",
            json!({"type":"object","required":["left","right"],"properties":{
                "left":{"type":"string","minLength":1},"right":{"type":"string","minLength":1}
            },"additionalProperties":false}),
        ),
        tool(
            "resume_remove",
            "Remove a strictly local resume only with explicit confirmation",
            json!({"type":"object","required":["name","confirm"],"properties":{
                "name":{"type":"string","minLength":1},"confirm":{"const":true}
            },"additionalProperties":false}),
        ),
        tool(
            "keyword_reply_add",
            "Add or update a local keyword-reply rule; it never sends a platform message",
            json!({"type":"object","required":["keyword","reply"],"properties":{
                "keyword":{"type":"string","minLength":1,"maxLength":MAX_KEYWORD_CHARS},
                "reply":{"type":"string","minLength":1,"maxLength":MAX_REPLY_CHARS}
            },"additionalProperties":false}),
        ),
        tool(
            "keyword_reply_list",
            "List local keyword-reply rules",
            empty(),
        ),
        tool(
            "keyword_reply_remove",
            "Remove one local keyword-reply rule",
            json!({"type":"object","required":["keyword"],"properties":{
                "keyword":{"type":"string","minLength":1,"maxLength":MAX_KEYWORD_CHARS}
            },"additionalProperties":false}),
        ),
        tool(
            "keyword_reply_match",
            "Match local text deterministically and return a suggestion only; never sends a platform message",
            json!({"type":"object","required":["message"],"properties":{
                "message":{"type":"string","minLength":1,"maxLength":MAX_MESSAGE_CHARS}
            },"additionalProperties":false}),
        ),
        tool(
            "stats",
            "Summarize exact strictly local workflow data",
            json!({"type":"object","properties":{
                "days":{"type":"integer","minimum":1}
            },"additionalProperties":false}),
        ),
        tool(
            "clean_preview",
            "Preview exact known local files; MCP never archives or removes them",
            json!({"type":"object","required":["target"],"properties":{
                "target":{"type":"string","enum":["jobs","history","shortlist","presets","reply_rules","watches","resumes","all"]}
            },"additionalProperties":false}),
        ),
    ]
}

fn name_schema(confirm: bool) -> Value {
    if confirm {
        json!({"type":"object","required":["name","confirm"],"properties":{
            "name":{"type":"string","minLength":1},"confirm":{"const":true}
        },"additionalProperties":false})
    } else {
        json!({"type":"object","required":["name"],"properties":{
            "name":{"type":"string","minLength":1}
        },"additionalProperties":false})
    }
}

fn search_named_schema(include_preset: bool) -> Value {
    let mut properties = json!({
        "name":{"type":"string","minLength":1,"maxLength":64},
        "query":{"type":"string","minLength":1},
        "platform":{"type":"string","enum":["all","zhipin","zhilian","qiancheng"]},
        "city":{"type":"string","minLength":1},"page":{"type":"integer","minimum":1},
        "limit":{"type":"integer","minimum":1},"company":{"type":"string","minLength":1},
        "salary":{"type":"string","minLength":1},"experience":{"type":"string","minLength":1},
        "education":{"type":"string","minLength":1},"job_type":{"type":"string","minLength":1},
        "welfare":{"type":"array","items":{"type":"string","minLength":1}}
    });
    if include_preset {
        properties["preset"] = json!({"type":"string","minLength":1});
        json!({"type":"object","required":["name"],"properties":properties,
            "oneOf":[{"required":["query"]},{"required":["preset"]}],
            "additionalProperties":false})
    } else {
        json!({"type":"object","required":["name","query"],"properties":properties,
            "additionalProperties":false})
    }
}

/// Renders one requested schema wrapper.
pub fn render(format: SchemaFormat) -> Result<Value, BossError> {
    let tools = tool_registry();
    Ok(match format {
        SchemaFormat::Native => {
            let commands = command_registry();
            let local_writes: BTreeSet<&str> = commands
                .iter()
                .flat_map(|command| command.local_write_targets.iter().copied())
                .collect();
            json!({
                "name":"boss",
                "read_only_remote":true,
                "platforms":["zhipin","zhilian","qiancheng"],
                "commands":commands,
                "mcp_tools":tools,
                "notes":{
                    "filters":"Search filters are local-only over fields returned in provider lists; no automatic detail fetch.",
                    "history":"History is BossKit local search-attempt history, not remote platform browsing history.",
                    "keyword_replies":"Keyword replies are deterministic local suggestions only and never send a platform message.",
                    "authentication":"login and logout are CLI-only private local credential operations. They are deliberately unavailable through MCP and login never performs a network validation request."
                },
                "risk":{"remote_writes":false,"local_writes":local_writes}
            })
        }
        SchemaFormat::OpenaiTools => Value::Array(
            tools.into_iter().map(|tool| json!({"type":"function","function":{
                "name":tool.name,"description":tool.description,"parameters":tool.input_schema
            }})).collect()
        ),
        SchemaFormat::AnthropicTools => Value::Array(
            tools.into_iter().map(|tool| json!({
                "name":tool.name,"description":tool.description,"input_schema":tool.input_schema
            })).collect()
        ),
        SchemaFormat::McpTools => serde_json::to_value(tools)
            .map_err(|error| BossError::ConfigJson(error.to_string()))?,
    })
}

fn tool(name: &'static str, description: &'static str, input_schema: Value) -> ToolDefinition {
    ToolDefinition {
        name,
        description,
        input_schema,
    }
}

fn platform_schema() -> Value {
    json!({"type":"object","properties":{
        "platform":{"type":"string","enum":["all","zhipin","zhilian","qiancheng"]}
    },"additionalProperties":false})
}

#[derive(Serialize)]
struct CommandDefinition {
    name: &'static str,
    remote_write: bool,
    local_write: bool,
    local_write_targets: &'static [&'static str],
}

fn command_registry() -> Vec<CommandDefinition> {
    [
        ("platforms", &[][..]),
        ("cities", &[]),
        ("search", &["history", "jobs_cache"]),
        ("ls", &[]),
        ("show", &[]),
        ("detail", &["jobs_cache"]),
        ("history", &[]),
        ("export", &["export_target"]),
        ("config ls", &[]),
        ("config get", &[]),
        ("config set", &["config"]),
        ("config reset", &["config"]),
        ("login", &["private_auth_store"]),
        ("logout", &["private_auth_store"]),
        ("status", &[]),
        ("doctor", &["doctor_probe"]),
        ("schema", &[]),
        ("shortlist add", &["shortlist"]),
        ("shortlist ls", &[]),
        ("shortlist annotate", &["shortlist"]),
        ("shortlist rm", &["shortlist"]),
        ("shortlist compare", &[]),
        ("preset add", &["presets"]),
        ("preset ls", &[]),
        ("preset show", &[]),
        ("preset rm", &["presets"]),
        ("reply add", &["reply_rules"]),
        ("reply ls", &[]),
        ("reply rm", &["reply_rules"]),
        ("reply match", &[]),
        ("watch add", &["watches"]),
        ("watch ls", &[]),
        ("watch show", &[]),
        ("watch rm", &["watches"]),
        ("watch run", &["history", "jobs_cache", "watches"]),
        ("resume init", &["resumes"]),
        ("resume ls", &[]),
        ("resume show", &[]),
        ("resume set", &["resumes"]),
        ("resume skills", &["resumes"]),
        ("resume clone", &["resumes"]),
        ("resume diff", &[]),
        ("resume import", &["resumes"]),
        ("resume export", &["resume_export_target"]),
        ("resume rm", &["resumes"]),
        ("stats", &[]),
        (
            "clean",
            &[
                "jobs_cache",
                "history",
                "shortlist",
                "presets",
                "reply_rules",
                "watches",
                "resumes",
            ],
        ),
        (
            "mcp",
            &[
                "doctor_probe",
                "history",
                "jobs_cache",
                "shortlist",
                "presets",
                "reply_rules",
                "watches",
                "resumes",
            ],
        ),
    ]
    .into_iter()
    .map(|(name, local_write_targets)| CommandDefinition {
        name,
        remote_write: false,
        local_write: !local_write_targets.is_empty(),
        local_write_targets,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_shapes_are_distinct_and_valid() {
        let openai = render(SchemaFormat::OpenaiTools).expect("openai");
        let anthropic = render(SchemaFormat::AnthropicTools).expect("anthropic");
        assert!(openai[0]["function"]["parameters"].is_object());
        assert!(anthropic[0]["input_schema"].is_object());
    }

    #[test]
    fn native_registry_contains_every_mcp_tool() {
        let native = render(SchemaFormat::Native).expect("native");
        assert_eq!(
            (
                native["mcp_tools"].as_array().map(Vec::len),
                tool_registry().len(),
            ),
            (Some(39), 39)
        );
    }

    #[test]
    fn authentication_commands_are_native_only() {
        let native = render(SchemaFormat::Native).expect("native");
        let commands = native["commands"].as_array().expect("commands");
        assert!(commands.iter().any(|command| command["name"] == "login"));
        assert!(commands.iter().any(|command| command["name"] == "logout"));
        assert!(
            tool_registry()
                .iter()
                .all(|tool| !matches!(tool.name, "login" | "logout"))
        );
    }

    #[test]
    fn search_and_mcp_truthfully_report_local_writes() {
        let native = render(SchemaFormat::Native).expect("native");
        let commands = native["commands"].as_array().expect("commands");
        let search = commands
            .iter()
            .find(|command| command["name"] == "search")
            .expect("search");
        let mcp = commands
            .iter()
            .find(|command| command["name"] == "mcp")
            .expect("mcp");
        assert_eq!(
            (
                search["local_write"].as_bool(),
                mcp["local_write"].as_bool()
            ),
            (Some(true), Some(true))
        );
    }

    #[test]
    fn native_risk_targets_are_derived_from_command_targets() {
        let native = render(SchemaFormat::Native).expect("native");
        let command_targets: BTreeSet<&str> = native["commands"]
            .as_array()
            .expect("commands")
            .iter()
            .flat_map(|command| {
                command["local_write_targets"]
                    .as_array()
                    .expect("targets")
                    .iter()
                    .map(|target| target.as_str().expect("string target"))
            })
            .collect();
        let risk_targets: BTreeSet<&str> = native["risk"]["local_writes"]
            .as_array()
            .expect("risk targets")
            .iter()
            .map(|target| target.as_str().expect("string target"))
            .collect();
        assert_eq!(command_targets, risk_targets);
    }
}
