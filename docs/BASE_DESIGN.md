# 基本設計書

## 1. 位置付けと適用範囲

ntfyを利用し、AIエージェントからユーザーへの通知を提供する
STDIO MCPサーバを設計する。初版は0.1.0とし、Windows/Linuxでの動作を考慮する。
MCPはCodex CLIとClaude Codeでの利用を想定し、両者向けの利用文書を維持する。
実機確認はLinux上のCodex CLIからntfy.shへ送り、
Windows 11のブラウザとAndroidのntfyアプリで受信する経路を対象とする。
CPU別・OS別・MCPクライアント別の実機検証や、iOSの確認を網羅しない。
OS・クライアント・ブラウザ・アプリの版は実施時に記録し、
具体版の事前承認を実装開始の条件としない。
最小・最大対応版や全版・全組合せの互換性を保証しない。
セルフホスト互換APIへの接続を妨げないが、全般の動作保証はしない。
Markdown指定と端末のMarkdown表示・通知表示置換は区別する。

要求の原本はリポジトリの `docs/REQUIREMENTS.md`。
CLIの操作・終了状態は[CLI仕様](CLI_SPECS.md)、
設定・認証ソース・保存とOS権限は[設定仕様](CONFIG_SPECS.md)を正とし、
同じ規則を各文書で重複管理しない。

要求2.2・2.5・6に従い、添付の有無にかかわらず、採用伝送方式で
本文を保持できない入力を送信前に拒否する。
具体的な受入条件は4.2に定め、ツールdescriptionと利用スキルへ明示する。

## 2. 責務と実行の流れ

| 責務 | 担当 |
| --- | --- |
| main | 終了状態の決定、安全な起動失敗の表示 |
| CLI・設定読込 | 引数解析、優先順位の適用、起動時検証 |
| 実行制御 | 不変の有効設定、共有HTTPクライアント、MCP生成・終了 |
| MCPアダプター | tools/list・tools/call、Schema、結果のMCP形式化 |
| 通知処理 | 系列ID確定、入力検証、種別変換、添付準備、結果集約 |
| 添付入力 | path読取／inline復元、実サイズ検査、表示名の確定 |
| ntfyクライアント | 固定通知先へのHTTP構築・送信・受領応答解釈 |

これは論理責務であり、ソースファイル分割の指定ではない。

通常MCPモードの起動はCLI解析、設定統合・検証、
クライアント生成、rmcp STDIO開始の順。
起動時に判定できる設定不備はMCP開始前に失敗させる。
初期化とプロトコル交渉はSDKに任せる。
起動・終了時の自動通知、通常起動時の自動認証試行は行わない。
認証確認・TOML設定保存モードの分岐と動作はCLI仕様に従う。
設定保存ではHTTPクライアント・MCPを開始しない。

notifyでは系列ID確定、入力検証、添付準備、ヘッダーを含む送信内容確定、
HTTP送信、結果集約、MCP結果返却の順に処理する。
入力・準備失敗ではHTTPを開始しない。
再試行では同じID・本文・添付バイトを使う。

通知処理は呼出しのfutureに紐付け、結果返却後も送信する独立タスクは作らない。
RequestContext::ct、接続終了、停止を監視し、取消後の新規試行を止める。
送信済み通知の巻戻しや取消後の結果受領を保証しない。
RunningService::waiting()で終了を監視し、明示停止時はclose()/cancel()を使う。
QuitReason::Closedだけで正常EOFと予期しない切断を区別しない。
SDK終了待ち1秒、その後明示構築するTokio runtimeの
shutdown_timeout待ち1秒で有限終了し、mainから戻る。
OSシグナル・終了コードはCLI仕様に従う。

## 3. MCP契約

### 3.1 単一notifyの入力

| 項目 | 型 | 必須・意味 |
| --- | --- | --- |
| kind | string enum | 必須。progress / completed / question / alert |
| message | string | 必須。Markdown元本文 |
| title | string | 任意。省略時はタイトルを送らない |
| sequence_id | string | 任意。指定した系列ID |
| attachment | object | 任意。sourceによるpath/inlineのoneOf |

| kind | priority | tags |
| --- | --- | --- |
| progress | low / 2 | hourglass_flowing_sand |
| completed | default / 3 | white_check_mark |
| question | high / 4 | question |
| alert | urgent / 5 | rotating_light |

任意priority・追加tag・進捗率専用入力は提供しない。進捗率は本文へ記述する。

| attachment.source | 必須 | 任意 |
| --- | --- | --- |
| path | source、path | filename。省略時はpath末尾名 |
| inline | source、encoding、data、filename | なし |

encodingはutf8またはbase64。1通知に添付1つ。
pathとdataの同時指定、複数添付、外部URL添付は受け付けない。
content_typeの入力は設けず、ntfyの実データ・ファイル名による検出に任せる。
利用者が指定するMIME型の保持は保証しない。

### 3.2 Schemaと実行時検証

公表inputSchemaは最上位object、必須kind/message、
添付は識別子付きoneOfとする。各入力objectの未知フィールドを拒否する。
任意項目は省略を許可し、nullは拒否する。
文字列のUnicode正規化・改行置換・trim・エスケープ自動修正を行わない。

| 対象 | 検証契約 |
| --- | --- |
| message | 非空・空白のみ不可。1,000 Unicodeスカラー値以下かつUTF-8で4,096バイト以下 |
| title | 非空、復号後1,024 UTF-8バイト以下。前後空白・改行・制御文字不可 |
| sequence_id | ASCII英数字・ハイフン・アンダースコアの1〜64文字 |
| filename | 非空、255 UTF-8バイト以下。ドット／ドット2個、パス区切り、改行・制御文字、trimで変わる値を拒否 |
| path | 4,096 UTF-8バイト以下。通常ファイルを対象とする |
| 添付実データ | 2,000,000バイト以下。0バイトも受理範囲 |

文字数は表示文字数ではなくUnicodeスカラー値で数え、
Markdown記号・空白・改行・結合文字も含める。
SchemaのmaxLengthだけに依存せずUTF-8バイト数も実行時に検査する。
filename/path上限は入力表現の契約であり、全OSのパス上限を保証しない。

