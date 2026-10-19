# CLI仕様書

## 1. 範囲と実行形式

CLI操作・モード別出力・終了状態は本書を正とする。
設定形式・優先順位・認証ソース・保存処理は[設定仕様](CONFIG_SPECS.md)、
MCP・通知HTTP・情報保護は[基本設計](BASE_DESIGN.md)に従う。
要求の原本は `docs/REQUIREMENTS.md`。要求原本との反映差分は基本設計に明記する。

```text
ntfy-mcp [OPTIONS]
```

サブコマンド・位置引数は設けない。初版の表示バージョンは0.1.0。

| オプション | 値 | 意味 |
| --- | --- | --- |
| -c, --config | PATH | 通常時は読込元、--save-config時は読込元兼保存先のTOML |
| --server-url | URL | このインスタンスのHTTPS base URL |
| --topic | TOPIC | このインスタンスの固定topic |
| --user | USER:PASSWORD | Basic認証一式を直接指定 |
| --auth-token | TOKEN | Bearer認証一式を直接指定 |
| --auth-file | PATH | 認証一式を格納したTOML |
| --check-auth | なし | 資格情報受理を確認し終了 |
| --save-config | なし | 統合した設定をconfig.tomlまたは--configの保存先へ保存し終了 |
| -h, --help | なし | ヘルプ表示 |
| -V, --version | なし | バージョン表示 |

値を取るオプションの重複、未知オプション、値不足は終了2。
--user、--auth-token、--auth-fileは同時指定不可。
--check-authと--save-configも同時指定不可。
モード切替フラグはCLIだけで指定し、環境変数・設定から有効にしない。
MCP登録コマンドには--check-auth/--save-configを残さない。

### 1.1 認証オプション

--userは最初のコロンでusernameとpasswordに分割する。
コロンなし、username/passwordの空値はCLI不備とする。
password内の後続コロンは保持し、文字列をtrimしない。
対話的なパスワード入力は行わず、stdinは資格情報入力に使わない。
--auth-tokenはTOKENをそのままBearer認証一式にする。
構築・検証条件は設定仕様の認証契約に従う。

これらのオプションは初期導入時の--check-authと--save-configに利用でき、
通常MCP起動時にも同じ優先順位で使用できる。
CLI認証一式は環境変数・設定authを一式で置換し、下位の値で欠落を補わない。

CLI値はシェル履歴・プロセス引数に現れ得る。
継続運用では--auth-fileや環境変数も選択できる。
アプリは認証値をヘルプ、診断、通常ログ、保存完了メッセージへ反射しない。
保存モードで認証値を設定ファイルへ記録することは、表示・ログ出力とは区別する。

### 1.2 ヘルプと診断

有効にCLI解析できた--help/--versionは設定・資格情報・環境変数を読まず、
ネットワーク・MCP・signal監視・ファイル保存を開始しない。
表示はstdout、終了0。clapのenv featureで実値を表示へ埋め込まない。
parser生エラーや引数値を表示せず、固定診断と静的usageをstderrへ出す。
設定内容の表示コマンドは設けない。

## 2. 起動とモード分岐

CLI解析 → help/versionなら表示・終了 → runtime作成 →
選択モードに応じた設定読込・統合・検証 → モード実行の順とする。

| モード | 実行 |
| --- | --- |
| 通常 | HTTPSクライアント生成 → SDK STDIO MCP開始 |
| --check-auth | HTTPSクライアント生成 → 資格情報受理の確認 → 結果JSON・終了 |
| --save-config | 設定保存 → 完了メッセージ・終了。HTTPクライアント生成・MCP開始なし |

通常起動時の自動認証チェック・自動通知は行わない。
通常モードのstdoutはMCP専用で、notifyの失敗はtool resultとして返しプロセスを継続する。
CLI/設定不備は安全なstderr診断、stdout空で終了2。
HTTPクライアント生成・SDK開始等の内部失敗は終了1。
依存エラーチェーン、生の引数・設定値をそのまま表示しない。

## 3. 設定保存モード

### 3.1 入力と保存先

--save-configは初期導入で設定のひな形を作り、
必要に応じて既存設定をCLI値で更新するためのモードとする。

保存先は--config PATHがあればそのパス、なければ既定config.toml。
その同じファイルを読込元とし、存在しなければ空の設定ソースから新規作成する。
不存在を許可するのはこのモードだけであり、読込権限不足・不正TOML等は無視しない。
既存の保存先がある場合は、正常に読み込めた通常ファイルだけを更新する。
symlink・ディレクトリ・特殊ファイルを保存先として受け付けない。

