use assert_cmd::Command;
use serde_json::Value;
use tempfile::tempdir;

fn run_json(data_dir: &std::path::Path, args: &[&str]) -> Value {
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", data_dir)
        .env_remove("BOSS_ZHIPIN_COOKIE")
        .env_remove("BOSS_ZHILIAN_COOKIE")
        .env_remove("BOSS_QIANCHENG_COOKIE")
        .env_remove("BOSS_LLM_API_KEY")
        .env_remove("BOSS_NOTIFY_WEBHOOK_URL")
        .args(args)
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).expect("json")
}

fn run_mcp(data_dir: &std::path::Path, requests: &[Value]) -> Vec<Value> {
    let mut input = requests
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    input.push('\n');
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", data_dir)
        .env_remove("BOSS_ZHIPIN_COOKIE")
        .env_remove("BOSS_ZHILIAN_COOKIE")
        .env_remove("BOSS_QIANCHENG_COOKIE")
        .env_remove("BOSS_NOTIFY_WEBHOOK_URL")
        .arg("mcp")
        .write_stdin(input)
        .output()
        .expect("run mcp");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("jsonrpc"))
        .collect()
}

#[test]
fn markdown_is_default_and_json_requires_explicit_flag() {
    let directory = tempdir().expect("temporary directory");
    let markdown = Command::cargo_bin("boss")
        .expect("binary")
        .env("BOSS_DATA_DIR", directory.path())
        .arg("cities")
        .output()
        .expect("markdown output");
    assert!(markdown.status.success());
    assert!(String::from_utf8_lossy(&markdown.stdout).starts_with("- **data**:"));

    let json = Command::cargo_bin("boss")
        .expect("binary")
        .env("BOSS_DATA_DIR", directory.path())
        .args(["--json", "cities"])
        .output()
        .expect("json output");
    assert!(json.status.success());
    let value: Value = serde_json::from_slice(&json.stdout).expect("json");
    assert_eq!(value["ok"], true);
}

fn seed_jobs(directory: &std::path::Path) {
    let jobs = serde_json::json!([
        {
            "id":"zhipin-job","platform":"zhipin","remote_id":"remote-1",
            "title":"Rust","company":"Example","city":"深圳","salary":"20K",
            "url":"https://example.test/1","description":"cached detail",
            "district":"","experience":"3年","education":"本科",
            "employment_type":"全职","skills":["Rust"],"welfare":["双休"],"address":""
        },
        {
            "id":"legacy-job","platform":"zhilian","remote_id":"remote-2",
            "title":"Rust","company":"Example","city":"深圳","salary":"21K",
            "url":"https://example.test/2"
        },
        {
            "id":"legacy-job-2","platform":"zhilian","remote_id":"remote-3",
            "title":"Senior Rust","company":"Example","city":"上海","salary":"30K",
            "url":"https://example.test/3"
        }
    ]);
    std::fs::write(
        directory.join("jobs.json"),
        serde_json::to_vec(&jobs).expect("serialize"),
    )
    .expect("write cache");
}

#[test]
fn boss_only_cli_rejects_platform_flags_and_ignores_legacy_cached_jobs() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());

    let jobs = run_json(directory.path(), &["ls", "--limit", "20"]);
    assert_eq!(jobs["data"].as_array().expect("jobs").len(), 1);
    assert_eq!(jobs["data"][0]["platform"], "zhipin");

    for command in [["show", "legacy-job"], ["detail", "legacy-job"]] {
        let output = Command::cargo_bin("boss")
            .expect("binary")
            .arg("--json")
            .env("BOSS_DATA_DIR", directory.path())
            .args(command)
            .output()
            .expect("run");
        assert!(!output.status.success());
        let error: Value = serde_json::from_slice(&output.stdout).expect("error json");
        assert_eq!(error["error"]["code"], "invalid_argument");
    }

    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args(["search", "rust", "--platform", "zhipin"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).expect("error json");
    assert_eq!(error["error"]["code"], "invalid_argument");
}

#[test]
fn mcp_stdio_enforces_search_preset_and_watch_relationships() {
    let directory = tempdir().expect("temporary directory");
    let responses = run_mcp(
        directory.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"preset_add","arguments":{"name":"saved","query":"rust"}
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                "name":"preset_add","arguments":{"name":"bad","query":"rust","preset":"saved"}
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                "name":"search_jobs","arguments":{}
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{
                "name":"watch_add","arguments":{"name":"missing"}
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{
                "name":"watch_add","arguments":{"name":"both","query":"rust","preset":"saved"}
            }}),
        ],
    );
    let tools = responses[0]["result"]["tools"].as_array().expect("tools");
    let search = tools
        .iter()
        .find(|tool| tool["name"] == "search_jobs")
        .expect("search");
    let watch = tools
        .iter()
        .find(|tool| tool["name"] == "watch_add")
        .expect("watch");
    assert!(
        search["inputSchema"]["anyOf"].is_array()
            && watch["inputSchema"]["oneOf"].is_array()
            && responses[1]["result"]["isError"] == false
            && responses[2]["error"]["code"] == -32602
            && responses[3]["error"]["code"] == -32602
            && responses[4]["error"]["code"] == -32602
            && responses[5]["error"]["code"] == -32602
    );
}

