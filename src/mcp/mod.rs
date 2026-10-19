/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 単一notifyのSDK受付と通知コアの接続。
//!
//! request取消とサーバ停止を呼出し別の取消へ橋渡しする。
//!

use std::path::PathBuf;

use rmcp::{ErrorData, RoleServer, ServerHandler};
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult,
    Implementation, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerConfig, Tool};
use rmcp::service::RequestContext;
use serde_json::Value;

use crate::core::Cancel;
use crate::http::{Service, Transport};

mod result;
mod schema;
pub(crate) mod session;

#[cfg(test)]
mod contract_tests;

#[cfg(test)]
mod resource_tests;

///
/// 一つの通知コアと固定cwdを全呼出しで共有するMCP受付。
///
pub struct NotifyAdapter<T> {
    /// 単一の実行枠と送信境界を持つ通知コア。
    service: Service<T>,
    /// 構築時に渡された起動場所。
    cwd: PathBuf,
    /// 自己完結した入出力Schemaを持つnotify定義。
    tool: Tool,
    /// EOF・signal・IO異常で全呼出しへ伝える停止。
    shutdown: Cancel,
}

impl<T: Transport> NotifyAdapter<T> {
    ///
    /// 設定済みコアとcwdから受付を準備する。
    /// # 引数
    /// - service/cwd: 共有する通知コアと構築時の作業場所。
    /// - shutdown: サーバ全体の停止通知。
    /// # 戻り値
    /// SDK受付。同梱Schemaの不整合は固定内部エラー。
    ///
    pub fn new(service: Service<T>, cwd: PathBuf, shutdown: Cancel)
        -> Result<Self, ErrorData>
    {
        Ok(Self { service, cwd, tool: schema::tool()?, shutdown })
    }

    ///
    /// 生argumentsを内部検証し、MCP結果へ変換する。
    /// # 引数
    /// - request: SDKが解析したtools/call外枠。
    /// - cancel: 呼出し単位の取消。
    /// # 戻り値
    /// 通知失敗はtool result、未知名は固定プロトコルエラー。
    ///
    pub async fn call(&self, request: CallToolRequestParams, cancel: &Cancel)
        -> Result<CallToolResult, ErrorData>
    {
        if request.name != "notify" {
            return Err(ErrorData::invalid_params(
                "指定されたツールはありません。", None));
        }
        let raw = Value::Object(request.arguments.unwrap_or_default());
        let notified = self.service.notify(raw, &self.cwd, cancel).await;
        result::convert(&notified)
    }
}

impl<T: Transport + 'static> ServerHandler for NotifyAdapter<T> {
    ///
    /// SDKのtools/callを共通受付へ渡す。
    /// # 引数
    /// - request/context: SDK要求と実行文脈。
    /// # 戻り値
    /// MCP結果。取消後も送信する独立taskは作らない。
    /// # 概要
    /// 既取消を先に伝え、開始後の取消も同じhandler内で通知する。
    ///
    async fn call_tool(&self, request: CallToolRequestParams,
        context: RequestContext<RoleServer>)
        -> Result<CallToolResponse, ErrorData>
    {
        /*
         * 既に取り消された要求から新しいHTTP試行を開始しない。
         */
        let cancel = Cancel::new();
        if context.ct.is_cancelled() || self.shutdown.is_cancelled() {
            cancel.cancel();
        }
        let call = self.call(request, &cancel);
        tokio::pin!(call);
        /*
         * 取消を優先し、コアの協調終了をこのfuture内で待つ。
         */
        tokio::select! {
            biased;
            _ = context.ct.cancelled() => {
                cancel.cancel();
                call.await.map(Into::into)
            }
            _ = self.shutdown.cancelled() => {
                cancel.cancel();
                call.await.map(Into::into)
            }
            result = &mut call => result.map(Into::into),
        }
    }

    ///
    /// 単一のnotify定義を一覧として返す。
    /// # 引数
    /// - request/context: SDKのページ要求と文脈。
    /// # 戻り値
    /// ページ継続を持たない一覧。
    ///
    async fn list_tools(&self, _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>)
        -> Result<ListToolsResult, ErrorData>
    {
        Ok(ListToolsResult::with_all_items(vec![self.tool.clone()]))
    }

    ///
    /// 既知のnotifyだけの定義を参照する。
    /// # 引数
    /// - name: SDKが問い合わせるツール名。
    /// # 戻り値
    /// notify定義。未知名はNone。
    ///
    fn get_tool(&self, name: &str) -> Option<Tool> {
        (name == "notify").then(|| self.tool.clone())
    }

    ///
    /// SDK標準メタデータとtools能力だけを設定する。
    /// # 戻り値
    /// アプリ名と版を持つサーバ情報。
    ///
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("ntfy-mcp", "0.1.0"))
    }
}