### 3.2 保存内容

CLI・選別した環境変数・既存設定・既定値を設定仕様の順に統合して保存する。
同時指定の--server-url、--topic、--user、--auth-token、--auth-fileを反映する。
選択された認証情報も[auth]へ実値で保存し、
認証ファイルへの参照や環境変数名へ置き換えない。
下位ソースの未選択資格情報は保存しない。

TOMLの安全なシリアライズで引用符・バックスラッシュ等をエスケープし、
値をTOMLやコメントの構文へ直接連結しない。
モードフラグ、--configのパス、--auth-fileのパスは設定キーとして保存しない。
既存ファイルのコメント・整形・キー順序の維持は保証しない。

topicがどのソースにもない場合は、初期ひな形としてtopicキーを省略し、
固定コメント `# topic = "YOUR_TOPIC"` を付ける。
このひな形は通常起動前にtopicの補完が必要。
明示された空・不正なtopicやURL/認証の不備は、省略へ読み替えず終了2。
通常MCPと認証確認では、統合後のtopicを必須とする。

### 3.3 書込・出力・終了

保存の上限・保護・一時ファイル・置換手順は設定仕様を正とする。
ネットワーク・購読・publish・添付・系列ID生成は行わない。
stdoutには固定の完了メッセージ「設定ファイルを保存しました。」だけを出し、
設定本文、資格情報、topic、保存先パスを表示しない。

| 終了コード | 条件 |
| --- | --- |
| 0 | 保存完了 |
| 1 | ファイル作成・書込・置換、内部処理、処理した停止の失敗 |
| 2 | CLI不備、読込元・保存対象・設定値の不備、設定サイズ超過 |

保存完了は認証受理・topic書込・通知成功を意味しない。
保存後の確認は別の--check-auth呼出しで行う。

## 4. 認証確認モード

### 4.1 目的と採用理由

--check-authはBasic/Bearer資格情報の受理確認に限定する。
topic書込権限、publish・添付の成功、端末表示は確認しない。

| 比較案 | 採否 |
| --- | --- |
| 専用確認を設けない | 不採用。通常notifyと認証だけの確認を分ける |
| GET topic/auth | 不採用。固定版は読取権限のみで、書込専用資格情報を誤判定し得る |
| GET v1/account | 採用。非匿名として資格情報が受理されたことを確認 |
| 実publish試験 | 不採用。通知・利用枠消費が発生する |

accountは匿名にもHTTP200を返すためstatusだけで成功にしない。
認証基盤なし・未知roleを誤った資格情報と断定しない。
固定ntfyのtopic/authにはwrite=1等の書込確認切替がない。

### 4.2 処理

1. 通常と同じ設定検証を行う。topicも必須だがACL照会には使わない。
2. mode:noneでは通信せず確認対象外とする。
3. Basic/Bearerでは設定base URL配下v1/accountへAuthorization付きGETを1回行う。
4. 開始からbody受領・検証まで5秒。応答上限は基本設計のHTTP上限と共通。
5. HTTP200・期待するJSON object・role=userまたはadminならaccepted。
6. 結果JSONを出して終了する。

内部retry、自動再試行、redirect、認証なしfallback、購読、
publish、添付アップロード、系列ID生成、設定保存は行わない。
通常通知の再試行契約をこのモードへ流用しない。
HTTPS/TLS/proxy・安全な診断は基本設計の共通方針を使う。

account生応答には他アクセストークン実値が含まれ得る。
role等の最小型だけを抽出し、他のフィールドを捨てる。
生応答、ユーザー名、ACL、トークンをログ・出力へ出さない。
上限超過を認証拒否と混同しない。
Bearer最終アクセス情報更新やサービス側request制限を伴い得るため、
完全無副作用とは説明しない。

### 4.3 JSON出力

設定検証後に確認モードへ入った場合はstdoutへ改行付きJSONを一つ出す。
通常MCPのstdoutとは別モードである。

| 項目 | 型・内容 |
| --- | --- |
| ok | boolean。受理確認時だけtrue |
| mode | check_auth |
| credentials | accepted / rejected / unconfirmed / not_configured |
| topic_write_permission | 常にnot_checked |
| publish | 常にnot_performed |
| attempts | 整数0または1 |
| http_status | 取得した整数またはnull |
| ntfy_error_code | 整合を検証した整数またはnull |
| error | 成功時null、失敗時category/code/messageのobject |