#[test]
fn invalid_limit_is_a_json_error() {
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .args(["search", "rust", "--limit", "0"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["code"], "invalid_argument");
}

#[test]
fn login_removed_phone_manual_and_repair_modes_before_network() {
    let directory = tempdir().expect("temporary directory");
    for flag in ["--phone", "--manual", "--repair"] {
        let output = Command::cargo_bin("boss")
            .expect("binary")
            .arg("--json")
            .env("BOSS_DATA_DIR", directory.path())
            .args(["login", flag])
            .output()
            .expect("run");
        assert!(!output.status.success());
        let error: Value = serde_json::from_slice(&output.stdout).expect("error json");
        assert_eq!(error["error"]["code"], "invalid_argument");
    }
}

#[cfg(unix)]
#[test]
fn login_requires_a_fresh_cookie_and_preserves_the_stored_session() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempdir().expect("temporary directory");
    let auth_dir = directory.path().join(".auth");
    std::fs::create_dir(&auth_dir).expect("auth directory");
    std::fs::set_permissions(&auth_dir, std::fs::Permissions::from_mode(0o700))
        .expect("auth permissions");
    let sessions = auth_dir.join("sessions.json");
    std::fs::write(
        &sessions,
        br#"{"selected_account":"default","accounts":{"default":{"role":"geek","zhipin":{"cookie":"wt2=fixture"}}}}"#,
    )
    .expect("session file");
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o600))
        .expect("session permissions");

    let before = std::fs::read(&sessions).expect("stored session");
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .env("BOSS_ZHIPIN_COOKIE", "wt2=ENV_COOKIE_MUST_NOT_BE_USED")
        .arg("login")
        .output()
        .expect("run");
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).expect("error json");
    assert_eq!(error["error"]["code"], "authentication_error");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--cookie-stdin"))
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("ENV_COOKIE_MUST_NOT_BE_USED"));
    assert_eq!(std::fs::read(&sessions).expect("stored session"), before);
}

#[test]
fn login_cookie_stdin_rejects_multiple_lines_without_echo_or_network() {
    let directory = tempdir().expect("temporary directory");
    let secret = "wt2=FRESH_COOKIE_MUST_NOT_APPEAR\nsecond=value\n";
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args(["login", "--cookie-stdin", "--role", "recruiter"])
        .write_stdin(secret)
        .output()
        .expect("run");
    assert!(!output.status.success());
    let rendered = String::from_utf8_lossy(&output.stdout);
    let error: Value = serde_json::from_slice(&output.stdout).expect("error json");
    assert_eq!(error["error"]["code"], "authentication_error");
    assert!(!rendered.contains("FRESH_COOKIE_MUST_NOT_APPEAR"));
    assert!(!directory.path().join(".auth").exists());
}

#[test]
fn recruiter_inbox_requires_an_explicit_account_and_supports_native_filters() {
    let directory = tempdir().expect("temporary directory");
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args(["recruiter", "inbox", "--pending", "--job", "AI应用提效官"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).expect("error json");
    assert_eq!(error["error"]["code"], "invalid_argument");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--account"))
    );

    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args([
            "--account",
            "lty",
            "recruiter",
            "inbox",
            "--all",
            "--page",
            "2",
        ])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).expect("error json");
    assert_eq!(error["error"]["code"], "invalid_argument");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("cannot be used with"))
    );
}

#[test]
fn recruiter_candidates_are_bounded_and_require_an_explicit_account() {
    let directory = tempdir().expect("temporary directory");
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args(["recruiter", "candidates", "视频剪辑"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).expect("error json");
    assert_eq!(error["error"]["code"], "invalid_argument");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--account"))
    );

    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args([
            "--account",
            "lty",
            "recruiter",
            "candidates",
            "video",
            "--limit",
            "6",
        ])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).expect("error json");
    assert_eq!(error["error"]["code"], "invalid_argument");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("between 1 and 5"))
    );
}

#[test]
fn recruiter_inbox_brief_is_a_native_candidate_projection() {
    let directory = tempdir().expect("temporary directory");
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args([
            "recruiter",
            "inbox",
            "--brief",
            "--job",
            "AI应用提效官",
            "--page",
            "3",
        ])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).expect("error json");
    assert_eq!(error["error"]["code"], "invalid_argument");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--account"))
    );
}

#[test]
fn recruiter_resumes_supports_bounded_batch_brief_without_account_fallback() {
    let directory = tempdir().expect("temporary directory");
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args(["recruiter", "resumes", "665249929", "586819258", "--brief"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).expect("error json");
    assert_eq!(error["error"]["code"], "invalid_argument");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--account"))
    );
}

#[test]
fn unknown_command_keeps_the_unlocalized_json_parse_error() {
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .arg("not-a-command")
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());

    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["code"], "invalid_argument");
    let message = value["error"]["message"].as_str().expect("message");
    assert!(
        message.contains("Usage: boss [OPTIONS] <COMMAND>"),
        "{message}"
    );
    assert!(!message.contains("<命令>"), "{message}");
}

#[test]
fn data_dir_selects_the_cache_used_by_ls() {
    let directory = tempdir().expect("temporary directory");
    let jobs = serde_json::json!([{
        "id":"cached-job","platform":"zhipin","remote_id":"remote",
        "title":"Rust","company":"Example","city":"深圳","salary":"20K",
        "url":"https://example.test/job"
    }]);
    std::fs::write(
        directory.path().join("jobs.json"),
        serde_json::to_vec(&jobs).expect("serialize"),
    )
    .expect("write cache");
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args(["ls", "--limit", "1"])
        .output()
        .expect("run");
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["data"][0]["id"], "cached-job");
}

