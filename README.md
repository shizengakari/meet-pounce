# meet-pounce

[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-Windows-0078d7.svg)]()
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Google カレンダーからオンライン会議や通常の予定を検出し、開始直前に Windows ネイティブのトースト通知を表示する常駐ツールです。通知から直接ミーティングに参加できます。

常駐時のメモリ使用量は 5 MB 以下で、アイドル時は CPU を使用しません。

## 主な特徴

- 通知の ON/OFF、タイミング、サウンド、音量ブーストを個別に設定できます
- 差分同期と即時メモリ解放により、メモリやバッテリーの消費を最小限に抑えます
- タスクトレイの右クリックメニューから設定画面の表示、手動同期、通知テスト、スタートアップ登録を行えます

## 使い方

`meet-pounce.exe` を起動します。初回起動時はブラウザで設定画面が開きます。

1. Google Cloud の OAuth Client ID と Client Secret を入力
2. 「Google アカウントを追加」からカレンダーを連携
3. 通知タイミングやサウンドを設定して保存

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