本文保持条件は4.2を正とする。受理できない入力を自動整形しない。
入力・読取・上限の制約はツールdescriptionと利用スキルで周知する。

rmcp3.5.1のParameters<NotifyInput>の自動変換は、
ハンドラー到達前にinvalid_paramsを生成し、入力値を診断へ含み得る。
notifyはargumentsオブジェクトを受け、内部で型変換・検証し、
安全なisError=trueのtool resultへ変換する。
公表Schemaと内部型・検証を一致させる。
JSON-RPC構文不正、不正なtools/call外枠、未知ツールは
SDKのプロトコルエラーとする。JSON-RPCは自作しない。

### 3.3 系列ID

有効な指定IDはそのまま採用する。空文字を省略と解釈しない。
不正な指定では採用IDをnullとし、送信前エラーとする。
省略時はgetrandomでOS乱数128ビットを得て、
32文字の小文字hexへ変換し、論理呼出しにつき一度生成する。
生成後の入力・読取・送信失敗にも返し、再試行で生成し直さない。
生成失敗やJSON-RPC不成立では生成ID返却を保証しない。

時刻・PID・連番だけの方式より複数ホストや再起動での衝突を避けやすく、
UUID固有機能を要しないためこの方式を採用する。
全履歴を保存した絶対的一意性や永続タスク管理は提供しない。
利用者指定IDの再利用は系列通知として許可する。
ntfy message_idとは区別し、表示置換・履歴重複排除は保証しない。

### 3.4 出力の型

成功・失敗を包含するoutputSchemaを提供する。
structuredContentと互換用TextContentに同じJSONを格納する。
成功時isError=false、入力・処理失敗時isError=true。

| 項目 | 型・意味 |
| --- | --- |
| ok | boolean。処理成功 |
| sequence_id | stringまたはnull。採用・生成済みID |
| receipt | not_sent / confirmed / unknown / rejected |
| attempts | 非負整数。実HTTP試行数、送信前失敗0、最大4 |
| duplicate_possible | boolean。本呼出し内の自動再送による重複可能性 |
| message_id | stringまたはnull。検証済みntfy ID |
| http_status | 整数またはnull。最終試行のHTTP状態 |
| ntfy_error_code | 整数またはnull。最終試行の整合したntfyコード |
| error | 成功時null。失敗時は下表object |
| wait | 待機判断があれば下表object、なければnull |

errorはcategory、code、固定テンプレートのmessage、
field（該当なければnull）、stop_reasonを持つ。
stop_reasonはnon_retryable / attempts_exhausted / budget_exhausted /
retry_after_invalid / cancelled。
上限で止まっても根本のcodeを保持する。

| wait項目 | 型・意味 |
| --- | --- |
| source | retry_after_seconds / retry_after_date / rate_limit_default / exponential / invalid |
| delay_seconds | 最後の待機秒。巨大値・不正はnull |
| waited_ms | 実際の再試行待機時間の合計（ミリ秒） |
| remaining_seconds | 最後の有効待機期限まで切上げ秒。終了済み0、算出不能null |
| retry_after_state | missing / valid / invalid / too_long |

waitは手動再送の許可ではない。日次・未知制限では残り0でも直ちに再送しない。
結果へ本文、topic、資格情報、添付URL、生添付、ローカルpath、
生のntfy応答を返さない。

### 3.5 入出力例

同系列の進捗と完了:

```json
{
  "kind": "progress",
  "message": "検証を開始しました。進捗は20%です。",
  "sequence_id": "task_example_01"
}
```

```json
{
  "kind": "completed",
  "message": "検証が完了しました。\n詳細は添付を参照してください。",
  "sequence_id": "task_example_01",
  "attachment": {
    "source": "path",
    "path": "/work/results/report.txt",
    "filename": "report.txt"
  }
}
```

直接データは次のように指定する:

```json
{
  "kind": "completed",
  "message": "結果を添付します。",
  "attachment": {
    "source": "inline",
    "encoding": "utf8",
    "data": "検証結果: 成功\n",
    "filename": "report.txt"
  }
}
```

バイナリ直接指定のattachment部分:

```json
{
  "source": "inline",
  "encoding": "base64",
  "data": "AAECAw==",
  "filename": "data.bin"
}
```

初回timeout後の再送成功（isError=false）:

```json
{
  "ok": true,
  "sequence_id": "task_example_01",
  "receipt": "confirmed",
  "attempts": 2,
  "duplicate_possible": true,
  "message_id": "AbCdEf123456",
  "http_status": 200,
  "ntfy_error_code": null,
  "error": null,
  "wait": {
    "source": "exponential",
    "delay_seconds": 1,
    "waited_ms": 1000,
    "remaining_seconds": 0,
    "retry_after_state": "missing"
  }
}
```

送信前失敗（isError=true）:

```json
{
  "ok": false,
  "sequence_id": "task_example_01",
  "receipt": "not_sent",
  "attempts": 0,
  "duplicate_possible": false,
  "message_id": null,
  "http_status": null,
  "ntfy_error_code": null,
  "error": {
    "category": "input",
    "code": "attachment_message_not_preservable",
    "message": "添付同送本文に文字としてのバックスラッシュnが含まれています。",
    "field": "message",
    "stop_reason": "non_retryable"
  },
  "wait": null
}
```

一時429でも60秒待機が予算に入らない場合（isError=true）:

```json
{
  "ok": false,
  "sequence_id": "task_example_01",
  "receipt": "rejected",
  "attempts": 1,
  "duplicate_possible": false,
  "message_id": null,
  "http_status": 429,
  "ntfy_error_code": 42901,
  "error": {
    "category": "rate_limit",
    "code": "transient_rate_limit",
    "message": "指定された待機と次の試行が送信時間の上限に収まりません。",
    "field": null,
    "stop_reason": "budget_exhausted"
  },
  "wait": {
    "source": "retry_after_seconds",
    "delay_seconds": 60,
    "waited_ms": 0,
    "remaining_seconds": 60,
    "retry_after_state": "valid"
  }
}
```