#[test]
fn account_resume_show_is_read_only_and_fails_without_a_session_before_network() {
    let directory = tempdir().expect("temporary directory");
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .env_remove("BOSS_ZHIPIN_COOKIE")
        .args(["account", "resume", "show"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["code"], "authentication_error");
    assert!(!directory.path().join(".auth").exists());
}

#[test]
fn chat_rejects_custom_message_arguments_without_echoing_their_value() {
    let secret = "GREETING_SECRET_MUST_NOT_APPEAR";
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .args(["chat", "greet", "zhipin-job", "--message", secret])
        .output()
        .expect("run");
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["code"], "invalid_argument");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
}

#[test]
fn chat_greet_requires_yes_before_using_runtime_credentials() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    std::fs::write(directory.path().join("config.json"), b"{invalid").expect("invalid config");
    let secret = "wt2=GREETING_COOKIE_MUST_NOT_APPEAR";
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .env("BOSS_ZHIPIN_COOKIE", secret)
        .args(["chat", "greet", "zhipin-job"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["code"], "invalid_argument");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!directory.path().join(".auth").exists());
}

#[test]
fn chat_send_requires_yes_before_service_discovery_and_never_echoes_text() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    std::fs::write(directory.path().join("config.json"), b"{invalid").expect("invalid config");
    let secret = "MESSAGE_BODY_MUST_NOT_APPEAR";
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .env("BOSS_ZHIPIN_COOKIE", "wt2=COOKIE_MUST_NOT_APPEAR")
        .args(["chat", "send", "zhipin-job", "--message", secret])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let rendered = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["code"], "invalid_argument");
    assert!(!rendered.contains(secret) && !rendered.contains("COOKIE_MUST_NOT_APPEAR"));
}

#[test]
fn chat_exchange_wechat_requires_yes_before_using_runtime_credentials() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    std::fs::write(directory.path().join("config.json"), b"{invalid").expect("invalid config");
    let secret = "wt2=EXCHANGE_COOKIE_MUST_NOT_APPEAR";
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .env("BOSS_ZHIPIN_COOKIE", secret)
        .args(["chat", "exchange-wechat", "zhipin-job"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let rendered = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["code"], "invalid_argument");
    assert!(!rendered.contains(secret) && !directory.path().join(".auth").exists());
}

#[test]
fn chat_send_rejects_invalid_text_before_credentials_without_echoing_it() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    let secret = "PRIVATE_LINE\nMUST_NOT_APPEAR";
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .env_remove("BOSS_ZHIPIN_COOKIE")
        .args(["chat", "send", "zhipin-job", "--message", secret, "--yes"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let rendered = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["code"], "invalid_argument");
    assert!(!rendered.contains("PRIVATE_LINE") && !rendered.contains("MUST_NOT_APPEAR"));
}

#[test]
fn chat_send_rejects_link_references_before_credentials() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .env("BOSS_ZHIPIN_COOKIE", "wt2=COOKIE_MUST_NOT_BE_USED")
        .args([
            "chat",
            "send",
            "zhipin-job",
            "--message",
            "项目地址 https://github.com/example/project",
            "--yes",
        ])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let rendered = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["code"], "invalid_argument");
    assert!(
        value["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("plain text"))
    );
    assert!(!rendered.contains("COOKIE_MUST_NOT_BE_USED"));
}

