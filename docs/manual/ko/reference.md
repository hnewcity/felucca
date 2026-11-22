# 참조

명령어, 필드, 엔드포인트, 예제 목록, 용어집을 정리합니다. 가이드가 *이유*를 설명한다면,
이 페이지는 *대상*을 나열합니다.

## CLI

두 프로그램이 공통으로 받는 인수입니다.

| 인수 | 환경 변수 | 설명 |
| --- | --- | --- |
| `--build-info` | — | 프로그램 이름, 버전, 프로토콜, 빌드 대상을 출력합니다 |
| `-c, --config <SRC>` | `FELUCCA_CONFIG` | TOML/JSON 파일, HTTP(S) URL, 인라인 JSON, base64 또는 `felucca://` URI. 에이전트에서는 반복 가능합니다 |
| `--expand-env` | `FELUCCA_EXPAND_ENV` | `${VAR}` / `${VAR:-default}`를 확장합니다 |
| `--config-token-env <NAME>` | `FELUCCA_CONFIG_TOKEN_ENV` | 원격 설정의 Bearer 토큰을 담는 변수입니다 |
| `--check` | — | 검증만 하고 바인드나 연결 없이 종료합니다 |
| `service` 하위 명령어 | — | 시스템 서비스 관리 |
| `update` 하위 명령어 | — | 독립 실행형 바이너리를 검증하고 설치합니다 |

`felucca-server`가 추가로 받는 인수입니다.

| 인수 | 환경 변수 | 설명 |
| --- | --- | --- |
| `--runtime-config <PATH>` | `FELUCCA_RUNTIME_CONFIG` | 런타임 설정 스냅샷의 경로입니다 |
| `--fingerprint` | — | `bridge.tls` 인증서의 SHA-256을 출력합니다 |
| `--hash-password` | — | stdin에서 비밀번호 한 줄을 읽어 PBKDF2 해시를 출력합니다. 최소 12 바이트입니다 |
| `--generate-token` | — | Bearer 토큰과 그 `token_hash`를 출력합니다 |

`felucca-agent`가 추가로 받는 인수입니다.

| 인수 | 기본값 | 설명 |
| --- | --- | --- |
| `--launch` | — | `--config`의 별칭입니다 |
| `--log-level <LVL>` | `info` | trace/debug/info/warn/error |
| `--log-file <PATH>` | — | 추가 기록하고 stderr에도 출력합니다. 상위 디렉터리가 존재해야 합니다 |
| `--log-json` | — | 구조화된 JSON 이벤트를 출력합니다 |
| `--log-max-bytes <N>` | 10 MiB | `--log-file`이 필요합니다 |
| `--log-backups <N>` | 5 | `--log-file`이 필요합니다 |
| `--log-rotate-interval <D>` | — | 예를 들어 `24h`. `--log-file`이 필요합니다 |

`service` 하위 명령어입니다.

| 인수 | 설명 |
| --- | --- |
| `--name <NAME>` | 서비스 인스턴스 이름입니다 |
| `--scope <user\|system>` | 기본값은 `user`입니다. `system`은 Windows에서만 쓸 수 있습니다 |
| `--dry-run` | 실행할 작업만 출력합니다 |
| `render` | 서비스 정의를 생성합니다. `--platform systemd\|launchd\|windows` |
| `install` / `start` / `stop` / `restart` / `status` / `uninstall` | 생명주기 관리 |
| `--workdir <DIR>` | 기본값은 설정이 있는 디렉터리입니다 |
| `--binary <PATH>` | 설치할 프로그램을 선택합니다 |
| `-- <ARGS>` | 추가 데몬 인수입니다 |

`update` 하위 명령어입니다.

| 인수 | 설명 |
| --- | --- |
| `--source <PATH_OR_URL>` | 독립 실행형 바이너리 또는 tar.gz/zip 아카이브 |
| `--sha256 <HEX>` | 16진수 64자리. 신뢰의 기준입니다 |
| `--check-only` | 교체하지 않고 검증만 합니다 |
| `--expect-version <V>` | 선택적인 버전 단정입니다 |
| `--token-env <NAME>` | 비공개 내려받기 Bearer 토큰을 담는 변수입니다 |

