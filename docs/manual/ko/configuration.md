# 설정 소스와 CLI

이 페이지에서는 설정이 어디서 오는지, 어떻게 전달되는지, 어떤 환경 변수가 적용되는지,
시작하지 않고 검증하는 방법을 설명합니다. 각 필드의 의미는 모듈 가이드를 보세요.

## 설정 파일

두 프로그램 모두 TOML과 JSON을 받습니다. 상대 경로는 프로세스 작업 디렉터리를 기준으로
해석되므로, `--workdir`와 systemd/launchd의 시작 디렉터리가 모두 영향을 줍니다.

```sh
felucca-server --config /srv/felucca/server.toml
felucca-agent  --config /srv/felucca/agent.toml
```

`--config`가 없으면 작업 디렉터리에서 `felucca-server.toml` /
`felucca-agent.toml`을 읽습니다. `-c`는 `--config`의 짧은 형태이며, 에이전트는
`--launch` 별칭도 받습니다.

모든 설정은 `deny_unknown_fields`를 사용합니다. 알 수 없는 필드는 오류이며, 조용히
무시되지 않습니다.

## 허용되는 소스

`--config`는 네 가지 형태를 받습니다.

| 형태 | 예제 | 세부 사항 |
| --- | --- | --- |
| 로컬 파일 | `conf/server.toml` | TOML 또는 JSON |
| HTTP(S) URL | `https://cfg.example/edge.json` | 최대 15초 대기하고 200을 요구합니다. 리다이렉트를 따르지 않고 압축 응답도 받지 않습니다. URL에는 userinfo와 프래그먼트를 쓸 수 없으며, 쿼리 파라미터로 서명된 내려받기 주소를 전달할 수 있습니다 |
| 인라인 JSON | `'{"bridge":{"bind":"0.0.0.0:14028"}}'` | JSON 객체를 직접 전달합니다 |
| base64 | `base64:eyJicmlkZ2Ui...` | 패딩이 없는 표준 base64 또는 base64url |

설정 하나의 크기는 최대 4 MiB입니다. `--config-token-env NAME`은 비공개 내려받기
주소의 Bearer 토큰을 환경 변수에서 읽습니다. 자격 증명은 로그에 기록되지 않습니다.
URL, 토큰, 인라인 본문은 설정 소스 이름으로 출력되지 않습니다.

원격 설정에서도 인증서, Geo 데이터, 파일 루트 경로는 여전히 **에이전트 머신**의
경로를 가리킵니다.

## 에이전트 실행 URI

`felucca-agent` 는 짧은 URI 도 받습니다. 기본으로는 연결만 하며 로컬 터널에는 TOML/JSON 이 필요합니다. `remote=1` 을 붙이면 터널과 접근 범위를 모두 서버가 배포하므로 명령 하나로 충분합니다.

```sh
felucca-agent --config 'felucca://bridge.example.com:14028?key_id=edge&vkey=YOUR_KEY&transport=tls'
felucca-agent --config 'felucca://bridge.example.com:14028?key_id=edge&vkey=YOUR_KEY&fingerprint=SHA256_HEX&remote=1'
```

| 필드 | 필수 | 세부 사항 |
| --- | --- | --- |
| Port | 예 | 0이 아니어야 합니다 |
| `vkey` | 예 | 공유 키입니다. 값은 퍼센트 인코딩해야 합니다 |
| `key_id` | 아니요 | 생략하면 기본 신원을 사용합니다 |
| `transport` | 아니요 | `tcp`/`tls`/`ws`/`wss`/`quic`/`kcp` |
| `fingerprint` | 아니요 | 고정 인증서 SHA-256 검증을 활성화합니다 |
| `ws_path` | 아니요 | WebSocket 경로 |
| `remote` | 아니요 | `1` 이면 서비스와 접근 범위를 모두 서버가 배포합니다. `remote.enabled` 와 `remote.console_scope` 를 켜고, 서버가 범위를 보내기 전까지는 로컬 주소만 허용합니다 |

알 수 없는 필드와 중복된 필드는 오류입니다.

## 에이전트 다중 인스턴스

`--config`는 반복할 수 있으며, 한 프로세스에서 최대 32개의 인스턴스를 실행합니다.