#[test]
fn recruiter_and_direct_chat_are_cli_only_and_schema_marks_confirmed_writes() {
    let directory = tempdir().expect("temporary directory");
    let responses = run_mcp(
        directory.path(),
        &[serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    assert!(
        responses[0]["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .all(|tool| !matches!(
                tool["name"].as_str(),
                Some(
                    "chat_greet" | "chat_send" | "chat_history" | "chat_inbox" | "recruiter_reply"
                )
            ))
    );

    let schema = run_json(directory.path(), &["schema", "--format", "native"]);
    let commands = schema["data"]["commands"].as_array().expect("commands");
    let greet = commands
        .iter()
        .find(|command| command["name"] == "chat greet")
        .expect("native greet");
    let send = commands
        .iter()
        .find(|command| command["name"] == "chat send")
        .expect("native send");
    let exchange = commands
        .iter()
        .find(|command| command["name"] == "chat exchange-wechat")
        .expect("native exchange-wechat");
    let history = commands
        .iter()
        .find(|command| command["name"] == "chat history")
        .expect("native history");
    let inbox = commands
        .iter()
        .find(|command| command["name"] == "chat inbox")
        .expect("native inbox");
    let recruiter_reply = commands
        .iter()
        .find(|command| command["name"] == "recruiter reply")
        .expect("native recruiter reply");
    assert!(
        greet["remote_write"] == true
            && greet["local_write"] == true
            && send["remote_write"] == true
            && send["local_write"] == true
            && exchange["remote_write"] == true
            && exchange["local_write"] == true
            && history["remote_write"] == false
            && history["local_write"] == true
            && inbox["remote_write"] == false
            && inbox["local_write"] == true
            && recruiter_reply["remote_write"] == true
            && recruiter_reply["local_write"] == true
    );
    assert_eq!(
        schema["data"]["risk"]["confirmed_platform_messages"],
        serde_json::json!([
            "chat greet",
            "chat send",
            "chat exchange-wechat",
            "recruiter reply"
        ])
    );
}

#[test]
fn chat_history_is_read_only_and_rejects_missing_jobs_before_credentials() {
    let directory = tempdir().expect("temporary directory");
    let secret = "wt2=HISTORY_COOKIE_MUST_NOT_APPEAR";
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .env("BOSS_ZHIPIN_COOKIE", secret)
        .args(["chat", "history", "missing-job"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let rendered = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["code"], "invalid_argument");
    assert!(!rendered.contains(secret));
    assert!(!directory.path().join(".auth").exists());
}

#[test]
fn chat_inbox_is_bounded_and_rejects_invalid_local_targets_before_credentials() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    let secret = "wt2=INBOX_COOKIE_MUST_NOT_APPEAR";

    for args in [
        vec!["chat", "inbox", "missing-job"],
        vec!["chat", "inbox", "zhipin-job", "zhipin-job"],
        vec!["chat", "inbox", "legacy-job"],
    ] {
        let output = Command::cargo_bin("boss")
            .expect("binary")
            .arg("--json")
            .env("BOSS_DATA_DIR", directory.path())
            .env("BOSS_ZHIPIN_COOKIE", secret)
            .args(args)
            .output()
            .expect("run");
        assert!(!output.status.success());
        let rendered = String::from_utf8_lossy(&output.stdout);
        let value: Value = serde_json::from_slice(&output.stdout).expect("json");
        assert_eq!(value["error"]["code"], "invalid_argument");
        assert!(!rendered.contains(secret));
        assert!(!directory.path().join(".auth").exists());
    }

    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args(["chat", "inbox", "a", "b", "c", "d", "e", "f"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["code"], "invalid_argument");
}

#[test]
fn chat_inbox_without_ids_is_a_safe_noop_without_cached_jobs() {
    let directory = tempdir().expect("temporary directory");
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args(["chat", "inbox"])
        .output()
        .expect("run");
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["count"], 0);
    assert_eq!(value["data"]["network_checked"], false);
    assert_eq!(value["data"]["verification"], "no_cached_zhipin_jobs");
}

#[test]
fn version_exits_successfully_on_stdout() {
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .arg("--version")
        .output()
        .expect("run");
    assert!(output.status.success());
    assert!(!output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn cities_reports_exact_shared_mapping() {
    let directory = tempdir().expect("temporary directory");
    let value = run_json(directory.path(), &["cities"]);
    assert_eq!(value["data"]["count"], 10);
}

#[test]
fn notification_preview_is_local_and_send_requires_confirmation_or_runtime_configuration() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    let preview = run_json(directory.path(), &["notify", "preview", "campaign.ready"]);
    assert_eq!(
        (
            preview["data"]["mode"].as_str(),
            preview["data"]["sent"].as_bool(),
            preview["data"]["payload"]["summary"]["cached_jobs"].as_u64(),
        ),
        (Some("local_notification_preview"), Some(false), Some(1))
    );
    assert!(!directory.path().join("notification_audit.json").exists());

    let unconfirmed = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .env_remove("BOSS_NOTIFY_WEBHOOK_URL")
        .args(["notify", "send", "campaign.ready"])
        .output()
        .expect("run");
    assert!(!unconfirmed.status.success());
    let value: Value = serde_json::from_slice(&unconfirmed.stdout).expect("json");
    assert_eq!(value["error"]["code"], "invalid_argument");
    assert!(!directory.path().join("notification_audit.json").exists());

    let unconfigured = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .env_remove("BOSS_NOTIFY_WEBHOOK_URL")
        .args(["notify", "send", "campaign.ready", "--yes"])
        .output()
        .expect("run");
    assert!(!unconfigured.status.success());
    let value: Value = serde_json::from_slice(&unconfigured.stdout).expect("json");
    assert_eq!(value["error"]["code"], "notification_error");
    let audit: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("notification_audit.json")).expect("audit"),
    )
    .expect("audit json");
    assert_eq!(
        audit,
        serde_json::json!([{
            "event":"campaign.ready", "status":"failure", "timestamp":audit[0]["timestamp"]
        }])
    );
}

#[test]
fn mcp_notification_tools_match_schema_and_reject_unconfirmed_send_without_network() {
    let directory = tempdir().expect("temporary directory");
    let responses = run_mcp(
        directory.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"notify_preview","arguments":{"event":"watch.complete"}
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                "name":"notify_send","arguments":{"event":"watch.complete","confirm":false}
            }}),
        ],
    );
    let tools = responses[0]["result"]["tools"].as_array().expect("tools");
    let send = tools
        .iter()
        .find(|tool| tool["name"] == "notify_send")
        .expect("notify send schema");
    assert_eq!(send["inputSchema"]["properties"]["confirm"]["const"], true);
    assert_eq!(responses[1]["result"]["isError"], false);
    assert_eq!(responses[2]["error"]["code"], -32602);
    assert!(!directory.path().join("notification_audit.json").exists());
}

#[test]
fn shortlist_full_cli_lifecycle_is_local() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    let added = run_json(
        directory.path(),
        &[
            "shortlist",
            "add",
            "zhipin-job",
            "--tags",
            "remote,rust,remote",
            "--note",
            "first",
        ],
    );
    assert_eq!(added["data"]["tags"].as_array().map(Vec::len), Some(2));
    let annotated = run_json(
        directory.path(),
        &[
            "shortlist",
            "annotate",
            "zhipin-job",
            "--add-tag",
            "priority",
            "--remove-tag",
            "remote",
            "--note",
            "updated",
        ],
    );
    assert_eq!(annotated["data"]["note"], "updated");
    let listed = run_json(
        directory.path(),
        &["shortlist", "list", "--tag", "priority"],
    );
    assert_eq!(listed["data"].as_array().map(Vec::len), Some(1));
    let compared = run_json(
        directory.path(),
        &["shortlist", "compare", "--tag", "priority"],
    );
    assert_eq!(compared["data"]["count"], 1);
    let removed = run_json(directory.path(), &["shortlist", "remove", "zhipin-job"]);
    assert_eq!(removed["data"]["job"]["id"], "zhipin-job");
}

