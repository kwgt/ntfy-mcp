# 開発中の検証方法

Rust 1.92.0とリポジトリの`Cargo.lock`を使用します。依存版は
`Cargo.toml`のexact指定を維持してください。
試験専用の`jsonschema = 0.59.0`もexact指定し、default featuresを無効に
しています。HTTP/fileの外部解決を有効にせず、既知の組込metaschemaと
各Schema内の局所参照だけでJSON Schema 2020-12を検証します。

## 通常の確認

リポジトリのルートで実行します。

```sh
cargo check --locked
cargo test --locked
cargo build --locked
```

通常のテストは外部通知・実認証・ローカルHTTP接続を行いません。
送信はメモリー内の`Transport`実装に置換します。設定と認証の入力は
明示fixtureまたは環境スナップショットを渡し、CLIバイナリ試験では
子プロセスの環境を消去して隔離した設定ファイルを指定します。
既存のユーザ設定やNTFY環境変数を試験の前提にしません。

領域別に確認する場合は、次のコマンドを使用できます。

```sh
cargo test --locked --lib cmd_args::config
cargo test --locked --test notify
cargo test --locked --test http
cargo test --locked --test mcp
cargo test --locked --test app
cargo test --locked --test cli
cargo test --locked --test acceptance
```

`cmd_args::config`の単体試験は優先順位、認証一式の選択、TOML、
サイズ・期限と保存を確認します。`notify`は入力、ID、ヘッダー、添付を
確認します。残存読取workerの試験はMCPライブラリ単体へ移し、同じServiceの
期限後HTTP0、busy、遅れた旧結果からの送信0とworker解放後の復旧を確認します。
`http`は送信内容、応答、再試行、Retry-After、取消、受領状態と認証確認を
模擬応答で確認します。`app`は停止後の有限待機と出力失敗を、`cli`は
実バイナリのhelp/version、不備、保存、認証なし確認と通常モードの
起動・終了状態を確認します。保存workerの失敗・取消はライブラリ内の
単体試験にも含みます。設定ファイル内部のDTOと読取・統合・保存処理は
非公開の`cmd_args::config`に置き、内部単体試験から確認します。
CLI解析は`cmd_args::parse_from`、設定読込・統合は
`Options::prepare_with`にfixtureを注入し、本体と同じ設定準備を通します。
本体の`cmd_args::parse`はCLIだけを先に解析し、`app::run`が生成する
明示runtime内で`Options::prepare`が実環境から設定を準備します。

`mcp`は同じSDK受付と送信mockで、同系列の通知、path/inline添付、
入力・読取・送信失敗、busy、取消後の継続を確認します。SDKのメモリー内
transportでもServerHandlerの受付を通します。結果変換のTextと
structuredContentの一致、同梱Schemaのobject根と局所参照の基本確認は
ライブラリ単体試験に含みます。Schemaは基本設計9章のNotifyInputと
NotifyResultから、必要な参照型を各$defsへ展開した静的JSONです。
大きいBase64直接添付は行長前段とメモリー内SDKを通し、応答を待ちます。
復元後2,000,000バイトはHTTP境界へ到達し、上限超過はHTTP0となることを
確認します。送信境界は固定403のmockです。伝送行の32MiB上限と
添付実サイズ上限、HTTP応答65,536バイト上限は別の境界です。

`cli`の`actual_tools_list_and_invalid_notifications_keep_stdio_alive`は
実バイナリを隔離設定・消去環境で起動し、initialize→tools/list→不正notify→
同じ接続の継続→EOFを通します。子終了後もstdout readerの完了まで待ち、
全てのstdout行をJSON-RPCとして検査します。読取・UTF-8エラーも捨てません。
不正notifyはHTTP試行0と安全なtool errorだけを返すことを確認し、正常通知の
実HTTPは呼びません。成功・系列・添付の送信は既存のSDK/mock試験で扱います。

実tools/listから得たinputSchema/outputSchemaを別々の根で取り出し、object根、
2020-12宣言、外部ref不在、各$defs内の全参照解決を確認します。その実取得値で
`jsonschema::draft202012`のvalidatorを各々作り、`tests/fixtures`の
`mcp_input_examples.json`と`mcp_output_examples.json`を照合します。各caseの
`valid`はSchema適合の期待値で、実行時のファイル存在や本文保持条件とは別です。
入力4枝・成功/失敗/受領不明/待機付き結果の正例と、必須・未知・null・混在・
const・型の負例を使います。実不正notifyの出力も取得したoutputSchemaへ
適合することを確認します。失敗診断はcase名/indexだけで、validatorの生エラーを
出力しません。

