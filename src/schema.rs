//! Shared command and MCP tool capability registry.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

use crate::BossError;
use crate::ai::MAX_AI_BASE_URL_CHARS;
use crate::campaign::{
    DEFAULT_MINIMUM_RESUME_SCORE, MAX_CAMPAIGN_NAME_CHARS, MAX_PLANS_PER_BUILD,
    MAX_RULE_VALUE_CHARS, MAX_STATE_NOTE_CHARS, MAX_TEMPLATE_CHARS,
};
use crate::notify::MAX_NOTIFICATION_EVENT_CHARS;
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
            "ai_profile_add",
            "Add or update credential-free local HTTPS OpenAI-compatible model metadata",
            json!({"type":"object","required":["name","base_url","model"],"properties":{
                "name":{"type":"string","minLength":1,"maxLength":64},
                "base_url":{"type":"string","minLength":9,"maxLength":MAX_AI_BASE_URL_CHARS},
                "model":{"type":"string","minLength":1,"maxLength":256}
            },"additionalProperties":false}),
        ),
        tool(
            "ai_profile_list",
            "List credential-free local AI model profiles",
            empty(),
        ),
        tool(
            "ai_profile_show",
            "Show one credential-free local AI model profile",
            name_schema(false),
        ),
        tool(
            "ai_profile_remove",
            "Remove one credential-free local AI model profile",
            name_schema(false),
        ),
        tool(
            "ai_draft",
            "Confirmed remote model call: generate text from one cached job and one typed local resume; never contacts a job platform",
            ai_operation_schema(),
        ),
        tool(
            "ai_score",
            "Confirmed remote model call: return strict fit score JSON for one cached job and one typed local resume; never contacts a job platform",
            ai_operation_schema(),
        ),
        tool(
            "notify_preview",
            "Render a bounded local notification summary; it never reads a webhook or uses network",
            notification_event_schema(false),
        ),
        tool(
            "notify_send",
            "Confirmed remote webhook notification with aggregate counts only; endpoint and response body are never stored",
            notification_event_schema(true),
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
            "campaign_policy_add",
            "Add or update a reusable local-only cached-job campaign policy",
            campaign_policy_schema(),
        ),
        tool(
            "campaign_policy_list",
            "List local campaign policies",
            empty(),
        ),
        tool(
            "campaign_policy_show",
            "Show one local campaign policy",
            name_schema(false),
        ),
        tool(
            "campaign_policy_remove",
            "Remove one local campaign policy",
            name_schema(false),
        ),
        tool(
            "campaign_blacklist_add",
            "Add a local company, description, or job blacklist rule",
            blacklist_schema(),
        ),
        tool(
            "campaign_blacklist_list",
            "List local campaign blacklist rules",
            empty(),
        ),
        tool(
            "campaign_blacklist_remove",
            "Remove a local campaign blacklist rule",
            blacklist_schema(),
        ),
        tool(
            "campaign_template_add",
            "Add or update a local greeting template with allow-listed placeholders only",
            json!({"type":"object","required":["name","body"],"properties":{
                "name":{"type":"string","minLength":1,"maxLength":MAX_CAMPAIGN_NAME_CHARS},
                "body":{"type":"string","minLength":1,"maxLength":MAX_TEMPLATE_CHARS}
            },"additionalProperties":false}),
        ),
        tool(
            "campaign_template_list",
            "List local greeting templates",
            empty(),
        ),
        tool(
            "campaign_template_show",
            "Show one local greeting template",
            name_schema(false),
        ),
        tool(
            "campaign_template_remove",
            "Remove one local greeting template",
            name_schema(false),
        ),
        tool(
            "campaign_template_render",
            "Render a local greeting preview from one cached job; it never sends a message",
            json!({"type":"object","required":["name","job_id"],"properties":{
                "name":{"type":"string","minLength":1,"maxLength":MAX_CAMPAIGN_NAME_CHARS},
                "job_id":{"type":"string","minLength":1}
            },"additionalProperties":false}),
        ),
        tool(
            "campaign_plan_create",
            "Create local manual-review dry-run plans from cached jobs; it never contacts a platform",
            json!({"type":"object","required":["policy"],"properties":{
                "policy":{"type":"string","minLength":1,"maxLength":MAX_CAMPAIGN_NAME_CHARS},
                "template":{"type":"string","minLength":1,"maxLength":MAX_CAMPAIGN_NAME_CHARS},
                "resume_name":{"type":"string","minLength":1,"maxLength":64},
                "limit":{"type":"integer","minimum":1,"maximum":MAX_PLANS_PER_BUILD}
            },"additionalProperties":false}),
        ),
        tool(
            "campaign_screen",
            "Score cached jobs against explicit local resume title and skills, then create local manual-review dry-run plans only",
            json!({"type":"object","required":["resume","policy"],"properties":{
                "resume":{"type":"string","minLength":1,"maxLength":64},
                "policy":{"type":"string","minLength":1,"maxLength":MAX_CAMPAIGN_NAME_CHARS},
                "template":{"type":"string","minLength":1,"maxLength":MAX_CAMPAIGN_NAME_CHARS},
                "limit":{"type":"integer","minimum":1,"maximum":MAX_PLANS_PER_BUILD,"default":20},
                "minimum_resume_score":{"type":"integer","minimum":0,"maximum":100,"default":DEFAULT_MINIMUM_RESUME_SCORE}
            },"additionalProperties":false}),
        ),
        tool(
            "campaign_plan_list",
            "List local-only campaign plans and their human workflow state",
            empty(),
        ),
        tool(
            "campaign_plan_transition",
            "Record a confirmed local human workflow transition; recorded_submitted is an attestation only and never contacts a platform",
            json!({"type":"object","required":["job_id","state","confirm"],"properties":{
                "job_id":{"type":"string","minLength":1,"maxLength":MAX_RULE_VALUE_CHARS},
                "state":{"type":"string","enum":["approved","rejected","recorded_submitted"]},
                "note":{"type":"string","minLength":1,"maxLength":MAX_STATE_NOTE_CHARS},
                "confirm":{"const":true}
            },"additionalProperties":false}),
        ),
        tool(
            "campaign_stats",
            "Summarize strictly local campaign policies, rules, templates, and plans",
            empty(),
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
                "target":{"type":"string","enum":["jobs","history","shortlist","presets","reply_rules","watches","resumes","campaign_policies","campaign_blacklist","greeting_templates","application_plans","ai_profiles","notification_audit","all"]}
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

fn campaign_policy_schema() -> Value {
    let fields = [
        "title",
        "company",
        "city",
        "district",
        "salary",
        "experience",
        "education",
        "employment_type",
        "skills",
        "welfare",
        "description",
        "address",
    ];
    let rule = json!({"type":"object","required":["field","value"],"properties":{
        "field":{"type":"string","enum":fields},
        "value":{"type":"string","minLength":1,"maxLength":MAX_RULE_VALUE_CHARS}
    },"additionalProperties":false});
    json!({"type":"object","required":["name"],"properties":{
        "name":{"type":"string","minLength":1,"maxLength":MAX_CAMPAIGN_NAME_CHARS},
        "include":{"type":"array","maxItems":32,"items":rule},
        "exclude":{"type":"array","maxItems":32,"items":rule},
        "required_welfare":{"type":"array","maxItems":16,"items":{"type":"string","minLength":1,"maxLength":MAX_RULE_VALUE_CHARS}},
        "monthly_salary_min":{"type":"integer","minimum":1},
        "monthly_salary_max":{"type":"integer","minimum":1},
        "minimum_score":{"type":"integer","minimum":0,"maximum":100}
    },"additionalProperties":false})
}

fn blacklist_schema() -> Value {
    json!({"type":"object","required":["kind","value"],"properties":{
        "kind":{"type":"string","enum":["company","description","job"]},
        "value":{"type":"string","minLength":1,"maxLength":MAX_RULE_VALUE_CHARS}
    },"additionalProperties":false})
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
                "read_only_remote":false,
                "platforms":["zhipin","zhilian","qiancheng"],
                "commands":commands,
                "mcp_tools":tools,
                "notes":{
                    "filters":"Search filters are local-only over fields returned in provider lists; no automatic detail fetch.",
                    "history":"History is BossKit local search-attempt history, not remote platform browsing history.",
                    "keyword_replies":"Keyword replies are deterministic local suggestions only and never send a platform message.",
                    "chat_messages":"Direct Zhipin greeting and text-message commands are being integrated and are not exposed through MCP.",
                    "campaign_screen":"Resume screening is deterministic and local-only over cached job title, skills, and description. It creates manual-review dry-run plans and never applies or sends a message.",
                "authentication":"login and logout are CLI-only private credential operations. Zhipin sessions are refreshed and verified through a browserless HTTPS and V8 challenge flow; other platforms use environment, stored, or hidden manual Cookie input. Authentication remains unavailable through MCP."
                },
                "risk":{
                    "remote_writes":true,
                    "confirmed_remote_notifications":["notify send"],
                    "confirmed_platform_messages":[],
                    "confirmed_model_calls":["ai draft","ai score"],
                    "all_remote_operations_read_only":false,
                    "local_writes":local_writes
                }
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

fn ai_operation_schema() -> Value {
    json!({"type":"object","required":["profile","job_id","resume_name","confirm"],"properties":{
        "profile":{"type":"string","minLength":1,"maxLength":64},
        "job_id":{"type":"string","minLength":1},
        "resume_name":{"type":"string","minLength":1,"maxLength":64},
        "confirm":{"const":true}
    },"additionalProperties":false})
}

fn notification_event_schema(confirm: bool) -> Value {
    let mut properties = json!({
        "event":{"type":"string","minLength":1,"maxLength":MAX_NOTIFICATION_EVENT_CHARS,
            "pattern":"^[a-z0-9][a-z0-9._-]{0,63}$"}
    });
    let mut required = vec!["event"];
    if confirm {
        properties["confirm"] = json!({"const":true});
        required.push("confirm");
    }
    json!({"type":"object","required":required,"properties":properties,
        "additionalProperties":false})
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
        ("campaign policy add", &["campaign_policies"]),
        ("campaign policy ls", &[]),
        ("campaign policy show", &[]),
        ("campaign policy rm", &["campaign_policies"]),
        ("campaign blacklist add", &["campaign_blacklist"]),
        ("campaign blacklist ls", &[]),
        ("campaign blacklist rm", &["campaign_blacklist"]),
        ("campaign template add", &["greeting_templates"]),
        ("campaign template ls", &[]),
        ("campaign template show", &[]),
        ("campaign template rm", &["greeting_templates"]),
        ("campaign template render", &[]),
        ("campaign plan create", &["application_plans"]),
        ("campaign screen", &["application_plans"]),
        ("campaign plan ls", &[]),
        ("campaign plan transition", &["application_plans"]),
        ("campaign stats", &[]),
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
        ("ai profile add", &["ai_profiles"]),
        ("ai profile ls", &[]),
        ("ai profile show", &[]),
        ("ai profile rm", &["ai_profiles"]),
        ("ai draft", &[]),
        ("ai score", &[]),
        ("notify preview", &[]),
        ("notify send", &["notification_audit"]),
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
                "campaign_policies",
                "campaign_blacklist",
                "greeting_templates",
                "application_plans",
                "ai_profiles",
                "notification_audit",
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
                "campaign_policies",
                "campaign_blacklist",
                "greeting_templates",
                "application_plans",
                "ai_profiles",
                "notification_audit",
            ],
        ),
    ]
    .into_iter()
    .map(|(name, local_write_targets)| CommandDefinition {
        name,
        remote_write: matches!(name, "notify send" | "mcp"),
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
        assert_eq!(native["mcp_tools"], json!(tool_registry()));
    }

    #[test]
    fn ai_profile_schema_reuses_the_shared_base_url_bound() {
        let profile_add = tool_registry()
            .into_iter()
            .find(|tool| tool.name == "ai_profile_add")
            .expect("AI profile tool");
        assert_eq!(
            profile_add.input_schema["properties"]["base_url"]["maxLength"],
            json!(MAX_AI_BASE_URL_CHARS)
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
    fn direct_chat_is_not_advertised_before_transport_integration() {
        let native = render(SchemaFormat::Native).expect("native");
        let commands = native["commands"].as_array().expect("commands");
        assert!(
            commands.iter().all(|command| !matches!(
                command["name"].as_str(),
                Some("chat greet" | "chat send")
            ))
        );
        assert_eq!(native["risk"]["confirmed_platform_messages"], json!([]));
        assert!(
            tool_registry()
                .iter()
                .all(|tool| !matches!(tool.name, "chat_greet" | "chat_send"))
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
