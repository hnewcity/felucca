# 외부 플랫폼 연동

`[node]`는 외부 제어 평면이 이 노드를 관리할 수 있게 합니다. 별도 토큰으로 관리 API를
호출하거나, 노드가 바깥으로 연결해 역방향 WebSocket으로 요청을 받을 수 있습니다. 설정
변경을 서명된 콜백으로 밀어낼 수도 있고, 외부 트래픽을 가져올 수도 있습니다. 이것은
Felucca 자체의 제어 프로토콜이며, nps와는 와이어 수준에서 호환되지 않습니다.

## 활성화와 범위

```toml
[node]
id = "edge-a"
state_path = "state/node-events.json"
changes_window = 1024
usage_interval = "10s"

[node.platform.panel]
enabled = true
token = "replace-with-a-random-platform-token"
scope = "account"
user = "office"
mode = "direct"
```

| 필드 | 값과 의미 |
| --- | --- |
| `scope` | `full`(전체 관리. `user`는 함께 설정할 수 없습니다) 또는 `account`(존재하는 `user`를 반드시 설정해야 하며, 그 사용자의 클라이언트와 터널만 관리하고 이 테넌트의 콜백 상태만 읽을 수 있습니다) |
| `mode` | `direct`(토큰이 공개 관리 HTTP API에 접근할 수 있습니다), `reverse`(역방향 WS RPC 전용. 이 토큰으로 관리 HTTP API를 직접 호출하면 401을 반환합니다), `dual`(양방향 모두) |
| `enabled = false` | 두 진입점이 모두 멈추고 기존 역방향 연결이 닫힙니다 |

토큰은 최소 16 바이트이며, 플랫폼마다 서로 다른 토큰을 써야 합니다. HTTP에서는
`Authorization: Bearer <token>`를 보내세요. 플랫폼 신원은 로컬 관리 계정과 별개입니다.
토큰, 범위, 연결 설정을 바꾸면 역방향 연결이 교체되며, 이전 연결의 요청은 현재 설정으로
검사합니다.

공개 주소는 HTTPS/WSS를 사용해야 합니다. 평문 HTTP/WS는 루프백 주소에만 허용됩니다.
WSS는 기본적으로 공개 CA를 검증하며, 대신
`trust = { mode = "pinned", fingerprint = "..." }`를 쓸 수 있습니다. 안전하지 않은 신뢰
방식은 허용되지 않습니다. HTTP 콜백은 공개 CA를 검증하고 리다이렉트를 따르지 않습니다.

## 직접 관리 API

쓰기는 JSON, `If-Match: "<configuration revision>"`, 고유한 `Idempotency-Key`를
사용합니다. 브라우저 쿠키 인증에서는 Origin과 CSRF 토큰도 추가로 필요합니다.

| 엔드포인트 | 동작 |
| --- | --- |
| `GET /api/node/status`, `/api/node/registration` | 노드 ID, 부팅 ID, 영속 epoch, 버전, 이벤트 창, 보이는 플랫폼 연결 상태 |
| `GET /api/node/usage` | 현재 범위의 클라이언트 온라인 상태, 연결 수, 누적 양방향 바이트 |
| `GET /api/node/changes?after=<epoch>:<sequence>&limit=100` | 영속 변경 창. `limit`는 1–512입니다 |
| `GET /api/node/callbacks?target=...&offset=0&limit=100` | 보이는 콜백 큐, 전달 통계, 페이지 단위 항목 |
| `POST /api/node/callbacks/replay` | `{"ids":["delivery-uuid"]}`. 실패 상태를 지우고 다시 전달합니다 |
| `POST /api/node/callbacks/delete` | 지정한 보이는 항목만 삭제합니다 |
| `POST /api/node/kick` | `{"ids":["client-name"]}`. 클라이언트를 영구히 비활성화하고 연결을 끊어 즉시 재연결하지 못하게 합니다 |
| `POST /api/node/sync` | 관리자가 영속 로컬 설정을 다시 활성화하고 오래된 런타임 상태, 캐시, 연결을 정리합니다 |
| `POST /api/node/traffic` | 외부 트래픽 델타를 가져와 클라이언트와 소유 사용자 카운터를 원자적으로 누적합니다 |
| `GET/PUT/DELETE /api/platforms/<name>` | 관리자의 플랫폼 관리 |
| `GET/PUT/DELETE /api/webhooks/<name>` | 관리자가 영속 웹훅을 생성, 수정, 삭제합니다 |

설정 변경 묶음, 전체 내보내기와 복원은 `/api/batch`와 `GET/PUT /api/config`를
재사용합니다. 동기화와 kick은 같은 설정 트랜잭션과 멱등 이력을 재사용하고 설정
리비전을 높입니다. 콜백 작업과 트래픽 가져오기는 설정 리비전을 바꾸지 않습니다.

