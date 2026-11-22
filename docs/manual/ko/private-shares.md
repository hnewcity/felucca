# 비공개 공유와 방문자

비공개 터널은 서버에 등록하지만 **공개 포트를 차지하지 않습니다**. 방문자인 다른
에이전트가 자신의 머신에 리스너를 열고, 별도의 공유 시크릿으로 인증되고 암호화된
브리지를 통해 그 터널에 접근합니다. 인터넷 전체가 아니라 특정 노드에만 내부 서비스를
노출할 때 사용합니다.

## 세 가지 설정

제공자 에이전트, 방문자 에이전트, 서버가 각각 한 곳씩 바뀝니다.

### 제공자: 터널을 비공개로 만들기

```toml
[[tunnel]]
id = 1
mode = "tcp"
public = false                 # Omit port and bind
backend = [{ addr = "127.0.0.1:3000" }]

[[tunnel]]
id = 3
mode = "socks5"
public = false
[tunnel.target_policy]
allow = ["127.0.0.0/8", "::1", "10.0.0.0/8"]
```

TCP, UDP, 파일, 프록시 진입점을 지원합니다. **HTTP 도메인 라우팅은 HTTP 리스너에
의존하므로 비공개로 만들 수 없습니다**. 비공개 TCP 터널이라면 임의의 HTTP나 HTTPS
백엔드를 전달할 수 있습니다. `port = 0`을 명시하는 것은 비공개 터널과 같습니다.

### 서버: 공유 선언

```toml
[share.office-web]
client = "office"              # Node name; for a server-local group use client = "local.<group-name>"
tunnel = 1                     # Stable tunnel ID
secret = "replace-this-shared-secret"
allow_clients = ["laptop"]     # An empty list allows every authenticated node that holds the share secret
direct = true                  # Allow P2P direct connection attempts
```

`secret`는 16–4096 바이트이며, 공유마다 서로 다른 값을 써야 합니다.
`acl_mode`/`acl_rules`는 방문자 브리지 연결의 출발지 주소를 검사합니다. 공유 시크릿만으로
익명 클라이언트로 연결할 수는 **없습니다**. 방문자에게는 일반 노드 신원과 vkey가 여전히
필요합니다. 서버는 `client.NAME.enabled = false`로 노드를 영구히 비활성화할 수
있습니다.

### 방문자: 로컬에서 리스닝

```toml
[[visitor]]
name = "office-web"
share = "office-web"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:18080"
mode = "tcp"                   # tcp | udp | socks5
max_connections = 64
udp_idle = "60s"
```

| 필드 | 설명 |
| --- | --- |
| `name` | 이 에이전트 안에서 고유해야 합니다 |
| `share` | 서버의 공유 이름입니다 |
| `mode` | `tcp`, `udp`, `socks5` 중 하나입니다 |
| `bind` | 고정 포트가 필요합니다. 시작할 때 바인딩하며, 포트가 사용 중이면 즉시 실패합니다 |
| `max_connections` | 1–4096, 기본값 64. TCP 연결 또는 UDP 출발지 세션을 셉니다 |
| `udp_idle` | 1초–1시간, 기본값 60초. 양방향 중 어느 쪽에서든 활동이 있으면 세션이 연장됩니다 |
| `acl_mode` / `acl_rules` | 방문자 리스너로 들어오는 연결의 출발지 주소를 검사합니다 |
| `target` | 동적 공유에서만 사용합니다. 아래를 보세요 |

모든 방문자는 에이전트의 연결 제한과 8 MiB UDP 큐잉 예산도 함께 공유합니다. 방문자는
인증서가 검증되는 TLS, WSS 또는 QUIC만 받아들입니다.

서버는 공유 ACL, 방문자 ACL, 제공자의 라우팅 ACL을 **방문자 브리지의 실제 출발지
주소**에 적용합니다. LAN에서 방문자 포트에 도달한 원시 주소는 신뢰할 수 있는 출발지로
서버에 전달되지 않습니다.

## 결과

위 설정대로라면 방문자 머신의 `127.0.0.1:18080`에 접근하면 office 에이전트의
`127.0.0.1:3000`에 도달하고, UDP `127.0.0.1:15353`은 office의 `127.0.0.1:5353`에
대응합니다.

