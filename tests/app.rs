/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 単発処理の取消・有限待機と安全な出力失敗。
//!

use std::io::{self, Write};
use std::time::Duration;

use ntfy_mcp::app::{self, Output};
use ntfy_mcp::core::{Cancel, Failure};

///
/// 停止に応答しないfutureも1秒で解放することを確認する。
/// # 戻り値
/// なし。
/// # 注記
/// 仮想時間を用い、実signalやworkerを生成しない。
///
#[tokio::test(start_paused = true)]
async fn stop_has_a_finite_grace_period() {
    let cancel = Cancel::new();
    let start = tokio::time::Instant::now();
    let output = app::interruptible(std::future::pending(),
        async { Ok(()) }, &cancel).await;
    assert!(cancel.is_cancelled());
    assert_eq!(output.exit_code, 1);
    assert_eq!(start.elapsed(), Duration::from_secs(1));
}

///
/// 協調的な取消で確認JSONを失わないことを確認する。
/// # 戻り値
/// なし。
/// # 注記
/// handlerの安全な出力をそのまま返し、追加待機しない。
///
#[tokio::test(start_paused = true)]
async fn cooperative_stop_keeps_handler_output() {
    let cancel = Cancel::new();
    let handler = async {
        cancel.cancelled().await;
        Output::display("{\"ok\":false}\n".into(), 1)
    };
    let output = app::interruptible(handler, async { Ok(()) },
        &cancel).await;
    assert_eq!(output.stdout.as_deref(), Some("{\"ok\":false}\n"));
    assert_eq!(output.exit_code, 1);
}

///
/// 出力不能のfixture。
///
struct Broken;

impl Write for Broken {
    ///
    /// 固定の書込失敗を返す。
    /// # 引数
    /// - bytes: 使用しない入力。
    /// # 戻り値
    /// 模擬I/O失敗。
    ///
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }

    ///
    /// flushには成功する。
    /// # 戻り値
    /// 成功。
    ///
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

///
/// 成功出力・失敗診断とも書込不能なら終了1にする。
/// # 戻り値
/// なし。
/// # 注記
/// OSエラーの文字列を端末へ出さない。
///
#[test]
fn broken_output_is_failure() {
    assert_eq!(Output { stdout: None, stderr: None, exit_code: 0 }
        .write_to(&mut Broken, &mut Broken), 0);
    assert_eq!(Output::display("safe\n".into(), 0)
        .write_to(&mut Broken, &mut Vec::new()), 1);
    assert_eq!(Output::failure(Failure::cancelled(), 2)
        .write_to(&mut Vec::new(), &mut Broken), 1);
}

///
/// 認証なしはfactoryを呼ばず、生成失敗も確認JSONにする。
/// # 戻り値
/// なし。
/// # 概要
/// 資格情報の有無による生成境界と安全な失敗出力を確認する。
/// # 注記
/// 不存在の乱数パスを既定源にし、実HTTPを生成しない。
///
#[tokio::test]
async fn check_factory_is_lazy_and_failure_is_json() {
    use ntfy_mcp::app::Inputs;
    use ntfy_mcp::cmd_args::{self, Action, Environment};
    /*
     * noneと資格情報ありを別ケースとして照合する。
     */
    for credential in [false, true] {
        let mut args = vec!["ntfy-mcp", "--check-auth", "--topic", "test"];
        if credential { args.extend(["--auth-token", "fake"]); }
        let Action::Prepare(options) = cmd_args::parse_from(args).unwrap()
            else { panic!("prepare") };
        let mut entropy = [0u8; 16];
        getrandom::fill(&mut entropy).unwrap();
        let name: String = entropy.iter().map(|b| format!("{b:02x}"))
            .collect();
        let input = Inputs { options, environment: Environment::new(),
            cwd: std::env::current_dir().unwrap(),
            default_path: Some(std::env::temp_dir().join(name)) };
        /*
         * factoryが呼ばれた場合だけ安全な生成失敗を返す。
         */
        let output = app::run_with_check(input, &Cancel::new(),
            |_, _| async {
                assert!(credential, "none must not construct transport");
                Err(Failure::new("internal", "preparation_failed", None))
            }).await;
        let value: serde_json::Value = serde_json::from_str(
            output.stdout.as_deref().unwrap()).unwrap();
        assert_eq!(output.exit_code, if credential { 1 } else { 3 });
        assert_eq!(value["attempts"], 0);
        assert_eq!(value["error"].as_object().unwrap().len(), 3);
        assert!(output.stderr.is_none());
    }
}
