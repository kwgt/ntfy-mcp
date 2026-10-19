/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 承認済みデータ契約に由来する独立した公開Schemaを読み込む。
//!
//! JSONはdocs/BASE_DESIGN.md第9章のNotifyInput/NotifyResultから、
//! 必要な参照型を各$defsへ展開し、内部参照を書き換えたもの。
//! 実行時に設計Markdownを参照しない。
//!

use std::sync::Arc;

use rmcp::ErrorData;
use rmcp::model::{JsonObject, Tool};

///
/// 同梱JSONをobjectとして読み込む。
/// # 引数
/// - text: ビルド時に同梱したSchema。
/// # 戻り値
/// Schema object。不整合は内容を表示しない内部失敗。
///
fn parse(text: &str) -> Result<Arc<JsonObject>, ErrorData> {
    serde_json::from_str(text).map(Arc::new).map_err(|_| {
        ErrorData::internal_error("ツール定義を準備できません。", None)
    })
}

///
/// notifyだけの正式なツール定義を準備する。
/// # 戻り値
/// 入出力Schemaと制約説明を持つツール定義。
/// # 概要
/// 静的な入出力契約と実行時制約の説明を一つの定義へ結合する。
///
pub(super) fn tool() -> Result<Tool, ErrorData> {
    /*
     * 同梱Schemaの解析後に、固定descriptionと合わせて公開する。
     */
    let input = parse(include_str!("input_schema.json"))?;
    let output = parse(include_str!("output_schema.json"))?;
    Ok(Tool::new_with_raw("notify", Some(concat!(
        "人間へMarkdown通知を送ります。kindはprogress/completed/",
        "question/alert。messageは非空・空白のみ不可、",
        "最大1000 Unicodeスカラー値かつUTF-8で4096バイト。",
        "添付はpath又はinlineで1件、復元後2000000バイト以下。",
        "titleは1024、filenameは255、pathは4096 UTF-8バイト以下。",
        "本文の前後空白は拒否します。添付同送では文字としての",
        "バックスラッシュ+nも拒否します。入力は自動整形しません。",
        "path読取は5秒の論理期限、同時通知は1件で使用中はbusy。",
        "sequence_idは同じタスクで再利用できます。",
        "成功はサーバ受領の確認で、端末表示や重複排除の保証では",
        "ありません。送信後の失敗は受領不明・重複の可能性があり、",
        "waitは即時再送の許可ではありません。"
    ).into()), input).with_raw_output_schema(output))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::tool;

    ///
    /// 一つのSchema内で参照先を解決する。
    /// # 引数
    /// - node/root: 走査する値と独立したSchema根。
    /// # 戻り値
    /// なし。参照漏れはassertで失敗する。
    /// # 概要
    /// 配列・object内の全参照が同じSchema内に存在することを調べる。
    ///
    fn references(node: &Value, root: &Value) {
        /*
         * 参照文字列は同一根で確認し、他の子要素は再帰走査する。
         */
        match node {
            Value::Object(map) => {
                for (key, value) in map {
                    if key == "$ref" {
                        let reference = value.as_str().unwrap();
                        assert!(reference.starts_with("#/$defs/"));
                        assert!(root.pointer(&reference[1..]).is_some());
                    } else { references(value, root); }
                }
            }
            Value::Array(items) => {
                for item in items { references(item, root); }
            }
            _ => {}
        }
    }

    ///
    /// 同梱した二つのSchemaのobject根と自己完結性を確認する。
    /// # 戻り値
    /// なし。
    /// # 注記
    /// 静的定義の基本検査で、実STDIO公開や全例の適合試験ではない。
    ///
    #[test]
    fn embedded_schemas_have_object_roots_and_local_references() {
        let tool = tool().unwrap();
        for object in [tool.input_schema, tool.output_schema.unwrap()] {
            let root = Value::Object((*object).clone());
            assert_eq!(root["type"], "object");
            assert_eq!(root["$schema"],
                "https://json-schema.org/draft/2020-12/schema");
            references(&root, &root);
        }
    }
}
