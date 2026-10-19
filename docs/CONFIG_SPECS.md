# 設定ファイル仕様

## 1. 範囲と参照

本書を設定の形式・キー・優先順位・認証ソース・保存・OS権限の正とする。
CLI操作は[CLI仕様](CLI_SPECS.md)、通知入力・HTTP・ログ・依存は
[基本設計](BASE_DESIGN.md)に従う。要求の原本は `docs/REQUIREMENTS.md`。

設定はTOMLとし、通常起動時に一度読み、不変の有効設定を作る。
認証専用ファイルもTOMLに統一する。
既定ファイルはconfig.tomlとし、旧JSON形式の自動検出・変換は行わない。
自動再読込・複数profile・includeは提供しない。
インスタンスごとに--configを指定し、固定通知先を分ける。
--save-configで統合設定を保存する場合は本書8章に従う。

## 2. ファイル形式と読込

BOMなしUTF-8のTOML documentとする。TOMLのコメントと改行を許可する。
重複キー・重複table・不正構文はパーサで拒否し、
未知キー・型不一致はアプリの構造検査で拒否する。
設定キーの値は文字列とtableに限定し、配列・日時等を許可しない。
TOMLはnullを持たない。配布例・生成ファイルはLFを用い、読込はCRLFも許可する。

設定と選択された認証ファイルはそれぞれ最大65,536バイト。
最大+1バイトまでの読取で判定し、metadataサイズだけに依存しない。
通常ファイルを対象とし、読込では通常ファイルへのsymlinkを許可する。
両ファイルのmetadata/open/readを合計5秒の論理期限で管理する。
同期I/Oの即時停止は保証せず、失敗時はMCP・保存・認証確認へ進まない。
残存blocking処理と有限終了は基本設計に従う。

通常MCP/--check-authでは、明示--configの不存在・読込失敗・解析失敗はエラー。
既定ファイル不存在だけはファイル値なしで継続する。
--save-configでは明示・既定いずれの保存先不存在も新規作成として扱うが、
存在するファイルの読込失敗・不正TOMLは無視しない。
--configと既定ファイルをマージしない。

--config/--auth-fileの相対pathは起動時作業ディレクトリ基準。
アプリ側でチルダ・環境変数・globを展開しない。
PathBufで扱い、パスを診断へ転記しない。
保存モードでは保存先symlinkを拒否する。通常読込のsymlink許可と区別する。

## 3. 既定の配置

`directories::ProjectDirs::from("", "", "ntfy-mcp").config_dir()`
配下のconfig.tomlを使い、独自の環境変数連結で再実装しない。

| OS | 既定パス |
| --- | --- |
| Linux | 絶対パスXDG_CONFIG_HOME配下ntfy-mcp/config.toml。それ以外はHOME配下.config/ntfy-mcp/config.toml |
| Windows | RoamingAppData Known Folder配下ntfy-mcp/config/config.toml。通常は%APPDATA%配下 |
| 共通 | --config PATH指定時はその一つだけ |

既定場所を取得できず--configもない場合はconfig_path_unavailableとして失敗する。

## 4. 設定キーと検証段階

| キー | 型 | 省略時 | 意味 |
| --- | --- | --- | --- |
| server_url | string | https://ntfy.sh/ | HTTPS base URL |
| topic | string | なし | 固定topic。通常MCP・認証確認の有効設定で必須 |
| auth | table | mode:none | 認証一式 |

server_urlはHTTPS・host必須、userinfo/query/fragment禁止。
base pathを保ち、末尾をディレクトリとして扱ってtopicを1セグメント追加する。
topicはASCII英数字・ハイフン・アンダースコアの1〜64文字。
URL/topicを自動trimせず、選択された空・不正値は失敗とする。
予約・禁止topic、サーバ権限やAPI配置まで起動時に包括確認しない。

ファイル内の全既知キーの型・未知キーは、上位CLIがあっても検査する。
必須値と認証方式の組合せの意味検証は、統合後の選択された値について行う。
ファイルだけでtopicがなくても、CLI/envで補える。
--save-configだけは、未指定topicを省略したひな形として保存できる。
明示の不正値はひな形化によって隠さない。

## 5. 認証一式

有効な認証設定と認証専用ファイルは次のいずれか。
選択されたauth tableではmodeを必須とし、自動推測しない。

| mode | 必須 | 不可 |
| --- | --- | --- |
| none | mode | username、password、token |
| basic | mode、username、password | token |
| bearer | mode、token | username、password |

Basicのusername/passwordは空でないUTF-8文字列。
usernameのコロン・制御文字を拒否する。passwordはtrimせず符号化へ渡す。
Bearer tokenは空でないASCIIのBearer token形式とし、空白・制御文字を拒否する。
Authorizationを構築できない値は実行前エラー。
noneはtopic隠蔽による利用であり、厳密な認証と区別する。

