# HTTP/HTTPS 리버스 프록시

이 페이지에서는 도메인과 경로로 내부 HTTP 서비스를 노출하는 방법을 설명합니다.
라우팅, 헤더와 Host 재작성, 리다이렉트, CORS, 오류 페이지, SNI, HTTP/2와 HTTP/3,
포트 공유, 전달 헤더를 다룹니다.

## HTTP 라우트 등록

```toml
# Server side
[http]
bind = "0.0.0.0:8080"
tls_bind = "0.0.0.0:8443"

# Agent side
[[tunnel]]
mode = "http"
port = 8080                       # Must match http.bind or http.tls_bind
hosts = ["app.example.test", "*.example.test"]
balance = "round_robin"

[tunnel.options.http]
path = "/api/"
scheme = "any"
rewrite_prefix = "/v2/"
host = "backend.internal"

[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`http` 터널은 별도 리스너를 열지 않습니다. `port`가 어떤 HTTP 리스너에 붙을지
결정하며, HTTP와 HTTPS 양쪽으로 도달할 수 있습니다.

## 라우팅 규칙

```toml
[tunnel.options.http]
path = "/api/"
scheme = "any"        # any | http | https
```

매칭 우선순위는 **정확한 도메인, 그다음 가장 긴 경로 프리픽스, 마지막으로 정확한
스킴**입니다. 도메인 이름은 대소문자를 구분하지 않으며, 끝의 점은 정규화합니다.

| 패턴 | 일치 범위 |
| --- | --- |
| 정확한 `app.example.test` | 그 도메인에만 일치합니다 |
| 와일드카드 `*.example.test` | `a.example.test`에 일치하지만, `example.test`나 `a.b.example.test`에는 일치하지 **않습니다** |

같은 도메인, 경로, 프로토콜로 중복 등록하면 거부합니다. 일치하는 라우트가 없는 요청은
404를 반환합니다. 같은 도메인에 여러 경로를 등록하고 서로 다른 터널에 배정할 수
있습니다.

## 요청과 응답 옵션

```toml
[tunnel.options.http]
rewrite_prefix = "/v2/"
host = "backend.internal"
cors = true
redirect = "https://new.example{path}"
force_https = true
request_headers = { x-source = "{remote_ip}", x-remove = "" }
response_headers = { x-proxy = "felucca" }
error_pages = { "502" = "<h1>Service unavailable</h1>" }
```

| 옵션 | 동작 |
| --- | --- |
| `rewrite_prefix` | 프리픽스를 재작성하고 나머지 경로와 쿼리 문자열은 유지합니다 |
| `host` | 백엔드로 보내는 Host를 재작성합니다 |
| `request_headers` | 헤더 템플릿입니다. 빈 값은 그 헤더를 제거합니다 |
| `response_headers` | 응답 헤더 템플릿입니다 |
| `cors` | `Access-Control-Allow-Origin: *`를 반환하며, 올바른 프리플라이트는 204를 바로 반환합니다. 프리플라이트에는 사이트 계정이 필요하지 않지만, 실제 요청에는 인증이 여전히 필요합니다 |
| `redirect` | 307을 반환합니다 |
| `force_https` | 301을 반환하며, 대상은 서버의 HTTPS 리스너 포트를 사용합니다(443은 생략). 등록할 때 HTTPS 리스너가 존재해야 합니다 |
| `error_pages` | 상태 코드별로 응답 본문을 교체합니다. 400..599를 지원하며 페이지당 최대 64 KiB입니다 |

템플릿은 `{host}`, `{path}`(쿼리 문자열 포함), `{scheme}`, `{remote_ip}`,
`{remote_port}`를 지원합니다. 값은 원래 요청에서 가져오며, 치환 결과는 다시 확장하지
않습니다. Content-Length나 Transfer-Encoding 같은 메시지 경계 헤더와 홉별 헤더는
재작성할 수 없으며, Host는 전용 필드를 씁니다.

사이트 인증은 `Authorization: Basic`을 사용하며, 실패하면 401을 반환합니다. 검증된
자격 증명은 백엔드로 전달하지 않습니다.

## TLS 모드

```toml
[tunnel.options.http]
tls_mode = "http"        # http | passthrough | offload
```

| 모드 | 동작 |
| --- | --- |
| `http` | 기본값입니다. 일반적인 HTTPS 리버스 프록시입니다 |
| `passthrough` | 원시 TLS 스트림을 SNI에 따라 백엔드로 넘기며, 방문자에게 백엔드 인증서가 그대로 보입니다. 이 모드에서는 `backend.tls`를 비활성화해야 합니다 |
| `offload` | 서버의 SNI 인증서로 TLS를 종료한 뒤, 임의의 TCP 콘텐츠를 백엔드로 전송합니다 |

`passthrough`과 `offload`는 SNI 전체를 차지하므로 HTTP 경로나 헤더 옵션, 사이트 인증과
함께 쓸 수 없습니다.

## 포트와 프로토콜

| 항목 | 설명 |
| --- | --- |
| 진입점 | HTTP/1.1, HTTP/2, HTTP/3 |
| 백엔드 | HTTP/1.1, HTTP/2(`backend_http2 = true`) |
| ALPN | TLS 진입점은 `h2`와 `http/1.1`를 지원하며, 평문 진입점은 HTTP/2 prior knowledge를 지원합니다 |
| WebSocket | 세 종류의 진입점과 두 종류의 백엔드 사이에서 자동으로 변환됩니다. HTTP/1.1 진입점은 101을 반환하고, HTTP/2나 HTTP/3는 `:protocol = websocket` 확장 CONNECT를 사용합니다 |
| HTTP/3 | `http.http3_bind`가 필요합니다. QUIC 연결 하나당 양방향 스트림은 최대 128개이며, 스트림별 수신 창은 1 MiB, 연결별은 16 MiB입니다. 30초 유휴 후 닫히며, 요청 헤더는 32 KiB로 제한됩니다 |

HTTP/3는 공개 진입 프로토콜일 뿐이며, 백엔드는 여전히 HTTP/1.1이나 HTTP/2를
사용합니다. 일반 HTTPS 응답에는 `Alt-Svc` 헤더가 포함되며, 외부에 매핑한 포트가 다를
때는 `response_headers`로 덮어쓸 수 있습니다.

## 포트 공유

`bridge.bind`, `http.bind`, `http.tls_bind`는 모두 **같은** IP:port가 될 수 있으며,
하나의 소켓이 핸드셰이크 특성으로 다중화합니다. 직접 연결한 TLS/WSS 에이전트는
`felucca/9` ALPN을 쓰고, 웹사이트는 `h2`/`http/1.1`를 쓰거나 ALPN을 쓰지 않습니다.
일반 `GET /ws`는 여전히 웹사이트 요청이며, 올바른 WebSocket 업그레이드만 브리지로
들어갑니다. QUIC 브리징과 HTTP/3도 같은 UDP 주소를 공유할 수 있으며, ALPN으로
다중화하고 브리지 인증서와 웹사이트 인증서는 따로 선택하고 갱신합니다.

## 클라이언트 주소

기본적으로 클라이언트가 보낸 `Forwarded`, `X-Forwarded-*`, `X-Real-IP` 헤더를 제거한
뒤, `http.proxy_accept`가 검증한 PROXY 출발지 또는 직접 연결한 출발지 주소에서
`X-Forwarded-For`, `X-Real-IP`, `X-Forwarded-Host`, `X-Forwarded-Proto`를 설정합니다.
이 재작성을 끄려면 `forwarded_headers = false`로 설정하세요. 프록시 체인 전체를 직접
관리하는 배포에 적합합니다. 홉별 헤더와 `Connection`에 명시된 헤더는 다음 홉으로
전달하지 않습니다.

## 다음 단계

- [인증서, SNI와 ACME](certificates.md)
- [백엔드, 밸런싱과 헬스 체크](backends.md)
- [파일 서비스와 WebDAV](file-services.md) — 도메인으로 제공하는 파일 진입점
- [접근 제어, 할당량과 스트림 부호화](security.md) — 도메인 할당량과 ACL