時刻・実待機値・IDは説明用の架空値である。

## 4. 送信と本文・添付の保持

### 4.1 HTTP形式

| 入力 | 送信 |
| --- | --- |
| 添付なし | 固定topicへPOST。bodyはUTF-8のMarkdown元本文 |
| 添付あり | 固定topicへPUT。bodyは読取・復元済み添付バイト |

共通ヘッダーはX-Markdown:true、X-Priority、X-Tags、X-Sequence-ID、
任意のX-Title。添付にはX-FilenameとX-Messageを追加する。
Content-Lengthは確定した実body長。
multipart、テンプレート、外部URL参照へ変更しない。

X-Title・X-Filename・X-Messageは、ASCIIだけの値も含め、
常に元の入力文字列をRFC2047のUTF-8 Base64 encoded-wordで一度符号化する。
入力にencoded-wordのような文字列があっても先に復号せず、
その文字列自体を符号化して包む。
元UTF-8を45バイト以下かつスカラー境界で分割し、
各wordを75バイト以下、word間をASCIIスペースで接続する。
サーバによる一段のMIME復号で元入力へ戻ることを前提とし、
復号後の入力上限も別に検査する。

### 4.2 本文保持条件

固定ntfyはX-MessageをMIME復号し、TrimSpace後、
文字としてのバックスラッシュ+nを実改行へ変換する。
4.1の符号化による一段のMIME復号で元入力に戻るため、
見かけがencoded-wordのASCII文字列であること自体は拒否理由としない。
その復号後にTrimSpaceまたは文字としてのバックスラッシュ+nの変換で
本文が変わる添付同送入力を送信前拒否する。
内部の実改行、日本語、Markdownコード例は受理可能な範囲で保持する。
判定空白集合は固定Go実装に合わせ、Rust側の判定との一致を検証する。

添付なしraw bodyにもTrimSpaceが適用される。
前後空白で変わる入力を送信前拒否する。
添付なしの文字としてのバックスラッシュ+nは許可する。

受理できない本文を自動trim、切捨て、分割、添付化しない。
本文だけ・別通知・アップロード後URL通知への自動変更をしない。
エージェントが明示的に整形して改めて呼び出す。

### 4.3 添付準備

HTTP開始前に両方式の実データを上限内で用意する。
UTF-8 inlineはコピー前に実バイト数を検査する。
Base64は標準アルファベット・正規padding・改行なしを受理する。
文字・paddingを検査し正確な復号長を算出してから確保し、
復号後にも実サイズを検査する。
符号化最大2,666,668文字だけでは2MB境界を判定しない。

pathは起動時作業ディレクトリ基準。チルダ・環境変数・glob展開なし。
通常ファイルを指すsymlinkは許可し、特殊ファイルは拒否する。
ファイルアクセスはローカル権限の範囲内とし、
MCP rootsをファイルアクセス権限として扱わない。
末尾名がUTF-8で表せなければ明示filenameを要求する。
専用spawn_blocking内で事前metadata、open後handleの通常ファイル確認、
最大2,000,001バイトの分割読取を行う。
metadataのサイズだけに依存せず、読取中の拡大も検出する。
外部同時更新に対する一貫したスナップショットは保証しない。

読取済みバイトは呼出し中のメモリーに保持し、再試行で読み直さない。
元ファイルを変更・削除せず、永続キャッシュ・一時ファイルを作らない。
アップロード失敗も通知失敗とし、本文だけへfallbackしない。

0バイトも受理し、応答attachmentのsize省略を0として解釈する。
null・負数・非整数を拒否し、0でも添付存在等の検査を省略しない。
固定ソースのfilenameによる添付分岐は確認済みだが、
0バイトの保存・download成功は未検証であり保証しない。

## 5. 時間・再試行・受領

### 5.1 実行枠と読取期限

重い通知処理はインスタンスで同時1件。
待ち行列を作らず使用中の別notifyはresource/busy、試行数0で返す。
pathのmetadata/open/read全体は5秒の論理期限とする。
分割読取の間に取消と単調時計を検査し、外側でも期限・取消を監視する。
期限超過はattachment_read_timeoutでHTTPへ進まない。
遅れて返った結果は破棄する。

開始済みblocking I/Oを即時abortする保証はない。
残存読取が終わるまで実行枠を保持し、workerの増殖を防ぐ。
残存workerは送信経路を持たず、後続notifyはbusyとなる。
必要ならプロセス再起動で復旧する。
5秒は物理的読取停止の保証ではない。
symlink差替え競合や停止するネットワークファイルシステムは排除できない。
終了時は2章の有限待機を使い、blocking完了を無期限に待たない。

### 5.2 HTTP予算

reqwest内部retryをretry(reqwest::retry::never())で無効化する。
アプリの単一ループで最大4試行を管理する。
HTTP future開始直前にattemptsを加算する。
各試行5秒に接続・TLS・アップロード・応答body取得・受領確認を含める。
read timeoutの反復で期限を延長しない。

初回HTTP開始時に単調時計で全体45秒期限を固定する。
入力検証・添付準備時間は含めない。
追加試行番号k=1,2,3に対し通常1,2,4秒、
一時429でRetry-After欠落時5,10,20秒を待つ。
エラー種別ごとに番号をリセットしない。
有効Retry-Afterを持つ429は指定値、
他の再試行HTTPは通常待機と指定値の大きい方を使う。
待機と次の5秒試行が予算に入らなければ終了し、
起床後にも残時間と取消を再確認する。
待機を短縮したりスケジューラ遅延分だけ上限を広げたりしない。

### 5.3 分類

通信エラーはTLS型を先に調べ、timeoutと明示I/O種別を次に調べる。
文字列部分一致を使わない。
HTTPヘッダー取得済みなら401/403/413等の確定した非再試行条件を先に適用し、
後続body取得失敗で再試行可能へ変えない。
429のbodyが取得できずcode不明なら停止する。