### 트래픽 가져오기

`state.path`를 반드시 설정해야 합니다.

```json
{"items":[{"client":"office","inlet_bytes":1234,"export_bytes":5678}]}
```

요청 하나는 최대 128개 항목을 담습니다. 바이트 수는 부호 없는 정수여야 하며, 같은
클라이언트의 여러 항목은 먼저 합칩니다. 모든 권한 검사를 통과할 때까지 아무것도
쓰지 않으며, 항목 하나라도 범위를 벗어나거나 올바르지 않으면 묶음 전체를 거부합니다.
**이 노드의 데이터 평면이 이미 계량하지 않은** 트래픽을 가져오세요. 노드가 이미 계량한
트래픽을 다시 가져오면 두 번 계산됩니다. 첫 가져오기는 스냅샷을 버전 2로 올리며, 이전
빌드는 이를 읽을 수 없습니다.

## 역방향 WSS 프로토콜 v1

`[node]`를 활성화하면 `GET /api/ws`가 HTTP/1.1 WebSocket 업그레이드를 받습니다. 네이티브
클라이언트는 Bearer 토큰을 사용합니다. 브라우저는 같은 출처의 로그인 쿠키를 쓰거나,
서브프로토콜 `["felucca-node-v1", "felucca-token.<unpadded base64url token>"]`로 토큰을
전달할 수 있습니다. 서버는 `felucca-node-v1`만 선택하며, URL 파라미터를 통한 인증은
받지 않습니다. 직접 관리 WebSocket은 동시에 최대 16개까지 유지합니다.

노드가 `reverse_url`로 연결하며, 핸드셰이크에 플랫폼 Bearer 토큰과
`felucca-node-v1` 서브프로토콜이 담깁니다. 플랫폼은 응답에서 그 서브프로토콜을
선택해야 합니다. 모든 애플리케이션 메시지는 JSON 텍스트입니다.

노드가 먼저 `hello`(node, boot, epoch, cursor, max_message, max_inflight,
heartbeat_ms)를 보내고, 플랫폼이 요청을 보냅니다.

```json
{"type":"request","id":"request-1","method":"GET","path":"/api/clients"}
```

```json
{"type":"request","id":"request-2","method":"POST","path":"/api/node/kick","headers":{"if-match":"\"3\"","idempotency-key":"kick-001"},"body":{"ids":["office"]}}
```

응답은 `{"type":"response","id":"request-1","status":200,"headers":{},"body":{...}}`
형태입니다. RPC는 일반 관리 핸들러의 인증, 테넌트 필터링, CAS, 멱등 동작을
재사용합니다. If-Match, Idempotency-Key, Accept, X-Felucca-CSRF는 전달할 수 있지만,
Host와 Authorization은 덮어쓸 수 없습니다. 로그인, 로그아웃, SSE, CPU 샘플링,
WebSocket 업그레이드 자체는 RPC를 통하지 않습니다.

| 제한 | 값 |
| --- | --- |
| 최대 메시지 | 2 MiB |
| 연결당 동시 RPC | 2 |
| 노드 전체 동시 RPC | 16. 초과 요청은 429를 반환합니다 |
| 진행 중인 요청 ID 중복 | 연결이 닫힙니다 |
| 요청 타임아웃 | 504. 같은 멱등 키로 쓰기를 재시도하세요 |

양쪽 모두 `{"type":"ping","nonce":1}`를 보낼 수 있습니다. 노드는 응답 없는 ping을
덮어쓰지 않으며, 하트비트 주기 두 번을 넘도록 대응하는 pong이 오지 않으면 연결을 닫고
1–30초의 백오프로 재연결합니다. 설정 변경은 이벤트를 보내고, 구독이 뒤처지면
재동기화를 보냅니다. `usage_interval`을 설정하면 노드가 테넌트 필터링한 사용량 메시지를
주기적으로 보냅니다.

## 서명된 콜백

```toml
[node.webhook.events]
user = "office"

[node.webhook.events.delivery]
enabled = true
url = "https://panel.example/felucca/events"
signing_key = "replace-with-a-random-callback-signing-key"
timeout = "5s"
retry_max = 2
retry_backoff = "2s"
queue_max = 100
```

요청 헤더는 다음과 같습니다.

| 헤더 | 내용 |
| --- | --- |
| `X-Felucca-Event-Id` | 전달 UUID. 자동 재시도와 수동 재생 내내 같게 유지됩니다 |
| `X-Felucca-Timestamp` | 이 요청의 밀리초 단위 Unix 시간 |
| `X-Felucca-Signature` | `v1=` 뒤에 소문자 16진수 HMAC-SHA256 |

서명 입력은 UTF-8 프리픽스에 **원시 요청 본문 바이트**를 그대로 이어 붙인 것입니다.