--auth-fileは、この認証一式をトップレベルに記述したTOMLを読む。
設定authと同じ型を使い、modeや資格情報を欠いたファイルを下位で補わない。

## 6. 優先順位

| 有効項目 | CLI | 環境変数 | 設定ファイル | 最後の既定値 |
| --- | --- | --- | --- | --- |
| server_url | --server-url | NTFY_MCP_SERVER_URL | server_url | https://ntfy.sh/ |
| topic | --topic | NTFY_MCP_TOPIC | topic | なし |
| 認証一式 | --user / --auth-token / --auth-fileの一つ | 下記の変数一式 | auth | mode:none |

--userはBasic、--auth-tokenはBearer、--auth-fileは指定ファイルのmodeを使う。
この三つは同時指定不可。同じ優先順位の認証ソースを複数受け付けない。
認証一式は丸ごと置換し、username/password/tokenを別ソースから混合しない。
noneの認証ファイルで下位の資格情報を使わない選択もできる。
設定パスは--configだけで選び、NTFY_MCP_CONFIGは設けない。

| 認証変数 | 対応 |
| --- | --- |
| NTFY_MCP_AUTH_MODE | mode |
| NTFY_MCP_USERNAME | Basic username |
| NTFY_MCP_PASSWORD | Basic password |
| NTFY_MCP_TOKEN | Bearer token |

認証変数が一つでもあれば一式を上位ソースとして選択し、AUTH_MODEを必須とする。
basicはUSERNAME/PASSWORD、bearerはTOKEN、noneはAUTH_MODEだけを許可する。
不完全・矛盾・空の上位ソースからfallbackせず、未選択資格情報を統合しない。
選択されたアプリ環境変数がUnicode文字列で表せなければ失敗する。
XDGやproxy等の環境変数は依存ライブラリ側の環境条件として別に扱う。

## 7. 資格情報の保存とOS権限

資格情報は設定auth・専用認証TOML・環境変数・CLIで指定できる。
OS keychain・暗号化保管・対話入力は提供しない。
--save-configは統合結果の認証実値を保存する。これ以外の通常実行で自動保存しない。
CLIの資格情報はログへ反射せず、平文TOMLへの意図した保存だけを認める。

| OS | 保護 |
| --- | --- |
| Linux | 秘密を含むファイルは実行ユーザー所有・0600、親0700。POSIX ACL追加許可とsymlink先も利用者が確認 |
| Windows | NTFS DACLで実行ユーザーと必要な管理主体に限定。広いUsers/Everyoneや継承を確認し、read-only属性を代用しない |

本体は既存の所有者・ACL・DACLの安全性を包括的に自動判定・修復しない。
保存時はLinuxの新規ファイル0600・新規ディレクトリ0700を指定する。
Windowsの新規ファイル・ディレクトリは親のDACLを継承するため、
保護したユーザー用ディレクトリで使用する。
既存の親ディレクトリの権限を本体が変更することはない。

Windows既定配置はRoaming配下。同期を避けたい場合は--config/--auth-fileで
ローカルの保護場所を選ぶ。--save-configは認証ファイルの内容を設定へ展開して保存するので、
保存先も同じ水準で保護する。環境変数も専用の秘密保管庫とは扱わない。
不要なコピー・通常ログ・設定一覧への露出を避ける。

## 8. --save-configの保存契約

1. 保存先は選択された--config、または既定config.tomlとする。
2. 存在する対象はsymlink・特殊ファイルを拒否し、通常ファイルとして上限内で読み込む。
   不存在は空ソースとする。認証ファイルは指定があれば必ず正常に読み込む。
3. CLI＞環境変数＞読込値＞既定値を統合し、選択された値を検証する。
   topic未指定だけは省略でき、固定の補完用コメントを付ける。
4. 保存するキーはserver_url・指定済みtopic・authだけ。
   authは選択された一式を実値で出力する。
   既存のコメント・整形・キー順序は保存せず、TOML serializerを使用する。
5. シリアライズ後も65,536バイト以下とし、超過なら保存前に失敗する。
   入力値をコメントやTOMLコードへ直接連結しない。
6. 保存先の親を必要に応じて作成し、同じディレクトリに推測困難な名前の一時ファイルを
   create_newで作る。Linuxでは作成時0600とし、既存ファイルを直接truncateしない。
7. 全内容の書込・flush・sync_allに成功した後、renameによる置換を行う。
   既存の通常ファイルは更新対象。置換前の失敗では旧ファイルを維持し、一時ファイルを可能な範囲で削除する。
   置換不能時に削除→書直しへ切り替えない。
8. 成功時だけ完了を返す。認証情報・topic・保存先・本文をstdout/stderrへ表示しない。

