# 포워드 프록시

Felucca는 NAT 뒤에서 SOCKS5 또는 HTTP 프록시 진입점을 노출할 수 있습니다. 이
페이지에서는 세 가지 모드, 계정 인증, 대상 ACL, UDP 동작을 설명합니다.

## 세 가지 모드

```toml
[[tunnel]]
mode = "socks5"
port = 9080
bind = "127.0.0.1"
acl_mode = "allow"
acl_rules = "127.0.0.0/8, ::1/128"

[[tunnel]]
mode = "httpproxy"
port = 9081
bind = "127.0.0.1"

[[tunnel]]
mode = "mixedproxy"
port = 9082
bind = "127.0.0.1"
```

| `mode` | 지원 범위 |
| --- | --- |
| `socks5` | CONNECT와 UDP ASSOCIATE. IPv4/IPv6. 에이전트 쪽에서 DNS를 조회합니다. BIND는 "unsupported command"를 반환합니다 |
| `httpproxy` | 절대 URL 요청과 CONNECT |
| `mixedproxy` | 요청의 시작 부분으로 SOCKS5와 HTTP를 구분합니다 |

포워드 프록시 예제는 `127.0.0.1`에만 바인딩합니다. 원격 접근이 필요하다면 출발지
허용 목록도 **반드시** 설정하세요. 그렇지 않으면 그 포트에 도달할 수 있는 누구나 여러분의
이그레스를 사용할 수 있습니다.

## 계정 인증

인증을 활성화하려면 현재 터널 아래에 계정 테이블을 추가합니다.

```toml
[tunnel.options.auth.users]
alice = "replace-with-a-password"
bob = "another-password"
```

| 항목 | 인증 방식 | 실패 응답 |
| --- | --- | --- |
| SOCKS5 | 사용자명과 비밀번호 방식이며, 익명으로의 **다운그레이드는 없습니다** | 인증 실패 |
| HTTP 포워드 프록시 / CONNECT | `Proxy-Authorization: Basic` | 407 |
| HTTP 리버스 프록시 사이트 | `Authorization: Basic` | 401 |

계정 테이블이 없으면 익명 접근을 허용합니다. 검증된 프록시 및 사이트 자격 증명은
요청이 백엔드로 전달되기 전에 제거됩니다. 설정과 등록 메시지에는 계정 비밀번호가
포함되므로, 브리지는 기본값인 TLS, WSS 또는 QUIC을 사용해야 합니다. `tcp`와 `ws`는
의도적으로 선택하는 평문 전송입니다.

파일 터널은 `tunnel.file.auth`를 사용하며, 이는 `tunnel.options.auth`와 함께 쓸 수
없습니다.

## 대상 정책

고정 백엔드를 쓰는 터널은 `target_policy`로 실제 대상을 제한합니다. 포워드 프록시는
특히 이것이 필요합니다.

```toml
[tunnel.target_policy]
allow = ["domain:api.example.com", "suffix:example.net", 'regex:^db[0-9]+\.internal$']
deny = ["127.0.0.0/8", "::1", "169.254.0.0/16"]
ports = [80, 443]
```

`allow`가 비어 있으면 어떤 대상을 허용할지 제한하지 않으며, `deny`가 항상 우선하고,
`ports`가 비어 있으면 0이 아닌 모든 포트를 허용합니다. 지원하는 규칙은 IP/CIDR,
정확한 도메인 이름을 뜻하는 `domain:`, 접미사를 뜻하는 `suffix:`(루트 도메인과 임의
깊이의 하위 도메인 포함), 정규 표현식을 뜻하는 `regex:`, 모든 대상을 뜻하는 `*`
입니다. 이름은 대소문자를 구분하지 않으며, 끝의 점은 정규화합니다.

DNS를 먼저 조회한 뒤, **원래 이름과 실제 IP**를 하나씩 검사합니다. 연결은 검사를 통과한
주소만 사용하고 다시 조회하지 않으므로, 허용된 도메인이 거부된 IP를 우회할 수 없습니다.
GeoIP와 GeoSite도 지원합니다.
[접근 제어, 할당량과 스트림 부호화](security.md)를 보세요.

## SOCKS5 UDP ASSOCIATE

- 제어 연결의 상대 IP만 받아들입니다. 요청이 포트를 지정하면 그 포트로 고정하고, 그렇지
  않으면 첫 유효 UDP 패킷의 출발지 포트에 연결합니다.
- 제어 TCP 연결을 닫으면 모든 대상 연관 관계가 삭제됩니다.
- 세션당 최대 64개의 대상을 허용하며, 각 대상은 8개 패킷을 큐에 담고, 유휴 항목은
  60초 뒤에 회수됩니다.
- 0이 아닌 FRAG는 버리며, SOCKS 프래그먼트 재조립은 구현되어 있지 않습니다.

## 속도와 할당량

- `tunnel.options.limits.rate_limit`은 이 에이전트의 모든 터널과 양방향이 공유하는
  초당 바이트 제한이며, 1초 분량의 초기 버스트를 허용합니다.
- `quota.flow_limit`은 양방향을 모두 계산하며, `quota.expires_at`는 Unix 초를
  사용합니다.
- 할당량은 주기적으로 검사하며 전송 중인 연결을 중단시키므로, 동시에 진행 중인
  트래픽은 할당량을 초과할 수 있습니다. 이 카운트는 엄격한 과금 용도가 아닙니다.

## 다음 단계

- [터널과 포트 매핑](tunnels.md)
- [비공개 공유와 방문자](private-shares.md) — 공개 포트가 없는 SOCKS 방문자
- [P2P 직접 연결과 릴레이 대체](p2p.md)