```text
felucca-callback-v1\n<timestamp>\n<event-id>\n<body>
```

받는 쪽은 서명을 검증하고, 시간 창을 확인하고, 이벤트 ID로 중복을 제거해야 합니다.
2xx만 성공으로 셉니다. `retry_max`는 첫 전달 이후 추가 시도 횟수이며, `retry_backoff`는
고정 간격입니다. 전달은 **최소 한 번**입니다. 성공 확인이 디스크에 기록되기 전에 노드가
종료되면, 재시작 후 같은 ID가 다시 전달될 수 있습니다. 콜백은 설정 시크릿이 아닌 변경
메타데이터만 담으며, 이벤트를 받은 뒤 필요한 리소스는 현재 인가된 API로 가져오세요.

`queue_max`는 대상별 영속 대기와 실패 항목 수를 제한합니다. 한도를 넘으면 가장 오래된
항목을 버리고 dropped로 계산하며, `0`은 최종 실패 항목을 보관하지 않습니다. 노드 전체
최대는 4096 항목입니다. 구독을 비활성화해도 기존 큐는 유지됩니다. 구독을 삭제하거나
수신 URL, 서명 키, 필터, 템플릿을 바꾸면 오래된 큐가 정리됩니다.

이벤트 창은 `node.started`, `config.changed`, `config.resync`와 클라이언트, 사용자,
중앙 집중 터널, 도메인의 생성, 수정, 삭제 메타데이터를 기록합니다. 조회는 테넌트로
필터링됩니다. 만료되었거나 너무 앞서 있거나 epoch가 맞지 않는 커서는 410/reset_required를
반환하며, 이 경우 현재 설정을 먼저 가져온 뒤 반환된 위치부터 구독을 이어가세요. 이
범위가 제한된 창은 전체 설정 diff를 보관하지 않으며 장기 감사 로그도 아닙니다.

## 실시간 구독과 템플릿

직접 연결과 역방향 WebSocket 연결 모두 이 **WS RPC 전용** 경로를 사용할 수 있습니다.
`GET /api/realtime/subscriptions`와 `GET/PUT/DELETE /api/realtime/subscriptions/<name>`
입니다. 구독은 영속 설정을 수정하지 않으며 If-Match도 필요하지 않습니다. 연결당 최대
32개 구독, 각각 최대 32 KiB이며, 연결이 끊기면 모두 해제됩니다.

```json
{
  "enabled": true,
  "selector": {"event_names":["client.updated"],"clients":["office"]},
  "content": {
    "body_template": {"message":"node {{event.client}} updated","event":"{{event}}"},
    "header_templates": {"x-event-name":"{{event.kind}}"}
  }
}
```

템플릿은 JSON 값이며 스크립트를 실행하지 않습니다. `{{node}}`, `{{id}}`, `{{epoch}}`,
`{{event}}`, `{{event.kind}}`와 리소스, 동작, 사용자, 클라이언트, 터널, 호스트, 대상,
시퀀스, 리비전, timestamp_ms, 트래픽을 지원합니다. 문자열 전체가 자리 표시자 하나이면
원래 JSON 타입이 유지됩니다. 템플릿은 인증, 쿠키, HTTP 길이/연결 헤더, `X-Felucca-*`
서명 헤더를 덮어쓸 수 없습니다.

## 트래픽 알림

`traffic_report_interval`(100ms–1h)과 `traffic_report_step_bytes`는 가져온 트래픽의
알림을 활성화합니다. 트래픽 할당량이 있는 클라이언트를 처음 가져올 때, 그리고 이후에는
시간이나 바이트 임계값에 도달할 때 `client.traffic.reported` 알림을 보냅니다. 알림은
`ephemeral: true`를 담고 있어 변경 이력, 실패 큐, 재시도에 들어가지 않습니다. 느린
수신자는 이를 놓칠 수 있으므로, 전체 데이터가 필요할 때는 `/api/node/usage`를
읽으세요.

## 검증 체크리스트

1. `GET /api/node/status`가 노드 ID, epoch, 플랫폼 연결 상태를 반환합니다.
2. 역방향 WSS 연결이 `hello` → `request` → `response` 왕복을 완료합니다.
3. 콜백 수신자가 서명 검증에 성공하고, 같은 이벤트 ID를 재생해도 비즈니스 효과가
   중복되지 않습니다.
4. `kick` 이후 클라이언트가 연결 해제되고 즉시 재연결할 수 없습니다.
5. 트래픽을 가져온 뒤 `/api/node/usage`의 누적 바이트가 즉시 변합니다.

## 다음 단계

- [관리 UI와 중앙 집중 설정](management.md)
- [felucca-server 모듈](server.md)
- 예제: [node-server.toml](../../../examples/node-server.toml)