#[test]
fn status_and_doctor_are_structured_and_offline() {
    let directory = tempdir().expect("temporary directory");
    let status = run_json(directory.path(), &["status"]);
    assert_eq!(status["data"]["network_checked"], false);
    assert_eq!(
        status["data"]["providers"][0]["session_health"]["state"],
        "missing"
    );
    assert_eq!(
        status["data"]["providers"][0]["session_health"]["next_action"],
        "boss login"
    );
    let doctor = run_json(directory.path(), &["doctor"]);
    assert_eq!(
        (
            doctor["data"]["network_checked"].as_bool(),
            doctor["data"]["status"].as_str()
        ),
        (Some(false), Some("warn"))
    );
}

#[test]
fn doctor_reports_invalid_config_as_local_error() {
    let directory = tempdir().expect("temporary directory");
    std::fs::write(directory.path().join("config.json"), b"{not-json").expect("write invalid");
    let doctor = run_json(directory.path(), &["doctor"]);
    assert_eq!(doctor["data"]["status"], "error");
}

#[test]
fn schema_formats_use_expected_wrappers() {
    let directory = tempdir().expect("temporary directory");
    let native = run_json(directory.path(), &["schema", "--format", "native"]);
    let openai = run_json(directory.path(), &["schema", "--format", "openai-tools"]);
    let anthropic = run_json(directory.path(), &["schema", "--format", "anthropic-tools"]);
    let mcp = run_json(directory.path(), &["schema", "--format", "mcp-tools"]);
    assert!(native["data"]["commands"].is_array());
    assert!(openai["data"][0]["function"]["parameters"].is_object());
    assert!(anthropic["data"][0]["input_schema"].is_object());
    let commands = native["data"]["commands"].as_array().expect("commands");
    let mcp_tools = mcp["data"].as_array().expect("mcp tools");
    assert_eq!(
        native["data"]["mcp_tools"].as_array().map(Vec::len),
        mcp["data"].as_array().map(Vec::len)
    );
    assert!(
        commands.iter().any(|command| command["name"] == "login")
            && commands.iter().any(|command| command["name"] == "logout")
            && commands
                .iter()
                .any(|command| command["name"] == "account list")
            && commands
                .iter()
                .any(|command| command["name"] == "account use")
            && commands
                .iter()
                .any(|command| command["name"] == "account resume show")
            && mcp_tools.iter().all(|tool| !matches!(
                tool["name"].as_str(),
                Some("login" | "logout" | "account_list" | "account_use" | "account_resume_show")
            ))
    );
}

#[test]
fn search_filter_validation_happens_before_network() {
    let directory = tempdir().expect("temporary directory");
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args(["search", "rust", "--company", " "])
        .output()
        .expect("run");
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert!(!output.status.success() && value["error"]["code"] == "invalid_argument");
}

#[test]
fn preset_and_watch_snapshots_have_local_lifecycles() {
    let directory = tempdir().expect("temporary directory");
    run_json(directory.path(), &["config", "set", "page_size", "7"]);
    let preset = run_json(
        directory.path(),
        &["preset", "add", "backend", "rust", "--company", "Example"],
    );
    assert_eq!(preset["data"]["spec"]["limit"], 7);
    run_json(
        directory.path(),
        &["watch", "add", "daily", "--preset", "backend"],
    );
    run_json(directory.path(), &["preset", "rm", "backend"]);
    let watch = run_json(directory.path(), &["watch", "show", "daily"]);
    assert_eq!(watch["data"]["spec"]["query"], "rust");
}

#[test]
fn keyword_reply_cli_lifecycle_returns_local_suggestions() {
    let directory = tempdir().expect("temporary directory");
    let added = run_json(
        directory.path(),
        &["reply", "add", " Offer ", " Thanks for reaching out. "],
    );
    assert_eq!(
        (
            added["data"]["keyword"].as_str(),
            added["data"]["reply"].as_str(),
        ),
        (Some("Offer"), Some("Thanks for reaching out."))
    );
    let listed = run_json(directory.path(), &["reply", "list"]);
    let matched = run_json(
        directory.path(),
        &["reply", "match", "A new OFFER is ready"],
    );
    let unmatched = run_json(directory.path(), &["reply", "match", "No update today"]);
    let removed = run_json(directory.path(), &["reply", "remove", "offer"]);
    assert!(
        listed["data"].as_array().map(Vec::len) == Some(1)
            && matched["data"]["matched"] == true
            && matched["data"]["rule"]["reply"] == "Thanks for reaching out."
            && unmatched["data"]["matched"] == false
            && unmatched["data"]["rule"].is_null()
            && removed["data"]["keyword"] == "Offer"
    );
}

