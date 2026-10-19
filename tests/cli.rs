/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 個人環境を継承しない実バイナリのCLIとSTDIO検証。
//!

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

#[path = "common/wire_schema.rs"]
mod wire_schema;

///
/// 所有するfixtureディレクトリ。
///
struct Directory(
    /// 自身だけが作成したディレクトリ。
    PathBuf,
);

impl Directory {
    ///
    /// 空の隔離ディレクトリを作る。
    /// # 戻り値
    /// 試験専用の場所。
    ///
    fn new() -> Self {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).unwrap();
        let name: String = bytes.iter().map(|b| format!("{b:02x}"))
            .collect();
        let path = std::env::temp_dir().join(format!("ntfy-cli-{name}"));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    ///
    /// 環境を消去して本体を実行する。
    /// # 引数
    /// - args: 非機密fixtureのCLI引数。
    /// # 戻り値
    /// 捕捉した出力と終了状態。
    ///
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ntfy-mcp"))
            .env_clear().current_dir(&self.0)
            .args(["--config", "config.toml"]).args(args)
            .output().unwrap()
    }

    ///
    /// 個人環境を継承しない通常MCP子プロセスを作る。
    /// # 戻り値
    /// STDIOを接続した子。fixture設定は呼出し側が作る。
    ///
    fn spawn(&self) -> Child {
        Command::new(env!("CARGO_BIN_EXE_ntfy-mcp"))
            .env_clear().current_dir(&self.0)
            .args(["--config", "config.toml"])
            .stdin(Stdio::piped()).stdout(Stdio::piped())
            .stderr(Stdio::piped()).spawn().unwrap()
    }
}

