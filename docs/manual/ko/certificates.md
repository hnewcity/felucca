# 인증서, SNI와 ACME

이 페이지에서는 세 종류의 인증서를 설명합니다. 브리지 인증서, 웹사이트 정적 인증서,
그리고 ACME로 자동 발급하고 갱신하는 인증서입니다.

## 브리지 인증서

```toml
[bridge]
bind = "0.0.0.0:14028"
tls = { cert = "conf/bridge.pem", key = "conf/bridge.key" }
cert_reload_interval = "30s"
```

`bridge.tls`를 생략하고 런타임 설정 저장을 활성화하면(`--init` 또는 관리 콘솔 포함) 서버는 스냅샷 옆의 `<snapshot>.identity.json`에 신원 정보를 저장합니다. 재시작과 백업 시 보존하세요. 런타임 저장이 없으면 임시 신원을 사용합니다. `--fingerprint`는 명시한 PEM 파일을 읽으며 자동 생성 지문은 실행 중인 콘솔과 시작 로그에서 확인할 수 있습니다.

PEM 파일은 `cert_reload_interval`(기본 30초)마다 다시 읽힙니다. 새로운 TLS/WSS와 QUIC
연결은 새 인증서를 사용하고, 기존 연결은 끊기지 않습니다. 인증서나 키 갱신이
불완전하면 이전 값을 유지합니다. `pinned` 신뢰를 쓰는 에이전트는 다음 재연결 전에
지문을 갱신해야 합니다. 올바른 CA로 검증하는 에이전트는 특정 인증서를 고정할 필요가
없습니다.

```sh
target/release/felucca-server --config conf/server.toml --fingerprint
```

에이전트 쪽에서는 두 가지 방법 중 하나를 선택합니다.

```toml
[trust]
mode = "pinned"
fingerprint = "<from --fingerprint>"

[trust]
mode = "webpki"        # CA-issued bridge certificate
```

## 웹사이트 인증서

```toml
[http.certificates."app.example.com"]
cert = "conf/app.pem"
key = "conf/app.key"

[http.certificates."*.example.com"]
cert = "conf/wildcard.pem"
key = "conf/wildcard.key"

[http.default_tls]
cert = "conf/default.pem"
key = "conf/default.key"
```

정확히 일치하는 항목이 와일드카드보다 우선하고, 와일드카드가 `default_tls`보다
우선합니다. 일치하는 항목이 없으면 핸드셰이크를 거부합니다. SNI와 HTTP Host가 서로
맞지 않으면 421을 반환합니다. 정적 인증서는 기본적으로 30초마다 다시 읽으며,
`http.cert_reload_interval`로 조정할 수 있습니다. 인증서는 자신의 개인 키와 맞아야만
교체되며, 실패하면 기존 인증서를 그대로 사용합니다.

HTTP/3와 HTTP/HTTPS는 같은 SNI 리졸버를 공유하며, ACME 갱신과 정적 PEM 파일의 핫
리로드도 함께 반영됩니다.

## ACME 발급

```toml
[http.acme]
email = "admin@example.com"
accept_tos = true
challenge = "http-01"          # http-01 | tls-alpn-01
cache_dir = "state/acme"
staging = false
domains = ["app.example.com"]
```

| 필드 | 설명 |
| --- | --- |
| `accept_tos` | CA의 이용 약관에 동의한 뒤에만 `true`로 설정하세요 |
| `challenge` | `http-01`은 `http.bind`에 도달하려면 공개 TCP 80이 필요합니다. `tls-alpn-01`은 `http.tls_bind`에 직접 도달하려면 공개 TCP 443이 필요하며, 이 경우 `http.bind`는 닫아도 됩니다 |
| `staging` | 먼저 스테이징으로 연결을 확인합니다. Let's Encrypt 스테이징으로 고정됩니다 |
| `directory` | 다른 CA를 지정합니다. 기본값은 Let's Encrypt 프로덕션입니다 |
| `cache_dir` | 계정과 인증서 캐시를 두는 디렉터리입니다. **유지하고 보호하세요** |
| `domains` | 발급 대상 이름을 명시적으로 제한합니다. 비어 있으면 등록된 정확한 도메인에서 매분 이름을 도출하며, 와일드카드는 건너뜁니다 |

계정은 `account.json`에 저장되고, 각 도메인의 인증서 체인과 개인 키는 하나의 JSON
파일에 함께 담기며 원자적으로 교체됩니다. Unix에서는 파일 모드 0600, 새 캐시
디렉터리는 0700을 사용합니다. 인증서는 시작할 때 캐시에서 복원됩니다. 갱신 시점은
인증서의 실제 유효 기간에서 산출되며, 그 기간의 3분의 1을 넘기지 않도록 여유를
둡니다. 검사는 매분 실행되고, 실패하면 1시간 뒤 다시 시도하며, 갱신이 실패하는 동안은
기존 인증서를 계속 사용합니다. 발급에 성공하면 핫으로 교체되어 기존 TLS 연결이
끊기지 않습니다.

**ACME는 HTTP/HTTPS 리버스 프록시 인증서만 관리합니다. 브리지 인증서나 에이전트의
고정 지문은 바꾸지 않습니다.**

### EAB와 ZeroSSL

```toml
[http.acme.eab]
key_id = "issued-by-your-ca"
hmac_key = "base64url-key-issued-by-your-ca"
```

ZeroSSL은 수동 EAB를 사용하거나, API 키 모드로 전환할 수 있습니다.

```toml
[http.acme]
directory = "https://acme.zerossl.com/v2/DV90"
staging = false
zerossl_api_key = "your-account-key"
```

API 키 모드는 계정을 처음 등록할 때 EAB를 받은 뒤 ACME으로 인증서를 발급합니다.
**수동 `eab`와 동시에 설정할 수는 없습니다.** 계정 캐시가 존재하면 새 EAB를 다시
요청하지 않습니다. API 키와 응답의 시크릿은 오류 메시지에 기록되지 않습니다.

## 배포 참고 사항

- 두 ACME 챌린지 모두 **와일드카드 인증서를 지원하지 않습니다**. 와일드카드 이름은
  외부에서 발급한 정적 인증서를 사용하세요.
- 애플리케이션이 8080/8443에서 리스닝한다면, 외부에서 80/443을 그 포트로
  연결하세요.
- 발급이 성공하기 전까지는 해당 도메인의 TLS 핸드셰이크가 실패할 수 있습니다.
  HTTP-01은 그동안 검증을 제공합니다.
- 앞단의 TLS를 종료하는 프록시는 챌린지 TLS를 그대로 전달해야 하며, `acme-tls/1`를
  포함하는 ALPN 값에 대해서만 챌린지 인증서를 반환해야 합니다.
- CA를 바꾸거나 스테이징과 프로덕션을 오갈 때는 새 `cache_dir`을 사용하세요.
- 리포지터리는 로컬의 모의 CA를 상대로 실제 CSR과 챌린지 흐름을 검증합니다. 공개
  CA에서의 실제 발급은 직접 도메인 환경에서 확인해야 합니다.

## 다음 단계

- [HTTP/HTTPS 리버스 프록시](http-services.md)
- [felucca-server 모듈](server.md)
- [서비스, 업데이트, 로깅과 모니터링](operations.md)