#[test]
fn campaign_cli_creates_only_deduplicated_local_manual_review_dry_runs() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    let jobs_before = std::fs::read(directory.path().join("jobs.json")).expect("cached jobs");
    run_json(
        directory.path(),
        &[
            "campaign",
            "policy",
            "add",
            "all-rust",
            "--include",
            "title:Rust",
            "--minimum-score",
            "100",
        ],
    );
    run_json(
        directory.path(),
        &[
            "campaign",
            "template",
            "add",
            "brief",
            "您好，{{company}} 的 {{title}} 职位。",
        ],
    );
    let first = run_json(
        directory.path(),
        &[
            "campaign",
            "plan",
            "create",
            "all-rust",
            "--template",
            "brief",
            "--limit",
            "3",
        ],
    );
    let listed = run_json(directory.path(), &["campaign", "plan", "list"]);
    let second = run_json(
        directory.path(),
        &[
            "campaign",
            "plan",
            "create",
            "all-rust",
            "--template",
            "brief",
            "--limit",
            "3",
        ],
    );
    assert!(
        first["data"]["mode"] == "manual_review"
            && first["data"]["dry_run"] == true
            && first["data"]["planned"] == 1
            && listed["data"].as_array().map(Vec::len) == Some(1)
            && listed["data"].as_array().is_some_and(|plans| plans
                .iter()
                .all(|plan| { plan["state"] == "manual_review" && plan["dry_run"] == true }))
            && second["data"]["planned"] == 0
            && second["data"]["skipped_existing"] == 1
            && std::fs::read(directory.path().join("jobs.json")).expect("cached jobs")
                == jobs_before
    );
}

#[test]
fn campaign_cli_binds_local_resume_and_records_human_transition_lifecycle() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    run_json(
        directory.path(),
        &["resume", "init", "candidate", "--title", "Rust Engineer"],
    );
    run_json(
        directory.path(),
        &["resume", "set", "candidate", "summary", "Local profile"],
    );
    run_json(
        directory.path(),
        &[
            "resume",
            "skills",
            "candidate",
            "--add",
            "Rust",
            "--add",
            "Tokio",
        ],
    );
    run_json(directory.path(), &["campaign", "policy", "add", "all"]);
    run_json(
        directory.path(),
        &[
            "campaign",
            "template",
            "add",
            "resume-brief",
            "{{resume_title}}: {{resume_summary}} ({{resume_skills}})",
        ],
    );
    let created = run_json(
        directory.path(),
        &[
            "campaign",
            "plan",
            "create",
            "all",
            "--template",
            "resume-brief",
            "--resume-name",
            "candidate",
        ],
    );
    let without_confirmation = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args(["campaign", "plan", "transition", "zhipin-job", "approved"])
        .output()
        .expect("transition without confirmation");
    assert!(!without_confirmation.status.success());
    let approved = run_json(
        directory.path(),
        &[
            "campaign",
            "plan",
            "transition",
            "zhipin-job",
            "approved",
            "--yes",
            "--note",
            "reviewed locally",
        ],
    );
    let recorded = run_json(
        directory.path(),
        &[
            "campaign",
            "plan",
            "transition",
            "zhipin-job",
            "recorded_submitted",
            "--yes",
        ],
    );
    let terminal = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args([
            "campaign",
            "plan",
            "transition",
            "zhipin-job",
            "rejected",
            "--yes",
        ])
        .output()
        .expect("terminal transition");
    let stats = run_json(directory.path(), &["campaign", "stats"]);
    let stored = std::fs::read_to_string(directory.path().join("application_plans.json"))
        .expect("stored plans");
    assert!(
        created["data"]["plans"][0]["resume_name"] == "candidate"
            && created["data"]["plans"][0]["resume_updated_at"].is_u64()
            && created["data"]["greeting_previews"][0]["text"]
                == "Rust Engineer: Local profile (Rust、Tokio)"
            && approved["data"]["state"] == "approved"
            && approved["data"]["state_note"] == "reviewed locally"
            && recorded["data"]["state"] == "recorded_submitted"
            && recorded["data"]["dry_run"] == true
            && !terminal.status.success()
            && stats["data"]["plans"]["recorded_submitted"] == 1
            && !stored.contains("Rust Engineer")
            && !stored.contains("Local profile")
            && !stored.contains("Rust、Tokio")
    );
}