待機・再試行の試験は必要に応じTokioの仮想時間を使用します。ファイル
入力と保存は試験自身が所有する一時fixtureを使用し、終了時に削除します。
入力、期待値、mockと再実行手順はテストコードを正とし、一時結果ファイルを
継続参照の根拠にしません。実行結果・失敗・未確認事項は作業記録で管理します。

## E1〜E3の共有ケースと実サービス用の明示実行

`tests/common/acceptance.rs`が本文・種別・系列ID・添付と期待値を共有します。
`tests/fixtures/acceptance.txt`の同じ小さいUTF-8データをpathと直接指定に使い、
試験が所有する一時ディレクトリへコピーします。設定源も同じ場所の不存在file、
環境スナップショットは空として個人設定を読みません。実行後は自身のfixtureを
片付け、元のリポジトリfixtureは変更しません。

| ケース | 入力と共有期待値 |
| --- | --- |
| E1 | 日本語・内部改行・Markdown本文のみ。progress、系列IDは省略して生成 |
| E2 | 同じ本文と小さいpath添付。progress、E1返却IDを明示して再利用 |
| E3 | 同じ本文とUTF-8直接添付。completed、E1返却IDを再利用 |

通常の`shared_cases_keep_series_payload_and_original`は既存mockの3成功だけで
同じServiceを通し、本文/添付/名前/原本不変とID引継ぎを確認します。実clientと
比較GETは生成しません。通常の`cargo test --locked`では次の実サービス試験を
ignoreし、環境変数が存在するだけでは送信しません。

`selected_case`は後続の実施指示時だけ、一回に一つのケースを選択します。
OP2では以下のコマンドを実行しません。ignored試験はコンパイル確認のみです。

| 環境変数 | 用途 |
| --- | --- |
| NTFY_MCP_ACCEPTANCE_CASE | E1/E2/E3の一つ。未知値・未指定は開始しない |
| NTFY_MCP_ACCEPTANCE_SERVER_URL | HTTPS base URLを実行環境から注入 |
| NTFY_MCP_ACCEPTANCE_TOPIC | 認証なしの試験用topicを実行環境から注入 |
| NTFY_MCP_ACCEPTANCE_SEQUENCE_ID | E2/E3で必須。E1のnotify.sequence_idを注入 |

URL/topic等は事前に環境へ設定し、値をコマンドやログへ展開表示しません。
環境変数名はこの試験専用で、製品の設定項目を追加するものではありません。
通常のNTFY_MCP認証/設定環境値を引き継がず、個人configも読みません。

後続の許可された実施時に一ケースだけ実行する形式は次のとおりです。
E2/E3ではCASE値だけを変え、E1で返ったIDを上記の環境変数へ渡します。

```sh
NTFY_MCP_ACCEPTANCE_CASE=E1 \
cargo test --locked --test acceptance -- \
  --ignored --exact selected_case --nocapture
```

1回の実行は1論理callです。内蔵retry以外の再送を追加せず、通常3call/3publish、
内蔵retry込みで全3ケース最大12publish（各call最大4試行）とします。
コアが受領成功を確認したE2/E3だけ添付比較GETを各1回、合計最大2回行います。
GETはHTTPS/資格情報なし・redirectなし・retryなし、body受領まで5秒、保持する
bytesは小さい共有fixtureの長さまでです。過大bodyも一致しないと判定して止めます。
URLは最終応答から試験内メモリーだけへ保持し、製品結果やログへ追加しません。
account GET、自動poll、次ケースの自動開始は行いません。

出力するJSONはケースID、`notify`の安全な結果、`attachment_comparison`、
`attachment_get_attempts`、`original_file_unchanged`です。本文/添付/URL/topic/
資格情報やreqwest生エラーは出しません。受領不明・失敗・比較不一致/失敗・原本
確認不能は安全な結果を出して試験失敗とし、次ケースや再送へ進みません。
受領成功と両端末到達は別の確認で、比較成功も受信アプリの表示確認を代用しません。

