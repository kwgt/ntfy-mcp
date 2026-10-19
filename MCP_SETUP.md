# ntfy-mcp 0.1.0 の導入とMCP設定

本体はSTDIOのMCPサーバです。ntfyへのREST通信だけにHTTPSとBasic/Bearerを
使います。MCP登録にHTTP URL、OAuth、MCP用Bearer設定を転用しません。
以下の登録・保存・削除コマンドは利用者が選択して実行する操作です。

## 1. ビルドと実行体

Rust 1.92.0と入手済みソースを使用します。ソースのルートで実行します。

```sh
cargo build --release --locked
./target/release/ntfy-mcp --version
./target/release/ntfy-mcp --help
```

表示版は`0.1.0`です。Linuxの実行体は`target/release/ntfy-mcp`、
Windowsでは`target\release\ntfy-mcp.exe`です。以降の例の
`/absolute/path/ntfy-mcp`と`/absolute/private/config.toml`を、実際の絶対パスへ
置き換えてください。空白を含むパスはシェルで引用し、設定内では独立した文字列に
します。WindowsではTOMLのliteral文字列や正しくescapeしたパスを使います。
クライアントの作業ディレクトリやPATHに依存しない登録を推奨します。

`--help`と`--version`は設定・資格情報を読まず、通信・保存を行いません。
通常起動はMCPクライアントから行います。ターミナルへの単独起動を通知確認に
使わず、STDIOに入力する資格情報や対話プロンプトもありません。

## 2. 通知先の設定

[config.example.toml](examples/config.example.toml)を保護した場所へコピーし、
コメントの`topic`行を有効にして、利用者専用のtopicに変更します。
例はtopic未指定のひな形で、そのまま通常起動できません。
`mode = "none"`のtopic隠蔽は厳密な認証ではありません。

| 項目 | 条件 |
| --- | --- |
| server_url | 既定https://ntfy.sh/。HTTPSとhost必須、userinfo/query/fragment禁止 |
| topic | 必須。ASCII英数字・ハイフン・アンダースコア、1〜64文字 |
| auth | 省略時none。下記のいずれかの認証一式 |

URL/topicは自動trimしません。base pathを保持してtopicを1セグメント追加します。
設定・認証専用ファイルはBOMなしUTF-8のTOML、それぞれ65,536バイト以下です。
未知キー、重複キー、型不一致を拒否します。

`--config`を省略した場合の既定配置は次のとおりです。

- Linux: 絶対パスの`XDG_CONFIG_HOME`があればその配下の
  `ntfy-mcp/config.toml`。それ以外は`$HOME/.config/ntfy-mcp/config.toml`。
- Windows: RoamingAppData Known Folder配下の`ntfy-mcp/config/config.toml`。
  通常は`%APPDATA%`配下です。

明示した`--config`だけを読み、既定ファイルとはマージしません。明示ファイルの
不存在・読込不良は失敗です。既定ファイルの不存在だけは他の設定で補えます。
相対`--config`/`--auth-file`は起動時作業ディレクトリ基準で、アプリはチルダや
環境変数・globを展開しません。設定は起動時に一度だけ読み、自動再読込しません。

## 3. 優先順位と認証の保管

各項目は **CLI > 選別した環境変数 > 選択TOML > 既定値** の順に選びます。

| 項目 | CLI | 環境変数 |
| --- | --- | --- |
| server_url | --server-url | NTFY_MCP_SERVER_URL |
| topic | --topic | NTFY_MCP_TOPIC |
| 認証一式 | --user / --auth-token / --auth-fileの一つ | 下記の認証変数一式 |

認証変数は`NTFY_MCP_AUTH_MODE`と、方式に応じた`NTFY_MCP_USERNAME`、
`NTFY_MCP_PASSWORD`、`NTFY_MCP_TOKEN`です。一つでもあればMODEが必須です。
`none`はMODEのみ、`basic`はUSERNAME/PASSWORD、`bearer`はTOKENを使います。
不要な方式の変数を残さず、上位の不完全な指定から下位へfallbackしません。
`NTFY_MCP_CONFIG`や環境変数によるモード切替はありません。
クライアントに継承される環境がTOMLを上書きしないか確認してください。

認証は一式で置換し、別ソースのusername/password/tokenを混合しません。
`--user`、`--auth-token`、`--auth-file`は同時指定できません。
Basicの`--user USER:PASSWORD`は最初のコロンで分割します。
CLI秘密値はシェル履歴・プロセス引数に現れ得ます。