| 条件 | category / code | 自動再試行 |
| --- | --- | --- |
| 入力・本文保持・Base64・ローカル上限 | input / 個別固定コード | なし |
| 添付open/read・読取期限 | attachment / attachment_read_failed、attachment_read_timeout | なし |
| 実行枠使用中 | resource / busy | なし |
| ID等の内部準備 | internal / preparation_failed | なし |
| TLS証明書・TLSプロトコル | tls / tls_failed | なし |
| timeout | transport / timeout | 対象 |
| 明確な一時I/O | transport / transient_io | 対象 |
| 未分類connect/DNS/通信 | transport / transport_unclassified | なし |
| 401/403/認証制限 | auth / authentication_failed、permission_denied、auth_rate_limited | なし |
| 413/添付容量・帯域 | limit / payload_or_attachment_limit | なし |
| 既知一時429 | rate_limit / transient_rate_limit | 対象 |
| 既知日次枠・容量 | limit / quota_or_capacity_exhausted | なし |
| 未知429 | rate_limit / rate_limit_unclassified | なし |
| 500/502/503/504 | server / server_error | 対象 |
| その他4xx・3xx・その他5xx | http / http_rejected または unexpected_http_status | なし |
| 成功応答の不正・不一致・過大 | protocol / invalid_publish_response | なし |
| 呼出し取消 | cancelled / cancelled | なし |

再試行対象でも取消・試行数・時間予算・不正Retry-Afterで停止する。

TLS型はrustls::Errorを探索し、
source()に加えio::Error::get_ref()も辿る。
訪問済み参照と探索上限で循環・過深を防ぎ、
探索しきれなければ未知通信として停止する。
一時I/OはConnectionRefused、ConnectionReset、ConnectionAborted、
BrokenPipe、TimedOut、UnexpectedEof、Interrupted、WouldBlockに限定する。
InvalidData、Other、識別不能DNSをis_connectだけで再試行しない。

| HTTP | ntfy code | 分類 |
| --- | --- | --- |
| 429 | 42901、42911 | 一時request制限、一時topic作成制限。再試行対象 |
| 429 | 42904 | サーバ総topic容量。対象外 |
| 429 | 42905、42908 | 日次帯域、日次message枠。対象外 |
| 429 | 42909 | 認証失敗制限。対象外 |
| 429 | 42902、42903、42906、42907、42910 | メール・購読・アカウント・予約・通話等。対象外 |
| 429 | 未知・欠落・不正・不整合 | 一時と断定せず対象外 |
| 413 | 41301 | サイズ・帯域・残容量等の複数原因。対象外 |
| 500/502/503/504 | 任意 | HTTP状態により対象 |

ntfy JSONのhttp/codeが実HTTP状態と整合する場合だけ詳細分類に使う。
不整合codeは返さずntfy_error_code=nullとし、安全な固定説明を使う。
429は未知制限、5xxは要求どおり再試行対象を維持する。
50002のpath、50003のBaseURL不備も独断で対象から除外しない。
サーバmessageの文言から推測しない。

### 5.4 Retry-After

単一ヘッダー値の前後HTTP OWSだけを除き、
ASCII非負整数秒またはHTTP-dateとして解析する。
符号・小数・複数値・空値・不正日付を拒否する。
日付内カンマを値の区切りにしない。
欠落時だけ既定待機を使い、
不正・複数はstop_reason=retry_after_invalidで停止する。
巨大な有効整数はtoo_longとして停止し、小さく丸めない。
overflow・単調時計加算不能でも待機を短縮しない。

日付はヘッダー受領時の壁時計との差を非負で求め、正の端数を切り上げる。
過去は0とし、以降は単調時計の期限へ変換する。
途中の壁時計補正で早期再送しないが、壁時計自体の正確性は保証しない。
HTTP-dateはIMF-fixdateと旧2形式を受理する。
httpdate1.0.3のRFC850二桁年の固定1970〜2069解釈を、
RFC9110の50年超未来なら過去の同じ末尾年とする規則へ補正する。
再試行不可のエラーはヘッダーがあっても再試行しない。

### 5.5 成功と受領集約

HTTP応答bodyは65,536バイトまでとし、Content-Lengthだけに依存しない。
状態・Retry-Afterをbody読取前に保存する。
HTTP200に加え、上限内JSON、event=message、
idがASCII英数字12文字、sequence_id一致を検査する。
本文・Markdown指定・priority・tagsを照合する。
default priority省略は3として解釈する。送信本文の正規化ではない。

添付時はattachment存在、size一致、name一致、有効なHTTPS URLを確認する。
追加download GETは行わず、URLは結果・ログにも出さない。
添付欠落を本文だけの成功としない。
検証失敗はinvalid_publish_response、receipt=unknownで自動再試行なし。
body取得自体のtimeoutは通信分類に従う。

| 全試行の状態 | 集約receipt |
| --- | --- |
| 有効な受領応答がある | confirmed |
| confirmedなし、受領可能性を否定できない試行あり | unknown |
| unknownなし、HTTP明示拒否あり | rejected |
| 全試行未送信と確定、または試行0 | not_sent |

HTTP拒否判定前のtimeout・送信開始後の切断・5xx・不正成功応答は原則unknown。
TLS/接続失敗でも未送信を証明できなければunknownへ倒す。
4xx明示拒否後も過去unknownを集約で保持する。
3xx等も保守的にunknownとする。
unknownの後に別HTTP試行を開始したらduplicate_possible=trueとし戻さない。
unknown一回で終了した場合はfalseでも外部再送が重複し得る。
別呼出しとの重複検出はしない。
HTTP状態とcodeは最終試行のものを返し、最終試行に応答がなければnull。
confirmedはサーバ受領であり端末表示ではない。

## 6. 情報保護と資源境界

通常モードのstdoutはSDK経由のMCPのみ。
診断はstderrへ固定文言と安全な数値・列挙値で出す。
汎用tracing/log subscriber、reqwest verbose、TLS鍵ログを有効にせず、
RUST_LOGで依存の生ログを開放しない。
config、SDK/serde/reqwestエラーやHTTP全体をDisplay/Debugで出さない。
エラー連鎖は分類にのみ使い、不正値・未知フィールド名も反射しない。
topic、資格情報、URLパス、本文、title、添付、filename、pathを通常ログに出さず、
系列IDは結果にのみ返す。
サーバmessage/topic/attachment URLを中継しない。
メモリーの確実なゼロ化・swap排除は保証しない。

