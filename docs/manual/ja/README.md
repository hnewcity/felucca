# Felucca ドキュメント索引 · 日本語

[中文](../README.md) · [English](en/README.md) · 日本語 · [한국어](ko/README.md) · [Français](fr/README.md) · [Español](es/README.md) · [Русский](ru/README.md)

これは Felucca のユーザー向けドキュメントです。最初のトンネルを動かすところから、各モジュールの設定と
使い方、そしてコマンド・フィールド・エンドポイント・用語のリファレンスまでを扱います。
扱うのは **Felucca の使い方** であり、内部実装ではありません。

## はじめに

| ページ | 内容 |
| --- | --- |
| [はじめに](get-started.md) | インストール、ローカルデモ、手動で最初のトンネルを動かす |
| [アーキテクチャと概念](architecture.md) | 構成要素、トラフィックの経路、用語集 |
| [設定の取得元と CLI](configuration.md) | 設定の取得元、環境変数、複数インスタンス、`--check` |

## ガイド

| ページ | 内容 |
| --- | --- |
| [felucca-server モジュール](server.md) | ブリッジ、クライアントテーブル、HTTP の受信、ポートの認可 |
| [felucca-agent モジュール](agent.md) | 接続、信頼、再接続、エグレスネットワーク、上流のプロキシ |
| [トンネルとポートマッピング](tunnels.md) | 8 つのモード、`[[mapping]]`、UDP、PROXY プロトコル |
| [バックエンド、負荷分散、ヘルスチェック](backends.md) | バックエンドのフィールド、5 つの戦略、追い出しと復旧 |
| [HTTP/HTTPS リバースプロキシ](http-services.md) | ドメインとパスのルーティング、書き換え、CORS、SNI、HTTP/2、HTTP/3 |
| [証明書、SNI、ACME](certificates.md) | ブリッジ証明書、固定証明書、発行と更新 |
| [フォワードプロキシ](forward-proxy.md) | SOCKS5 / HTTP / 混合のエントリ、アカウント、対象 ACL |
| [ファイルサービスと WebDAV](file-services.md) | ディレクトリの機能、読み取り専用の制御、Range、WebDAV メソッド |
| [サーバー上のローカルな対象と入れ子ルート](local-services.md) | `[local.*]`、バックエンドのエイリアス、16 ホップのループ制御 |
| [プライベート共有とビジター](private-shares.md) | 公開ポートなしでの TCP/UDP/ファイル/プロキシ アクセス |
| [P2P 直接接続とリレーへのフォールバック](p2p.md) | 使い捨てチケット、ホールパンチング、QUIC 直接接続、フォールバック |
| [アクセス制御、クォータ、ストリーム符号化](security.md) | 送信元と対象の ACL、GeoIP、階層的なクォータ、圧縮と暗号化 |
| [管理 UI と集中構成](management.md) | 管理 UI、アカウントのロール、API、構成のプッシュ配信 |
| [外部プラットフォーム連携](platform.md) | プラットフォームトークン、逆方向の WSS RPC、署名付きコールバック、トラフィックの取り込み |
| [システムサービス、更新、ログ、監視](operations.md) | systemd/launchd/SCM、更新、ログのローテーション、メトリクス |

## リファレンス

| ページ | 内容 |
| --- | --- |
| [リファレンス](reference.md) | CLI、フィールド早見表、管理 API、サンプル一覧、数値の上限、用語集 |

## 関連ドキュメント

- [アーキテクチャ](../../architecture.md) — crate の責務の境界
- [設定とデプロイ](../../configuration.md) — フィールド単位の詳細
- [実装状況](../../implementation-status.md) · [検証記録](../../validation.md)

一つ上の階層にある参考資料は中文と英文のままです。
