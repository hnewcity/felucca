# 관리 UI와 중앙 집중 설정

`felucca-server`는 기기, 서비스, 비공개 접근 및 설정 관리 화면을 제공합니다. 일상적인 변경은 온라인으로 동기화되며 변경하지 않은 서비스는 계속 실행됩니다. 전역 리스너 등 기반 설정은 전달 서비스를 다시 로드하지만 관리 API는 유지됩니다.

## 설정 파일 편집 없이 연결하기

1. **기기 → 기기 연결**에서 이름과 서버 주소를 입력하고 운영체제와 프로세서를 선택하세요. 클라이언트 패키지를 풀고 `Connect.command`(macOS) 또는 `Connect.cmd`(Windows)를 여세요. Linux에서는 압축을 푼 폴더에서 `sh connect.sh`를 실행하세요. 프로그램과 연결 정보가 포함되어 별도 다운로드나 편집이 필요 없습니다.
2. **서비스 → 서비스 추가**에서 기기와 종류, 해당 기기의 앱 주소를 선택합니다. 공개 접근은 서버 포트나 도메인을 사용하고 비공개 접근은 허용한 수신 기기에 제공됩니다. 식별자와 인증 정보는 자동으로 생성됩니다.
3. 수신 기기도 개별 클라이언트 패키지를 사용하며 표시된 로컬 주소로 접속합니다. 로그인 후 자동 실행하려면 현재 연결 창을 닫고 `Install.command`, `Install.cmd` 또는 `sh install.sh`를 실행하세요. 클라이언트는 개인 폴더에 저장됩니다.

주 메뉴는 개요, 기기, 서비스, 비공개 접근, 모니터링, 설정입니다. 계정, 할당량, 진단, 서버 설정과 외부 연동은 설정에서 관리합니다. 일반 작업과 고급 설정에 JSON 편집기가 필요하지 않습니다.

설정에서 직접 연결을 활성화하면 수신 기기는 직접 연결을 먼저 시도하고 실패하면 서버 중계를 사용합니다. 비활성화하면 중계를 사용합니다. 시스템 전체 VPN이 아닌 서비스별 연결입니다. 파일 공유는 기기에 존재하는 폴더를 허용해야 합니다. 연결 파일에는 인증 정보가 있으므로 보관하세요. 수신 기기는 원본 파일을 사용하고 분실 시 새 연결을 만드세요. 기기, 서비스, 접근 권한 변경은 전체 전달 서비스를 다시 시작하지 않습니다. 변경하거나 비활성화한 서비스의 연결만 종료합니다. 다른 서비스와 이름 변경은 연결을 유지합니다. 구형 agent는 해당 기기만 다시 연결합니다. 브리지, HTTP, P2P 리스너 등 기반 설정은 다시 로드합니다.

서버에 있는 클라이언트 빌드만 제공됩니다. Apple 공증을 받지 않은 macOS 빌드는 표시되며 정상 다운로드 후 실행이 차단될 수 있습니다. 시스템 보호를 끄지 말고 관리자에게 정식 릴리스를 요청하세요.

## 활성화

```sh
target/release/felucca-server --hash-password      # read one password line from stdin; >=12 bytes
target/release/felucca-server --generate-token     # Bearer token and its stored hash
```

```toml
[admin]
bind = "127.0.0.1:8088"
state_path = "state/runtime-config.json"
profiling = true
# required for non-loopback binds: the origin the browser uses; use https://
# unless you accept plaintext management traffic (then http://host:port)
# origin = "https://admin.example.com"

[admin.account.root]
role = "admin"
password_hash = "pbkdf2-sha256$600000$..."
```

설정 파일은 TOML 또는 JSON입니다. 예제는 관리 주소 `http://127.0.0.1:8088`와 계정
`root`를 사용합니다. 예제 비밀번호는 로컬 데모 전용이므로, 실제 배포에서는 여러분의
해시로 교체하세요.

## 계정과 역할

