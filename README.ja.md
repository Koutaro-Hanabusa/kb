# kb

[English](README.md)

高速な Markdown ナレッジベース。ノートは Git リポジトリ内の通常のファイルで、
`kb` は [`nb`](https://github.com/xwmx/nb) と同じコマンド体系（サブコマンド、別名、
ID、ファイル形式）を扱えます。速度はおよそ二桁速くなっています。

```sh
kb search "AI Search"       # 全ノートブックを全文検索（約 16 ms）
kb 3                        # アイテム 3 を表示
kb home:knowledge/12        # ノートブックとフォルダを指定
kb new -t "Title" --folder knowledge --content - <<< "body"
kb open knowledge/          # fzf で選び、glow でプレビューして編集
kb sync                     # Markdown をコミット、pull、push
```

## インストール

### Nix flake（このリポジトリでの使い方）

flake の入力に追加し、パッケージを PATH に置きます。

```nix
# flake.nix
inputs.kb = {
  url = "github:Koutaro-Hanabusa/kb";
  inputs.nixpkgs.follows = "nixpkgs";
};

# home-manager
home.packages = [ kb.packages.${system}.default ];
```

更新には `nix flake update kb` を実行します。

### Cargo

```sh
cargo install --git https://github.com/Koutaro-Hanabusa/kb kb-cli
```

### ソースからビルド

```sh
git clone https://github.com/Koutaro-Hanabusa/kb && cd kb
cargo build --release      # target/release/kb
cargo test
```

実行自体に追加の依存関係はありません。`sync`、`status`、`history` には `git` が必要です。
`fzf`、`glow`、`bat`、`openssl`、`gpg` は利用できる場合にだけ使われ、なくても動きます。
検出結果は `kb env --long` で確認できます。

## リリース

[`tagpr`](https://github.com/Songmu/tagpr) が `master` への変更に対するリリース PR を
維持します。その PR をマージすると `Cargo.toml` のバージョン更新、`v<version>` タグ、
GitHub Release の作成まで行います。crates.io への公開はしません。Nix パッケージも同じ
`Cargo.toml` のバージョンを読むため、メタデータは自動で同期します。

マージする PR に `minor` または `major` ラベルを付けると、その SemVer 更新を指定できます。
ラベルがなければ tagpr はパッチ版を提案します。ワークフローがリリース PR を作成できるように、
**Settings → Actions → General → Workflow permissions** で **Allow GitHub Actions to create
and approve pull requests** を有効にしてください。

### 初回利用

既存の `nb` ナレッジベースを指定すれば、レイアウトも ID もそのまま使えます。

```sh
kb ls                       # 既定は ~/.nb、または $KB_ROOT
```

新規に始める場合はこちらです。

```sh
kb init                     # ~/.nb/home を Git リポジトリとして作成
```

## 速度

macOS、ウォームキャッシュで 811 ノート / 13.5 MB を計測しました。`kb` は 50 回の平均値、
`nb` はこの所要時間では平均を取る必要がないため 1 回の結果です。

| | `nb` 7.25.4 | `kb` | |
| --- | ---: | ---: | ---: |
| 起動のみ（`--version`） | 0.30 s | **0.013 s** | 23× |
| 1 ノートブックを一覧表示 | 1.04 s | **0.016 s** | 65× |
| 1 ノートブックを検索 | 8.21 s | **0.016 s** | **513×** |

13.5 MB のデータ量は問題ではありません。`nb` は 27,000 行の Bash スクリプトで、起動のたびに
自分自身を解析します（そのため最低でも 0.3 秒かかります）。検索中はノートごとに `git`、`sed`、
`awk` を起動します。18.5 秒の検索時に CPU 使用率が 38% だったのは、ファイルを読むよりプロセスを
待っていたためです。`kb` は各ファイルを一度だけ読み、ripgrep のマッチャーで走査します。

インデックスやデータベースは使いません。すべてのコマンドがツリーを走査します。個人の
ナレッジベース程度の規模では、インデックスの管理コストは節約できる時間を上回ります。

## レイアウト

ナレッジベースは *ノートブック* のディレクトリで、各ノートブックは通常 Git リポジトリである
Markdown のディレクトリです。

```
~/.nb/                  # または $KB_ROOT
├── .current            # `kb use` で選択したノートブック
├── home/               # ノートブック
│   ├── .index          # このディレクトリの ID
│   └── knowledge/
│       ├── .index
│       └── *.md
└── work/
```

## `nb` との互換性

既存の習慣、スクリプト、ノートブックをそのまま持ち込めます。すべての `nb` サブコマンドは、
短縮名を含めて同じように解決されます。

互換性は振る舞いについての主張なので、同じ操作を両方のツールに実行し、生成物を diff して
検証しています。

```sh
./scripts/compat-check.sh     # PATH 上に nb が必要
```

ファイル名、ノート本文、ID の解決と欠番、ブックマークと todo の形式、`.index` / `.pindex` /
`.archived`、設定ファイルまで、17 項目を比較しています。現行版ではすべて一致しています。

**アイテム参照。** コマンドは `[<notebook>:][<folder>/]<id|filename|title>` を受け取ります。

```sh
kb 3                     # 現在のノートブックのアイテム 3
kb home:knowledge/12     # ノートブックとフォルダを指定
kb show "My Note Title"  # タイトルで指定
kb                       # 引数なしなら現在のノートブックを一覧表示
```

ID は各ディレクトリの `.index` ファイルにあり、アイテムの ID はその行番号です。削除時は行を
消すのではなく空にするため、ID は再利用されません。去年控えた参照も、欠番を含めて同じノートを
指し続けます。これは `nb` 自体との比較で検証済みです。

**ファイル名。** タイトルからのファイル名は `nb` と同じ規則です。ASCII は小文字化し、空白と
`: / \ ? *` はアンダースコアに置換し、それ以外はそのまま残します。
`日本語UIライティング - 句点のルール` → `日本語uiライティング_-_句点のルール.md`。

**ファイル。** ノートは Markdown、ブックマークは `*.bookmark.md`、todo は `# [ ]` / `# [x]`
見出しを持つ `*.todo.md` です。暗号化アイテムは `*.enc`（`-md sha256` を使う OpenSSL AES-256、
または設定した GPG）です。ピン留めは `.pindex`、アーカイブ済みノートブックには `.archived` が
あります。`kb` で暗号化したノートを `nb` で復号でき、逆も可能なことを検証しています。

**設定。** `kb` は `~/.kbrc` を読み、同じ 12 個の設定名を持つ
`export NAME="${NAME:-value}"` 形式で `~/.nbrc` にフォールバックします。どちらもパースではなく
source するため、実行時に分岐できます。

## `nb` との差分

### 追加機能

- **フロントマター。** ノートに `title` / `tags` / `created` / `updated` を持たせ、一覧を日付順に
  並べ、メタデータで絞り込めます。`nb` はこれらのファイルを変更せずに読めます（ヘッダーを無視します）。
- **`kb migrate`** — 既存ノートにフロントマターを補完します。追記だけで、本文は書き換えません。
- **`kb tags`** — すべてのタグと、そのタグを持つノート数を表示します。
- **`kb pick`** — fzf でノートを選び、`glow` でプレビューします。フォルダを渡した `kb open` と
  `kb peek` も同様に動作します。
- **`kb reconcile`** — 実際にディスクにある内容から `.index` を再構築します。
- `search` と `ls` の **`--json`**。
- **共通フィルター。** `-n/--notebook`、`-t/--tag`、`-s/--since 7d`、`--limit` を、集合を選ぶ
  コマンド全体で使えます。

### 対応範囲

すべての `nb` サブコマンドと孫サブコマンドを解決します。後者は 44 個あり、目視ではなく
`nb help` と機械的に照合しています。

次の二つは、ここでは不要な処理のため `nb` より少ない機能です。

- **`env install` / `env update`** は `nb` の Web UI 用アセットを取得します。`kb browse` は
  自己完結しているため、代わりに `kb env check` が外部ツールを報告します。
- **`notebooks select`** はセレクターからノートブックを解決して出力します。`nb` 版は現在の
  ノートブックを永続化せずに設定しますが、それはこの一回の実行中だけ意味がある動作です。

### 振る舞いの違い

- **`browse`** はより小さな Web アプリケーションです。ノートの描画、`[[wiki links]]` と `#tags` の
  解決、検索、追加 / 編集 / 削除を提供しますが、`nb` の 677 行ある組み込み UI の再実装ではありません。
- **`kb open <folder>`** は fzf ピッカーを開いてからエディタを起動します。`nb` はファイルブラウザ
  （`ranger`、`mc` など）を使います。`kb show <folder>` は引き続き一覧表示です。
- **ターミナルなし、エディタなし。** `kb` は `$EDITOR` を起動する前に tty を確認し、自動実行
  セッションでハングする代わりにパスを報告します。
- **`kb sync`** は Markdown のみをステージし、ほかのファイルがすでにステージされている場合は、
  それらを巻き込まず拒否します。
- **色付き出力** はパス、タイトル、行番号のみの最小限です。テーマはありません。

## コマンド

| コマンド | 用途 |
| --- | --- |
| `add` (`a`, `new`, `+`) | ノートを作成。`/` を含むか拡張子を持つ単独引数はパス、それ以外は内容として扱う |
| `ls` / `list` | アイテムを一覧表示 |
| `search` (`q`, `grep`) | ノート本文を検索。既定は正規表現、smart case |
| `show` (`s`) / `peek` (`p`) / `open` (`o`) | アイテムを表示、またはシステムに渡す |
| `edit` (`e`) | 追記、前置、上書き、または `$EDITOR` を起動 |
| `delete` (`d`, `rm`) / `move` (`mv`) / `copy` (`cp`) | ID を維持してアイテムを管理 |
| `bookmark` (`bm`) | URL をブックマークし、タイトルを取得 |
| `todo` (`todos`, `tasks`) / `do` / `undo` | Todo とチェックボックスを管理 |
| `pin` / `unpin` / `archive` / `unarchive` | 順序とノートブックの状態を管理 |
| `browse` | 組み込み Web アプリ |
| `notebooks` / `use` (`u`) / `count` / `folders` | ノートブックと構造を管理 |
| `sync` / `status` / `git` / `history` / `remote` | Git 操作 |
| `settings` / `set` / `unset` / `env` | 設定 |
| `plugins` | `*.kb-plugin` / `*.nb-plugin` をインストール・実行 |
| `import` / `export` / `run` / `shell` / `completions` | その他 |
| `migrate` / `pick` / `tags` / `reconcile` | 追加機能 |

集合を選ぶコマンドでは、以下のフィルターを共通で使えます。

```
-n, --notebook <NAME>   1 つのノートブックに限定
-t, --tag <TAG>         タグを必須にする。複数指定するとすべて必要
-s, --since <WHEN>      指定日または期間（7d、3w、2026-01-01）以降に更新
    --limit <N>         最大件数
```

`search` は `-F`（リテラル）、`-i`/`-s`（大文字・小文字）、`-m`（ノートごとの一致数）、
`-l`（パスのみ）、`--json` も受け取ります。

### マイグレーション

`kb migrate` は、これらがないノートに `title`、`tags`、`created`、`updated` を追加します。各値は
次の規則で機械的に導出されます。

- **title** — 最初の見出しか本文行。長ければ最初の文の切れ目で短縮し、最後の手段としてファイル名
- **tags** — ノートの最上位ディレクトリ
- **created** — `nb` 形式のタイムスタンプ付きファイル名、なければ最初のコミット
- **updated** — ファイルに触れた最後のコミット

本文は書き換えません。既存のフロントマターキーはバイト単位で保ったまま、欠けているものだけを
追記するため、diff は純粋な挿入です。`--apply` を渡すまではプレビューで、`--allow-dirty` を渡さない
限り作業ツリーが汚れている場合は拒否します。ここにある 797 ノートの移行では、5,569 行を追加し、
削除は 0 行でした。

## 設定

設定は `~/.kbrc` にあり、`kb settings`、`kb set`、`kb unset` で管理します。`~/.nbrc` は
フォールバックとして読まれるため、既存の `nb` 設定はそのまま使えます。`kb` が書き込むのは
自身の設定ファイルだけです。

```sh
kb settings list          # すべての設定と値
kb set default_extension org
kb set 5                  # 設定は名前または番号で指定可能
```

設定ファイルは source されるため、実行時に分岐できます。

```bash
# ~/.kbrc
if [[ -n "${CLAUDECODE:-}" ]]; then
  export KB_EDITOR="cat"   # 自動実行セッションでエディタ待ちしない
else
  export KB_EDITOR="nvim"
fi
```

環境変数は設定ファイルより優先されます。

| 変数 | 効果 |
| --- | --- |
| `KB_ROOT` | ナレッジベースの場所（既定は `~/.nb`） |
| `KB_NOTEBOOK` | 新しいノートを作るノートブック |
| `KBRC_PATH` / `NBRC_PATH` | 設定ファイルの場所 |
| `KB_*` / `NB_*` | 任意の設定。例: `KB_ENCRYPTION_TOOL=gpg` |
| `NO_COLOR` | ANSI スタイルを無効化 |

`KB_NOTEBOOK` がない場合、コマンドは `kb use` で選択したノートブックを対象にします。それもなければ、
`~/.is_work_pc` がある場合は `work`、なければ `home` です。仕事と個人のノートは別リポジトリにあり、
混ぜてしまうと戻すのが面倒なためです。

## 構成

- **`kb-core`** — ノート、ID、検索、Git、ブックマーク、todo、レンダリング、暗号化、プラグインを持つライブラリ
- **`kb-cli`** — `kb` バイナリ

CLI と `browse` Web アプリケーションは、どちらも `kb-core` の薄いラッパーです。

## ライセンスと謝辞

`kb` は **GNU Affero General Public License v3.0 以降**でライセンスされています
（[LICENSE](LICENSE) を参照）。

これは William Melody による [`nb`](https://github.com/xwmx/nb) のコマンド体系を再実装したものです。
`nb` 自体も AGPLv3 です。`nb` のコードは一切コピーしておらず、`kb` は Rust で一から書かれています。
ただし相互運用できるように、コマンド、識別子、ディスク上の形式を意図的に再現しています。また既存の
`.enc` ファイルを復号するために必要な OpenSSL 呼び出しの詳細は、`nb` のソースを読んで確認しました。
そのため `kb` は、互換性を持つ作品と同じ条件でライセンスされています。

811 ノートで起動コストが無視できなくなったため置き換えることになった `nb` と、William Melody に
感謝します。
