# felucca-agent 모듈

이 페이지에서는 `felucca-agent`의 배포와 설정을 설명합니다. 서버 연결, 인증서 신뢰,
재연결, 이그레스 네트워킹, 업스트림 프록시, 로깅, 다중 인스턴스를 다룹니다.

## 최소 구성

```toml
server = "bridge.example.com:14028"
key_id = "agent-1"
vkey = "change-me-to-a-long-random-string"
transport = "tls"          # tcp | tls | ws | wss | quic | kcp

[trust]
mode = "pinned"
fingerprint = "0000...0000"   # from the server's --fingerprint

[[tunnel]]
mode = "tcp"
port = 8080
[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`server`에는 포트를 붙인 호스트명이나 IP 주소를 지정합니다. `key_id`는 서버의
`[client.<key_id>]` 테이블 이름과 일치해야 합니다.

## 전송과 신뢰

`transport`는 `tcp`, `tls`, `ws`, `wss`, `quic`, `kcp`를 사용할 수 있습니다. 공개
브리지에서는 TLS/WSS/QUIC을 권장합니다. `tcp`와 `ws`는 상대를 인증하지만 트래픽을
암호화하지 않으므로, 평문 전송은 의도적으로 선택해야 합니다.

`[trust]`는 서버 인증서를 어떻게 검증할지 결정합니다.

| `mode` | 동작 |
| --- | --- |
| `webpki` | 공개 CA를 상대로 일반적으로 검증합니다. `fingerprint`는 설정하지 마세요 |
| `pinned` | 인증서 전체의 SHA-256을 고정합니다. 핸드셰이크 서명은 검증하지만 도메인 이름과 유효 기간은 검증하지 않습니다. 인증서가 바뀌면 지문을 갱신하세요 |
| `insecure` | 일시적인 진단 용도로만 사용합니다 |

`bridge.tls`에 고정 PEM을 설정한 뒤에는 `felucca-server --fingerprint`로 지문을
확인하세요.

## 연결과 재연결

| 필드 | 기본값 | 설명 |
| --- | --- | --- |
| `reconnect_min` | — | 백오프의 하한 |
| `reconnect_max` | — | 백오프의 상한 |
| `dial_timeout` | — | 단일 dial의 타임아웃 |
| `max_connections` | — | 이 에이전트의 연결 상한 |

에이전트는 연결이 끊기면 자동으로 재연결하며, 서버는 리스너와 세션 상태를 다시
등록합니다. 트래픽 카운터는 재연결 후에도 유지됩니다.

## 이그레스 네트워킹

```toml
[network]
local_ip = "192.0.2.10"
forward_local_ip = true
dns = ["192.0.2.53:53"]

[network.proxy]
kind = "socks5"           # http (CONNECT) | socks5
server = "192.0.2.20:1080"
remote_dns = true
[network.proxy.auth]
username = "edge"
password = "replace-with-proxy-password"
```

| 필드 | 설명 |
| --- | --- |
| `local_ip` | 브리지 이그레스 IP를 바인딩합니다. 이 머신이 실제로 소유한 주소여야 하며, 인터페이스 이름은 허용되지 않습니다 |
| `forward_local_ip` | 백엔드 연결과 헬스 프로브도 이 주소를 사용합니다. 기본적으로는 브리지만 이 주소를 바인딩합니다 |
| `dns` | `IP:port` 형태의 명시적 목록입니다. 비어 있으면 시스템 리졸버를 사용합니다. 응답은 TTL 기준으로 캐시되며 A/AAAA를 지원하고 UDP 절단 후에는 TCP로 대체합니다 |
| `network.proxy` | 업스트림 프록시입니다. 브리지의 TCP/TLS/WS/WSS만 통과시키며, 백엔드 요청이나 NTP UDP는 전달하지 않습니다 |

대상 ACL은 **이름 조회로 실제 반환된 IP 주소**를 대상으로 검사하고, 에이전트는 검사를
통과한 주소로 연결합니다. 따라서 두 번째 조회로 우회가 생길 수 없습니다. `local_ip`와
DNS 주소는 같은 주소 패밀리에 있어야 합니다. QUIC/KCP 브리지는 TCP 업스트림 프록시와
함께 사용할 수 없습니다.

## 시간 동기화

서버와 에이전트 모두 브리지 인증 시간을 보정하기 위해 `[ntp]` 섹션을 사용할 수 있습니다.

```toml
[ntp]
server = "time.example.com:123"
interval = "5m"
timeout = "5s"
max_offset = "24h"
```

기본적으로는 시간 서비스에 질의하지 않습니다. `interval`은 1초에서 24시간 사이입니다.
`timeout`은 양수여야 하고 30초를 넘을 수 없으며, `interval`보다 작아야 합니다.
`max_offset`은 양수여야 하고 7일을 넘을 수 없습니다. 첫 질의가 실패하면 시스템 시간을
사용합니다. 보정은 시스템 시계를 바꾸지 않으므로, TLS 인증서 유효 기간과 할당량 만료는
여전히 시스템 시간을 따릅니다. NTP 출처를 신뢰할 수 있어야 합니다. 일반 SNTP는 시간
출처를 인증하지 않습니다.

에이전트의 NTP 질의는 에이전트 자체의 DNS와 이그레스 설정을 사용하고, 서버는 시스템
DNS와 기본 이그레스를 사용합니다.

## 중앙 집중 설정 수신

```toml
[remote]
enabled = true