認証専用ファイルは[auth.example.toml](examples/auth.example.toml)のように
トップレベルへ記述します。例のBearerトークンは架空です。
設定本体へ記述する場合は、同じキーを`[auth]`テーブルに入れます。

```toml
# 認証専用ファイルでのBasicの形。値は架空です。
mode = "basic"
username = "EXAMPLE_USER"
password = "EXAMPLE_PASSWORD"
```

```toml
# 下位の資格情報を使わない認証専用ファイル
mode = "none"
```

Basicは空でないusername/password、Bearerは空白・制御文字のない有効な
ASCIIトークンが必要です。選択した方式に不要なキーを混在させません。
実資格情報を文書、登録コマンド例、通常ログ、会話へ貼り付けないでください。
OS keychain・暗号化保管・対話入力は提供しません。環境変数も秘密保管庫では
ありません。認証ファイルを使う例は次の形です。

```sh
/absolute/path/ntfy-mcp --config /absolute/private/config.toml \
  --auth-file /absolute/private/auth.toml
```

Linuxでは秘密を含むファイルを実行ユーザー所有の0600、親を0700にします。
POSIX ACLやsymlink先の権限も確認してください。WindowsではNTFS DACLを
実行ユーザーと必要な管理主体に限定し、Users/Everyoneへの広い権限と継承を
確認します。read-only属性は代用になりません。本体は既存所有者・ACL/DACLを
包括的に検査・修復しません。Windowsの既定場所はRoaming配下なので、同期を
避ける場合は`--config`/`--auth-file`でローカルの保護場所を選んでください。

## 4. 単発の保存と認証確認

次の保存は通信せず、未作成の明示ファイルも作れます。

```sh
/absolute/path/ntfy-mcp --config /absolute/private/config.toml --save-config
```

保存先が既存なら、その同じファイルを読み、CLI・選択環境変数と統合して更新します。
既存ファイルの不正TOMLは無視しません。topicが全ソースにない場合だけ、
`# topic = "YOUR_TOPIC"`のコメント付きひな形を保存できます。
明示的に空・不正なtopicを指定すると失敗します。

`--auth-file`や環境変数から選んだ認証も、保存先の`[auth]`へ**実値**で保存します。
参照先や環境変数名を記録する機能ではありません。保存先も同様に保護してください。
Linuxで本体が新規作成するファイルは0600、ディレクトリは0700です。
Windowsの新規作成物は親DACLを継承するため、先に保護した親を用意します。
本体は既存の親権限を変更しません。保存先symlinkや特殊ファイルは拒否し、
コメント・整形・キー順序の維持は保証しません。同じ設定への並列保存は避けます。
成功時は固定の完了メッセージだけを表示します。認証受理や通知成功は未確認です。

認証確認は通常登録とは別に、利用者が意図して行う単発操作です。

```sh
/absolute/path/ntfy-mcp --config /absolute/private/config.toml --check-auth
```

Basic/Bearerではbase URL配下の`v1/account`へGETを1回行い、5秒以内・応答
65,536バイト以下で資格情報の受理を調べます。retry・redirectはしません。
通知は送りませんが、アクセス情報更新やサービス利用枠への影響があり得ます。
`mode = "none"`なら通信せず確認対象外です。topicは設定として必須ですが、
**topic書込権限、publish、添付、端末表示は確認しません**。
通常起動時の自動認証確認もありません。

| 操作 | 終了コード |
| --- | --- |
| --save-config | 0保存完了、1書込/内部/停止失敗、2CLI/設定/対象不備 |
| --check-auth | 0受理確認、1拒否/通信等、2CLI/設定不備、3未設定/確認不能 |
| 通常MCP | 0通常EOF/処理したsignal、1検出した実行失敗、2CLI/設定不備 |

認証確認は安全なJSONを出力し、`credentials`と`topic_write_permission`等を
返します。生応答・資格情報は表示しません。notifyの失敗はtool resultとして
返し、通常は接続を継続します。詳細は[CLI仕様](docs/CLI_SPECS.md)を参照してください。

## 5. インスタンスごとに通知先を固定する

用途ごとに保護した設定ファイルと登録名を分けます。各インスタンスは単一の
サーバ/topicへ固定され、`notify`引数から通知先や認証を変更できません。
設定を変更したらクライアント側でサーバを再起動します。
path添付の相対パスはサーバ起動時の作業ディレクトリ基準なので、エージェントから
添付を渡す際も絶対パスを使うと曖昧さを避けられます。

## 6. Codex CLIへのSTDIO登録

