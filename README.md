# ntfy-mcp 0.1.0

AIエージェントからユーザーへ、[ntfy](https://ntfy.sh/)を使って進捗・完了・
判断依頼・障害を通知するRust製MCPサーバです。トランスポートはSTDIOです。
通知先のHTTPSサーバとtopicをインスタンスごとに固定し、単一の`notify`ツールを
提供します。汎用ntfyクライアントではありません。

## 提供する機能

- Markdown本文と任意のタイトルを送信します。通知種別は`progress`、
  `completed`、`question`、`alert`です。優先度とタグは種別から決まります。
- 本文は最大1,000 Unicodeスカラー値、かつUTF-8で4,096バイト以下です。
  前後空白など本文を保持できない入力は拒否し、自動整形・切捨てはしません。
- 添付は1通知につき1つ、実データで最大2,000,000バイトです。ローカルpathと
  UTF-8/Base64直接指定に対応し、元ファイルを変更しません。
- `sequence_id`を返し、同じタスクの後続通知で再利用できます。
  表示の置換や重複排除を保証するIDではありません。
- HTTPSと認証なし・Basic・Bearerに対応します。資格情報や通知本文を通常ログへ
  出しません。受領不明の再試行では重複する可能性を結果へ含めます。

## 導入

入手済みソースのルートで、Rust 1.92.0を使ってビルドします。
公開済みバイナリやリリースの存在を前提にしません。

```sh
cargo build --release --locked
./target/release/ntfy-mcp --version
```

設定のtopicを用意してから、MCPクライアントへ実行体と設定ファイルの絶対パスを
登録してください。通常起動だけで通知や認証確認は行いません。

- [MCP_SETUP.md](MCP_SETUP.md): ビルド、設定・認証の保管、CLI、
  Codex CLI / Claude CodeへのSTDIO登録、通知なし確認、更新・削除。
- [SKILL_SETUP.md](SKILL_SETUP.md): スキルの配置と共通SKILL.md本文。
- [設定例](examples/config.example.toml)と[認証専用例](examples/auth.example.toml):
  実情報を含まない例です。そのまま通常運用に使わず、必要項目を補ってください。
- [docs/TESTING.md](docs/TESTING.md): ローカル試験と明示実行の実サービス試験。
  通常のテストは外部へ送信しません。実サービス試験の実行は別操作です。

## 対応と確認範囲

0.1.0はLinuxとWindowsを考慮した設計で、Codex CLIとClaude Code向けの
設定手順を提供します。Windows native、Claude Code、iOS、セルフホスト全般、
全バージョンの互換性を保証するものではありません。

Linux / Rust 1.92.0でビルドと通常60件の試験成功を確認しています。
実サービス試験1件は通常実行ではignoredです。

2026-10-19、本体0.1.0をLinux 6.12.62+rpt-rpi-2712で使用し、Codex CLI
0.163.0の一時設定とApp Server登録MCP APIから本文通知を実行しました。
path添付とUTF-8直接添付は共有ケースrunnerで確認しました。3件ともサーバ受領を
確認し、添付2件は取得内容一致と原本不変を確認しています。Windows 11 /
Firefox 158.0、Android 17 / ntfyアプリ1.25.2での到達・本文・添付操作は
利用者確認済みです。これらは実施環境の記録で、対応版範囲の保証ではありません。

永続登録、実スキル配置・読込、モデルの自律的なツール選択、Codex経由の添付2方式
そのものの実サービス確認は未実施です。Markdown送信・サーバ受領と、端末への
到達・本文や添付の操作確認を区別し、Markdownの具体的な描画形式は保証しません。
実施経路・結果と未確認範囲の詳細は[検証方法と確認範囲](docs/TESTING.md)を
参照してください。

詳細な契約は[要求仕様](docs/REQUIREMENTS.md)、[基本設計](docs/BASE_DESIGN.md)、
[CLI仕様](docs/CLI_SPECS.md)、[設定仕様](docs/CONFIG_SPECS.md)を参照してください。

## ライセンス

[MIT License](LICENSE)。Copyright (C) 2026 Hiroshi KUWAGATA。
