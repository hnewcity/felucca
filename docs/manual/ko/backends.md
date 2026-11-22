# 백엔드, 밸런싱과 헬스 체크

이 페이지에서는 백엔드 설정, 다섯 가지 밸런싱 전략, 헬스 체크가 백엔드를 축출하고
복구하는 방식을 설명합니다.

## 백엔드

```toml
[[tunnel]]
mode = "tcp"
port = 9000
bind = "127.0.0.1"
balance = "round_robin"

[[tunnel.backend]]
addr = "127.0.0.1:3000"
weight = 2

[[tunnel.backend]]
addr = "backend.internal:3001"
weight = 1
tls = true
[tunnel.backend.trust]
mode = "pinned"
fingerprint = "<backend SHA-256>"
```

| 필드 | 설명 |
| --- | --- |
| `addr` | 필수입니다. 포트를 붙인 호스트명 또는 IP 주소 |
| `weight` | `weighted` 전략에서 쓰는 양의 정수 가중치 |
| `tls` | 백엔드로의 TLS를 활성화합니다. 기본적으로 WebPKI 검증을 사용하며, SNI는 주소의 호스트명에서 가져옵니다 |
| `trust` | 자체 서명 백엔드에는 `mode = "pinned"`와 `fingerprint`를 함께 사용합니다 |

`balance`은 `[[tunnel]]`에 속하는 값이며, `[[tunnel.backend]]` 뒤에 쓰면 **안 됩니다**.
헬스 체크와 데이터 연결은 같은 TLS 설정을 사용합니다.

## 밸런싱 전략

| 전략 | 백엔드를 고르는 방식 |
| --- | --- |
| `round_robin` | 정상 백엔드를 순환합니다 |
| `weighted` | 양의 정수 `weight`에 따라 연결을 분배합니다 |
| `least_connections` | 현재 열린 연결 수가 가장 적은 백엔드를 고릅니다 |
| `source_hash` | 원래 사용자 IP 주소에서 안정적으로 백엔드를 고릅니다 |
| `failover` | 설정 순서상 첫 번째 정상 백엔드를 고릅니다 |

선택은 **에이전트**에서 일어납니다. 카운트는 백엔드 연결이 닫히거나 취소될 때
해제됩니다. 가중치는 연결 수를 분배할 뿐, 바이트 수를 분배하지는 않습니다.

동작 세부 사항은 다음과 같습니다.

- 모든 백엔드가 비정상으로 표시되어도 하나는 여전히 시도합니다. 잘못 설정된 프로브
  때문에 터널 전체가 멈추는 일을 막기 위함입니다.
- 단일 연결 실패는 오류를 보고할 뿐, 같은 요청 안에서 다른 백엔드로 **다시 시도하지
  않습니다**.
- `source_hash`는 출발지 ACL이나 PROXY 프로토콜이 검증한 원래 주소를 사용합니다.

## 헬스 체크

```toml
[tunnel.health]
type = "http"                # "tcp" | "http"
path = "/healthz"
interval = "10s"
timeout = "2s"
failures_to_evict = 3
successes_to_restore = 2
expect_status = [200, 204]
```

| 필드 | 설명 |
| --- | --- |
| `type = "tcp"` | 연결을 수립할 수 있는지만 확인합니다 |
| `type = "http"` | 지정한 `path`로 요청하며, 기본적으로 2xx를 정상으로 받아들입니다 |
| `expect_status` | 허용할 상태 코드를 명시적으로 나열합니다 |
| `interval` | 프로브 간격 |
| `timeout` | 단일 프로브의 타임아웃 |
| `failures_to_evict` | 연속 실패가 몇 회이면 백엔드를 축출하는지 |
| `successes_to_restore` | 연속 성공이 몇 회이면 백엔드를 복구하는지 |

HTTP 프로브는 백엔드 주소를 Host로 사용합니다. 연속 실패는 백엔드를 축출하고 연속
성공은 백엔드를 복구하며, 둘 다 로그로 남습니다.

UDP 터널과 동적 대상 프록시(socks5/httpproxy/mixedproxy)에는 TCP나 HTTP 백엔드 프로브를
붙이면 **안 됩니다**. 고정된 백엔드가 없기 때문입니다. 대상이 디렉터리 권한 핸들로
결정되는 file 모드도 마찬가지입니다.

## HTTP 백엔드 풀링

HTTP 터널의 백엔드 연결 풀은 클라이언트 세션, 터널, 출발지 IP, 대상, HTTP 오리진
단위로 분리됩니다. 오리진 하나당 유휴 연결은 최대 8개이고 유휴 타임아웃은 30초이며,
바깥 테이블은 최대 256개 항목을 담습니다. PROXY 헤더 전송이 활성화된 경우 풀은
출발지 포트 단위로도 분리됩니다. 여러 HTTP/2 요청이 하나의 연결을 공유할 수 있습니다.

`response_timeout`(서버 `[http]`)은 요청을 보낸 뒤 백엔드의 응답 헤더를 받기까지의
시간을 제한합니다. 대용량 파일의 느린 업로드에는 그에 맞게 값을 늘리세요. 응답
본문은 백프레셔를 사용하며, 전체를 버퍼링하지 않습니다.

## 다음 단계

- [터널과 포트 매핑](tunnels.md)
- [HTTP/HTTPS 리버스 프록시](http-services.md)
- [접근 제어, 할당량과 스트림 부호화](security.md)
