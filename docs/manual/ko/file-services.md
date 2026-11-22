# 파일 서비스와 WebDAV

`mode = "file"`는 별도의 HTTP 서버 없이 에이전트에서 직접 파일을 서빙합니다. 전용 공개
포트를 쓸 수도 있고, 서버의 HTTP 도메인 진입점을 공유할 수도 있습니다. WebDAV, Range
요청, 디렉터리 목록, 계정별 인증을 지원합니다.

## 전용 포트

```toml
[[tunnel]]
id = 100
mode = "file"
port = 9001
bind = "127.0.0.1"

[tunnel.file]
root = "public"
prefix = "/files"
read_only = true
auto_index = true

[tunnel.file.auth.users]
reader = "replace-this-password"
```

| 필드 | 설명 |
| --- | --- |
| `root` | 필수입니다. 에이전트에 **이미 존재하는** 디렉터리이며, 프로세스 작업 디렉터리를 기준으로 합니다. 시작할 때 디렉터리 핸들을 엽니다 |
| `prefix` | URL 프리픽스 |
| `read_only` | 기본값은 `true`입니다 |
| `auto_index` | 디렉터리 목록을 제공합니다 |
| `auth.users` | 계정 테이블입니다. 비워 두면 익명으로 서빙합니다 |

`..`, 절대 경로, 심볼릭 링크로 `root` 밖으로 나가는 요청은 허용되지 않으며, COPY/MOVE의
대상도 같은 제약을 받습니다. `root`와 파일 계정 설정은 에이전트에 남아 있으며, 서버
등록 메시지에 **넣지 않습니다**.

## HTTP 도메인 진입점

`hosts`를 설정하면 파일 서비스가 서버의 HTTP/HTTPS/HTTP3 도메인 진입점을 사용하고,
`port`는 그 진입점과 일치해야 합니다.

```toml
[[tunnel]]
mode = "file"
port = 8443
hosts = ["files.example.com"]

[tunnel.file]
root = "public"
read_only = false
```

이 모드에서는 `tunnel.options.http.backend_http2 = true`도 설정할 수 있습니다. 출발지
ACL, 할당량, 속도 제한은 계속 적용됩니다. file 모드는 네트워크 백엔드, 대상 네트워크
정책, PROXY 출력, 원시 TLS 모드를 설정하지 **않습니다**.

## WebDAV 메서드

| 모드 | 허용 메서드 |
| --- | --- |
| 읽기 전용(기본) | `GET`, `HEAD`, `PROPFIND`, `OPTIONS` |
| `read_only = false` | `PUT`, `DELETE`, `MKCOL`, `COPY`, `MOVE`, `LOCK`, `UNLOCK` 추가 |

잠금은 같은 파일 터널의 여러 연결에서 공유되며, 에이전트를 재시작하면 해제됩니다.
Range 요청, 조건부 요청, 디렉터리 목록, `index.html`를 지원합니다. 대용량 파일은
청크 단위로 전송하고 백프레셔를 사용하며, 전체를 캐시하지 않습니다.

## 중앙 집중 설정의 파일

서버가 파일 서비스를 배포할 때는 에이전트에 미리 선언된 이름 붙은 디렉터리 권한만
참조할 수 있습니다.

```toml
# agent
[remote.file_root.documents]
path = "/srv/documents"
read_only = true
```

```toml
# server: inside [[managed.<client>.tunnel]]
[tunnel.file]
root = "documents/shared"
```

서버는 임의의 절대 경로를 배포할 수 없습니다. 경로와 심볼릭 링크가 허용된 디렉터리
밖으로 나갈 수 없으며, 읽기 전용 디렉터리는 원격 설정으로 쓰기 가능하게 바꿀 수
없습니다.

## 검증 체크리스트

1. `felucca-agent --config agent.toml --check`가 통과합니다.
2. `curl -i http://127.0.0.1:9001/files/`가 디렉터리 목록이나 `index.html`을
   반환합니다.
3. `Range: bytes=0-99` 요청이 206을 반환합니다.
4. 읽기 전용 모드에서 `PUT`가 405를 반환합니다.
5. `curl --path-as-is 'http://127.0.0.1:9001/files/../../etc/passwd'`가 거부됩니다.

## 다음 단계

- [터널과 포트 매핑](tunnels.md)
- [HTTP/HTTPS 리버스 프록시](http-services.md)
- [관리 UI와 중앙 집중 설정](management.md)
- 예제: [file-server.toml](../../../examples/file-server.toml) ·
  [file-agent.toml](../../../examples/file-agent.toml)