## 설정 구조

| 파일 | 최상위 내용 |
| --- | --- |
| server | `allowed_ports`, `max_tunnels`, `max_connections`, `[bridge]`, `[http]`, `[admin]`, `[state]`, `[geo]`, `[ntp]`, `[log]`, `[p2p]`, `[node]`, `[user.*]`, `[client.*]`, `[local.*]`, `[share.*]`, `[managed.*]` |
| agent | `server`, `key_id`, `vkey`, `transport`, `reconnect_min/max`, `dial_timeout`, `max_connections`, `[trust]`, `[network]`, `[mux]`, `[kcp]`, `[ntp]`, `[remote]`, `[geo]`, `[[tunnel]]`, `[[mapping]]`, `[[visitor]]`, `[p2p]`, `[log]` (에이전트는 명령줄 인수를 사용합니다) |

`allowed_ports`는 첫 번째 TOML 테이블 헤더보다 앞에 와야 합니다. 모든 테이블은
`deny_unknown_fields`를 활성화합니다.

## 터널 필드 빠른 참조

| 필드 | 적용 모드 | 설명 |
| --- | --- | --- |
| `id` | 전체 | 안정적인 ID입니다. 트래픽 스냅샷을 활성화하면 고정하세요 |
| `mode` | 전체 | tcp/udp/http/socks5/httpproxy/mixedproxy/file/transparent |
| `port` | 비공개를 제외한 전체 | 서버 포트입니다. `http`는 HTTP 리스너와 일치해야 합니다 |
| `bind` | tcp/udp/proxy/file | IP 주소만 지정합니다 |
| `public` | 전체 | `false`는 비공개 공유를 뜻합니다 |
| `hosts` | http, file | 도메인 목록입니다. `*.example.com`를 지원합니다 |
| `balance` | 고정 백엔드 | round_robin/weighted/least_connections/source_hash/failover |
| `backend` | 고정 백엔드 | `addr`, `weight`, `tls`, `trust` |
| `health` | 고정 백엔드 | `type`, `path`, `interval`, `timeout`, `failures_to_evict`, `successes_to_restore`, `expect_status` |
| `acl_mode`/`acl_rules` | 전체 | 출발지 ACL. off/allow/deny |
| `target_policy` | 고정 백엔드와 포워드 프록시 | `allow`/`deny`/`ports` |
| `options.auth.users` | 포워드 프록시, HTTP | 계정 테이블입니다 |
| `options.http.*` | http | 경로, 재작성, 헤더, 리다이렉트, CORS, 오류 페이지, tls_mode, backend_http2 |
| `options.payload.*` | 전체 | `compress`, `encrypt` |
| `options.limits.*` | 전체 | `max_connections`, `rate_limit`, `quota` |
| `options.proxy_protocol` | tcp/udp/http/tls | off/v1/v2 |
| `options.proxy_accept` | 프록시 진입점 | PROXY 헤더를 받아들입니다 |
| `file` | file | `root`, `prefix`, `read_only`, `auto_index`, `auth` |

## 관리 API 요약

인증은 Origin과 CSRF가 있는 쿠키 또는 `Authorization: Bearer <token>`입니다. 쓰기에는
`If-Match: "<revision>"`과 고유한 `Idempotency-Key`가 필요합니다. 요청 본문 제한은
2 MiB입니다.

| 묶음 | 경로 |
| --- | --- |
| 세션 | `/api/login`, `/api/session`, `/api/logout` |
| 리소스 | `/api/users`, `/api/clients`, `/api/locals`, `/api/shares`, `/api/accounts`. 각각 `<id>`도 있습니다 |
| 중앙 집중 설정 | `/api/managed/<client>`, `/api/managed/<client>/tunnels/<id>` |
| 서버 로컬 터널 | `/api/locals/<group>/tunnels/<id>` |
| 전체 설정 | `/api/config`, `/api/config?secrets=1`, `/api/validate`, `/api/batch` |
| 실시간 | `/api/events`(SSE), `/metrics`, `/api/metrics`, `/api/profile` |
| 플랫폼 | `/api/node/*`, `/api/platforms/<name>`, `/api/webhooks/<name>`, `/api/ws`, `/api/realtime/subscriptions` |