HTTPS only、証明書・ホスト名検証有効、redirectなし、
TLS early dataなし、認証なしfallbackなしとする。
Authorizationを固定通知先の各requestへ付け、sensitive指定する。
reqwestのシステム／環境proxy既定挙動は維持する。
proxy失敗時に直結やHTTPへ黙って切り替えない。
資格情報のソース・保存・権限は設定仕様を正とする。

publish項目は本文・title・固定priority/tag・sequence_id・添付のallowlist。
電話・SMS・メール・任意actions・任意ヘッダー・query・外部URL添付を提供しない。
有料publish項目を構築しない。

MCPはLFを除く1行32MiB（33,554,432バイト）まで。
実添付上限とは別であり、JSONエスケープによる増加を考慮した値である。
SDKのread_until蓄積に先行するAsyncRead前段で行バイト数を検査し、
超過時は安全な診断で接続を閉じる。
途中行取消でもカウンターを失わない。
超過行からID/系列IDを抽出できる保証やtool result返却保証は設けない。
JSON-RPC解析・応答はSDKが担当する。
同時実行枠はSDK全体のメモリー絶対上限ではない。
大量要求や遅いstdoutへの耐性は別途検証し、信頼するローカルクライアントを前提とする。

## 7. 依存と対応基準

Rust1.92.0、edition2024を開発・初版検証基準とする。
以下はCargo指定を=付きexact版で固定し、実装時に全体Cargo.lockを生成・管理する。
宣言MSRV最大1.88をプロジェクトの最低保証と読み替えない。
更新は理由・互換性・検証を示して判断する。

| 直接依存 | 版 | 機能・用途 |
| --- | --- | --- |
| rmcp | 3.5.1 | default-features=false、server/macros/transport-io。公式STDIO SDK |
| tokio | 1.53.3 | macros/rt-multi-thread/time/fs/sync/io-util/signal |
| reqwest | 0.13.5 | default-features=false、rustls/json/stream |
| directories | 6.0.0 | XDG/Windows Known Folder |
| clap | 4.6.7 | derive。CLI |
| serde | 1.0.229 | derive。入出力と設定 |
| serde_json | 1.0.154 | JSON |
| anyhow | 1.0.104 | 通常のResult統一 |
| base64 | 0.22.1 | inline復元・RFC2047 |
| getrandom | 0.4.3 | default-features=false。OS乱数 |
| httpdate | 1.0.3 | HTTP-date解析と二桁年補正 |
| rustls | 0.23.46 | default-features=false。TLSエラー型識別 |
| toml | 1.1.8 | default-features=false、std/serde/parse/display。設定のTOML解析・保存 |

schemarsはrmcpから再公開されるため直接追加しない。
MCP HTTP/OAuth用依存を追加しない。
設定・認証ファイルはTOMLとし、toml1.1.8を直接依存に採用する。
公式資料の版表記は1.1.8+spec-1.1.0、宣言MSRVは1.85。
他の依存との統合ビルドは未検証である。
base64はreqwestの推移依存0.23.1との重複を認識したうえで、
実経路で検証した0.22.1を直接依存に採用する。
directories6.0.0はrust-version未宣言であり、
本体でのWindows/Linux両OS検証は未実施である。
rustls直接指定はTLS基盤の置換ではなく型識別用であり、
reqwestと単一の型になることをlockで照合する。
reqwestが使うrustls/aws-lc/OS証明書検証には
C/C++コンパイラー等の準備が必要で、Rustだけのビルドとは扱わない。
本体の統合ビルドは実利用するLinux環境で確認する。
Windows nativeや別CPUでのビルド・実機検証を受入の必須条件としない。
Windowsのパス・設定配置・権限・終了処理等の設計は維持する。

## 8. 根拠、比較、要求対応、引継ぎ

### 8.1 方式の根拠と確認範囲

ntfyソースはv2.29.0の固定commit
`f18d1137c63a26e85b91ce639e7bc72bb9a82736`を参照した。
実ntfy.shの稼働版を特定したものではない。

添付同送の固定テンプレート外枠案は、添付分岐が評価より先にあり、
添付なしも評価後に本文変換されるため不採用。
二段階URL通知は一通知・アップロード要件を変えるため採用しない。
保持できない入力の送信前拒否とRFC2047を採用する。

実経路試験で本文・Markdown・系列ID・priority/tagの
応答とlive取得の一致、28バイト添付download一致、元ファイル保持、
path/directを確認した。
最大1,000個の4バイト文字は4,000バイト、
X-Message6,638バイト・91wordsで通過した。
添付なしの試験では送信応答一致を確認したが、
取得本文の再確認用証跡は保持されていない。

全Unicode、最大title/filenameとの組合せ、2MB・0バイト境界、
認証付き利用、Windows、本体統合、MCP接続、端末表示は未検証。
方式選定と実装・受入完了を区別する。
この未検証一覧は、全条件の実機確認を必須化するものではない。
後続の検証範囲は1章と要求1.1に従う。

### 8.2 要求との対応

| 要求 | 対応 |
| --- | --- |
| 1、1.1 | 1・2章、7章。STDIO、対象・保証範囲 |
| 2.1〜2.3 | 3章。通知情報、本文、固定種別 |
| 2.5 | 4章、5.1。本文と1添付、path/direct、上限、fallbackなし |
| 2.6 | 3.3〜3.4、5.5。ID確定・返却と受領集約 |
| 3.1〜3.2 | 6章、設定仕様。有料不使用、HTTPS・秘匿 |
| 3.3〜3.4 | 設定仕様、CLI仕様。優先順位と起動前失敗 |
| 4.1〜4.3 | 5章、3.4。時間・分類・重複・結果 |
| 5〜6 | 入力契約を実装・利用手順へ周知。認証試行はCLI仕様 |
| 2.2・2.5・6の本文保持条件 | 1章・4.2。添付の有無に応じた送信前拒否 |

