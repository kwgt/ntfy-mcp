/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 実tools/listで取得したSchemaの独立した根と局所参照を検査する。
//!

use serde_json::Value;

///
/// 取り出したSchemaだけで全ての局所参照を解決できることを確認する。
/// # 引数
/// - root: 実tools/listから取り出した一方のSchema。
/// # 戻り値
/// なし。外部refや根の不整合は試験失敗。
///
pub fn independent(root: &Value) {
    assert_eq!(root["type"], "object");
    assert_eq!(root["$schema"],
        "https://json-schema.org/draft/2020-12/schema");
    assert!(root["$defs"].is_object());
    references(root, root);
}

///
/// 実取得Schemaだけでvalidatorを作り、正負の代表例を照合する。
/// # 引数
/// - root: 一方のSchema根。
/// - examples: 名前・Schema適合期待値・入力値を持つ非機密fixture。
/// # 戻り値
/// 実応答の照合にも再利用する2020-12 validator。
///
pub fn examples(root: &Value, examples: &[Value]) -> jsonschema::Validator {
    independent(root);
    let validator = match jsonschema::draft202012::new(root) {
        Ok(value) => value,
        Err(_) => panic!("cannot build validator for the captured schema"),
    };
    for (index, example) in examples.iter().enumerate() {
        let expected = example["valid"].as_bool().unwrap();
        let name = example["name"].as_str().unwrap();
        assert_eq!(validator.is_valid(&example["value"]), expected,
            "schema fixture {index}: {name}");
    }
    validator
}

///
/// 固定契約のrefを各Schemaの根へ解決する。
/// # 引数
/// - node/root: 走査位置と独立したSchema根。
/// # 戻り値
/// なし。外部参照・別の識別根を許可しない。
/// # 概要
/// objectと配列を走査し、全てのrefが自分の$defs内にあることを確認する。
///
fn references(node: &Value, root: &Value) {
    /*
     * Schema内に別の根や動的な参照先が紛れ込まないよう確認する。
     */
    match node {
        Value::Object(map) => {
            assert!(!map.contains_key("$id"));
            assert!(!map.contains_key("$dynamicRef"));
            for (key, value) in map {
                if key == "$ref" {
                    let reference = value.as_str().unwrap();
                    assert!(reference.starts_with("#/$defs/"));
                    assert!(root.pointer(&reference[1..]).is_some());
                } else { references(value, root); }
            }
        }
        Value::Array(values) => {
            for value in values { references(value, root); }
        }
        _ => {}
    }
}