以下は永続登録です。既存の同名登録がある場合は内容を確認してから更新します。
ユーザー共通の`~/.codex/config.toml`へ登録するコマンド例です。

```sh
codex mcp add ntfy-notify -- /absolute/path/ntfy-mcp \
  --config /absolute/private/config.toml
```

`--`の後が子プロセスのcommand/argsです。`--save-config`/`--check-auth`を
登録引数に含めません。秘密値を`--env`へ直接書く登録例も使いません。
手動設定では次の形です。既存の他サーバ設定を置き換えないでください。

```toml
[mcp_servers.ntfy-notify]
command = "/absolute/path/ntfy-mcp"
args = ["--config", "/absolute/private/config.toml"]
```

trusted projectだけで使う場合は`.codex/config.toml`という別scopeを選べます。
user/projectへ重複登録せず、プロジェクトの信頼と有効な設定を確認してください。
プロセス単位の`codex -c`上書きは永続登録とは別です。

## 7. Claude CodeへのSTDIO登録

プロジェクトだけで使うlocal scopeの例です。

```sh
claude mcp add --transport stdio --scope local ntfy-notify -- \
  /absolute/path/ntfy-mcp --config /absolute/private/config.toml
```

localは既定で`~/.claude.json`のプロジェクト別設定、userは同ファイルの共通設定、
projectはプロジェクトの`.mcp.json`です。`--scope user`または`project`へ
変える場合は共有範囲を確認し、一つを選びます。project設定を共有する場合も
資格情報は記録せず、実行体・保護設定のパスが各環境に合うよう管理します。

## 8. 通知せずに接続を確認する

最初は`--version`/`--help`を確認します。次に選んだクライアントで登録情報と
接続状態を確認し、`notify`だけが公開されることを確認します。
Codexは`codex mcp list`とTUIの`/mcp`、Claudeは`claude mcp list`と`/mcp`が
入口です。listは登録情報の確認であり、実接続や通知成功の証明とは分けて扱います。
設定一覧に機密パス等が含まれる場合もあるため、その出力を公開しないでください。

接続確認では通知ツールを呼ばず、`--check-auth`や実サービス用のignored試験も
実行しません。通常MCP起動とinitialize/tools/list自体はpublishしません。
MCP接続後の実通知は、利用者が意図して依頼するときに行います。
サーバ受領の成功は、受信端末への表示・Markdownレンダリング成功を意味しません。
受領不明や`duplicate_possible`がある結果を見て、無条件に手動再送しないでください。

スキル配置と通知依頼例は[SKILL_SETUP.md](SKILL_SETUP.md)を参照してください。
詳しい入力・結果契約は[基本設計](docs/BASE_DESIGN.md)、試験条件は
[TESTING](docs/TESTING.md)を参照してください。

## 9. 失敗時、更新、削除

- 起動前の設定不備は終了2、stderrの固定診断を確認します。topic補完、HTTPS、
  認証一式、ファイルの存在・権限・サイズ、継承環境を見直します。秘密を含む設定を
  丸ごとログへ出したり、TLS無効化・HTTP切替・認証なし再送で回避しません。
- SDK/STDIO・stdout書込など検出した実行失敗は終了1です。stdoutはMCP専用です。
  シェルwrapperの挨拶やデバッグ表示を混ぜず、診断はstderrで扱ってください。
- 正常EOF、処理したCtrl-C/Linux SIGTERMは新規送信停止・取消後に終了0です。
  cleanup失敗は1です。SDK待機1秒とruntime待機1秒で有限終了します。
- 更新はクライアント側で停止後、入手済み新ソースを`--locked`で再ビルドし、
  絶対パスと`--version`を確認して再起動します。設定の自動移行はありません。
- 登録解除は選んだscopeで行います。Codexは`codex mcp remove ntfy-notify`、
  Claudeは`claude mcp remove --scope local ntfy-notify`です。手動project登録は
  対応する項目だけを削除し、他サーバ設定を残します。
- 登録解除だけではバイナリ・設定・認証ファイルは消えません。不要になった自身の
  ファイルだけを削除し、バックアップやコピーも扱いを確認してください。

## 公式資料と確認範囲

2026-10-19に公式資料とローカルhelpを照合しました。ローカル版は
Codex CLI 0.163.0 / Claude Code 2.1.88です。登録・実接続を実測済みとは
していません。Windows nativeやClaude実機の動作保証を示すものでもありません。

- [Codex MCP](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)
- [Claude Code MCP](https://code.claude.com/docs/en/mcp)
- [設定仕様](docs/CONFIG_SPECS.md)