[remote.target_policy]
allow = ["10.0.0.0/8"]
ports = [80, 443]

[remote.file_root.documents]
path = "/srv/documents"
read_only = true
```

`remote.enabled`는 반드시 명시해야 합니다. 주 연결은 인증서가 검증되는
TLS/WSS/QUIC을 사용해야 하며, 평문 연결로는 설정을 요청할 수 없습니다.
`remote.target_policy`는 에이전트 머신 자체에 대한 단단한 경계이며, 배포되는 각
터널과 교집합으로 적용됩니다. `remote.file_root`는 이름 붙은 디렉터리 권한을 미리
선언하며, 서버는 그 권한 이름 안의 경로만 배포할 수 있습니다. 읽기 전용 디렉터리는
원격 설정으로 쓰기 가능하게 바꿀 수 없습니다.
[관리 UI와 중앙 집중 설정](management.md)도 함께 보세요.

`remote.console_scope = true` 로 설정하면 서버의 `[managed.<client-id>]` 에 있는 `target_policy` 가 로컬 `remote.target_policy` 를 대체하므로 콘솔의 기기 편집에서 범위를 바꿀 수 있습니다. 서버에 지정이 없으면 로컬 범위를 그대로 씁니다. 켜지 않은 경우 `target_policy` 가 포함된 배포 설정은 거부됩니다. 디렉터리 권한은 항상 로컬 설정으로만 정해집니다. 콘솔이 만든 연결 패키지는 이 옵션을 켭니다.

## 로깅

```sh
felucca-agent -c agent.toml --log-level debug --log-file /data/agent.log --log-json
```

`--log-file`을 지정하면 기본적으로 10 MiB 파일 5개를 보관하는 로테이션이 활성화됩니다.
로그 파일에는 추가 기록 방식으로 쓰이고, stderr로도 출력됩니다. 상위 디렉터리는 이미
존재해야 합니다. `--log-max-bytes`, `--log-backups`, `--log-rotate-interval`은
`--log-file`이 필요합니다. 원시 시크릿은 로그에 기록되지 않습니다.

## 다중 인스턴스

```sh
felucca-agent --config office.toml --config home.json --check
felucca-agent --config office.toml --config home.json
```

하나의 프로세스는 최대 32개의 인스턴스를 지원합니다.
[설정 소스와 CLI](configuration.md)도 함께 보세요.

## P2P와 방문자 스위치

| 테이블 | 효과 |
| --- | --- |
| `[p2p] enabled = true` | 이 에이전트가 제공자로서 직접 연결을 받아들일 수 있게 합니다 |
| `[[visitor]]` | 다른 사람의 비공개 공유에 접근하기 위해 자신의 머신에서 리스닝합니다 |

[P2P 직접 연결과 릴레이 대체](p2p.md)와
[비공개 공유와 방문자](private-shares.md)도 함께 보세요.

## 다음 단계

- [터널과 포트 매핑](tunnels.md)
- [백엔드, 밸런싱과 헬스 체크](backends.md)
- [서비스, 업데이트, 로깅과 모니터링](operations.md) — 시스템 서비스로 설치하기
