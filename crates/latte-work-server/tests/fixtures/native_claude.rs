//! Deterministic native executable fixture. Built only with test-fixture; no model service.
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    time::Duration,
};
fn terminal_fixture(args: &[std::ffi::OsString]) {
    let dir = std::path::Path::new(&args[1]);
    let leaf = args[0] == "--terminal-leaf";
    let name = if leaf { "leaf" } else { "root" };
    let file = std::fs::File::create(dir.join(format!("{name}.lock"))).unwrap();
    file.lock().unwrap();
    std::fs::write(
        dir.join(format!("{name}.ready")),
        std::process::id().to_string(),
    )
    .unwrap();
    let mut child = if leaf {
        None
    } else {
        Some(
            std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--terminal-leaf")
                .arg(dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    // Fixture has a safety lifetime if a failed test loses its Server.
    std::thread::sleep(Duration::from_secs(30));
    if let Some(child) = &mut child {
        let _ = child.kill();
        let _ = child.wait();
    }
    drop(file);
}
fn emit(value: Value) {
    println!("{value}");
    std::io::stdout().flush().unwrap();
}
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "connect-existing") {
        // SSH launcher fixture: preserve standard handles while forwarding to
        // the real Server with an isolated state directory. No model is involved.
        let status =
            std::process::Command::new(std::env::var_os("LATTE_NATIVE_BRIDGE_SERVER").unwrap())
                .arg("connect-existing")
                .arg("--state-dir")
                .arg(std::env::var_os("LATTE_NATIVE_BRIDGE_STATE").unwrap())
                .status()
                .unwrap();
        std::process::exit(status.code().unwrap_or(1));
    }
    if args
        .first()
        .is_some_and(|arg| arg == "--terminal-tree" || arg == "--terminal-leaf")
    {
        terminal_fixture(&args);
        return;
    }
    if args.iter().any(|arg| arg == "--version") {
        println!("native-fixture 1");
        return;
    }
    if args.iter().any(|arg| arg == "--help") {
        println!("  --permission-mode <mode> (choices: \"default\", \"acceptEdits\", \"plan\")");
        return;
    }
    // Bound fixtures even if a failing test loses its server cleanup path.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(45));
        std::process::exit(2);
    });
    let at = args.iter().position(|arg| arg == "--settings").unwrap();
    let path = std::path::Path::new(&args[at + 1]);
    latte_work_platform::fs::verify_private_file(&std::fs::File::open(path).unwrap()).unwrap();
    let _: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut waiting = false;
    let mut approval = None;
    let mut turns = 0;
    for line in std::io::stdin().lock().lines() {
        let value: Value = serde_json::from_str(&line.unwrap()).unwrap();
        match value["type"].as_str().unwrap_or_default() {
            "control_request" => {
                if value["request"]["subtype"] == "interrupt" && waiting {
                    waiting = false;
                    approval = None;
                    emit(json!({"type":"result","subtype":"success","is_error":false}));
                }
                emit(
                    json!({"type":"control_response","response":{"subtype":"success","request_id":value["request_id"],"response":{"commands":[{"name":"compact","description":"Fixture command"}]}}}),
                );
                if value["request"]["subtype"] == "initialize" {
                    emit(
                        json!({"type":"system","subtype":"init","session_id":"native-fixture-session"}),
                    );
                }
            }
            "user" => {
                let text = value["message"]["content"].as_str().unwrap();
                turns += 1;
                let mut log = std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(std::env::var_os("LATTE_NATIVE_PROMPT_LOG").unwrap())
                    .unwrap();
                writeln!(log, "{text}").unwrap();
                if text == "wait" {
                    waiting = true;
                    continue;
                }
                if text == "approve" {
                    waiting = true;
                    let id = format!("native-approval-{turns}");
                    emit(json!({"type":"control_request","request_id":id,
                        "request":{"subtype":"can_use_tool","tool_name":"Write",
                        "input":{"file_path":"approved.txt","content":"approved"}}}));
                    approval = Some(id);
                    continue;
                }
                if text == "edit" {
                    std::fs::write("task.txt", "fixture edit\n").unwrap();
                }
                if text == "edit-pair" {
                    std::fs::write("a.txt", "after a\n").unwrap();
                    std::fs::write("z.txt", "after z\n").unwrap();
                }
                emit(
                    json!({"type":"assistant","message":{"content":[{"type":"text","text":format!("native:{text}")}]}}),
                );
                emit(json!({"type":"result","subtype":"success","is_error":false}));
            }
            "control_response" => {
                let id = approval.take().expect("approval must be pending");
                assert_eq!(value["response"]["request_id"], id);
                if value["response"]["response"]["behavior"] == "allow" {
                    assert_eq!(
                        value["response"]["response"]["updatedInput"],
                        json!({"file_path":"approved.txt","content":"approved"})
                    );
                    let mut log = std::fs::OpenOptions::new()
                        .append(true)
                        .create(true)
                        .open(std::env::var_os("LATTE_NATIVE_ACTION_LOG").unwrap())
                        .unwrap();
                    writeln!(log, "{id}").unwrap();
                    std::fs::write("approved.txt", "approved").unwrap();
                } else {
                    assert_eq!(value["response"]["response"]["behavior"], "deny");
                }
                waiting = false;
                emit(json!({"type":"result","subtype":"success","is_error":false}));
            }
            _ => panic!("unsupported fixture message"),
        }
    }
}
