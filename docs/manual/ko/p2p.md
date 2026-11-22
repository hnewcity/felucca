# P2P 직접 연결과 릴레이 대체

양쪽이 모두 NAT 뒤에 있을 때, Felucca는 QUIC 직접 연결을 위해 UDP 홀 펀칭을 시도하고
실패하면 서버 릴레이로 대체할 수 있습니다. 애플리케이션 데이터는 서버를 통과하지
않으며, 주 연결은 계속 제어와 계정 처리를 담당합니다. 서버와 제공자 에이전트, 방문자
에이전트는 함께 업그레이드해야 합니다.

## 서버: 관찰 포트

```toml
[bridge]
bind = "0.0.0.0:14028"

[p2p]
bind = "0.0.0.0:8025"        # Listen on this host
advertise = "203.0.113.10:8025"   # External UDP address both ends can reach, without a scheme
timeout = "5s"
max_pending = 128
```

| 필드 | 기본값 | 제약 |
| --- | --- | --- |
| `bind` | — | 로컬 리스닝 주소 |
| `advertise` | — | 외부에서 도달할 수 있는 UDP 주소. URL 스킴은 **붙이지 않습니다** |
| `timeout` | `5s` | 500ms–10s |
| `max_pending` | `128` | 1–4096 |

`advertise`는 KCP, QUIC, HTTP/3이 쓰는 UDP 포트와 겹치면 안 됩니다. 설정의
`127.0.0.1`는 모든 프로세스가 같은 호스트에서 실행되는 데모에만 맞습니다. 네트워크를
넘어 사용하려면 서버의 공개 도메인 이름과 포트로 바꾸세요.

## 제공자: 직접 연결 허용

```toml
# provider agent
[p2p]
enabled = true
max_sessions = 64        # Default 64, maximum 1024
bind_ip = "192.0.2.10"   # Optional; fixes the local UDP IP

[[tunnel]]
id = 1
mode = "tcp"
public = false
backend = [{ addr = "127.0.0.1:3000" }]
```

```toml
# server: a share must enable direct explicitly
[share.office-web]
client = "office"
tunnel = 1
secret = "replace-this-shared-secret"
allow_clients = ["laptop"]
direct = true
```

서버, 로컬 제공자, 공유는 각각 **명시적으로 활성화**해야 합니다(서버의 `[p2p]`,
제공자의 `[p2p] enabled = true`, 공유의 `direct = true`). 양쪽의 주 연결은 인증서가
검증되는 TLS, WSS 또는 QUIC을 사용해야 합니다. `bind_ip`를 생략하면 에이전트의 이그레스
IP 설정을 사용하며, 백엔드 dial에 쓰는 이그레스는 바꾸지 않습니다.

## 방문자: 연결 성향

```toml
# visitor agent
[[visitor]]
name = "office-web"
share = "office-web"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:18080"
mode = "tcp"
connection = "prefer_direct"    # relay | prefer_direct | direct
```

| `connection` | 동작 |
| --- | --- |
| `relay` | 기본값입니다. 기존 릴레이를 사용합니다 |
| `prefer_direct` | 직접 연결을 먼저 시도하고, 직접 연결을 수립할 수 없고 애플리케이션 데이터도 보내지 않았다면 릴레이로 대체합니다 |
| `direct` | 직접 연결만 받아들입니다. 직접 연결이 불가능하면 로컬 연결을 닫습니다 |

## 링크가 수립되는 과정

1. 주 연결이 먼저 노드와 공유 시크릿 인증을 완료합니다. 서버는 방문자 신원과 양쪽의
   출발지 ACL, 연결 할당량을 검사합니다.
2. 서버가 터널, 출발지, 대상, UDP 종류, 인코딩 옵션, PROXY 정보에 연결된 **일회용
   티켓**을 발급하고, 제공자의 노드 키는 전달하지 않습니다.
3. 양쪽은 서로 다른 티켓 키로 관찰된 UDP 주소를 탐색합니다. 서버는 수명이 짧고 출발지에
   묶인 쿠키로 회신 경로를 확인하고, 유효한 탐색 횟수와 티켓 수 및 수명을 제한합니다.
   알 수 없는 티켓에는 응답하지 않습니다.