연결과 원시 트래픽은 방문자와 제공자, 그리고 두 사용자와 터널 리소스에 모두
계산됩니다. 한 사용자가 양쪽을 모두 소유하면 두 사용량을 합합니다. UDP 길이 헤더와
공유 인증 메시지는 비즈니스 트래픽으로 계산하지 않습니다.

## 동적 대상

서버 공유가 `dynamic = true`이면 방문자가 대상을 지정할 수 있습니다.

```toml
[[visitor]]
name = "office-proxy"
share = "office-proxy"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:11080"
mode = "socks5"
target = "internal.example:443"
```

- 제공자는 SOCKS5, HTTP 포워드 프록시, 혼합 프록시 터널이어야 합니다. UDP 대상은
  SOCKS5나 혼합 프록시가 필요합니다.
- 제공자 쪽의 실제 조회된 IP와 대상 정책은 여전히 검사됩니다.
- 동적 공유는 프록시 진입점에 **별도의 프록시 계정이 없어야** 합니다. 계정이 설정되어
  있으면 거부하며, 공유 시크릿으로 프록시 자체의 인증을 우회할 수 없게 하기
  위함입니다.
- `target`이 없으면 방문자는 전체 서비스 진입점에 연결하며, 파일과 프록시 계정은 여전히
  정상적으로 인증합니다.
- TCP 방문자는 SOCKS CONNECT나 HTTP 프록시 세션을 통과시킬 수 있습니다. SOCKS
  UDP ASSOCIATE가 반환한 서버 UDP 주소는 방문자에서 로컬 주소로 자동 매핑되지
  않습니다.

완전한 SOCKS5 접근이 필요하면 방문자를 `mode = "socks5"`로 설정하고 `target` 없이 동적
공유를 참조하세요. SOCKS CONNECT와 UDP ASSOCIATE는 방문자 쪽에서 해석되고, 모든
목적지가 공유 인증과 제공자의 대상 정책을 통과합니다. `[visitor.auth.users]`는 로컬
SOCKS 계정을 설정하며, 계정이 있으면 방문자는 익명 인증으로 다운그레이드하지 않습니다.
`max_connections`는 SOCKS 제어 연결과 UDP 대상 연관 관계를 함께 세므로, UDP를 쓸 때는
최소 2 이상으로 설정하세요.

## 관리와 런타임

- 관리의 "공유 접근" 페이지에서 공유를 생성, 편집, 삭제합니다. 시크릿은 기본적으로
  가려지며, 편집할 때 필드를 비워 두면 현재 값이 유지됩니다.
- operator는 자신이 소유한 사용자의 제공자 측 공유만 관리할 수 있으며, 읽기 전용 계정은
  변경할 수 없습니다.
- API는 `/api/shares`와 `/api/shares/NAME`입니다. 쓰기는 설정 버전과 멱등 요청 키를
  유지합니다.
- 브리지가 끊긴 뒤에도 로컬 리스너는 유지되고, 기존 세션은 끝까지 처리되며, 새 요청은
  재연결 뒤에 재개됩니다.
- 공유 변경은 일반 Daemon 재로드를 통해 적용되며, 기존 연결은 끊깁니다.
- 이 경로는 서버 릴레이를 지납니다. 직접 링크는
  [P2P 직접 연결과 릴레이 대체](p2p.md)를 보세요.

## 검증 체크리스트

1. 서버의 `--check`와 두 에이전트의 `--check`가 통과합니다.
2. 방문자의 `127.0.0.1:18080`가 제공자의 `127.0.0.1:3000` 콘텐츠를 반환합니다.
3. `secret`이 틀린 방문자 연결은 거부됩니다.
4. `allow_clients`에 없는 노드는 거부됩니다.
5. 서버에 새 공개 리스너가 생기지 않았습니다(`ss -ltnp`로 비교).

## 다음 단계

- [P2P 직접 연결과 릴레이 대체](p2p.md)
- [felucca-agent 모듈](agent.md)
- [관리 UI와 중앙 집중 설정](management.md)
- 예제: [visitor-server.toml](../../../examples/visitor-server.toml) ·
  [private-agent.toml](../../../examples/private-agent.toml) ·
  [visitor-agent.toml](../../../examples/visitor-agent.toml)
