# wgpu と標準 Spout では、共有テクスチャを直接使えない

Windows 上で wgpu（DX12 または Vulkan）から Resolume などの標準 Spout 送信を受けるとき、フレームは GPU 上で一度コピーする。共有テクスチャそのものをサンプリングする経路は、この組み合わせでは成立しない。

1〜3 節で理由、4 節でいまの実装、5 節で用語を置く。プロトコルの細部は [research/spout2.md](research/spout2.md) を見る。

## 1. 対象は「標準 Spout + wgpu」だけ

ここで言うセットアップは次の三つが同時に揃ったときを指す。

- 送信側が標準 Spout 2.007 の共有テクスチャを出す（Resolume Avenue、TouchDesigner、OBS、公式 SDK の既定）
- 受信側（または送信側）が wgpu の DX12 / Vulkan バックエンドを使う
- 受信アプリが Resolume など既存の D3D11 送信と相互運用する

対象外は、自前の D3D11 デバイスだけで完結する経路と、macOS の Syphon / IOSurface である。後者は実装があるが実機未確認で、本稿では扱わない。

## 2. Spout が渡すハンドルを D3D12 は開けない

標準 Spout の共有テクスチャは `D3D11_RESOURCE_MISC_SHARED` で作られ、ハンドルは `IDXGIResource::GetSharedHandle` で取る。これは NT ハンドルではない、古い KMT ハンドルである。

D3D12 の `ID3D12Device::OpenSharedHandle` が受け付けるのは、`CreateSharedHandle` が出した NT ハンドルだけだ。wgpu の DX12 デバイスは、Resolume が出した共有テクスチャを `ID3D12Resource` として開けない。

公式の `spoutDX12` も同じ制約を踏まえ、`D3D11On12CreateDevice` で D3D11 を重ね、`CreateWrappedResource` したあと `CopyResource` している。`sp2-wgpu` の DX12 経路もこれに倣う。送信も同じで、普通の wgpu テクスチャをラップしただけではレガシー共有にはならない。

`ID3D11On12Device2::UnwrapUnderlyingResource` や `ID3D12CompatibilityDevice::CreateSharedResource` は、自前で確保したリソースには使える。ただし前者は `OpenSharedResource` した KMT テクスチャへの適用が文書化されておらず、後者は最初から互換共有として作った専用リソースに限る。どちらも標準 Spout / Resolume との互換経路ではない。

## 3. Vulkan はハンドルを開けるが、同期が足りない

`VK_KHR_external_memory_win32` と `VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_KMT_BIT` なら、KMT ハンドルを `VkImage` に取り込める。メモリの別名化自体はできる。

足りないのは GPU 側の同期である。標準 Spout の送信は、名前付き CPU ミューテックス（`<name>_SpoutAccessMutex`）を取り、D3D11 で書いて `Flush()` し、ミューテックスを返す。keyed mutex も、受信側が待てる共有 fence も出さない。Lynn Jarvis 自身、複数受信で止まるため keyed 共有は使わないと書いている。

Vulkan が外部画像を直接サンプルするには、送信側が参加する GPU 同期（keyed mutex か共有 fence）と、`VK_QUEUE_FAMILY_EXTERNAL` からの所有権移行が要る。wgpu の `Texture` ではそのバリアを毎フレーム正確に書けない。公式 SpoutVulkan 受信も、取り込んだ画像からコピーしている。

## 4. いまの実装は GPU コピーに固定している

Windows の `sp2-wgpu` は次の経路だけを公開する。

- DX12: D3D11On12 経由で Spout の D3D11 テクスチャへ `CopyResource` する
- Vulkan: KMT ハンドルを取り込み、wgpu が所有するテクスチャへコピーする

`ReceiveMode::Shared` は Windows では構築時に拒否する。送信の `WgpuSender::send` も、アプリケーションのテクスチャから共有テクスチャへコピーする。

NT ハンドルと共有 fence を両方出す独自プロトコルにすれば、D3D12 同士では直接共有できる余地がある。その送信は Resolume が開けない。このクレートの目的は既存 Spout アプリとの相互運用なので、その道は採らない。

## 5. 用語

| 用語 | 意味 |
| --- | --- |
| KMT ハンドル | `GetSharedHandle` が返す古い共有ハンドル。NT ハンドルではない |
| NT ハンドル | `CreateSharedHandle` が返すハンドル。D3D12 の `OpenSharedHandle` が受け付ける側 |
| D3D11On12 | D3D12 デバイスとキューの上に D3D11 を載せる Microsoft の橋渡し |
| アクセスミューテックス | 送信名に紐づく CPU ロック。テクスチャを触るあいだだけ取る |

最終更新: 2026-09-15