| `role` | 권한 |
| --- | --- |
| `admin` | 모든 리소스와 계정을 관리합니다. `user` 필드는 설정하지 않습니다 |
| `operator` | 배정된 `user`의 노드와 중앙 집중 터널을 관리합니다 |
| `viewer` | 배정된 `user`의 리소스를 읽기 전용으로 조회합니다 |

operator와 viewer 계정은 반드시 존재하는 사용자를 참조해야 합니다. 서버 로컬 서비스는
로컬 디렉터리에 접근할 수 있으므로, admin만 수정할 수 있습니다. 사용자 할당량, 관리
계정, 전체 설정도 admin 전용입니다. 활성화된 admin 계정은 최소 한 개는 유지하세요.
`disabled = true`는 계정을 비활성화합니다.

비밀번호는 무작위 솔트와 고정 600000회 반복으로 PBKDF2-SHA256를 사용합니다. 세션은
메모리에 보관합니다. 쿠키는 HttpOnly와 SameSite=Strict를 갖고, HTTPS를 설정하면
Secure도 함께 표시됩니다. 브라우저 쓰기 요청에는 같은 출처의 Origin과 CSRF 토큰이
필요합니다. 비밀번호 변경, 계정 비활성화, 계정 삭제는 기존 세션을 무효화하며, 프로세스를
재시작해도 쿠키 세션은 무효화됩니다.

자동화에는 Bearer 토큰을 사용하세요. `--generate-token`이 출력한 `token_hash`를 계정의
`token_hash`에 넣고, 각 호출마다 원시 토큰을 보냅니다. Bearer 요청에는 쿠키 CSRF
토큰이 필요하지 않지만, 계정 권한 검사는 그대로 적용됩니다. **원시 토큰은 서버 설정에
절대 기록하지 마세요.**

## 쓰기 트랜잭션

모든 쓰기 요청에는 두 개의 헤더가 필요합니다.

```text
If-Match: "current configuration revision"
Idempotency-Key: unique identifier for this operation
```

- 오래된 리비전은 409를 반환합니다.
- 같은 ID에 같은 요청이면 이전 결과를 반환하고 데이터 평면을 **다시 재시작하지
  않습니다**. 같은 ID에 다른 요청이면 400을 반환합니다.
- 최근 성공한 작업 256개가 설정과 함께 저장되므로, DELETE 같은 작업의 재시도는
  재시작한 뒤에도 인식됩니다.
- `durable = false`는 디렉터리 동기화가 아직 확인되지 않았음을 뜻합니다.
- 기기, 서비스, 접근 권한 변경은 전체 전달 서비스를 다시 시작하지 않습니다. 변경하거나 비활성화한 서비스의 연결만 종료합니다. 다른 서비스와 이름 변경은 연결을 유지합니다. 구형 agent는 해당 기기만 다시 연결합니다. 브리지, HTTP, P2P 리스너 등 기반 설정은 다시 로드합니다.

관리 바인드 주소, 인증서 파일 경로, 공개 출처, 상태 경로를 바꾸려면 프로세스를 재시작해야
합니다. 서버가 실행 중일 때는 바꿀 수 없습니다.

## UI 페이지

| 페이지 | 내용 |
| --- | --- |
| 개요 | 현재 계정에서 볼 수 있는 기기, 서비스 및 연결 상태입니다. |
| 기기 | 연결된 컴퓨터나 서버를 선택하여 서비스를 관리하세요. |
| 서비스 | 기기와 접속할 앱을 선택하세요. Felucca가 선택한 접속 주소로 연결합니다. |
| 비공개 접근 | 비공개 서비스를 선택하고 다른 기기의 연결 파일을 만드세요. 서비스의 공개 포트는 필요 없습니다. |
| 모니터링 | 호스트 지표, 연결 수 및 누적 트래픽을 확인합니다. |
| 설정 | 서버 연결, 접근 권한, 외부 연동을 관리합니다. 일상 작업은 주 탐색에서 할 수 있습니다. |

## API