```sh
felucca-agent --config edge-a.toml --config edge-b.json
felucca-agent --config 'https://cfg.example/edge.json' --config-token-env CONFIG_TOKEN
```

각 인스턴스는 연결, 백오프, 라우트, 프로브, 연결 제한을 각자 유지합니다. 모든 설정을
먼저 검증하고 초기화한 뒤에야 연결이 시작되며, `server` + `key_id` 조합이 중복되면
오류입니다. 한 서버에 연결할 수 없어도 다른 인스턴스는 계속 동작하고, 종료 신호는 모든
인스턴스를 닫습니다.

## 환경 변수 확장

기본적으로 꺼져 있습니다. `--expand-env`(또는 `FELUCCA_EXPAND_ENV=true`)를 설정하면
해석된 문자열 값이 확장됩니다.

```toml
server = "${FELUCCA_SERVER:-127.0.0.1:9001}"
key_id = "${FELUCCA_KEY_ID:-agent-1}"
vkey = "${FELUCCA_VKEY}"
```

`${VAR}`는 변수가 존재해야 합니다. `${VAR:-default}`는 변수가 없거나 비어 있을 때
기본값을 사용합니다. `$$`는 문자 그대로의 `$`입니다. 확장은 한 번만 실행되고, 필드
이름은 건드리지 않으며, 문자열을 숫자로 변환하지 않습니다. 결과가 4 MiB를 넘으면
거부합니다.

## 런타임 설정 스냅샷

```sh
felucca-server --config bootstrap.toml --runtime-config conf/runtime.json
```

경로는 `FELUCCA_RUNTIME_CONFIG`로도 설정할 수 있습니다. 첫 시작에 성공하면 정규화된
설정이 원자적으로 저장되고, 이후 시작에서는 이 스냅샷을 먼저 읽습니다. `--check`와
`--fingerprint`도 같은 스냅샷을 읽습니다. 파일이 깨져 있으면 시작이 실패하며, 시작
파일로 대체하지 않습니다. `[state].path`의 트래픽 파일과 같은 경로로 설정**하지
마세요**.

스냅샷은 최대 16 MiB이며 실제 시크릿을 담고 있습니다. Unix에서는 새 파일의 권한이
0600, 새 디렉터리가 0700이고, 파일 잠금으로 다른 데몬이 동시에 사용하는 것을
막습니다.

관리 UI에서 설정을 바꾸는 것도 같은 메커니즘을 사용합니다.
[관리와 중앙 집중 설정](management.md)을 보세요.

## 검증과 로깅

```sh
felucca-server --config conf/server.toml --check
felucca-agent  --config conf/agent.toml  --check
```

`--check`는 TOML/JSON 구조, 필드 제약, 포트, ACL, Geo 데이터를 검증합니다. 백엔드에
연결하지 **않고**, 공개 DNS를 검증하지 **않으며**, 인증서 파일의 내용도 확인하지
**않습니다**. 인증서 내용과 바인드 충돌은 시작 시점에 검사합니다.

에이전트의 로깅 플래그는 다음과 같습니다.

```sh
felucca-agent -c agent.toml --log-level info \
  --log-file /data/agent.log --log-json \
  --log-max-bytes 10485760 --log-backups 5 --log-rotate-interval 24h
```

`RUST_LOG`는 양쪽의 로그 레벨을 덮어씁니다. 서버 로깅은 설정된 `[log]` 테이블로
기록되며, 바꾸려면 재시작이 필요합니다.

## 환경 변수

| 변수 | 효과 |
| --- | --- |
| `FELUCCA_CONFIG` | `--config`와 동일합니다(에이전트에서는 반복 가능) |
| `FELUCCA_RUNTIME_CONFIG` | 서버의 `--runtime-config`와 동일합니다 |
| `FELUCCA_EXPAND_ENV` | `--expand-env`와 동일합니다 |
| `FELUCCA_CONFIG_TOKEN_ENV` | 원격 설정 Bearer 토큰을 담은 변수의 이름입니다 |
| `RUST_LOG` | 로그 필터 레벨을 덮어씁니다 |

## 다음 단계

- [felucca-server 모듈](server.md)
- [felucca-agent 모듈](agent.md)
- [시스템 서비스, 업데이트, 로깅과 모니터링](operations.md)