#[test]
fn campaign_screen_is_local_ranked_deduplicated_and_resume_bound() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    let jobs_before = std::fs::read(directory.path().join("jobs.json")).expect("jobs");
    run_json(
        directory.path(),
        &["resume", "init", "candidate", "--title", "Rust"],
    );
    run_json(
        directory.path(),
        &[
            "resume",
            "skills",
            "candidate",
            "--add",
            "Rust",
            "--add",
            "UnmatchedSkill",
        ],
    );
    run_json(directory.path(), &["campaign", "policy", "add", "all"]);
    run_json(
        directory.path(),
        &[
            "campaign",
            "template",
            "add",
            "screened",
            "{{resume_title}} / {{resume_skills}} / {{title}}",
        ],
    );

    let first = run_json(
        directory.path(),
        &[
            "campaign",
            "screen",
            "--resume",
            "candidate",
            "--policy",
            "all",
            "--template",
            "screened",
            "--limit",
            "3",
        ],
    );
    let second = run_json(
        directory.path(),
        &[
            "campaign",
            "screen",
            "--resume",
            "candidate",
            "--policy",
            "all",
        ],
    );
    let persisted =
        std::fs::read_to_string(directory.path().join("application_plans.json")).expect("plans");
    assert_eq!(
        first["data"]["plans"]
            .as_array()
            .expect("plans")
            .iter()
            .map(|plan| plan["job_id"].as_str().expect("job id"))
            .collect::<Vec<_>>(),
        vec!["zhipin-job"]
    );
    assert!(
        first["data"]["mode"] == "resume_screening_manual_review"
            && first["data"]["planned"] == 1
            && first["data"]["plans"]
                .as_array()
                .is_some_and(
                    |plans| plans.iter().all(|plan| plan["state"] == "manual_review"
                        && plan["dry_run"] == true
                        && plan["policy_score"] == 100
                        && plan["resume_score"] == 75
                        && plan["title_match"] == true
                        && plan["matched_skills"] == serde_json::json!(["Rust"]))
                )
            && first["data"]["greeting_previews"]
                .as_array()
                .is_some_and(|previews| previews.iter().all(|preview| {
                    preview["sent"] == false
                        && preview["text"]
                            .as_str()
                            .is_some_and(|text| text.contains("Rust / Rust /"))
                        && !preview["text"]
                            .as_str()
                            .is_some_and(|text| text.contains("UnmatchedSkill"))
                }))
            && second["data"]["planned"] == 0
            && second["data"]["skipped_existing"] == 1
            && !persisted.contains("greeting_previews")
            && !persisted.contains("Rust / Rust / Rust")
            && std::fs::read(directory.path().join("jobs.json")).expect("jobs") == jobs_before
    );
}

