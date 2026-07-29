# tinymemory

[English](README.md) | **日本語**

コーディングエージェントのための、軽量・高速・ローカルな長期記憶 — **Claude Code** と **Codex CLI** が同じ記憶を共有します。

```
/remember   →   /clear   →   記憶は自動で読み込まれる
```

- **Rustバイナリ1個(約1MB)+ プレーンなMarkdownファイル。** デーモンなし、DBなし、MCPサーバーなし、Webアプリなし、APIキーなし、埋め込みモデルなし。
- **両エージェントで1つのストア。** Claude Code も Codex も同じ `~/.tinymemory` を読み書き。
- **recallは全自動。** `SessionStart` フックが新しいセッション(`/clear` 直後を含む)にプロジェクトの記憶を自動注入。Claude Codeでは読み込み確認の1行(`tinymemory: loaded 2 facts + 1 session (project: myapp)`)が表示されるので、正常に読み込まれたことが目で確認できます。Codexではフック実行中に固定のステータス行(`Loading tinymemory`)が表示されます。
- **データはあなたのもの。** 記憶はすべてfrontmatter付きMarkdownファイル。`cat` で読め、手で編集でき、grepでき、gitに入れられる。
- **日本語検索に強い。** トークナイザなしの部分一致なので「人事」「経費」のような2文字語も正確にヒット。

## なぜ作ったか

既存のエージェント記憶ツールは重い: 常駐ワーカー、Node/Bun/Pythonのランタイム連鎖、ベクタDB、Webビューア、大量のMCPツール。インストール失敗の大半は記憶そのものではなくその周辺機構が原因です。tinymemoryはそれらを意図的にすべて排除しました。要約はエージェントが書き(文脈を持っているので)、バイナリはファイルの保存と検索だけを行い(それが得意なので)、他には何も動きません。

## インストール

```sh
cargo install tinymemory
tinymemory init          # Claude Code / Codex 両方のフックとスキルを設定
```

片方だけなら `tinymemory init claude` / `tinymemory init codex`。結果は `tinymemory doctor` で確認できます。

Claude Code はプラグインとして入れることもできます:

```
/plugin marketplace add arkrtm/tinymemory
/plugin install tinymemory@tinymemory
```

(いずれの場合もバイナリは必要です: `cargo install tinymemory`)

> 注: プラグイン経由で入れた場合、スキル名は名前空間付きになります — `/tinymemory:remember` と `/tinymemory:recall`。`tinymemory init` 経由なら以下の表の短い `/remember` / `/recall` が使えます。

**Codex の初回のみ必要な手順:**
- `codex` を起動し、信頼レビュー(`/hooks`)で tinymemory のフックを一度承認する。
- 任意: デフォルトのサンドボックス下でエージェントが承認プロンプトなしに保存できるようにするには `~/.codex/config.toml` に追記:
  ```toml
  [sandbox_workspace_write]
  writable_roots = ["/Users/you/.tinymemory"]
  ```

## 使い方

| セッション内 | 何が起きるか |
|---|---|
| `/remember`(Codex: `$remember`) | エージェントが構造化されたセッション要約を書き、`tinymemory save` で保存。 |
| `/clear` | コンテキストがリセットされ、SessionStartフックが新セッションに記憶を自動注入。Claude Codeでは `tinymemory: loaded …` の確認行が表示される。 |
| `/recall`(Codex: `$recall`) | セッション途中での手動recall。 |
| `/dream`(Codex: `$dream`) | 溜まった記憶をエージェントが整理: 重複factの統合、古い決定の更新、superseded削除、古いセッションのアーカイブ。 |

remember → clear → recall のループは実質2操作(`/remember` と `/clear`)。recallの操作は不要です。

**Dreaming(記憶の整理)。** Claude Code純正のAuto Dream(Claude自身の記憶ファイル専用)と同じ発想で、tinymemoryのストアも定期的な整理で品質が保たれます — ただしtinymemoryでは、エージェントがあなたのセッション内で行うので過程が見えます。`tinymemory dream` が候補を機械的に抽出し(supersededエントリ、recallインデックスから溢れたセッション、90日以上前のfact)、dreamスキルがエージェントに各項目を現コードと照合させ、factの統合・更新・削除と古いセッションの `archive`(`archive/` サブディレクトリへの可逆的な移動)を行わせます。

**Dreamingは自動で発火します。** 整理債務が溜まると(superseded 3件以上、または未インデックスセッション5件以上)、次のセッション開始時に注入される記憶ブロックの末尾に「他の作業の前に整理せよ」という指示が付き、**ユーザーの操作ゼロで、コンテキストが空いているセッション冒頭に**エージェントがdreamを実行します。`/dream` での手動実行も可能で、`save` も債務蓄積時にリマインドを表示します。整理が済むと指示は消えます。

### CLI

```
tinymemory save     [--title T] [--type session|fact] [--tags a,b] [--global] [--project P]   # 本文はstdin
tinymemory recall   [--budget 6000] [--sessions 3]
tinymemory search   <query> [--limit 10] [--json]     # 部分一致、日本語OK
tinymemory list / show <id> / delete <id> / archive <id>
tinymemory dream    # 整理レポート(/dreamスキルの入力)
tinymemory init [claude|codex|all] / doctor
```

## 保存のしくみ

```
~/.tinymemory/memories/
├── myrepo-1a2b3c4d/                  # プロジェクトごと(git root-commitで識別:
│   ├── 20260729-153000-a1b2.md       #  worktree・クローン・移動したリポジトリは
│   └── 20260728-101500-77ff.md       #  同じ記憶を共有)
└── _global/                          # ユーザー全体のfactと好み
```

各記憶はfrontmatter付きMarkdown(`id`, `type: session|fact`, `title`, `tags`, `created`)。種類は2つだけ:

- **session** — 何をしたか、決定事項、現在の状態、次のステップ(`/remember` 時にエージェントが記述)
- **fact** — 永続的な知識: ビルドコマンド、不変条件、好み(ユーザー全体なら `--global`)

**決定の更新:** 同じタイトルでfactを保存し直すと、recallにはタイトルごとに最新の1件だけが表示されます。`save` は `Supersedes <id>` と出力するので、エージェントは不要になった古いエントリを `tinymemory delete` できます。全項目が日付付き・新しい順なので、新しい決定は常に古い決定より上に表示されます。

`recall` は予算制限付きブロックを組み立てます: fact全件 → 直近セッション要約3件 → それより古いセッションの1行インデックス。深掘りはエージェントが `tinymemory search` / `tinymemory show <id>` で行えます。

## 設計上の制約(このツールがやらないこと)

- バックグラウンドプロセス・ポート・`~/.tinymemory` 以外の状態を持たない
- LLM API呼び出しをしない — 要約はセッション内のエージェントが書く
- トランスクリプトを解析しない — 両CLIともセッションファイルは内部フォーマットと明言している
- ベクタ検索をしない — 記憶の規模(数千件)ではMarkdownのスコア付き部分一致スキャンが数msで完了し、短い日本語語彙ではtrigramインデックスより正確

Claude Code の auto-memory や Codex のネイティブmemoriesはツール単位の記憶です。tinymemoryのニッチは**共有された透明な**ストア: 両エージェントが読み書きする1組のファイルで、人間が確認・編集できることです。

## ライセンス

MIT