4. 제공자는 관찰에 사용한 것과 같은 UDP 소켓을 유지하면서, 방문자가 관찰한 엔드포인트로
   홀 펀칭 패킷을 보냅니다.
5. 방문자는 QUIC TLS 1.3으로 직접 연결하며, 제공자의 임시 인증서 지문을 고정하고, 묶인
   티켓과 두 개의 논스로 양방향 HMAC를 완료합니다.
6. 백엔드 선택, 헬스 상태, 대상 ACL, 파일 인증, 디렉터리 권한 핸들은 계속 원래 서비스
   엔진을 그대로 사용합니다.

홀 펀칭이 실패하면 제공자의 시도와 양쪽의 할당량을 먼저 정리한 뒤에야 릴레이 연결을
허용합니다. 포워딩이 시작된 뒤 링크가 끊겨도 애플리케이션 데이터를 **다시 보내지는
않습니다**. 주 연결이 끊리거나, 노드가 비활성화되거나, 할당량이 소진되거나, 서비스가
멈추면 관련 직접 링크가 닫힙니다.

## 계정 처리와 제한

- 애플리케이션 데이터는 양쪽 사이를 QUIC으로 이동하며, 서버의 데이터 릴레이는 통과하지
  않습니다.
- 제공자는 방향별로 계정 처리 대기 중인 데이터 블록을 최대 하나씩 유지합니다(TCP
  블록 ≤ 16 KiB, UDP ≤ 65507 바이트).
- 서버는 전송 전에 양쪽의 트래픽과 속도 범위를 검사하고 스케줄한 뒤, 전송이 끝나면 실제
  바이트 수를 커밋합니다. 중복된 시퀀스 번호, 겹치는 인가, 할당량 초과 커밋은
  거부됩니다.
- 계산은 **디코딩한 애플리케이션 바이트**를 기준으로 하며, 프로브, TLS, 압축 오버헤드,
  UDP 프레임 헤더, TCP PROXY 헤더는 제외합니다.
- 직접 연결의 계정 처리는 인증된 제공자의 보고에 의존하며, 서버는 직접 페이로드를
  스스로 볼 수 없습니다. 제공자가 충돌하거나 마지막 보고 전에 제어 연결이 끊기면,
  보냈지만 커밋하지 않은 블록이 영속 카운트에 기록되지 않을 수 있습니다. 정상 종료는
  확인 메시지를 통해 남은 데이터를 처리합니다.
- 블록 단위 할당량 요청은 주 연결 RTT에 대한 의존을 키우므로, 이를 서버와 분리된 무기한
  오프라인 직접 링크로 **설명해서는 안 됩니다**.

## 지원하는 서비스

비공개와 공개 TCP, UDP, 제한된 파일 서비스를 지원하며, 동적 공유의 SOCKS와 HTTP 포워드
프록시 대상도 지원합니다. 사이트 HTTP/SNI 진입점, 서버 로컬 및 중첩 라우트, 별도 프록시
계정이 필요한 대상은 원래 진입점을 우회하지 않습니다. `prefer_direct`는 원래 릴레이를
사용하고, `direct`는 거부합니다. PROXY 헤더가 활성화된 UDP 백엔드는 릴레이를
사용합니다. TCP PROXY 헤더는 제공자가 서버가 인증한 출발지 주소로 백엔드에
기록합니다.

## 배포 경계

목적지 의존 매핑이 있거나 UDP가 꺼져 있는 등의 경우에는 릴레이를 사용합니다. 실제 공개
NAT 종류와 통신사 네트워크는 동작하는 배포 환경에서 직접 검증해야 하며, 로컬 시뮬레이션을
공개 네트워크 결과로 취급하면 안 됩니다.

## 다음 단계

- [비공개 공유와 방문자](private-shares.md)
- [felucca-server 모듈](server.md)
- [접근 제어, 할당량과 스트림 부호화](security.md)
- 예제: [p2p-server.toml](../../../examples/p2p-server.toml) ·
  [p2p-provider.toml](../../../examples/p2p-provider.toml) ·
  [p2p-visitor.toml](../../../examples/p2p-visitor.toml)
