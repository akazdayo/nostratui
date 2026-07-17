# nostr-ratatui

`ratatui` で動く、Vim風キーバインドのNostr TUIクライアントです。

対応範囲:

- NIP-01: リレー接続、イベント購読、テキストノート、プロフィール
- NIP-05: プロフィールのDNS識別子をHTTPSで検証（`✓` は検証済み、`?` は未検証）
- NIP-25: `+` / `-` / 絵文字リアクションの送受信
- NIP-19: `npub` / `note` のBech32表示
- NIP-08: 従来の `#[index]` メンション送受信（入力時は `@npub...` / `nostr:note...`）
- NIP-18: リポストの送受信

## 起動

```sh
nix develop
cargo run
```

投稿する場合は秘密鍵を環境変数で渡します。秘密鍵はファイルへ保存しません。

```sh
NOSTR_SECRET_KEY=nsec1... cargo run
```

未指定時は閲覧専用です。リレーは複数指定できます。

```sh
cargo run -- --relay wss://relay.example.com --relay wss://relay2.example.com
```

## キー操作

| キー | 操作 |
| --- | --- |
| `j` / `k` | 下 / 上へ移動 |
| `g` / `G` | 先頭 / 末尾へ移動 |
| `l` / `Enter` | 詳細を開く |
| `h` / `Esc` | 詳細を閉じる |
| `i` / `o` | 新規投稿 |
| `r` | 選択ノートへ返信 |
| `+` / `-` | Like / Dislike |
| `e` | 絵文字リアクション入力 |
| `R` | リポスト |
| `Ctrl-S` | 入力を送信 |
| `Esc` | 入力を破棄 |
| `q` | 終了 |

> [!IMPORTANT]
> NIP-08は廃止済み仕様ですが、指定された既存イベントとの互換性のため対応しています。投稿時の `@npub...` / `nostr:npub...` / `nostr:note...` は `#[index]` と対応タグへ変換されます。