成功例:

```json
{
  "ok": true,
  "mode": "check_auth",
  "credentials": "accepted",
  "topic_write_permission": "not_checked",
  "publish": "not_performed",
  "attempts": 1,
  "http_status": 200,
  "ntfy_error_code": null,
  "error": null
}
```

認証なしの例:

```json
{
  "ok": false,
  "mode": "check_auth",
  "credentials": "not_configured",
  "topic_write_permission": "not_checked",
  "publish": "not_performed",
  "attempts": 0,
  "http_status": null,
  "ntfy_error_code": null,
  "error": {
    "category": "check",
    "code": "no_credentials",
    "message": "確認対象のBasicまたはBearer資格情報が設定されていません。"
  }
}
```

### 4.4 終了状態

| 終了コード | 条件 |
| --- | --- |
| 0 | 資格情報受理を確認 |
| 1 | 認証拒否、TLS/通信/timeout、明示的サーバ失敗 |
| 2 | CLI/設定不備。確認モードへ入らずstdout空 |
| 3 | 資格情報なし、API非対応、匿名/未知role、形式/サイズ等で確認不能 |

401/403はcredentials=rejected、終了1。
429と5xxは再試行せず終了1。Retry-Afterの自動待機なし。
匿名・未知role、不正JSON・過大応答、404等はunconfirmed、終了3。
未知HTTP状態もunconfirmed、終了3。通信失敗等の受理不明もunconfirmedとする。

## 5. 通常モードの終了と停止

| 状況 | 終了・動作 |
| --- | --- |
| STDIO EOF／正常cleanup | 0 |
| 処理したCtrl-C、Linux SIGTERM | 新規送信停止・取消、有限cleanup後0 |
| CLI/設定不備 | 2、MCP開始前 |
| SDK/transport/stdout書込等の実行失敗 | 1 |
| notifyの入力・送信失敗 | MCP結果を返して継続 |

EOFだけで正常終了と予期しない切断を完全に区別しない。
検出済みtransport異常は終了1へ伝える。
Tokio signal機能でCtrl-CとLinux SIGTERMを監視し、
基本設計のSDK待機1秒＋runtime待機1秒の有限終了を使う。
signal登録不能なら安全なstderr診断後その監視を無効にし、EOF終了は維持する。

確認モードで処理した停止は終了1、可能ならunconfirmedのJSONを返す。
保存モードは置換前の停止で一時ファイルを片付けて終了1とする。
置換完了後の停止は設定保存を取り消せず、完了通知を受け取れない場合もある。
CLI/設定読込中の処理した停止は終了1、stdout空。
送信済み通知の取消や停止結果のクライアント受領を保証しない。
Windowsの強制終了・コンソール強制閉鎖はgraceful終了の保証対象外である。

## 6. 操作例

以下は説明用の架空値で、コマンドは未実行。

```sh
ntfy-mcp --save-config --topic EXAMPLE_TOPIC --user 'EXAMPLE_USER:EXAMPLE_PASSWORD'
ntfy-mcp --save-config --config /work/config/config.toml --topic EXAMPLE_TOPIC --auth-token EXAMPLE_TOKEN
ntfy-mcp --config /work/config/config.toml --check-auth --auth-token EXAMPLE_TOKEN
ntfy-mcp --config /work/config/config.toml
```

初回保存後は、保存モードのフラグを外して認証確認・通常起動を行う。
通常起動にも--user/--auth-tokenを指定できる。
Windowsでは.exeを使用し、空白入りパスは起動元シェルまたはMCPホストの引数配列で一つにまとめる。

## 7. 根拠と検証観点

- [clap Command](https://docs.rs/clap/4.6.7/clap/struct.Command.html)
- [Tokio signal](https://github.com/tokio-rs/tokio/tree/tokio-1.53.3/tokio/src/signal)
- [ntfy固定ルーティング](https://github.com/binwiederhier/ntfy/blob/f18d1137c63a26e85b91ce639e7bc72bb9a82736/server/server.go)
- [ntfy固定account](https://github.com/binwiederhier/ntfy/blob/f18d1137c63a26e85b91ce639e7bc72bb9a82736/server/server_account.go)

CLI認証の排他・分割・秘匿、保存時の統合・新規作成・更新・失敗・取消、
保存モードで通信やMCPを開始しないこと、help/versionの無副作用、
確認モードのHTTP/role/形式/上限/取消、通常MCPとsignal/EOFを実装・受入検証する。