保存処理は起動読取の5秒や通知の45秒とは別のローカルI/Oであり、
同期書込の即時停止やOS強制終了・電源断時の復旧までは保証しない。
保存中も停止を監視し、置換前の遅延したblocking結果から勝手に置換を続けない。
書込・同期を行うworkerは置換を担当せず、制御側が結果と取消を確認してから置換を開始する。
置換の開始をコミット点とし、それ以降に届く停止で保存を取り消せる保証は設けない。
一時ファイルの片付けは可能な範囲で行う。
機密データの一時ファイル名をログへ出さない。
保護されていない親の差替えや他プロセスとの同時更新を完全には防がず、
同一設定ファイルへの並列保存を利用者側で避ける。

明示--configが存在しない場合の作成は保存モードのための例外。
通常MCP/認証確認の明示ファイル不存在エラーは維持する。
旧JSONからの自動移行や、指定していない別ファイルの検索・読込は行わない。

## 9. TOML例

設定に認証を含める例。値はすべて架空:

```toml
server_url = "https://ntfy.sh/"
topic = "EXAMPLE_TOPIC"

[auth]
mode = "bearer"
token = "EXAMPLE_TOKEN"
```

認証専用ファイル:

```toml
mode = "basic"
username = "EXAMPLE_USER"
password = "EXAMPLE_PASSWORD"
```

topic未指定で作成した初期ひな形:

```toml
server_url = "https://ntfy.sh/"
# topic = "YOUR_TOPIC"

[auth]
mode = "none"
```

## 10. ファイル全体のOpenAPI Schema

以下はTOMLそのものではなく、解析後objectの型定義である。
HTTP APIを追加せず、pathsを空にする。
ConfigFileはファイルの構造検査用で、全キー任意・topic省略可能とする。
authはRawAuthで既知キーと型だけを検査し、未選択認証ソースへ
mode必須や方式組合せの意味検証を先行させない。
選択されたauth-fileはAuthFile、統合した認証はAuthConfigで検証する。
通常MCP／認証確認のEffectiveConfigは既定値適用後のため
server_url/topic/authをすべて必須とする。
SavedConfigは既存設定・CLI・環境変数・既定値を統合した保存結果用で、
server_urlと厳密なauthを必須とし、topic未指定だけを省略とコメントで表す。
存在するtopicは厳密に検証し、明示不正値を保存可能とはしない。
保存する表現はTOMLであり、このJSONは型定義の表現である。
ファイルのUTF-8/BOM・バイト上限・読取期限、TOML構文・重複キー、
認証ソース選択、URLの構造上の安全性、秘密を含む保存の保護は
Schemaだけでは保証せず、本文の実行時規則を適用する。