API는 JSON을 사용하고, 요청 본문은 최대 2 MiB까지 받으며, 브라우저의 교차 출처 요청은
거부합니다. 읽기는 보통 `{revision, config}`를 반환하며, 설정을 읽을 때는 기본적으로
시크릿을 가립니다. 편집할 때 기존 `<redacted>` 자리 표시자를 유지하면 그 필드의 현재
값이 그대로 유지됩니다. 새 리소스의 자격 증명에는 이 자리 표시자를 쓸 수 없습니다.

| 경로 | 메서드와 목적 |
| --- | --- |
| `/api/login` | POST `{account, password}`. Origin이 필요합니다 |
| `/api/session` | GET 현재 신원과 CSRF 토큰을 조회합니다 |
| `/api/logout` | POST 쿠키 세션을 해제합니다 |
| `/api/overview` | GET 온라인 노드, 연결, 트래픽, 등록된 터널을 조회합니다 |
| `/api/users`, `/api/users/<id>` | GET 목록 또는 단일 항목 조회. 단일 항목에는 PUT과 DELETE |
| `/api/clients`, `/api/clients/<id>` | GET 목록 또는 단일 항목 조회. 단일 항목에는 PUT과 DELETE |
| `/api/managed/<client>` | GET, PUT 중앙 집중 설정 전체. DELETE는 로컬 설정을 복원합니다 |
| `/api/managed/<client>/tunnels/<id>` | GET, PUT, DELETE 단일 터널 |
| `/api/locals`, `/api/locals/<id>` | GET 목록 또는 단일 항목 조회. 단일 항목에는 PUT과 DELETE |
| `/api/locals/<group>/tunnels/<id>` | PUT, DELETE 서버 로컬 터널 |
| `/api/shares`, `/api/shares/<name>` | GET, PUT, DELETE 비공개 공유 |
| `/api/accounts`, `/api/accounts/<id>` | GET 목록 조회. 단일 항목에는 PUT과 DELETE |
| `/api/config` | GET 시크릿이 가려진 전체 설정. PUT은 전체 설정을 가져옵니다 |
| `/api/config?secrets=1` | 자격 증명을 포함해 명시적으로 내보내는 admin 전용 기능입니다 |
| `/api/validate` | POST 리스너를 열지 않고 전체 설정을 검증합니다 |
| `/api/batch` | POST 원자적인 변경 묶음. 최대 128개 작업 |
| `/api/events` | GET 설정 리비전 변경을 위한 SSE. 하트비트는 10초이며 세션을 다시 확인합니다 |
| `/metrics`, `/api/metrics` | GET Prometheus 메트릭. 계정 권한으로 필터링합니다 |
| `/api/profile` | POST 사용 시간이 제한된 CPU 플레임 그래프를 내려받습니다 |

배치는 리비전 하나와 작업 ID 하나를 사용합니다. 모든 작업을 먼저 검증한 뒤 적용하며,
하나라도 권한 밖이거나 설정이 올바르지 않으면 앞선 작업은 하나도 커밋되지 않습니다.

```json
{
  "operations": [
    {
      "method": "PUT",
      "path": "/api/managed/office/tunnels/1",
      "value": {
        "id": 1,
        "enabled": false,
        "mode": "tcp",
        "port": 18080,
        "backend": [{"addr": "127.0.0.1:3000"}]
      }
    }
  ]
}
```

계정을 `PUT`할 때 `password`를 보내면 서버가 해시를 생성합니다. 생략하면 기존
`password_hash`를 유지합니다.

## 중앙 집중 설정

서버는 에이전트가 실행해야 할 설정을 `[managed.<client-id>]`에 저장합니다.

```toml
[[managed.office.tunnel]]
id = 1
mode = "tcp"
port = 18080
bind = "127.0.0.1"
backend = [{ addr = "127.0.0.1:3000" }]
```

에이전트는 `[remote] enabled = true`를 명시적으로 활성화해야 하며, 주 연결은 인증된
TLS/WSS/QUIC을 사용해야 합니다. 서버는 실제 브리지가 암호화되어 있는지도 확인하므로,
평문 연결로는 설정을 요청할 수 없습니다. 배포는 다이제스트를 붙인 표준 JSON을 사용하며,
설정의 자격 증명은 제어 메시지 디버그 로그에 나타나지 않습니다.

