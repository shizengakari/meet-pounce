# meet-pounce

[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-Windows-0078d7.svg)]()
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Google カレンダーの予定から Google Meet / Zoom / Teams の URL を検出し、開始直前に Windows ネイティブのトースト通知を表示する常駐ツールです。通知から直接ミーティングに参加できます。

常駐時のメモリ使用量は約 2〜3 MB で、アイドル時は CPU を使用しません。

## 使い方

`meet-pounce.exe` を起動します。初回起動時はブラウザで設定画面が開きます。

1. Google Cloud Console の OAuth Client ID / Secret を入力
2. 「Google アカウントを追加」からカレンダーを連携
3. 通知する時間やサウンドを選択して保存

保存後はタスクトレイに常駐します。

### オプション

```powershell
# 設定画面を開く
meet-pounce.exe --settings

# 通知テスト
meet-pounce.exe --test
```

## ビルド

```powershell
cargo build --release
```

## ライセンス

MIT