```json
{
  "openapi": "3.1.2",
  "info": {
    "title": "ntfy-mcp parsed TOML configuration contracts",
    "version": "0.1.0",
    "description": "TOML解析後のobjectに対する型定義。HTTP endpointを定義しない。"
  },
  "paths": {},
  "components": {
    "schemas": {
      "RawAuth": {
        "type": "object",
        "properties": {
          "mode": {
            "type": "string"
          },
          "username": {
            "type": "string"
          },
          "password": {
            "type": "string"
          },
          "token": {
            "type": "string"
          }
        },
        "required": [],
        "additionalProperties": false,
        "description": "設定ファイル内authの構造検査。既知キーの型・未知キー・nullを検査し、未選択ソースの必須値や方式組合せはここで検査しない。"
      },
      "AuthNone": {
        "type": "object",
        "properties": {
          "mode": {
            "type": "string",
            "const": "none"
          }
        },
        "required": [
          "mode"
        ],
        "additionalProperties": false
      },
      "AuthBasic": {
        "type": "object",
        "properties": {
          "mode": {
            "type": "string",
            "const": "basic"
          },
          "username": {
            "type": "string",
            "minLength": 1,
            "description": "コロン・制御文字を実行時に拒否する。"
          },
          "password": {
            "type": "string",
            "minLength": 1,
            "description": "自動trimせずBasic符号化へ渡す。秘密値をログや診断へ反射しない。"
          }
        },
        "required": [
          "mode",
          "username",
          "password"
        ],
        "additionalProperties": false
      },
      "AuthBearer": {
        "type": "object",
        "properties": {
          "mode": {
            "type": "string",
            "const": "bearer"
          },
          "token": {
            "type": "string",
            "minLength": 1,
            "pattern": "^[A-Za-z0-9._~+/-]+=*(?![\\s\\S])",
            "description": "ASCIIのBearer token形式。空白・制御文字を拒否する。"
          }
        },
        "required": [
          "mode",
          "token"
        ],
        "additionalProperties": false
      },
      "AuthConfig": {
        "oneOf": [
          {
            "$ref": "#/components/schemas/AuthNone"
          },
          {
            "$ref": "#/components/schemas/AuthBasic"
          },
          {
            "$ref": "#/components/schemas/AuthBearer"
          }
        ],
        "description": "選択された認証一式。mode必須。異なるソースや方式の資格情報を混合しない。"
      },
      "AuthFile": {
        "$ref": "#/components/schemas/AuthConfig"
      },
      "ConfigFile": {
        "type": "object",
        "properties": {
          "server_url": {
            "type": "string",
            "description": "未選択値は構造・型を検査し、有効値のHTTPS URL等の意味検証は統合後に行う。"
          },
          "topic": {
            "type": "string",
            "description": "ファイルでは省略可能。保存ひな形では未指定topicを省略しコメントを記す。明示不正値は有効値の検証で拒否する。"
          },
          "auth": {
            "$ref": "#/components/schemas/RawAuth"
          }
        },
        "required": [],
        "additionalProperties": false,
        "description": "TOML解析後の設定object。全キー任意で、topic欠落を許可する。重複キー・不正TOML・BOM・ファイル上限等はパース／読取段階で検査する。",
        "x-runtime-validation": [
          "TOML parse before schema validation",
          "file bytes <= 65536",
          "regular file and logical read deadline",
          "unselected auth semantics deferred"
        ]
      },
      "EffectiveConfig": {
        "type": "object",
        "properties": {
          "server_url": {
            "type": "string",
            "format": "uri",
            "pattern": "^[Hh][Tt][Tt][Pp][Ss]://",
            "description": "host必須。userinfo/query/fragmentなし。base pathとディレクトリ末尾を保持してURL構築する。",
            "x-runtime-validation": [
              "HTTPS URL with host",
              "no userinfo, query or fragment",
              "preserve base path",
              "no automatic trim"
            ]
          },
          "topic": {
            "type": "string",
            "minLength": 1,
            "maxLength": 64,
            "pattern": "^[A-Za-z0-9_-]{1,64}(?![\\s\\S])"
          },
          "auth": {
            "$ref": "#/components/schemas/AuthConfig"
          }
        },
        "required": [
          "server_url",
          "topic",
          "auth"
        ],
        "additionalProperties": false,
        "description": "通常MCP／認証確認モードで、CLI・環境変数・ファイル・既定値を統合して検証した有効設定。保存ひな形とは異なりtopic必須。",
        "x-runtime-validation": [
          "CLI auth is one mutually exclusive choice: --user, --auth-token or --auth-file",
          "CLI auth overrides entire environment/file auth",
          "selected environment auth overrides entire file auth",
          "no fallback from invalid selected source"
        ]
      },
      "SavedConfig": {
        "type": "object",
        "properties": {
          "server_url": {
            "type": "string",
            "format": "uri",
            "pattern": "^[Hh][Tt][Tt][Pp][Ss]://",
            "description": "host必須。userinfo/query/fragmentなし。base pathとディレクトリ末尾を保持してURL構築する。",
            "x-runtime-validation": [
              "HTTPS URL with host",
              "no userinfo, query or fragment",
              "preserve base path",
              "no automatic trim"
            ]
          },
          "topic": {
            "type": "string",
            "minLength": 1,
            "maxLength": 64,
            "pattern": "^[A-Za-z0-9_-]{1,64}(?![\\s\\S])"
          },
          "auth": {
            "$ref": "#/components/schemas/AuthConfig"
          }
        },
        "required": [
          "server_url",
          "auth"
        ],
        "additionalProperties": false,
        "description": "既存設定・CLI・環境変数と既定値を統合し、保存するTOMLを解析したobject。server_urlと厳密なauthは必須。topic未指定は省略できるが、存在するtopicは厳密に検証する。",
        "x-runtime-validation": [
          "save merged configuration as TOML, not JSON",
          "omitted topic gets a template comment",
          "explicit invalid selected topic fails before save",
          "safe file replacement follows configuration specification"
        ]
      }
    }
  }
}
```


## 11. 根拠と検証観点

- [OpenAPI 3.1.2 Schema Object](https://spec.openapis.org/oas/v3.1.2.html#schema-object)
- [tomlクレート](https://docs.rs/toml/1.1.8%2Bspec-1.1.0/toml/)
- [directories ProjectDirs](https://docs.rs/directories/6.0.0/directories/struct.ProjectDirs.html)
- [Microsoft icacls](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/icacls)
- [Rust rename](https://doc.rust-lang.org/std/fs/fn.rename.html)

TOML解析・重複/未知/型、CLI/env/fileと認証非混合、
保存時の統合・上限・資格情報のエスケープ・新規/更新・symlink拒否・失敗/取消、
Linuxの作成modeとWindows継承DACL、base path・OS配置を実装・受入検証する。
