# felucca-server 모듈

이 페이지에서는 `felucca-server`의 배포와 설정을 설명합니다. 브리지 리스너, 클라이언트
테이블, HTTP 인그레스, 포트 인가, 관리 리스너, 서버 로컬 서비스, 외부 플랫폼 스위치를
다룹니다.

## 최소 구성

```toml
[bridge]
bind = "0.0.0.0:14028"
ws_path = "/ws"
tls = { cert = "conf/bridge.pem", key = "conf/bridge.key" }

[client.agent-1]
vkey = "change-me-to-a-long-random-string"

[log]
level = "info"
```

`bridge.tls`를 생략하고 런타임 설정 저장을 활성화하면(`--init` 또는 관리 콘솔 포함) 서버는 스냅샷 옆의 `<snapshot>.identity.json`에 신원 정보를 저장합니다. 재시작과 백업 시 보존하세요. 런타임 저장이 없으면 임시 신원을 사용합니다. `--fingerprint`는 명시한 PEM 파일을 읽으며 자동 생성 지문은 실행 중인 콘솔과 시작 로그에서 확인할 수 있습니다.

```sh
target/release/felucca-server --config conf/server.toml --check
target/release/felucca-server --config conf/server.toml
```

## 브리지 리스너

| 필드 | 기본값 | 설명 |
| --- | --- | --- |
| `bridge.bind` | `0.0.0.0:14028` | TCP, TLS, WS, WSS가 하나의 소켓을 공유하며 핸드셰이크 특성으로 구분합니다 |
| `bridge.quic_bind` | 없음 | 독립적인 UDP 주소입니다. TCP와 같은 포트 번호를 쓸 수 있습니다 |
| `bridge.kcp_bind` | 없음 | KCP UDP 리스너입니다. QUIC/HTTP3과 UDP 포트를 공유할 수 없습니다 |
| `bridge.ws_path` | `/ws` | WS/WSS 경로입니다. 양쪽이 일치해야 하며, 다른 경로의 업그레이드는 거부합니다 |
| `bridge.tls` | 없음 | `{ cert, key }`. 생략하면 임시 인증서를 생성합니다 |
| `bridge.cert_reload_interval` | `30s` | PEM 재로드 간격입니다. 재로드는 기존 연결을 끊지 않습니다 |
| `bridge.proxy_accept` | 꺼짐 | 신뢰할 수 있는 로드 밸런서의 PROXY 헤더를 받아들입니다 |
| `[bridge.mux]` | 아래 참조 | mux 창과 예산입니다 |
| `[bridge.kcp]` | 아래 참조 | KCP 파라미터입니다 |

`[bridge.mux]`는 `initial_stream_window`, `max_stream_window`, `max_session_window`,
`max_send_queue`, `max_control_queue`, `max_streams`, `accept_backlog`,
`open_timeout`을 노출합니다. 에이전트 쪽에도 같은 필드를 갖는 `[mux]` 섹션이 있습니다.
네이티브 QUIC 스트림은 이 설정을 사용하지 않습니다.

`[bridge.kcp]`는 `mtu`, `send_window`, `receive_window`, `interval_ms`, `nodelay`,
`fast_resend`, `congestion_control`, `session_timeout`, `data_shards`,
`parity_shards`, `max_sessions`를 받습니다. 기본 FEC 설정은 데이터 10개에 패리티
3개이므로, 한 그룹의 13개 패킷 중 3개를 잃어도 복구할 수 있습니다. 양쪽은 같은
MTU와 샤드 수를 사용해야 합니다.

## 클라이언트 테이블

각 에이전트는 `[client.<key_id>]` 테이블을 하나씩 가집니다. `key_id`는 서버가 보는
에이전트의 이름이며, 에이전트 자신의 `key_id`는 테이블 이름과 일치해야 합니다.

| 필드 | 설명 |
| --- | --- |
| `vkey` | 필수입니다. 상호 HMAC 인증에 쓰는 공유 시크릿입니다. 길이와 무작위성은 사용자가 책임집니다 |
| `remark` | 메모입니다. 관리 페이지에 표시됩니다 |
| `user` | `[user.<name>]` 테이블과 연결하며, 계층형 할당량, ACL, 테넌트 권한에 사용됩니다 |
| `enabled` | `false`이면 이 클라이언트를 영구히 비활성화하고 연결을 끊습니다 |
| `allow_config_push` | 이 에이전트가 터널을 등록할 수 있는지입니다. `[[managed.<id>.tunnel]]` 설정을 배포하려면 `true`로 설정하세요 |
| `max_tunnels` | 이 에이전트의 터널 상한입니다 |
| `max_connections` | 이 에이전트의 연결 상한입니다 |
| `rate_limit` | 모든 터널과 양방향이 공유하는 초당 바이트 제한입니다 |
| `acl_mode` / `acl_rules` | 이 클라이언트의 공개 진입점에 접근하기 위한 출발지 ACL입니다 |
| `quota` | `{ flow_limit, expires_at }`. 단위는 바이트와 Unix 초입니다 |