전체 필드는 [관리 UI와 중앙 집중 설정](management.md)과
[외부 플랫폼 연동](platform.md)을 보세요.

## 예제 설정 목록

| 파일 | 내용 |
| --- | --- |
| [felucca-server.toml](../../../examples/felucca-server.toml) | 최소 서버 예제 |
| [felucca-agent.toml](../../../examples/felucca-agent.toml) | 최소 에이전트 예제 |
| [features-server.toml](../../../examples/features-server.toml) | QUIC 브리지 + HTTP/HTTPS + 인증서 |
| [features-agent.toml](../../../examples/features-agent.toml) | 모든 프록시 모드, 로드 밸런싱, 헬스 체크 |
| [shared-server.toml](../../../examples/shared-server.toml) | TCP/UDP 포트 공유 + HTTP/3 |
| [shared-agent.toml](../../../examples/shared-agent.toml) | 에이전트 쪽과 대량 포트 매핑 |
| [acme-server.toml](../../../examples/acme-server.toml) | ACME 서버 |
| [zerossl-server.toml](../../../examples/zerossl-server.toml) | ZeroSSL과 TLS-ALPN-01 |
| [policy-server.toml](../../../examples/policy-server.toml) / [policy-agent.toml](../../../examples/policy-agent.toml) | 계층형 ACL, 할당량, 영속화 |
| [file-server.toml](../../../examples/file-server.toml) / [file-agent.toml](../../../examples/file-agent.toml) | 파일 서비스 |
| [network-agent.toml](../../../examples/network-agent.toml) | 업스트림 프록시, 사용자 지정 DNS, NTP |
| [payload-agent.toml](../../../examples/payload-agent.toml) | Snappy와 독립 스트림 암호화 |
| [kcp-server.toml](../../../examples/kcp-server.toml) / [kcp-agent.toml](../../../examples/kcp-agent.toml) | KCP 브리지 |
| [local-server.toml](../../../examples/local-server.toml) | 서버 로컬 대상과 중첩 전달 |
| [visitor-server.toml](../../../examples/visitor-server.toml) / [private-agent.toml](../../../examples/private-agent.toml) / [visitor-agent.toml](../../../examples/visitor-agent.toml) | 비공개 제공자와 방문자 |
| [p2p-server.toml](../../../examples/p2p-server.toml) / [p2p-provider.toml](../../../examples/p2p-provider.toml) / [p2p-visitor.toml](../../../examples/p2p-visitor.toml) | P2P와 대체 경로 |
| [admin-server.toml](../../../examples/admin-server.toml) | 관리 서버 |
| [managed-agent.toml](../../../examples/managed-agent.toml) | 중앙 관리 에이전트 |
| [node-server.toml](../../../examples/node-server.toml) | 외부 플랫폼과 노드 관리 |

설정 경로는 프로세스 작업 디렉터리를 기준으로 합니다. 예제 26개는 모두 설정 검증을
통과합니다.

## 수치 제한

