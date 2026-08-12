use std::{
    env, fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

struct McpProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: Receiver<String>,
}

impl McpProcess {
    fn spawn(session_env: Option<&Path>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bugboard-mcp"));
        command
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match session_env {
            Some(path) => {
                command.env("BUGBOARD_SESSION_ENV", path);
            }
            None => {
                command.env_remove("BUGBOARD_SESSION_ENV");
            }
        }
        let mut child = command.spawn().expect("spawn bugboard-mcp stdio server");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });

        Self {
            child,
            stdin,
            stdout: receiver,
        }
    }

    fn send(&mut self, message: Value) {
        serde_json::to_writer(&mut self.stdin, &message).expect("write json-rpc message");
        self.stdin.write_all(b"\n").expect("write json-rpc newline");
        self.stdin.flush().expect("flush json-rpc message");
    }

    fn request(&mut self, message: Value, id: i64) -> Value {
        self.send(message);
        self.response(id)
    }

    fn response(&self, id: i64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();

        while Instant::now() < deadline {
            let timeout = deadline.saturating_duration_since(Instant::now());
            let Ok(line) = self.stdout.recv_timeout(timeout) else {
                break;
            };
            let value: Value = serde_json::from_str(&line).expect("stdout json-rpc message");
            if value.get("id").and_then(Value::as_i64) == Some(id) {
                return value;
            }
            seen.push(value);
        }

        panic!("timed out waiting for response id {id}; seen: {seen:?}");
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn stdio_serves_tools_and_reports_missing_session_as_tool_error() {
    let mut server = McpProcess::spawn(None);

    let initialized = server.request(
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "bugboard-mcp-smoke", "version": "0.0.0"}
            }
        }),
        1,
    );
    assert!(initialized.get("error").is_none(), "{initialized}");
    assert_eq!(
        initialized
            .pointer("/result/serverInfo/name")
            .and_then(Value::as_str),
        Some("bugboard-mcp")
    );

    server.send(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    }));

    let tools = server.request(
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        2,
    );
    let tool_names = tools
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .expect("tools/list array")
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(tool_names.contains(&"bugboard_auth_status"), "{tools}");

    let auth_status = server.request(
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "bugboard_auth_status", "arguments": {}}
        }),
        3,
    );

    assert_eq!(auth_status.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        auth_status.pointer("/result/structuredContent/error/code"),
        Some(&json!("config_missing"))
    );
}

struct TempSessionEnv {
    path: PathBuf,
}

impl TempSessionEnv {
    fn new(contents: &str) -> Self {
        let path = env::temp_dir().join(format!(
            "bugboard-mcp-stdio-smoke-{}-{}.env",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos()
        ));
        fs::write(&path, contents).expect("write temp session env");
        Self { path }
    }
}

impl Drop for TempSessionEnv {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[test]
fn stdio_initializes_with_cookie_only_external_session_env() {
    let session_env = TempSessionEnv::new("BUGBOARD_COOKIE=\"session=value\"\n");
    let mut server = McpProcess::spawn(Some(&session_env.path));

    let initialized = server.request(
        json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "bugboard-mcp-smoke", "version": "0.0.0"}
            }
        }),
        11,
    );
    assert!(initialized.get("error").is_none(), "{initialized}");
}