#[test]
fn campaign_screen_mcp_schema_and_arguments_match_the_local_cli_surface() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    run_json(
        directory.path(),
        &["resume", "init", "candidate", "--title", "Rust"],
    );
    run_json(
        directory.path(),
        &["resume", "skills", "candidate", "--add", "Rust"],
    );
    run_json(directory.path(), &["campaign", "policy", "add", "all"]);
    let responses = run_mcp(
        directory.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"campaign_screen","arguments":{
                    "resume":"candidate","policy":"all","limit":1
                }
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                "name":"campaign_screen","arguments":{
                    "resume":"candidate","policy":"all","minimum_resume_score":101
                }
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                "name":"campaign_screen","arguments":{
                    "resume":"candidate","policy":"all","unexpected":true
                }
            }}),
        ],
    );
    let schema = responses[0]["result"]["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "campaign_screen"))
        .expect("campaign_screen schema");
    assert!(
        schema["inputSchema"]["required"] == serde_json::json!(["resume", "policy"])
            && schema["inputSchema"]["properties"]["minimum_resume_score"]["default"] == 40
            && responses[1]["result"]["isError"] == false
            && responses[1]["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("resume_screening_manual_review"))
            && responses[2]["error"]["code"] == -32602
            && responses[3]["error"]["code"] == -32602
    );
}

#[test]
fn campaign_screen_rejects_empty_missing_resume_and_invalid_score() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    run_json(directory.path(), &["resume", "init", "empty"]);
    run_json(directory.path(), &["campaign", "policy", "add", "all"]);

    let run_failure = |args: &[&str]| {
        Command::cargo_bin("boss")
            .expect("binary")
            .arg("--json")
            .env("BOSS_DATA_DIR", directory.path())
            .env_remove("BOSS_ZHIPIN_COOKIE")
            .env_remove("BOSS_ZHILIAN_COOKIE")
            .env_remove("BOSS_QIANCHENG_COOKIE")
            .env_remove("BOSS_LLM_API_KEY")
            .env_remove("BOSS_NOTIFY_WEBHOOK_URL")
            .args(args)
            .output()
            .expect("run")
    };

    let empty = run_failure(&["campaign", "screen", "--resume", "empty", "--policy", "all"]);
    assert!(!empty.status.success());
    let empty: Value = serde_json::from_slice(&empty.stdout).expect("empty resume error");
    assert_eq!(empty["error"]["code"], "invalid_argument");
    assert!(
        empty["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("non-empty title or at least one skill"))
    );

    let missing = run_failure(&[
        "campaign", "screen", "--resume", "missing", "--policy", "all",
    ]);
    assert!(!missing.status.success());
    let missing: Value = serde_json::from_slice(&missing.stdout).expect("missing resume error");
    assert!(
        missing["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("not found: missing"))
    );

    let invalid_score = run_failure(&[
        "campaign",
        "screen",
        "--resume",
        "empty",
        "--policy",
        "all",
        "--minimum-resume-score",
        "101",
    ]);
    assert!(!invalid_score.status.success());
    let invalid_score: Value =
        serde_json::from_slice(&invalid_score.stdout).expect("invalid score error");
    assert_eq!(invalid_score["error"]["code"], "invalid_argument");

    run_json(
        directory.path(),
        &["resume", "init", "candidate", "--title", "Rust"],
    );
    run_json(
        directory.path(),
        &[
            "resume",
            "set",
            "candidate",
            "summary",
            "Private screening summary",
        ],
    );
    run_json(
        directory.path(),
        &[
            "campaign",
            "template",
            "add",
            "summary",
            "{{resume_summary}}",
        ],
    );
    let summary = run_failure(&[
        "campaign",
        "screen",
        "--resume",
        "candidate",
        "--policy",
        "all",
        "--template",
        "summary",
    ]);
    assert!(!summary.status.success());
    let summary: Value = serde_json::from_slice(&summary.stdout).expect("summary error");
    assert!(
        summary["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("cannot use {{resume_summary}}"))
    );
    assert!(!directory.path().join("application_plans.json").exists());
}

#[test]
fn resume_cli_is_typed_and_requires_deletion_confirmation() {
    let directory = tempdir().expect("temporary directory");
    run_json(
        directory.path(),
        &["resume", "init", "base", "--title", "Engineer"],
    );
    run_json(
        directory.path(),
        &[
            "resume",
            "set",
            "base",
            "basics.email",
            "person@example.test",
        ],
    );
    let skills = run_json(
        directory.path(),
        &["resume", "skills", "base", "--add", "Rust", "--add", "rust"],
    );
    assert_eq!(skills["data"]["skills"].as_array().map(Vec::len), Some(1));
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .args(["resume", "rm", "base"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let removed = run_json(directory.path(), &["resume", "rm", "base", "--yes"]);
    assert_eq!(removed["data"]["name"], "base");
}

#[test]
fn ai_profile_lifecycle_and_confirmation_gate_never_need_a_network() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    run_json(
        directory.path(),
        &["resume", "init", "candidate", "--title", "Rust Engineer"],
    );
    let profile = run_json(
        directory.path(),
        &[
            "ai",
            "profile",
            "add",
            "local",
            "https://model.example/v1/",
            "example-model",
        ],
    );
    let listed = run_json(directory.path(), &["ai", "profile", "ls"]);
    let shown = run_json(directory.path(), &["ai", "profile", "show", "local"]);
    let stored =
        std::fs::read_to_string(directory.path().join("ai_profiles.json")).expect("profile store");
    assert!(
        profile["data"]["base_url"] == "https://model.example/v1"
            && listed["data"].as_array().map(Vec::len) == Some(1)
            && shown["data"]["model"] == "example-model"
            && !stored.contains("BOSS_LLM_API_KEY")
            && !stored.contains("api_key")
    );

    let unconfirmed = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .env_remove("BOSS_LLM_API_KEY")
        .args(["ai", "draft", "local", "zhipin-job", "candidate"])
        .output()
        .expect("unconfirmed draft");
    assert!(!unconfirmed.status.success());
    let unconfirmed: Value = serde_json::from_slice(&unconfirmed.stdout).expect("error json");
    assert_eq!(unconfirmed["error"]["code"], "invalid_argument");

    let missing_key = Command::cargo_bin("boss")
        .expect("binary")
        .arg("--json")
        .env("BOSS_DATA_DIR", directory.path())
        .env_remove("BOSS_LLM_API_KEY")
        .args(["ai", "score", "local", "zhipin-job", "candidate", "--yes"])
        .output()
        .expect("missing key score");
    assert!(!missing_key.status.success());
    let missing_key: Value = serde_json::from_slice(&missing_key.stdout).expect("error json");
    assert_eq!(missing_key["error"]["code"], "ai_api_key_missing");

    let removed = run_json(directory.path(), &["ai", "profile", "rm", "local"]);
    assert_eq!(removed["data"]["name"], "local");
}

#[test]
fn clean_preview_and_archive_preserve_config_and_report_recovery_paths() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    run_json(directory.path(), &["config", "set", "page_size", "7"]);
    run_json(directory.path(), &["preset", "add", "p", "rust"]);
    run_json(directory.path(), &["reply", "add", "offer", "Thanks"]);
    let reply_preview = run_json(directory.path(), &["clean", "--target", "reply_rules"]);
    let preview = run_json(directory.path(), &["clean", "--target", "all"]);
    assert!(preview["data"]["preview"] == true && directory.path().join("jobs.json").exists());
    let cleaned = run_json(directory.path(), &["clean", "--target", "all", "--yes"]);
    let archived_paths_exist = cleaned["data"]["files"].as_array().is_some_and(|files| {
        files
            .iter()
            .filter(|file| file["archived"] == true)
            .all(|file| {
                file["recovery_path"]
                    .as_str()
                    .is_some_and(|path| std::path::Path::new(path).is_file())
            })
    });
    let stats = run_json(directory.path(), &["stats", "--days", "30"]);
    let mcp = run_mcp(
        directory.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"clean_preview","arguments":{"target":"all"}
            }}),
        ],
    );
    let clean_schema = mcp[0]["result"]["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "clean_preview"));
    let mcp_preview: Value = serde_json::from_str(
        mcp[1]["result"]["content"][0]["text"]
            .as_str()
            .expect("clean preview text"),
    )
    .expect("clean preview json");
    assert!(
        cleaned["data"]["files"].as_array().map(Vec::len) == Some(13)
            && reply_preview["data"]["files"].as_array().map(Vec::len) == Some(1)
            && reply_preview["data"]["files"][0]["target"] == "reply_rules"
            && cleaned["data"]["action"] == "archive"
            && cleaned["data"]["recoverable"] == true
            && archived_paths_exist
            && directory.path().join("config.json").exists()
            && !directory.path().join("jobs.json").exists()
            && !directory.path().join("reply_rules.json").exists()
            && stats["data"]["file_bytes"]["jobs"] == 0
            && stats["data"]["file_bytes"]["reply_rules"] == 0
            && clean_schema
                .and_then(|tool| tool["description"].as_str())
                .is_some_and(|description| description.contains("never archives or removes"))
            && mcp_preview["data"]["action"] == "preview"
            && mcp_preview["data"]["recoverable"] == false
            && mcp_preview["data"]["archive_transaction"].is_null()
            && mcp_preview["data"]["files"]
                .as_array()
                .is_some_and(|files| files.iter().all(|file| file["archived"] == false))
    );
}
