# ntfy-mcp 0.1.0 のスキル設置

この手順は[本体のMCP登録](MCP_SETUP.md)が完了し、クライアントで`notify`を
利用できることを前提とします。スキルはエージェントへの指示で、本体・設定・
資格情報をインストールするものではありません。配置だけで通知は送りません。

## 1. 配置先を一つ選ぶ

スキル名は`ntfy-notify`です。クライアントごとにprojectまたはuserの一方を
選び、同名の古いスキルがあれば内容を確認してから更新してください。

| クライアント | project | user |
| --- | --- | --- |
| Codex CLI | .agents/skills/ntfy-notify/ | ~/.agents/skills/ntfy-notify/ |
| Claude Code | .claude/skills/ntfy-notify/ | ~/.claude/skills/ntfy-notify/ |

各ディレクトリに`SKILL.md`を作ります。末尾の「共通SKILL.md本文」コードブロックを
そのまま保存してください。両クライアントで共通のYAML front matterと本文です。
UTF-8/BOMなし・LFを使い、外側のコードフェンスをファイルへ含めません。
必要に応じて、MCP登録名`ntfy-notify`を実際に選んだ登録名に合わせます。
複数インスタンスがある場合は依頼された通知先の登録を使い、推測で選びません。
スキルへtopic・トークン・パスワードを埋め込まないでください。

Codexで任意の表示情報を付ける場合だけ、選んだディレクトリの
`agents/openai.yaml`を次の内容で作成します。

```yaml
interface:
  display_name: "ntfy通知"
  short_description: "ユーザー依頼に沿う進捗・完了・判断依頼・障害の通知"
  default_prompt: "ntfy-notifyで依頼したタスクの通知を扱ってください。"
```

これは任意の表示metadataです。STDIO登録にHTTP用のURL/transport依存metadataを
追加しません。MCP登録は別途必要です。Claude用の`allowed-tools`も必須ではなく、
ここでは指定しません。クライアントの権限設定をこの手順で広げないでください。

## 2. 認識確認・利用・更新・削除

クライアントを再起動するか、そのクライアントのスキル再読込手順を使い、
`ntfy-notify`が認識されることを確認します。読込確認のための通知は送りません。
スキルを名前で指定し、通知の契機を利用者から具体的に依頼できます。例えば:

- 「ntfy-notifyを使い、この調査の完了を通知してください。」
- 「長い処理の区切りで進捗を知らせ、判断が必要なら質問通知を送ってください。」
- 「このタスクの進捗と完了には同じ系列IDを使ってください。」

通知内容に秘密や不要な個人情報を含めず、依頼された頻度・範囲で使います。
スキルの読込、接続確認、文書内の例の存在を実送信の依頼と解釈しません。
本体の自動retryと、エージェントによる新しい呼出しは区別してください。

更新では選んだ`SKILL.md`をこの文書の本文に合わせ、任意metadataと登録名も
確認して再読込します。別の配布用本文や自動インストーラは必要ありません。
削除では自分で配置した`ntfy-notify`ディレクトリだけを取り除きます。
スキル削除はMCP登録解除や設定・資格情報削除を行いません。

## 3. 確認範囲と資料

2026-10-19時点の公式資料に基づく手動配置手順です。手元の版は
Codex CLI 0.163.0 / Claude Code 2.1.88ですが、実配置・実読込・実通知は
まだ確認していません。Windows/Claude実接続の動作保証を示しません。