実施時の既存の記録・停止・再開手順を維持します。

- 開始前に、同じ作業記録のE1〜E3一覧へ実行経路と「開始中」を残します。
  Codex実操作かrunnerかを記録し、同じケースを別経路で二重送信しません。
- 終了後、試行数・受領結果・両受信先・添付比較結果を同じ一覧へ記録します。
  生の本文/添付/応答や秘密値を記録せず、別のローカル台帳を作りません。
- 受領不明・想定外失敗・比較失敗なら次のケースを止めます。開始中のまま
  中断したケースは自動再送せず、試行数不明なら当該call最大4試行消費済みと
  扱い、過去結果・未開始ケースと既存実施指示を確認してから判断します。
- 比較だけの失敗で同じpublishをやり直したり、追加GETで補ったりしません。
  再実行可能なコードであることを再送許可とせず、過去と再実行の結果を分けます。

このrunnerはServiceの実サービス用経路です。Linux/CodexのSTDIO導入・実操作、
Windows11ブラウザ/Android受信は後続の受入で確認し、runnerだけで代用しません。

## 現段階の境界

設定が有効な通常モードは、単一ServiceとMCP受付を生成しSDK STDIOを開始
します。request取消とサーバ停止は別々の取消通知として同じhandler内で
コアへ接続します。起動時cwdは設定と通知で共通です。実STDIO子プロセスで
初期化後EOF、初期化不正、stdout失敗を確認し、LinuxではSDK初期化中・
開始後のSIGINT/SIGTERMも確認します。この試験は通知HTTPを開始しません。
準備中停止と単発モード停止は1、正常EOFと処理した通常signalは0、
検出したSDK/IO異常とSDK cleanup期限超過は1です。
SDKのClosedだけで予期しない切断を完全には識別しません。SDK内部で処理して
継続する構文・外枠不備の履歴も独自解析せず、アプリへ返された初期化異常と
IO観測で検出した失敗を終了状態へ反映します。

静的Schemaと内部入力・通知準備・結果型の契約項目、代表例を明示照合
します。文字数とUTF-8バイト数など、Schema条件と実行時条件を分けて確認
します。行長前段はLFを除く32MiBを
SDKの蓄積前に検査し、途中行のreadfuture取消後も行位置を保持します。
実子プロセスへ32MiB超の未完行を送り、stdinを保持したまま固定診断・終了1を
確認します。大きい書込も子の終了deadlineと並行に行い、試験を有限にします。
JSON-RPCはSDKが解釈し、超過行からのID抽出やtool resultは保証しません。
行上限はSDK全体のメモリー絶対上限ではありません。実バイナリの`tools/list`
から取得した各Schemaの独立検証と、上記STDIOフローを通常試験に含めます。

`--check-auth`のBasic/Bearer実行は製品機能として実サーバへGETします。
通常テストではその実HTTP導線を実行せず、送信境界を置換します。
認証なしの確認と設定保存はHTTPクライアントを生成しません。

実サービス通知、Linux上のCodexとの接続、Windows 11ブラウザとAndroidでの
受信確認は未実施です。上記の実サービス用ignored導線はコンパイル確認だけで、
実publish/比較GETはまだ実行していません。後続の明示的な実施指示と送信枠に
従ってください。認証・境界・失敗の
通常確認はlocal/mockを使用し、実認証アカウントの準備を必須にしません。

Windows上の実行、他のCPU・OS・MCPクライアント・受信アプリの版網羅は
この検証結果に含めません。実施した環境と版は実施時に記録します。
配布用cross-buildとGitHubリリースは別作業です。
本体固有の残る総合検証はMS2.3、利用文書や実利用経路の確認は後続計画で
扱います。CP2.2の承認・クローズや次の作業開始を試験成功から推定しません。

停止時は処理へ取消を通知し最大1秒待ち、そのfutureを解放した後、runtimeの
残存workerを最大1秒待ちます。物理I/Oの即時中止や保存rename後の巻戻しは
保証しません。SDK cleanupの1秒超過は仮想futureで確認します。実OS signalは
試験が所有するLinux子プロセスで確認し、終了しない実workerや詰まった
実pipeを使う負荷試験は行っていません。出力のないstdoutでmainが再度lockを
取得しない導線も維持します。