本体固有の条件はローカルまたは模擬環境で検証する。
Retry-Afterの全形式・二桁年・不正/複数/巨大値、
5/45秒と4試行境界、TLS型グラフ・未知エラー、状態不整合、
残存読取busyと有限終了、通常ファイル差替え、
32MiB途中行取消・2MB直接入力のJSON解析負荷、
秘密を含む不正入力のログ非露出を検証する。
ASCIIの見かけのencoded-word、日本語、改行を含むヘッダーについて、
一度の符号化とサーバによる一段の復号で元入力へ戻ることを検証する。
実際のtools/listから取得したinputSchemaとoutputSchemaを個別に取り出し、
それぞれ単独で全参照を解決でき、object根と入出力例の適合を確認する。
実サービス・実機確認は1章の実利用経路に限定し、環境版は実施時に記録する。
認証は模擬環境で確認し、実認証アカウントの準備を必須化しない。
Basic/BearerやWindows固有挙動の機能仕様を削るものではなく、
未実施の実サービス認証・他環境確認を成功済みとも扱わない。
利用スキル・設定手順はこの入力契約と制約に対応させる。

### 8.3 一次資料

- [OpenAPI 3.1.2 Schema Object](https://spec.openapis.org/oas/v3.1.2.html#schema-object)
- [MCP tools](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)
- [STDIO](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
- [取消](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation)
- [rmcp3.5.1](https://github.com/modelcontextprotocol/rust-sdk/tree/rmcp-v3.5.1)
- [SDK終了・取消](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.1/crates/rmcp/src/service.rs)
- [SDK入力抽出](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.1/crates/rmcp/src/handler/server/tool.rs)
- [SDK STDIO読取](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.5.1/crates/rmcp/src/transport/async_rw.rs)
- [ntfy publish](https://docs.ntfy.sh/publish/)
- [固定ntfy publish](https://github.com/binwiederhier/ntfy/blob/f18d1137c63a26e85b91ce639e7bc72bb9a82736/server/server.go)
- [固定ntfyヘッダー](https://github.com/binwiederhier/ntfy/blob/f18d1137c63a26e85b91ce639e7bc72bb9a82736/server/util.go)
- [固定ntfy上限](https://github.com/binwiederhier/ntfy/blob/f18d1137c63a26e85b91ce639e7bc72bb9a82736/server/config.go)
- [固定ntfyエラー](https://github.com/binwiederhier/ntfy/blob/f18d1137c63a26e85b91ce639e7bc72bb9a82736/server/errors.go)
- [reqwest0.13.5](https://docs.rs/reqwest/0.13.5/reqwest/struct.ClientBuilder.html)
- [内部retry](https://github.com/seanmonstar/reqwest/blob/v0.13.5/src/retry.rs)
- [Tokio blocking](https://github.com/tokio-rs/tokio/blob/tokio-1.53.3/tokio/src/task/blocking.rs)
- [TLSのI/O格納](https://github.com/rustls/tokio-rustls/blob/v/0.26.4/src/common/mod.rs)
- [Rust1.92 io::Error](https://github.com/rust-lang/rust/blob/1.92.0/library/std/src/io/error.rs)
- [RFC9110 Retry-After](https://www.rfc-editor.org/rfc/rfc9110.html#section-10.2.3)
- [RFC9110 HTTP-date](https://www.rfc-editor.org/rfc/rfc9110.html#section-5.6.7)
- [httpdate1.0.3](https://docs.rs/httpdate/1.0.3/httpdate/)
- [getrandom0.4.3](https://docs.rs/getrandom/0.4.3/getrandom/)
- [rustls0.23.46](https://github.com/rustls/rustls/tree/v/0.23.46)
- [toml1.1.8+spec-1.1.0](https://docs.rs/toml/1.1.8%2Bspec-1.1.0/toml/)

## 9. OpenAPI 3.1.2データ型定義

以下はMCP argumentsとstructuredContentのデータ契約を表す。
HTTP APIを追加するものではなく、pathsは空である。
OpenAPI 3.1.2のSchema Objectを用い、JSONで記述する。
OpenAPI文書は契約の記述用であり、components内の型をそのまま切り出して
MCPのinputSchema/outputSchemaへ設定しない。
公開するSchemaはJSON Schema 2020-12とし、根に
$schema=https://json-schema.org/draft/2020-12/schemaとtype:objectを持たせる。
NotifyInputに対応するinputSchema、NotifyResultに対応するoutputSchemaへ、
推移的に必要な参照型も含めてそれぞれの$defsへコピーし、全ての内部参照を
#/components/schemas/型名から#/$defs/型名へ変換する。
変換後は各公開Schemaだけで参照を解決できる自己完結した構成とする。
SDKから生成する場合も同じ条件を満たし、OpenAPI全体の存在を前提としない。
UTF-8バイト数、Goの空白判定、ファイル読取・本文保持、
系列ID生成、符号化後ヘッダー、予算と取消は実行時検証であり、
Schemaのdescriptionとx-runtime-validationに区別して記す。
文字列のmaxLengthはUnicode文字数の制約でありUTF-8バイト上限ではない。

```json
{
  "openapi": "3.1.2",
  "info": {
    "title": "ntfy-mcp notify data contracts",
    "version": "0.1.0",
    "description": "STDIO MCPのデータ型を記述する。HTTP endpointを定義しない。"
  },
  "paths": {},
  "components": {
    "schemas": {
      "SequenceId": {
        "type": "string",
        "description": "ASCII英数字・ハイフン・アンダースコアの系列ID。",
        "minLength": 1,
        "maxLength": 64,
        "pattern": "^[A-Za-z0-9_-]{1,64}(?![\\s\\S])"
      },
      "MessageId": {
        "type": "string",
        "description": "検証済みntfyメッセージID。",
        "pattern": "^[A-Za-z0-9]{12}(?![\\s\\S])",
        "minLength": 12,
        "maxLength": 12
      },
      "Filename": {
        "type": "string",
        "description": "表示名。UTF-8で255バイト以下。空白・制御文字・前後trim・パス区切りの検査は実行時にも行う。",
        "minLength": 1,
        "maxLength": 255,
        "not": {
          "enum": [
            ".",
            ".."
          ]
        },
        "pattern": "^[^/\\\\\\r\\n]+(?![\\s\\S])",
        "x-runtime-validation": [
          "UTF-8 bytes <= 255",
          "no control characters",
          "unchanged by trim"
        ]
      },
      "AttachmentPath": {
        "type": "object",
        "properties": {
          "source": {
            "type": "string",
            "const": "path"
          },
          "path": {
            "type": "string",
            "description": "起動時作業ディレクトリ基準。UTF-8で4096バイト以下。通常ファイル・読取上限・論理期限を実行時に検査する。",
            "minLength": 1,
            "maxLength": 4096,
            "x-runtime-validation": [
              "UTF-8 bytes <= 4096",
              "regular file",
              "read at most 2000001 bytes",
              "logical read deadline"
            ]
          },
          "filename": {
            "$ref": "#/components/schemas/Filename"
          }
        },
        "required": [
          "source",
          "path"
        ],
        "additionalProperties": false
      },
      "AttachmentUtf8": {
        "type": "object",
        "properties": {
          "source": {
            "type": "string",
            "const": "inline"
          },
          "encoding": {
            "type": "string",
            "const": "utf8"
          },
          "data": {
            "type": "string",
            "description": "文字列のUTF-8実バイトが2000000以下であることをコピー前に検査する。空データも受理する。",
            "maxLength": 2000000,
            "x-runtime-validation": [
              "UTF-8 bytes <= 2000000 before copy"
            ]
          },
          "filename": {
            "$ref": "#/components/schemas/Filename"
          }
        },
        "required": [
          "source",
          "encoding",
          "data",
          "filename"
        ],
        "additionalProperties": false
      },
      "AttachmentBase64": {
        "type": "object",
        "properties": {
          "source": {
            "type": "string",
            "const": "inline"
          },
          "encoding": {
            "type": "string",
            "const": "base64"
          },
          "data": {
            "type": "string",
            "description": "標準Base64・正規padding・改行なし。paddingを含む正確な復号長と未使用bitを検査し、確保前と復号後に2000000バイト上限を確認する。",
            "maxLength": 2666668,
            "pattern": "^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?(?![\\s\\S])",
            "x-runtime-validation": [
              "canonical Base64 padding and unused bits",
              "decoded bytes <= 2000000 before allocation",
              "decoded bytes <= 2000000 after decoding"
            ]
          },
          "filename": {
            "$ref": "#/components/schemas/Filename"
          }
        },
        "required": [
          "source",
          "encoding",
          "data",
          "filename"
        ],
        "additionalProperties": false
      },
      "Attachment": {
        "oneOf": [
          {
            "$ref": "#/components/schemas/AttachmentPath"
          },
          {
            "$ref": "#/components/schemas/AttachmentUtf8"
          },
          {
            "$ref": "#/components/schemas/AttachmentBase64"
          }
        ]
      },
      "NotifyInput": {
        "type": "object",
        "properties": {
          "kind": {
            "type": "string",
            "enum": [
              "progress",
              "completed",
              "question",
              "alert"
            ]
          },
          "message": {
            "type": "string",
            "description": "Markdown元本文。Unicodeスカラー数とUTF-8バイト数を検査する。空白のみ・trimで変化する本文を拒否し、添付時は文字としてのバックスラッシュ+nも拒否する。無断変換しない。",
            "minLength": 1,
            "maxLength": 1000,
            "x-runtime-validation": [
              "Unicode scalar count <= 1000",
              "UTF-8 bytes <= 4096",
              "not whitespace-only",
              "unchanged by ntfy TrimSpace",
              "with attachment: no literal backslash followed by n"
            ]
          },
          "title": {
            "type": "string",
            "description": "省略時は送らない。UTF-8で1024バイト以下。前後空白・改行・制御文字を拒否する。",
            "minLength": 1,
            "maxLength": 1024,
            "x-runtime-validation": [
              "UTF-8 bytes <= 1024",
              "unchanged by trim",
              "no newline or control characters"
            ]
          },
          "sequence_id": {
            "$ref": "#/components/schemas/SequenceId"
          },
          "attachment": {
            "$ref": "#/components/schemas/Attachment"
          }
        },
        "required": [
          "kind",
          "message"
        ],
        "additionalProperties": false,
        "description": "任意項目は省略できるがnullは不可。Schemaが満たされても実行時検証の合格を意味しない。",
        "x-runtime-validation": [
          "one notification with at most one attachment",
          "metadata and encoded headers validated before HTTP",
          "priority and tag derived only from kind"
        ]
      },
      "NotifyError": {
        "type": "object",
        "properties": {
          "category": {
            "type": "string",
            "enum": [
              "input",
              "attachment",
              "resource",
              "internal",
              "tls",
              "transport",
              "auth",
              "limit",
              "rate_limit",
              "server",
              "http",
              "protocol",
              "cancelled"
            ]
          },
          "code": {
            "type": "string",
            "description": "分類表に対応する固定機械判別コード。",
            "minLength": 1
          },
          "message": {
            "type": "string",
            "description": "入力値や生のサーバ応答を含めない安全な固定テンプレート。",
            "minLength": 1
          },
          "field": {
            "type": [
              "string",
              "null"
            ],
            "description": "該当入力フィールド名。該当なしはnull。入力値を反射しない。"
          },
          "stop_reason": {
            "type": "string",
            "enum": [
              "non_retryable",
              "attempts_exhausted",
              "budget_exhausted",
              "retry_after_invalid",
              "cancelled"
            ]
          }
        },
        "required": [
          "category",
          "code",
          "message",
          "field",
          "stop_reason"
        ],
        "additionalProperties": false
      },
      "NotifyWait": {
        "type": "object",
        "properties": {
          "source": {
            "type": "string",
            "enum": [
              "retry_after_seconds",
              "retry_after_date",
              "rate_limit_default",
              "exponential",
              "invalid"
            ]
          },
          "delay_seconds": {
            "type": [
              "integer",
              "null"
            ],
            "minimum": 0,
            "description": "最後の選択待機秒。巨大値・不正ではnull。"
          },
          "waited_ms": {
            "type": "integer",
            "minimum": 0,
            "description": "実際の再試行待機合計ミリ秒。"
          },
          "remaining_seconds": {
            "type": [
              "integer",
              "null"
            ],
            "minimum": 0,
            "description": "最後の有効待機期限までの切上げ秒。終了済み0、算出不能null。"
          },
          "retry_after_state": {
            "type": "string",
            "enum": [
              "missing",
              "valid",
              "invalid",
              "too_long"
            ]
          }
        },
        "required": [
          "source",
          "delay_seconds",
          "waited_ms",
          "remaining_seconds",
          "retry_after_state"
        ],
        "additionalProperties": false,
        "description": "自動再試行の待機判断。手動再送の許可ではない。待機・期限の整合は実行時に計算する。",
        "allOf": [
          {
            "if": {
              "properties": {
                "source": {
                  "const": "invalid"
                }
              },
              "required": [
                "source"
              ]
            },
            "then": {
              "properties": {
                "delay_seconds": {
                  "type": "null"
                },
                "retry_after_state": {
                  "const": "invalid"
                }
              }
            }
          },
          {
            "if": {
              "properties": {
                "retry_after_state": {
                  "const": "too_long"
                }
              },
              "required": [
                "retry_after_state"
              ]
            },
            "then": {
              "properties": {
                "delay_seconds": {
                  "type": "null"
                }
              }
            }
          }
        ]
      },
      "NotifySuccess": {
        "type": "object",
        "properties": {
          "ok": {
            "type": "boolean",
            "const": true
          },
          "sequence_id": {
            "$ref": "#/components/schemas/SequenceId"
          },
          "receipt": {
            "type": "string",
            "const": "confirmed"
          },
          "attempts": {
            "type": "integer",
            "minimum": 1,
            "maximum": 4
          },
          "duplicate_possible": {
            "type": "boolean"
          },
          "message_id": {
            "$ref": "#/components/schemas/MessageId"
          },
          "http_status": {
            "type": "integer",
            "const": 200
          },
          "ntfy_error_code": {
            "type": "null"
          },
          "error": {
            "type": "null"
          },
          "wait": {
            "anyOf": [
              {
                "$ref": "#/components/schemas/NotifyWait"
              },
              {
                "type": "null"
              }
            ]
          }
        },
        "required": [
          "ok",
          "sequence_id",
          "receipt",
          "attempts",
          "duplicate_possible",
          "message_id",
          "http_status",
          "ntfy_error_code",
          "error",
          "wait"
        ],
        "additionalProperties": false,
        "allOf": [
          {
            "if": {
              "properties": {
                "duplicate_possible": {
                  "const": true
                }
              },
              "required": [
                "duplicate_possible"
              ]
            },
            "then": {
              "properties": {
                "attempts": {
                  "minimum": 2
                }
              }
            }
          }
        ],
        "description": "有効な受領応答を検証した成功。端末表示成功を意味しない。"
      },
      "NotifyFailure": {
        "type": "object",
        "properties": {
          "ok": {
            "type": "boolean",
            "const": false
          },
          "sequence_id": {
            "anyOf": [
              {
                "$ref": "#/components/schemas/SequenceId"
              },
              {
                "type": "null"
              }
            ]
          },
          "receipt": {
            "type": "string",
            "enum": [
              "not_sent",
              "unknown",
              "rejected"
            ]
          },
          "attempts": {
            "type": "integer",
            "minimum": 0,
            "maximum": 4
          },
          "duplicate_possible": {
            "type": "boolean"
          },
          "message_id": {
            "type": "null"
          },
          "http_status": {
            "type": [
              "integer",
              "null"
            ],
            "minimum": 100,
            "maximum": 999
          },
          "ntfy_error_code": {
            "type": [
              "integer",
              "null"
            ],
            "minimum": 0
          },
          "error": {
            "$ref": "#/components/schemas/NotifyError"
          },
          "wait": {
            "anyOf": [
              {
                "$ref": "#/components/schemas/NotifyWait"
              },
              {
                "type": "null"
              }
            ]
          }
        },
        "required": [
          "ok",
          "sequence_id",
          "receipt",
          "attempts",
          "duplicate_possible",
          "message_id",
          "http_status",
          "ntfy_error_code",
          "error",
          "wait"
        ],
        "additionalProperties": false,
        "allOf": [
          {
            "if": {
              "properties": {
                "duplicate_possible": {
                  "const": true
                }
              },
              "required": [
                "duplicate_possible"
              ]
            },
            "then": {
              "properties": {
                "attempts": {
                  "minimum": 2
                }
              }
            }
          },
          {
            "if": {
              "properties": {
                "attempts": {
                  "const": 0
                }
              },
              "required": [
                "attempts"
              ]
            },
            "then": {
              "properties": {
                "receipt": {
                  "const": "not_sent"
                },
                "duplicate_possible": {
                  "const": false
                },
                "http_status": {
                  "type": "null"
                },
                "ntfy_error_code": {
                  "type": "null"
                }
              }
            }
          }
        ],
        "description": "失敗結果。errorを必須とし、送信前失敗はattempts=0。系列ID未確定時だけsequence_id=null。"
      },
      "NotifyResult": {
        "type": "object",
        "oneOf": [
          {
            "$ref": "#/components/schemas/NotifySuccess"
          },
          {
            "$ref": "#/components/schemas/NotifyFailure"
          }
        ],
        "description": "structuredContentに返し、同じJSONをTextContentへ格納する。isErrorはokの反転。",
        "x-runtime-validation": [
          "MCP isError == !ok",
          "receipt aggregated across attempts",
          "duplicate flag sticky once replay after unknown starts",
          "last attempt status retained",
          "HTTP attempt <= 5 seconds; total <= 45 seconds; maximum 4 attempts"
        ]
      }
    }
  }
}
```