관리 평면은 터널 내용만 바꿀 수 있습니다. 새 에이전트 신원, 출구 프록시, 서버 로컬 인가
경계는 배포할 수 없습니다. 파일 서비스는 이름 붙은 디렉터리 권한을 미리 설정해야
합니다. [파일 서비스와 WebDAV](file-services.md)를 보세요.

예외로, 에이전트가 `remote.console_scope = true` 를 설정하면 `[managed.<client-id>]` 의 `target_policy` 가 대상 범위를 대체합니다.

| 작업 | 효과 |
| --- | --- |
| `tunnel.enabled = false` | 설정과 안정적인 ID는 유지하지만, 리스너를 등록하지 않고 백엔드 프로브도 시작하지 않습니다 |
| `managed` 항목을 빈 상태로 유지 | 그 노드의 중앙 집중 터널을 모두 중단합니다 |
| `managed` 항목 전체를 삭제 | 에이전트의 시작 시점 로컬 터널 설정을 복원합니다 |

관리 페이지는 "게시됨", "노드 적용 대기", "비활성화"를 구분해 표시합니다.

## 검증 체크리스트

1. `felucca-server --config server.json --check`가 통과합니다.
2. 브라우저에서 관리 주소를 열어 로그인하고, JS 오류가 없는지 확인합니다.
3. TCP 터널을 만들고 백엔드가 실제로 콘텐츠를 반환하는지 확인합니다.
4. `GET /api/config`에 평문 시크릿이 없는지 확인합니다.
5. 서버를 재시작한 뒤에도 설정과 터널이 남아 있고, 같은 `Idempotency-Key`로 재시도하면
   변경이 두 번 적용되지 않는지 확인합니다.

## 다음 단계

- [외부 플랫폼 연동](platform.md) — 다른 시스템에서 이 노드를 관리하기
- [felucca-agent 모듈](agent.md)의 `[remote]` 섹션
- [서비스, 업데이트, 로깅과 모니터링](operations.md)
- 예제: [admin-server.toml](../../../examples/admin-server.toml) ·
  [managed-agent.toml](../../../examples/managed-agent.toml)

콘솔에서 백엔드 가중치와 TLS, 상태 검사, HTTP 경로, 서비스 인증, ACL과 할당량, 파일 서비스, 로컬 터널 그룹 및 포트 매핑을 구조화 양식으로 설정할 수 있습니다.  계정 페이지에서 관리 토큰을 생성, 교체 및 폐기할 수 있으며 새 토큰은 저장 후 다운로드됩니다.

검증 시 재시작이 필요한 설정을 표시합니다. 재시작 내보내기는 자격 증명이 포함된 시작 설정, 실행 스냅샷 및 오프라인 교체 안내를 제공합니다. 먼저 프로세스를 중지하고 기존 파일을 백업하세요. 실행 스냅샷은 시작 파일보다 우선하며 `--runtime-config`로 지정한 경로도 포함됩니다. 트래픽 및 플랫폼 저널을 유지하고 실행 중인 스냅샷을 덮어쓰지 마세요.

진단에는 터널별 트래픽, 기존 백엔드 검사 결과, 활성 P2P 세션 및 최근 중계 이벤트가 표시됩니다. 관리자는 활성 인증서를 확인하고 실패한 ACME 요청을 재시도하며 PEM 인증서와 키를 설정 초안에 업로드하고 자격 증명을 가린 제한된 로그 끝부분을 읽을 수 있습니다. 선택적 진단 스트림이 없는 이전 agent는 사용 불가로 표시되지만 전달은 계속됩니다. 진단을 읽을 때 추가 검사는 실행하지 않습니다. Agent 및 방문자 설정 생성은 로컬 디렉터리와 대상 승인 경계를 유지합니다. 생성 파일을 agent 호스트에서 검토하고 시작 전에 그곳에서 `--check`를 실행하세요. 서비스와 업데이트 명령은 검토용이며 콘솔에서 실행하지 않습니다.