| 항목 | 제한 |
| --- | --- |
| 설정 하나 | 4 MiB |
| 런타임 설정 스냅샷 | 16 MiB |
| 에이전트 인스턴스 | 32 |
| 중첩 라우팅 홉 | 16 |
| 배치 작업 | 항목 128개 |
| 트래픽 가져오기 | 요청당 항목 128개 |
| 보관하는 멱등 기록 | 256(콜백 작업과 트래픽 가져오기는 각각 256) |
| 콜백 큐 | 대상별 `queue_max`, 노드 전체 4096 |
| 직접 관리 WebSocket | 16 |
| WS RPC 동시성 | 연결당 2 / 노드 전체 16 |
| 실시간 구독 | 연결당 항목 32개, 항목당 32 KiB |
| HTTP 백엔드 연결 풀 | 오리진당 유휴 8개, 유휴 30초, 바깥 맵 항목 256개 |
| HTTP/3 | 연결당 스트림 128개, 스트림당 1 MiB / 연결 창 16 MiB, 헤더 32 KiB |
| UDP | 출발지당 65507 바이트. 터널당 연관 관계 256개 |
| SOCKS5 UDP | 세션당 목적지 64개, 각각 큐에 8개 패킷 |
| 오류 페이지 | 400..599, 페이지당 64 KiB |
| HTTP 도메인 매칭 | 정확한 도메인이 와일드카드보다 우선합니다 |

## 환경과 빌드

| 항목 | 값 |
| --- | --- |
| 최소 Rust | 1.88 |
| 검증 환경 | macOS Intel, Rust/Cargo 1.98.1 |
| 워크스페이스 테스트 | 581 통과 / 0 실패 / 0 무시 (2026-09-17) |
| 바이너리 | `target/release/felucca-server`, `target/release/felucca-agent` |

```sh
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
sh scripts/cargo-external.sh test  --workspace --locked
sh scripts/cargo-external.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo-external.sh fmt --all --check
```

같은 대상 디렉터리에는 한 번에 하나의 Cargo 명령만 실행하세요. 소스 코드와 포함된
페이지를 편집하기 전에 빌드, 테스트, rustdoc를 끝내세요. 테스트에는 로컬 TCP/UDP 포트의
리스닝 권한이 필요합니다.

## 용어집

| 용어 | 의미 |
| --- | --- |
| agent | 사설 네트워크 안에서 실행되고 외부로 연결하는 `felucca-agent` 프로세스입니다 |
| bridge | 서버와 에이전트 사이의 인증된 장기 연결입니다 |
| client / `[client.<id>]` | 서버가 하나의 에이전트를 위해 두는 신원 테이블입니다. `key_id`가 테이블 이름입니다 |
| key_id | 서버가 보는 에이전트의 이름입니다 |
| vkey | 에이전트와 서버가 공유하는 브리지 시크릿입니다 |
| transport | 브리지 전송 방식입니다. tcp/tls/ws/wss/quic/kcp |
| trust | 에이전트가 서버 인증서를 검증하는 방식입니다. webpki/pinned/insecure |
| tunnel | 공개 진입점 하나를 백엔드 집합에 연결하는 매핑입니다 |
| backend | 에이전트가 실제로 연결하는 대상입니다 |
| mapping | 대량 일대일 포트 매핑입니다 |
| local group | 에이전트가 필요 없는 서버 자체 서비스의 묶음입니다 |
| share | 서버가 손님에게 터널 하나의 사용을 허가하는 것입니다 |
| visitor | 공유 시크릿을 갖고 로컬에서 리스닝하는 에이전트입니다 |
| provider | 비공개 공유나 P2P 구성에서 서비스를 제공하는 쪽입니다 |
| managed | 서버가 에이전트에 배포하는 설정입니다 |
| mux | TCP 계열 전송 위의 범위가 제한된 멀티플렉싱입니다 |
| FEC | KCP 포워드 오류 정정입니다. 기본적으로 데이터 10 + 패리티 3입니다 |
| quota / rate_limit | 트래픽 할당량과 초당 바이트 속도 제한입니다 |
| snapshot | 트래픽이나 런타임 설정을 한 번에 영속화한 것입니다 |

## 관련 문서

- [아키텍처](../../architecture.md) — 크레이트 소유권 경계
- [설정과 배포](../../configuration.md) — 필드별 상세 설명
- [P2P](../../p2p.md), [비공개 터널과 공유 접근](../../visitors.md),
  [관리와 중앙 집중 설정](../../management.md),
  [노드와 외부 플랫폼](../../node.md), [운영과 배포](../../operations.md),
  [투명 TCP](../../transparent.md)
