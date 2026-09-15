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

## 使い方

Windows または macOS で実行してください。Linux では各 API が `Error::Unsupported` を返します。
Syphon の発見はメインスレッドの RunLoop 依存なので、毎フレーム `sp2::pump_events` を呼んでください (Windows では no-op です)。

### CPU (統一 API)

```rust
use sp2::{PixelBuffer, PixelFormat, Receiver, ReceiverBackend, Sender, SenderBackend};

// 送信
let mut sender = Sender::new("My Sender", 640, 360, PixelFormat::Bgra8Unorm)?;
let pixels = vec![0u8; 640 * 360 * 4];
sender.send_pixels(PixelBuffer::packed(&pixels, 640, 360, PixelFormat::Bgra8Unorm)?)?;

// 受信 (引数なしはアクティブセンダー)
let mut receiver = Receiver::connect("My Sender")?;
let mut pixels = Vec::new();
if let Some(frame) = receiver.receive_pixels(&mut pixels)? {
    println!("{}x{} frame {}", frame.width, frame.height, frame.frame_id);
}
```

### wgpu

`COPY_SRC` 付きの `wgpu::Texture` をそのまま送れます。バックエンドは自動選択です
(Metal / D3D12 / Vulkan)。非対応時は CPU 経路に落ち、`WgpuSender::path()` で確認できます。

```rust
use sp2::PixelFormat;
use sp2_wgpu::{ReceiveMode, WgpuReceiver, WgpuSender};

// 送信: 描画先テクスチャを毎フレーム渡す
let mut sender = WgpuSender::new(
    &device,
    &queue,
    "My App",
    1280,
    720,
    PixelFormat::Bgra8Unorm,
)?;
sender.send(&frame_texture)?;
println!("transfer path: {:?}", sender.path());

// 受信: texture() をサンプラーに渡せば表示できる
let mut receiver = WgpuReceiver::connect(&device, &queue, "My App")?;
if let Some(frame) = receiver.receive()? {
    if let Some(texture) = receiver.texture() {
        // bind group を組み、fullscreen blit などへ
        let _ = (frame, texture);
    }
}

// 共有テクスチャを直接参照したい場合 (Metal / Vulkan)
let mut shared = WgpuReceiver::with_mode(
    &device,
    &queue,
    Some("My App"),
    ReceiveMode::Shared,
)?;
```

## Examples

Windows / macOS 上で、リポジトリルートから実行します。

| コマンド | 内容 |
| --- | --- |
| `cargo run -p sp2 --example list_senders` | センダー / サーバー一覧 |
| `cargo run -p sp2 --example sender_cpu -- [name] [width] [height]` | CPU グラデーションを 60fps で送信 |
| `cargo run -p sp2 --example receiver_cpu -- [sender name]` | 解像度・fps・平均色を表示 |
| `cargo run -p sp2-wgpu --example wgpu_sender -- [name] [width] [height]` | 回転三角形を描画して送信 |
| `cargo run -p sp2-wgpu --example wgpu_receiver -- [sender name] [--shared]` | 受信テクスチャを全画面表示 |
| `cargo run -p sp2-wgpu --example relay -- [source] [output]` | 受信 → 色反転 → 別センダーとして送信 |

```text
# 端末 1: 送信
cargo run -p sp2-wgpu --example wgpu_sender -- "sp2 wgpu Sender" 1280 720

# 端末 2: 受信 (名前省略時はアクティブセンダー)
cargo run -p sp2-wgpu --example wgpu_receiver -- "sp2 wgpu Sender"

# ゼロコピー参照 (Metal / Vulkan。上書き競合があり得る)
cargo run -p sp2-wgpu --example wgpu_receiver -- "sp2 wgpu Sender" --shared

# バックエンド指定 (Windows)
WGPU_BACKEND=dx12 cargo run -p sp2-wgpu --example wgpu_sender
WGPU_BACKEND=vulkan cargo run -p sp2-wgpu --example wgpu_sender
```

`sender_cpu` と `wgpu_receiver`、Resolume / TouchDesigner / VDMX / OBS など既存アプリとも相互に接続できます。

## ステータス

開発初期段階です。詳細は [docs/implementation-plan.md](docs/implementation-plan.md) を参照してください。

- 調査メモ: [docs/research/spout2.md](docs/research/spout2.md), [docs/research/syphon.md](docs/research/syphon.md)
- ライセンス方針: [docs/licensing.md](docs/licensing.md)

## ライセンス

BSD 2-Clause License。詳細は [LICENSE](LICENSE) を参照してください。

本プロジェクトは Spout2 (Lynn Jarvis, BSD 2-Clause) および Syphon (Tom Butterworth & Anton Marini, Simplified BSD)
の公開情報を参照して実装されています。各プロジェクトの著作権表示とライセンスは [NOTICE](NOTICE) に掲載しています。
本プロジェクトは各プロジェクトの公式実装ではなく、いずれのプロジェクトとも提携・推奨関係にありません。