- [Codex Skills](https://learn.chatgpt.com/docs/build-skills)
- [Claude Code Skills](https://code.claude.com/docs/en/skills)
- [MCP設定手順](MCP_SETUP.md)
- [入力・結果の基本設計](docs/BASE_DESIGN.md)

## 4. 共通SKILL.md本文

以下のコードブロック全体が保存する`SKILL.md`です。JSON例は入力形式の説明で、
自動実行するものではありません。

````markdown
---
name: "ntfy-notify"
description: "ユーザーから依頼されたタスクの進捗・完了・判断依頼・障害をntfyで通知する。"
---

# ntfyによるユーザー通知

登録済みのntfy MCPサーバの`notify`ツールを使う。標準登録名は`ntfy-notify`。
スキルの読込や接続確認だけでは送信しない。ユーザーが求めた通知の契機・頻度・
対象タスクに従う。既存の通知依頼の範囲で進捗や完了を扱い、無関係な通知を増やさない。
複数の登録があれば依頼された通知先のものを使う。通知先と認証は本体設定で固定され、
ツール引数やスキルへtopic・URL・資格情報を追加しない。

## 通知種別と系列

| kind | 使う場面 | priority / tag |
| --- | --- | --- |
| progress | 長い処理の進捗や区切り | low / hourglass_flowing_sand |
| completed | タスクの通常完了 | default / white_check_mark |
| question | ユーザーの判断が必要 | high / question |
| alert | 障害・セキュリティ・期限切迫 | urgent / rotating_light |

kindとmessageは必須。任意priority、追加tag、進捗率専用フィールドはない。
進捗率が必要なら本文に書く。任意項目は省略し、nullや未知フィールドを渡さない。

最初の呼出しでsequence_idを省略すると本体が生成して結果へ返す。同じタスクの
後続進捗・完了では、その返却IDを保持して再利用する。生成済みなら失敗結果にも
返るが、不正入力や生成失敗等でnullならIDが得られたと扱わない。
明示IDはASCII英数字・ハイフン・アンダースコアの1〜64文字。
自動retryは同じIDと内容を保持する。sequence_idはntfyのmessage_idと異なり、
端末上の表示置換、履歴の重複排除、永続的タスク管理を保証しない。

## 本文とタイトル

messageはMarkdownの元本文。非空・空白のみ不可で、最大1,000 Unicodeスカラー値、
かつUTF-8で4,096バイト以下。表示文字数ではなく、Markdown記号、空白、内部改行、
結合文字や複数要素の絵文字もスカラー値として数える。

添付の有無にかかわらず、前後空白によりサーバ側のtrimで変化する本文は拒否される。
内部の実改行や日本語は受理可能な範囲で保持する。添付同送時は、文字としての
バックスラッシュに続くnも拒否される。添付なしではこの2文字は許可される。
JSONの改行escapeを復号した実改行とは区別する。

自動trim、Unicode正規化、改行置換、切捨て、分割、本文の添付化はしない。
本文だけ・別通知・アップロード後URL通知への自動fallbackもない。
不適合なら意図を保つ要約や整形をエージェントが明示的に行い、依頼の範囲で
新しい入力を作る。黙って内容を変えて再送しない。

任意titleは非空、UTF-8で1,024バイト以下。前後空白、改行、制御文字を含めない。
省略すればタイトルを送らない。本文・タイトルに秘密情報を載せない。

## 添付

1通知につき添付は1つ。本文と同じ通知でアップロードする。外部URL添付や複数添付、
pathとdataの混在は不可。content_type入力はない。
実データの上限は共通で2,000,000バイト。0バイトも受理範囲だが、上限以下でも
回線・サーバ制限により送信できるとは限らない。

path方式は通常ファイルを指定し、元ファイルを変更・削除しない。
相対pathはサーバ起動時の作業ディレクトリ基準で、チルダ・環境変数・globの
展開はしない。曖昧さを避けるには絶対pathを使う。pathはUTF-8で4,096バイト以下。
filename省略時は末尾名を使い、末尾名がUTF-8でなければ明示filenameが必要。
読取は5秒の論理期限で、同期I/Oの即時停止は保証されない。

```json
{
  "kind": "progress",
  "message": "調査の中間報告です。",
  "attachment": {"source": "path", "path": "/absolute/path/report.txt"}
}
```

直接指定はsource:inline、encoding:utf8またはbase64、data、filenameが必須。
UTF-8はその実バイト数、Base64は復元後の実サイズで上限を判断する。
Base64は標準アルファベット・正規padding・改行なしとし、符号化文字数だけで
上限内と判断しない。MCP入力1行の32MiB制限は添付実サイズ上限とは別。

```json
{
  "kind": "completed",
  "message": "調査が完了しました。",
  "attachment": {
    "source": "inline",
    "encoding": "utf8",
    "data": "調査結果です。\n",
    "filename": "report.txt"
  }
}
```

Base64方式は同じinline構造のencodingをbase64にし、dataへ標準Base64を入れる。
例えば1バイトのAはdata:"QQ=="。filenameは非空でUTF-8の255バイト以下。
ドット1個/2個、パス区切り、改行・制御文字、前後trimで変化する名前を使わない。
ローカル読取・サイズ超過・アップロード失敗で本文だけに切り替えない。

## 結果と再送判断

structuredContentと互換用Textには同じJSONが返る。同じ結果を二度の通知と数えない。
ok:true/isError:falseはサーバの受領確認であり、端末表示やMarkdown描画の確認ではない。
失敗はok:false/isError:true。受信端末を追加pollして確認する機能はない。

| 結果項目 | 読み方 |
| --- | --- |
| sequence_id | 採用・生成済み系列ID。nullなら未確定 |
| attempts | 実HTTP試行数。送信前失敗0、最大4 |
| receipt | not_sent未送信、confirmed受領確認、unknown受領不明、rejected拒否 |
| duplicate_possible | 本呼出し内の自動再送による重複可能性。手動再送の安全保証ではない |
| message_id | 検証済みntfyメッセージID。系列IDと別 |
| http_status / ntfy_error_code | 得られた最終試行の状態/コード、なければnull |
| error | category/code/message/field/stop_reason。成功時null |
| wait | 自動retryの待機判断、なければnull。手動再送許可ではない |

error.categoryはinput、attachment、resource、internal、tls、transport、auth、
limit、rate_limit、server、http、protocol、cancelledに分かれる。
codeとfieldを使って入力・読取・busy・認証・制限などを区別し、生エラーを要求しない。
stop_reasonはnon_retryable、attempts_exhausted、budget_exhausted、
retry_after_invalid、cancelled。根本codeと打切り理由を両方見る。

本体は1試行5秒、最初のHTTP送信から待機込み45秒、初回を含む最大4試行で
対象の一時障害だけ自動retryする。添付読取はこの45秒に含まない。
同時通知は1件で使用中はbusy。busyでも無制限に再呼出ししない。

waitにはsource、delay_seconds、waited_ms、remaining_seconds、
retry_after_stateがある。remaining_secondsは最後の有効待機期限までの切上げ秒、
0は期限経過、nullは算出不能。日次枠や未知制限では0でも即時再送しない。
不正/巨大Retry-Afterなどで待てない場合も、短縮待機して再送しない。

入力・読取・サイズ不備はHTTP前に直す。TLSや認証の失敗をHTTP切替・証明書検証無効・
認証なし再送で回避しない。受領unknown、duplicate_possible、取消や中断時は、
既に届いた可能性をユーザーへ伝え、無条件の手動再送や別経路の二重送信を止める。
後のretryが成功しても過去試行による重複可能性は残り得る。
既に同じケースを送ったrunner等を確認用に再実行しない。

利用者への報告は必要な安全な結果だけとし、topic、資格情報、本文・添付の生データ、
ローカルpath、添付URL、生応答をログや報告へ転載しない。
````
