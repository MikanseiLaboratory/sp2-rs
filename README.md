# sp2-rs

Spout2 (Windows) / Syphon (macOS) 互換の GPU テクスチャ共有 SDK の純 Rust 実装です。

`sp2-rs` は C++ / Objective-C ライブラリのラッパーではありません。公開されているプロトコル
(共有メモリレイアウト、カーネルオブジェクト名、分散通知名、メッセージ形式) を Rust で再実装し、
既存の Spout2 / Syphon 対応アプリケーション (Resolume, TouchDesigner, VDMX, OBS など) と
そのまま相互運用できることを目標としています。

## 特徴

- **統一 API**: `sp2::Sender` / `sp2::Receiver` / `sp2::Directory` で OS を意識せずに送受信できます。
- **trait ベース**: `sp2_core::{SenderBackend, ReceiverBackend, DirectoryBackend}` を各 OS 実装が実装しています。
- **ネイティブ経路**: Windows では `ID3D11Texture2D`、macOS では `IOSurface` / `MTLTexture` に直接アクセスできます。
- **wgpu 連携**: `sp2-wgpu` で D3D12 (D3D11On12 ブリッジ) / Vulkan (KMT ハンドルインポート) / Metal (IOSurface) から直接送受信できます。

## クレート構成

| クレート | 役割 |
| --- | --- |
| `sp2-core` | OS 非依存の型と trait |
| `sp2-spout` | Spout2 プロトコル実装 (Windows) |
| `sp2-syphon` | Syphon プロトコル実装 (macOS) |
| `sp2` | 統一 API (umbrella) |
| `sp2-wgpu` | wgpu 連携 |

## ステータス

開発初期段階です。詳細は [docs/implementation-plan.md](docs/implementation-plan.md) を参照してください。

- 調査メモ: [docs/research/spout2.md](docs/research/spout2.md), [docs/research/syphon.md](docs/research/syphon.md)
- ライセンス方針: [docs/licensing.md](docs/licensing.md)

## ライセンス

BSD 2-Clause License。詳細は [LICENSE](LICENSE) を参照してください。

本プロジェクトは Spout2 (Lynn Jarvis, BSD 2-Clause) および Syphon (Tom Butterworth & Anton Marini, Simplified BSD)
の公開情報を参照して実装されています。各プロジェクトの著作権表示とライセンスは [NOTICE](NOTICE) に掲載しています。
本プロジェクトは各プロジェクトの公式実装ではなく、いずれのプロジェクトとも提携・推奨関係にありません。
