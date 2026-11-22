# 터널과 포트 매핑

터널은 Felucca의 기본 단위입니다. 공개 진입점 하나와 백엔드 집합으로 이루어집니다. 이
페이지에서는 공통 필드, 여덟 가지 모드, 대량 포트 매핑, PROXY 프로토콜을 설명합니다.

## 공통 터널 필드

```toml
[[tunnel]]
id = 1                 # Stable ID; pin it once traffic snapshots are enabled
mode = "tcp"           # Entry protocol
port = 8080            # Port opened on the server
bind = "0.0.0.0"       # IP only, no port attached
remark = "internal web app"
public = true          # false means a private share that does not take a public port
balance = "round_robin"   # Goes inside [[tunnel]], not after [[tunnel.backend]]

[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`bind`는 포트가 아닌 IP 주소를 받으며, 포트는 `tunnel.port`에서 옵니다. 모든
인터페이스에서 리스닝하려면 `bind`를 생략하세요. `id`를 설정하지 않으면 사용 가능한
양의 정수가 하나 배정됩니다. 트래픽 스냅샷을 활성화한 뒤에는 배열 순서에 의존하지 말고
`id`를 명시하세요.

## 모드

| `mode` | 진입점 동작 | 대상 출처 |
| --- | --- | --- |
| `tcp` | 공개 TCP 포트, 원시 바이트 전달 | `backend` |
| `udp` | 공개 UDP 포트, 데이터그램 경계 유지 | `backend` |
| `http` | 서버의 HTTP, HTTPS, HTTP/3 리스너를 재사용하고 도메인과 경로로 라우팅 | `backend` |
| `socks5` | SOCKS5 포워드 프록시, CONNECT와 UDP ASSOCIATE | 클라이언트가 지정 |
| `httpproxy` | HTTP 포워드 프록시, 절대 URL과 CONNECT | 클라이언트가 지정 |
| `mixedproxy` | 혼합 진입점. 요청의 시작 부분으로 SOCKS5와 HTTP를 구분 | 클라이언트가 지정 |
| `file` | 파일 서빙과 WebDAV | 에이전트 호스트의 디렉터리 |
| `transparent`(별칭 `tcptrans`) | 커널에서 원래 목적지 주소를 읽습니다 | 커널 |

`http` 터널은 별도 리스너를 열지 **않습니다**. `port`는 서버의 `http.bind`,
`http.tls_bind`, `http.http3_bind`와 일치해야 하며, 여러 HTTP 터널이 하나의 포트를
공유할 수 있습니다. `tunnel.bind`은 HTTP 라우팅에 영향을 주지 않습니다. HTTP 리스너는
서버가 활성화하며, 에이전트의 `allowed_ports` 제한을 받지 않습니다.

## 대량 포트 매핑

`[[mapping]]`은 TCP와 UDP만 지원하며, 포트 범위를 순서대로 일대일로 매핑합니다.

```toml
[[mapping]]
mode = "tcp"
server_ports = "9001-9003,10000"
target_ports = "8001-8003,8080"
target_host = "127.0.0.1"
bind = "0.0.0.0"
```

양쪽 개수는 같아야 하며, 서버 포트가 중복되면 안 됩니다. 전개된 터널은 명시적인
`[[tunnel]]` 항목과 함께 검증되며, 서버의 포트 인가와 터널 상한도 그대로 적용됩니다.
`target_host`는 도메인 이름, IPv4 주소, IPv6 주소를 쓸 수 있으며 포트를 담을 수
없습니다. `tls`/`trust`, `health`, `acl`, `options`, `target_policy`도 설정할 수
있습니다.

## UDP 동작

- 출발지 IP:port마다 별도의 연관 관계가 수립되고, 에이전트 쪽의 백엔드 소켓 하나에
  고정됩니다.
- 빈 패킷과 데이터그램 경계는 유지됩니다. 최대 데이터그램은 65507 바이트이며, 실제
  네트워크에서 쓸 수 있는 MTU는 더 낮을 수 있습니다.
- 터널당 최대 256개의 출발지 연관 관계를 허용하며, 각각 8개 패킷을 큐에 담습니다.
  큐가 가득 차면 패킷을 버리고, 연관 관계는 60초 유휴 뒤에 회수됩니다.
- 데이터그램은 신뢰성 있는 터널 스트림으로 전달되며 QUIC DATAGRAM이 **아닙니다**.
  따라서 손실이 있는 링크에서는 재전송을 기다립니다.
- UDP 통계는 데이터그램 페이로드만 포함하며, 내부 길이 프레임은 포함하지 않습니다.

## PROXY 프로토콜

원래 출발지 주소와 진입점 목적지 주소를 백엔드로 전달합니다.

```toml
[tunnel.options]
proxy_protocol = "v1"     # "off" | "v1" | "v2"
```

TCP, HTTP, TLS에서는 백엔드 연결마다 헤더 하나를 보냅니다. UDP는 v2만 지원하며,
데이터그램마다 DGRAM 헤더를 덧붙입니다. 헤더를 포함해 65507 바이트를 넘는 패킷은
버립니다. 백엔드는 PROXY 프로토콜을 파싱할 수 있어야 합니다.

받는 쪽에서는 해당 리스너의 `proxy_accept`를 설정하세요.

```toml
[http.proxy_accept]
mode = "required"                    # optional | required
trusted = ["192.0.2.10", "2001:db8:1::/64"]
bridge.proxy_accept = { ... }
[tunnel.options.proxy_accept]
mode = "optional"
trusted = ["10.0.0.0/8"]
```

`optional`은 헤더 없는 직접 연결도 허용하고, `required`는 헤더가 없으면 거부합니다.
이 기능을 활성화하면 `trusted`은 비어 있으면 안 됩니다. 출발지 ACL, 출발지 해싱,
HTTP 전달 헤더는 모두 검증된 원래 주소를 사용합니다. 같은 포트의 브리지와 HTTP
리스너는 같은 정책으로 설정해야 합니다. 수신은 5초로 제한되며, UDP 리스너는 PROXY
헤더를 받지 않습니다.

## 스트림별 부호화

```toml
[tunnel.options.payload]
compress = true     # Snappy, default false
encrypt = true      # ChaCha20-Poly1305, default false
```

이것은 TCP와 UDP, 파일과 WebDAV 전송, HTTP 리버스 프록시와 포워드 프록시의 데이터
스트림에 적용됩니다. 압축을 활성화하면 데이터를 암호화하기 전에 압축합니다. 스트림마다
새로운 nonce를 교환하고, 해당 에이전트의 vkey에서 두 개의 독립적인 방향별 키를
도출합니다. 양쪽 설정이 맞지 않으면 협상을 거부하며, 평문으로의 자동 대체는 없습니다.
자세한 내용은 [접근 제어, 할당량과 스트림 부호화](security.md)를 보세요.

## 다음 단계

- [백엔드, 밸런싱과 헬스 체크](backends.md)
- [HTTP/HTTPS 리버스 프록시](http-services.md)
- [포워드 프록시](forward-proxy.md)
- [파일 서비스와 WebDAV](file-services.md)
- [서버 로컬 대상과 중첩 라우트](local-services.md)
