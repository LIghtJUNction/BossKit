use assert_cmd::Command;
use serde_json::Value;
use tempfile::tempdir;

fn run_json(data_dir: &std::path::Path, args: &[&str]) -> Value {
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .env("BOSS_DATA_DIR", data_dir)
        .env_remove("BOSS_ZHIPIN_COOKIE")
        .env_remove("BOSS_ZHILIAN_COOKIE")
        .env_remove("BOSS_QIANCHENG_COOKIE")
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
        .env("BOSS_DATA_DIR", data_dir)
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
            "id":"zhilian-job","platform":"zhilian","remote_id":"remote-2",
            "title":"Rust","company":"Example","city":"深圳","salary":"21K",
            "url":"https://example.test/2"
        },
        {
            "id":"zhilian-job-2","platform":"zhilian","remote_id":"remote-3",
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
fn platforms_lists_all_registered_adapters() {
    let output = Command::cargo_bin("boss")
        .expect("binary")
        .arg("platforms")
        .output()
        .expect("run");
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["data"].as_array().expect("platforms").len(), 3);
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
        .args(["search", "rust", "--limit", "0"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["code"], "invalid_argument");
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
        .env("BOSS_DATA_DIR", directory.path())
        .args(["ls", "--limit", "1"])
        .output()
        .expect("run");
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["data"][0]["id"], "cached-job");
}

#[test]
fn help_and_version_exit_successfully_on_stdout() {
    for argument in ["--help", "--version"] {
        let output = Command::cargo_bin("boss")
            .expect("binary")
            .arg(argument)
            .output()
            .expect("run");
        assert!(output.status.success());
        assert!(!output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn cities_reports_exact_shared_mapping() {
    let directory = tempdir().expect("temporary directory");
    let value = run_json(directory.path(), &["cities"]);
    assert_eq!(value["data"]["count"], 10);
}

#[test]
fn config_lifecycle_and_defaults_affect_ls() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    let set_platform = run_json(directory.path(), &["config", "set", "platform", "zhilian"]);
    assert_eq!(set_platform["data"]["new"], "zhilian");
    let set_limit = run_json(directory.path(), &["config", "set", "page_size", "1"]);
    assert_eq!(set_limit["data"]["new"], 1);
    let listed = run_json(directory.path(), &["ls"]);
    assert_eq!(
        (
            listed["data"].as_array().map(Vec::len),
            listed["data"][0]["platform"].as_str()
        ),
        (Some(1), Some("zhilian"))
    );
    let reset = run_json(directory.path(), &["config", "reset"]);
    assert_eq!(reset["data"]["key"], "all");
    let defaults = run_json(directory.path(), &["config", "get", "page_size"]);
    assert_eq!(defaults["data"]["source"], "default");
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
    assert_eq!(
        native["data"]["mcp_tools"].as_array().map(Vec::len),
        mcp["data"].as_array().map(Vec::len)
    );
}

#[test]
fn detail_history_and_export_cli_surfaces_are_local() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    let history = serde_json::json!([{
        "timestamp":1,"query":"rust","platform":"zhipin","city":"深圳",
        "page":1,"limit":20,"filters":{"company":null,"salary":null,
        "experience":null,"education":null,"employment_type":null,"welfare":[]},
        "providers":[{"platform":"zhipin","count":1,"error_code":null}]
    }]);
    std::fs::write(
        directory.path().join("history.json"),
        serde_json::to_vec(&history).expect("history json"),
    )
    .expect("history fixture");
    let detail = run_json(directory.path(), &["detail", "zhipin-job"]);
    let listed = run_json(
        directory.path(),
        &["history", "--platform", "zhipin", "--limit", "1"],
    );
    let structured = run_json(
        directory.path(),
        &["export", "--format", "csv", "--limit", "1"],
    );
    let output_path = directory.path().join("exports").join("jobs.csv");
    let output_text = output_path.display().to_string();
    let written = run_json(
        directory.path(),
        &[
            "export",
            "--format",
            "csv",
            "--limit",
            "1",
            "--output",
            &output_text,
        ],
    );
    assert!(
        detail["data"]["description"] == "cached detail"
            && listed["data"].as_array().map(Vec::len) == Some(1)
            && structured["data"]["format"] == "csv"
            && structured["data"]["jobs"].is_array()
            && written["data"]["jobs"].is_null()
            && output_path.is_file()
    );
}

#[test]
fn search_filter_validation_happens_before_network() {
    let directory = tempdir().expect("temporary directory");
    let output = Command::cargo_bin("boss")
        .expect("binary")
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
        .env("BOSS_DATA_DIR", directory.path())
        .args(["resume", "rm", "base"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let removed = run_json(directory.path(), &["resume", "rm", "base", "--yes"]);
    assert_eq!(removed["data"]["name"], "base");
}

#[test]
fn clean_preview_and_archive_preserve_config_and_report_recovery_paths() {
    let directory = tempdir().expect("temporary directory");
    seed_jobs(directory.path());
    run_json(directory.path(), &["config", "set", "page_size", "7"]);
    run_json(directory.path(), &["preset", "add", "p", "rust"]);
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
        cleaned["data"]["files"].as_array().map(Vec::len) == Some(6)
            && cleaned["data"]["action"] == "archive"
            && cleaned["data"]["recoverable"] == true
            && archived_paths_exist
            && directory.path().join("config.json").exists()
            && !directory.path().join("jobs.json").exists()
            && stats["data"]["file_bytes"]["jobs"] == 0
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