같은 `key_id`로 새 연결이 들어오면 기존 연결을 대체합니다. 기존 리스너는 먼저 멈추고
해제된 뒤 포트가 다시 바인드됩니다. 트래픽 카운터와 대역폭 제한기는 같은 프로세스 안에서
재연결해도 유지됩니다.

## 포트 인가

`allowed_ports`는 최상위 필드이며, **반드시 첫 번째 TOML 테이블 헤더보다 앞에 와야
합니다**.

```toml
allowed_ports = [8080, { from = 9000, to = 9100 }]
```

빈 목록은 1024보다 큰 모든 TCP/UDP 포트를 허용합니다. `max_tunnels`와
`max_connections`는 에이전트별로 계산되는 최상위 전역 제한입니다.

## HTTP 인그레스

```toml
[http]
bind = "0.0.0.0:8080"
tls_bind = "0.0.0.0:8443"      # set to "" to disable HTTPS
http3_bind = "0.0.0.0:8443"    # optional, HTTP/3 UDP ingress
forwarded_headers = true
response_timeout = "100s"
cert_reload_interval = "30s"

[http.certificates."app.example.com"]
cert = "conf/app.pem"
key = "conf/app.key"

[http.default_tls]
cert = "conf/default.pem"
key = "conf/default.key"

[http.proxy_accept]
mode = "required"
trusted = ["192.0.2.10", "2001:db8:1::/64"]
```

자세한 사용법은 [HTTP/HTTPS 리버스 프록시](http-services.md)와
[인증서, SNI와 ACME](certificates.md)를 보세요.

## 기타 최상위 테이블

| 테이블 | 목적 | 관련 문서 |
| --- | --- | --- |
| `[admin]` / `[admin.account.<name>]` | 관리 리스너, 계정과 역할 | [관리 UI와 중앙 집중 설정](management.md) |
| `[state]` | 트래픽 스냅샷 영속화 | [접근 제어, 할당량과 스트림 부호화](security.md) |
| `[user.<name>]` / `[user.<name>.limits]` | 사용자별 할당량과 집계 | [접근 제어, 할당량과 스트림 부호화](security.md) |
| `[local.<name>]` | 서버 로컬 서비스 묶음 | [서버 로컬 대상과 중첩 라우트](local-services.md) |
| `[share.<name>]` | 비공개 공유 인가 | [비공개 공유와 방문자](private-shares.md) |
| `[managed.<client>]` | 중앙 집중 설정 배포 | [관리 UI와 중앙 집중 설정](management.md) |
| `[node]` / `[node.platform.<name>]` / `[node.webhook.<name>]` | 외부 플랫폼 | [외부 플랫폼 연동](platform.md) |
| `[p2p]` | P2P 관찰 포트 | [P2P 직접 연결과 릴레이 대체](p2p.md) |
| `[geo]` | GeoIP/GeoSite 데이터 파일 | [접근 제어, 할당량과 스트림 부호화](security.md) |
| `[ntp]` | 브리지 인증 시간 보정 | [felucca-agent 모듈](agent.md) |
| `[log]` / `[log.rotation]` | 로깅과 로테이션 | [서비스, 업데이트, 로깅과 모니터링](operations.md) |
| `allowed_ports` | 포트 인가 | 현재 페이지 |

## 유용한 명령어

| 명령어 | 목적 |
| --- | --- |
| `--check` | 설정을 검증하고 포트를 바인드하지 않고 종료합니다 |
| `--fingerprint` | `bridge.tls` 인증서의 SHA-256을 출력합니다. 고정 PEM이 설정되어 있어야 합니다 |
| `--hash-password` | stdin에서 비밀번호 한 줄을 읽어 관리 비밀번호 해시를 출력합니다. 비밀번호는 최소 12 바이트입니다 |
| `--generate-token` | 관리 Bearer 토큰과 그 저장 해시를 출력합니다 |
| `--runtime-config PATH` | 런타임 설정 스냅샷을 활성화합니다 |
| `--build-info` | 프로그램 이름, 버전, 프로토콜, 빌드 대상을 출력합니다 |

## 생명주기

- `SIGINT`와 `SIGTERM`은 리스너와 현재 연결을 닫은 뒤 종료합니다.
- 시작 파일을 바꾸려면 프로세스를 재시작해야 합니다. 관리 리스너를 통해 바꾼 내용은
  런타임 설정 스냅샷 트랜잭션을 거치며, 포워딩된 연결은 끊기지만 이후 에이전트가
  재연결합니다.
- 관리 바인딩, 인증서 파일 경로, 공개 출처, 상태 경로를 바꾸는 것은 **프로세스
  재시작이 필요합니다**.
- 트래픽 카운터는 에이전트가 재연결해도 유지됩니다. 서버 재시작 후에도 유지되려면
  `[state]`를 설정해야 합니다.

## 다음 단계

- [felucca-agent 모듈](agent.md) — 내부 서비스를 올립니다
- [터널과 포트 매핑](tunnels.md)
- [HTTP/HTTPS 리버스 프록시](http-services.md)
- [서비스, 업데이트, 로깅과 모니터링](operations.md) — systemd / launchd / Windows 서비스