impl Drop for Directory {
    ///
    /// 自身のfixtureだけを削除する。
    /// # 戻り値
    /// なし。
    ///
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

///
/// 表示が設定読取に先行し、不備を秘密なしで返すことを確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 不正TOMLがあってもhelp/versionだけは成功する。
/// # 注記
/// env_clearと明示パスでユーザの設定を参照しない。
///
#[test]
fn displays_and_invalid_input_are_safe() {
    let dir = Directory::new();
    std::fs::write(dir.0.join("config.toml"), "invalid TOML [").unwrap();
    /*
     * 表示だけでは設定を解析しない。
     */
    for argument in ["--help", "--version"] {
        let output = dir.run(&[argument]);
        assert!(output.status.success());
        assert!(!output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    let output = dir.run(&["--unknown", "FAKE_SECRET"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Usage:"));
    assert!(!stderr.contains("FAKE_SECRET"));
    let output = dir.run(&[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

///
/// 保存・none確認・初期化前EOFを同じ実バイナリで確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 新規保存と更新を確認し、HTTPを必要とするモードは実行しない。
/// # 注記
/// 例示資格情報だけを保存し、端末出力に含めない。
///
#[test]
fn save_check_none_and_normal_dispatch() {
    let dir = Directory::new();
    let output = dir.run(&["--save-config"]);
    assert!(output.status.success());
    let text = std::fs::read_to_string(dir.0.join("config.toml")).unwrap();
    let value: toml::Value = toml::from_str(&text).unwrap();
    assert!(value.get("topic").is_none());
    /*
     * 保存済み設定を読み、CLIを統合して置換する。
     */
    let output = dir.run(&["--save-config", "--topic", "fixture",
        "--user", "example:FAKE_PASSWORD"]);
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout)
        .contains("FAKE_PASSWORD"));
    let text = std::fs::read_to_string(dir.0.join("config.toml")).unwrap();
    let value: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(value["topic"].as_str(), Some("fixture"));
    assert_eq!(value["auth"]["password"].as_str(), Some("FAKE_PASSWORD"));
    /*
     * 認証をnoneへ戻し、実GETを行わないモードだけを実行する。
     */
    std::fs::write(dir.0.join("config.toml"),
        "topic = 'fixture'\n[auth]\nmode = 'none'\n").unwrap();
    let output = dir.run(&["--check-auth"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap();
    assert_eq!(value["credentials"], "not_configured");
    assert_eq!(value["attempts"], 0);
    assert_eq!(value["error"].as_object().unwrap().len(), 3);
    let output = dir.run(&[]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

///
/// 自身が起動したMCP子と出力観測。
///
struct Session {
    /// この試験が所有する子。
    child: Child,
    /// EOFを明示できる入力。
    input: Option<ChildStdin>,
    /// 読取threadからのstdout行。
    output: Receiver<String>,
}

impl Session {
    ///
    /// stdout読取だけを別threadで行い、応答待ちを有限にする。
    /// # 引数
    /// - dir: この試験の設定と作業場所。
    /// # 戻り値
    /// 子セッション。
    /// # 概要
    /// stdout行を観測用channelへ渡し、読取異常も試験失敗として保持する。
    ///
    fn new(dir: &Directory) -> Self {
        /*
         * 子プロセスと観測用channelを準備する。
         */
        let mut child = dir.spawn();
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (sender, output) = mpsc::channel();
        /*
         * stdout全行と読取異常をchannelへ渡し、終了時に全て回収する。
         */
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => if sender.send(line).is_err() { break; },
                    Err(_) => {
                        let _ = sender.send("stdout read failed".into());
                        break;
                    }
                }
            }
        });
        Self { child, input, output }
    }

    ///
    /// 固定fixtureのJSON-RPCを送る。
    /// # 引数
    /// - text: LF区切りのSDK入力。
    /// # 戻り値
    /// なし。
    ///
    fn send(&mut self, text: &str) {
        self.input.as_mut().unwrap().write_all(text.as_bytes()).unwrap();
        self.input.as_mut().unwrap().flush().unwrap();
    }

    ///
    /// stdoutがJSON-RPCだけであることを確認する。
    /// # 戻り値
    /// 最初の応答。
    ///
    fn response(&self) -> serde_json::Value {
        let line = self.output.recv_timeout(Duration::from_secs(3)).unwrap();
        protocol_line(&line)
    }

    ///
    /// 子の終了を有限に待ち、安全な診断だけを取得する。
    /// # 引数
    /// - expected: 期待する終了状態。
    /// # 戻り値
    /// stderr文字列。
    /// # 概要
    /// 子終了とstdout観測の完了を有限に待ち、全stdout行を検査する。
    ///
    fn finish(&mut self, expected: i32) -> String {
        wait(&mut self.child, expected);
        let mut stderr = String::new();
        self.child.stderr.as_mut().unwrap().read_to_string(&mut stderr)
            .unwrap();
        /*
         * 子終了後にreaderもEOFへ達するまで、残るstdout全行を検査する。
         */
        loop {
            match self.output.recv_timeout(Duration::from_secs(1)) {
                Ok(line) => { protocol_line(&line); }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    panic!("stdout reader did not finish after child exit")
                }
            }
        }
        stderr
    }
}

///
/// 端末表示が混ざらず、stdoutの各行がJSON-RPC外枠であることを確認する。
/// # 引数
/// - line: 子プロセスのstdout行。
/// # 戻り値
/// 解釈済みJSON。プロトコル本体の実装には使用しない。
///
fn protocol_line(line: &str) -> serde_json::Value {
    let value: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(value["jsonrpc"], "2.0");
    if value.get("method").is_some() {
        assert!(value["method"].is_string());
    } else {
        assert!(value.get("id").is_some());
        assert!(value.get("result").is_some() ^ value.get("error").is_some());
    }
    value
}

impl Drop for Session {
    ///
    /// 試験失敗でも所有する子を残さない。
    /// # 戻り値
    /// なし。
    ///
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

///
/// 子の終了を無期限に待たない。
/// # 引数
/// - child/expected: 所有する子と期待状態。
/// # 戻り値
/// なし。期限超過は子を回収して失敗する。
///
fn wait(child: &mut Child, expected: i32) {
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(expected));
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("MCP process did not exit within the finite test deadline");
}

///
/// 実SDKの最小初期化入力。
/// # 戻り値
/// initializeとinitialized通知。
///
fn initialize() -> &'static str {
    concat!("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",",
        "\"params\":{\"protocolVersion\":\"2024-11-05\",",
        "\"capabilities\":{},\"clientInfo\":{\"name\":\"fixture\",",
        "\"version\":\"0\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
}

///
/// 初期化後EOF・初期化不正・stdout失敗を終了状態へ反映する。
/// # 戻り値
/// なし。
/// # 概要
/// 実STDIOは開始終了だけを確認し、notifyの実HTTPは呼ばない。
/// # 注記
/// env_clearと明示fixture、有限応答待ちを使う。
///
#[test]
fn stdio_startup_eof_and_failures_have_safe_exit_states() {
    let dir = Directory::new();
    std::fs::write(dir.0.join("config.toml"), "topic='fixture'\n").unwrap();
    let mut session = Session::new(&dir);
    session.send(initialize());
    assert_eq!(session.response()["id"], 1);
    drop(session.input.take());
    assert!(session.finish(0).is_empty());
    /*
     * 初期化不正はEOFを後置しても正常終了にしない。
     */
    let mut session = Session::new(&dir);
    session.send(concat!("{\"jsonrpc\":\"2.0\",",
        "\"method\":\"notifications/initialized\",",
        "\"params\":{\"FAKE_SECRET\":1}}\n"));
    drop(session.input.take());
    let stderr = session.finish(1);
    assert!(stderr.contains("mcp_failed"));
    assert!(!stderr.contains("FAKE_SECRET"));
    /*
     * stdout readerだけを閉じ、stdinは保持して書込失敗を確認する。
     */
    let mut child = dir.spawn();
    drop(child.stdout.take());
    let mut input = child.stdin.take().unwrap();
    input.write_all(initialize().as_bytes()).unwrap();
    input.flush().unwrap();
    wait(&mut child, 1);
    drop(input);
}

///
/// SDK初期化中と開始後の処理したUnix signalは有限終了0になる。
/// # 戻り値
/// なし。
/// # 概要
/// SDKのping又はinitialize応答で準備完了を確認してからsignalを送る。
/// # 注記
/// この試験自身が起動した子だけへSIGINT/SIGTERMを送る。
///
#[cfg(target_os = "linux")]
#[test]
fn handled_signals_exit_zero_during_and_after_initialization() {
    let dir = Directory::new();
    std::fs::write(dir.0.join("config.toml"), "topic='fixture'\n").unwrap();
    for initialized in [false, true] {
        for signal in ["-INT", "-TERM"] {
            let mut session = Session::new(&dir);
            if initialized { session.send(initialize()); }
            else {
                session.send(concat!("{\"jsonrpc\":\"2.0\",\"id\":1,",
                    "\"method\":\"ping\"}\n"));
            }
            assert_eq!(session.response()["id"], 1);
            let sent = Command::new("/bin/kill").arg(signal)
                .arg(session.child.id().to_string()).status().unwrap();
            assert!(sent.success());
            assert!(session.finish(0).is_empty());
        }
    }
}

///
/// 実起動の32MiB超未完行はstdinを保持しても固定診断で終了する。
/// # 戻り値
/// なし。
/// # 概要
/// 大入力の書込と子終了待ちを並行し、書込停止も期限で回収する。
/// # 注記
/// 自身の子だけを使い、超過bytesはnotifyやHTTPへ到達しない。
///
#[test]
fn oversized_unfinished_line_closes_without_waiting_for_eof() {
    let dir = Directory::new();
    std::fs::write(dir.0.join("config.toml"), "topic='fixture'\n").unwrap();
    let mut session = Session::new(&dir);
    session.send(initialize());
    assert_eq!(session.response()["id"], 1);
    let mut input = session.input.take().unwrap();
    let (release, held) = mpsc::channel();
    /*
     * 書込が詰まっても、主threadは子を期限付きで回収できる。
     */
    let writer = std::thread::spawn(move || {
        let _ = input.write_all(b"FAKE_SECRET");
        let chunk = [b'x'; 65_536];
        for _ in 0..512 {
            if input.write_all(&chunk).is_err() { break; }
        }
        let _ = input.flush();
        let _ = held.recv();
        drop(input);
    });
    let stderr = session.finish(1);
    release.send(()).unwrap();
    writer.join().unwrap();
    assert!(stderr.contains("mcp_line_too_long"));
    assert!(!stderr.contains("FAKE_SECRET"));
}

///
/// 実バイナリの公開Schemaと安全な失敗、同じ接続の継続を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// initialize、tools/list、送信前失敗、継続、EOFを実STDIOで通す。
/// # 注記
/// 正常notifyは送らず、正例は実取得Schemaのvalidatorだけへ渡す。
///
#[test]
fn actual_tools_list_and_invalid_notifications_keep_stdio_alive() {
    let dir = Directory::new();
    std::fs::write(dir.0.join("config.toml"), concat!(
        "server_url='https://example.invalid/'\ntopic='fixture'\n")).unwrap();
    let mut session = Session::new(&dir);
    session.send(initialize());
    let initialized = session.response();
    assert_eq!(initialized["id"], 1);
    assert_eq!(initialized["result"]["serverInfo"]["name"], "ntfy-mcp");
    assert_eq!(initialized["result"]["serverInfo"]["version"], "0.1.0");
    assert!(initialized["result"]["capabilities"]["tools"].is_object());
    /*
     * 実tools/listの二つのSchemaを別々の根として取り出す。
     */
    session.send("{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n");
    let listed = session.response();
    assert_eq!(listed["id"], 2);
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "notify");
    let input_schema = tools[0]["inputSchema"].clone();
    let output_schema = tools[0]["outputSchema"].clone();
    let input_examples: Vec<serde_json::Value> = serde_json::from_str(
        include_str!("fixtures/mcp_input_examples.json")).unwrap();
    let output_examples: Vec<serde_json::Value> = serde_json::from_str(
        include_str!("fixtures/mcp_output_examples.json")).unwrap();
    wire_schema::examples(&input_schema, &input_examples);
    let output_validator = wire_schema::examples(&output_schema,
        &output_examples);
    /*
     * 不適合入力だけを本体へ渡し、外部送信なしのtool errorを確認する。
     */
    for (index, example) in input_examples.iter().filter(|case| {
        !case["valid"].as_bool().unwrap()
    }).enumerate() {
        let request_id = index + 10;
        let mut request = serde_json::to_string(&serde_json::json!({
            "jsonrpc":"2.0","id":request_id,"method":"tools/call",
            "params":{"name":"notify","arguments":example["value"]}
        })).unwrap();
        request.push('\n');
        session.send(&request);
        let response = session.response();
        assert_eq!(response["id"], request_id);
        assert_eq!(response["result"]["isError"], true);
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["ok"], false);
        assert_eq!(structured["attempts"], 0);
        assert_eq!(structured["receipt"], "not_sent");
        assert!(output_validator.is_valid(structured), "actual failure case {}",
            example["name"].as_str().unwrap());
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(&serde_json::from_str::<serde_json::Value>(text).unwrap(),
            structured);
        assert!(!response.to_string().contains("FAKE_SECRET"));
    }
    /*
     * 未知ツールのprotocol error後も同じ接続と公開定義を維持する。
     */
    session.send(concat!("{\"jsonrpc\":\"2.0\",\"id\":90,",
        "\"method\":\"tools/call\",\"params\":{\"name\":\"FAKE_SECRET\"}}\n"));
    let unknown = session.response();
    assert_eq!(unknown["id"], 90);
    assert_eq!(unknown["error"]["code"], -32602);
    assert!(!unknown.to_string().contains("FAKE_SECRET"));
    session.send("{\"jsonrpc\":\"2.0\",\"id\":91,\"method\":\"tools/list\"}\n");
    let continued = session.response();
    assert_eq!(continued["id"], 91);
    assert_eq!(continued["result"]["tools"], listed["result"]["tools"]);
    drop(session.input.take());
    assert!(session.finish(0).is_empty());
}
