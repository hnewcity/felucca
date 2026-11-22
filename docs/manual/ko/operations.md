# 서비스, 업데이트, 로깅과 모니터링

이 페이지에서는 서비스 관리자에 Felucca를 등록하고, 바이너리를 안전하게 업데이트하고,
로그를 로테이션하고, 메트릭과 CPU 프로파일링을 사용하는 방법을 설명합니다.

## 시스템 서비스로 설치

두 프로그램은 같은 하위 명령어를 사용하며, `--name`으로 여러 인스턴스를 지원합니다.

```sh
felucca-server service --name office render --config /srv/felucca/server.toml
felucca-server service --name office --dry-run install --config /srv/felucca/server.toml
felucca-server service --name office install --config /srv/felucca/server.toml
felucca-server service --name office start
felucca-server service --name office status
felucca-server service --name office restart
felucca-server service --name office stop
felucca-server service --name office uninstall
```

| 플랫폼 | 관리자 | 기본 범위 |
| --- | --- | --- |
| Linux | systemd | `user`. `--scope system`은 머신 단위입니다 |
| macOS | launchd | `user`. `--scope system`은 머신 단위입니다 |
| Windows | SCM | `system`만 지원합니다 |

`render --platform systemd|launchd|windows`는 셸로 명령을 조합하지 않고도 지원되는 모든
호스트의 정의를 보여줍니다. `--workdir`의 기본값은 설정이 있는 디렉터리이며, 설정 안의
상대 경로는 그 위치를 기준으로 해석됩니다. `--binary`은 설치할 프로그램을 선택합니다.
추가 데몬 인수는 `--` 뒤에 넣습니다. 예를 들어
`-- --log-file /srv/felucca/agent.log`처럼 사용합니다.

설치는 같은 이름의 기존 서비스를 **덮어쓰지 않으며**, 서비스를 즉시 시작하지도 않습니다.
Linux에서는 서비스를 활성화하고, macOS에서는 다음 로그인/시작 시 또는 `start`를 명시적으로
실행할 때 부트스트랩합니다. 부팅 시 존재해야 하는 사용자 systemd 서비스는 배포자가
lingering을 활성화해야 하고, macOS 사용자 서비스는 그래픽 로그인 세션에 의존합니다.
머신 단위 서비스는 서비스 관리자의 기본 신원으로 실행되므로, 실행 사용자를 배포 환경에
맞게 수정하세요. Windows에서 설치 신원은 `NT AUTHORITY\LocalService`이며, 배포자가 설정,
로그, 상태, 파일 루트 디렉터리에 대한 권한을 직접 부여해야 합니다.

제거는 실행 파일, 설정, 로그, 인증서, 런타임 상태를 그대로 남겨 둡니다.

검증된 범위는 다음과 같습니다. 리포지터리는 정의 생성, macOS plist 파싱, 격리된 복사본
업데이트, 실제 CPU 프로파일링을 테스트합니다. 또한 Linux arm64 서버 이미지를 빌드해
OrbStack에서 실행했으며, 관리 로그인, 모니터링, 브리지 TLS, 컨테이너 재생성 후에도
유지되는 영속성을 확인했습니다.
배포 기록을 보세요. 시스템 서비스 설치와
Linux/Windows 서비스 관리자는 아직 검증하지 않았습니다.

## 바이너리 업데이트

신뢰할 수 있는 배포 채널에서 독립 실행형 실행 파일의 SHA-256을 받은 뒤, 실행 중인
프로그램을 검증하거나 교체합니다.

```sh
felucca-server update --source /downloads/felucca-server --sha256 <64-hex-digest> --check-only
felucca-server update --source /downloads/felucca-server --sha256 <64-hex-digest> --expect-version 0.1.0
felucca-agent  update --source https://releases.example/felucca-agent --sha256 <64-hex-digest>
```

다이제스트는 **신뢰의 기준**이며, 신뢰할 수 있는 배포 매니페스트에서 가져와야 합니다.
입력은 독립 실행형 바이너리여야 하므로, tar.gz/zip은 먼저 압축을 푸세요. HTTPS
내려받기는 인증서를 검증하고 리다이렉트를 따르지 않으며, 비공개 내려받기에는
`--token-env VARIABLE_NAME`을 사용합니다. 자격 증명은 로그에 기록되지 않습니다.
내려받기는 512 MiB와 5분으로 제한됩니다. `--version` 확인은 10초로 제한되며, 다이제스트가
일치한 뒤에만 실행되고 서버/에이전트 종류와 선택적 버전을 검사합니다. 실패하면 임시
파일을 정리하고, 업데이트 잠금이 동시 교체를 막습니다. 교체는 원래 파일 권한을 유지합니다.

이후에는 `service ... restart`를 명시적으로 실행해야 하며, 이미 실행 중인 프로세스는
기존 버전을 계속 사용합니다. 읽기 전용 이미지 파일 시스템에서는 컨테이너 안에서 업데이트가
동작하지 않으므로, 이미지를 교체하세요.

프로토콜 버전이 바뀌면 서버와 에이전트를 함께 업그레이드해야 합니다. 패키징 스크립트는
실제로 `--build-info`를 실행하며, 프로토콜 버전이나 대상이 다른 바이너리를 섞는 것을
거부합니다.

## 로깅

서버 쪽은 다음과 같이 설정합니다.

```toml
[log]
level = "info"
file = "/data/felucca.log"
json = true

[log.rotation]
max_bytes = 10485760
max_files = 5
interval = "24h"
```

출력은 파일과 stderr에 모두 기록됩니다. 로테이션은 완료된 이벤트 사이에서 일어나므로,
크기 임계값을 넘긴 이벤트가 하나 더 포함될 수 있습니다. 보관 파일은 `.1`부터 `.5`까지
번호가 붙고, `.1`이 가장 최신입니다. `interval`은 현재 파일을 연 시점부터 측정하며,
재시작하면 주기가 다시 시작됩니다. 배타적 잠금이 여러 프로세스가 같은 경로를 로테이션하는
것을 막고, Unix에서는 새 로그 파일과 잠금 파일이 0600입니다.

**서버 로그 설정을 바꾸려면 재시작이 필요하며, 관리 평면은 이 설정의 온라인 변경을
거부합니다.** 에이전트는 대신 명령줄 인수를 사용합니다.
[felucca-agent 모듈](agent.md)을 보세요.

## 모니터링

`GET /metrics`와 `GET /api/metrics`는 Prometheus 텍스트를 반환합니다. 관리 계정 Bearer
토큰이나 유효한 세션을 사용하세요. **익명 메트릭 엔드포인트는 없으며**, 메트릭은 계정
사용자의 공개 범위를 따릅니다.

제공하는 항목은 서비스 상태, 설정 리비전, 노드 온라인/활성 상태, 연결 수, 누적 노드
트래픽, 터널 준비 상태, 누적 터널 트래픽입니다. 트래픽 카운트는 기존 영속 MeterStore를
재사용하므로, `[state]`를 활성화하면 서버를 재시작해도 유지됩니다. 관리 UI의 모니터링
페이지는 호스트 CPU, 물리 메모리, 파일 시스템 용량, 네트워크 속도를 Prometheus 텍스트나
설정 설명 없이 대화형 선 그래프로 보여줍니다. 샘플은 5초마다 들어와 1시간 동안 메모리에
남고, 프로세스를 재시작하면 이력이 지워집니다. 5분, 15분, 1시간 창을 선택할 수 있습니다.
마우스를 올리거나 클릭해 고정하거나 화살표 키로 정확한 샘플 시각, 로컬 시간대, 값을 확인할
수 있습니다. 범위 슬라이더를 끌거나 확대/초기화 버튼을 써서 네 그래프를 함께 확대할 수
있으며, 실시간 갱신은 선택한 절대 시간 창을 유지합니다. 카드에는 보이는 관측값의 최솟값,
샘플 평균, 최댓값이 표시되고, 네트워크 범례에서 수신/송신 시리즈를 켜고 끌 수 있습니다. 호스트
메트릭은 admin 전용이며, 다른 역할은 권한이 있는 리소스의 트래픽과 연결만 봅니다.
구조화된 엔드포인트는 `GET /api/monitor?window=900`입니다(지원 창: 300, 900, 3600초).

## CPU 프로파일링

`[admin] profiling = true`를 설정하면 admin이 JSON 본문 `{"seconds":5}`로
`POST /api/profile`을 호출하거나, 모니터링 페이지에서 CPU 플레임 그래프를 내려받을 수
있습니다. Linux와 macOS를 지원하며, 다른 플랫폼은 활성화를 거부합니다. 한 번의 실행은
49 Hz로 1초에서 30초까지 가능하고, 프로세스에서 동시에는 하나의 작업만 허용되며, 출력은
8 MiB로 제한됩니다. HTTP 요청이 끊겨도 샘플링 잠금이 먼저 풀리지 않고, 데몬을 멈추면
샘플링도 취소됩니다. 유휴 기간에 샘플이 없으면 설명이 담긴 SVG를 반환합니다. 릴리스 빌드는
프로파일링에 필요한 함수 심볼은 유지하지만 디버그 라인 정보는 버립니다. 라인 단위 조회는
디버그 정보를 넣어 직접 빌드한 바이너리가 필요합니다.

## Docker

```sh
docker build --target server -t felucca-server:local .
docker build --target agent  -t felucca-agent:local .

docker run --rm -p 14028:14028 \
  --mount type=bind,src=/srv/felucca/config,dst=/etc/felucca,readonly \
  --mount type=bind,src=/srv/felucca/data,dst=/data \
  felucca-server:local
```

이미지는 기본적으로 UID/GID 10001을 사용합니다. 설정 디렉터리는 읽기 전용으로 마운트하고,
이미지가 데이터 디렉터리에 쓸 수 있게 하세요. 설정에서 필요한 추가 TCP/UDP 포트를 명시적으로
열고, 컨테이너 안에서 도달 가능한 주소로 바인드하세요. 다중 아키텍처 이미지는 Dockerfile의
네이티브 대상을 Buildx로 빌드하면 됩니다. `STOPSIGNAL`은 SIGTERM이며, 프로그램의 종료
경로와 일치합니다.

### 로컬 OrbStack

OrbStack을 시작한 뒤, 리포지터리 루트에서 `sh scripts/run-orbstack.sh`를 실행하세요. 이
스크립트는 관리 UI를 포함해 서버만 빌드하고 배포합니다. 호스트에는 Docker Compose,
Python 3, OpenSSL이 필요합니다. `http://127.0.0.1:8088`을 열고
`conf/orbstack/credentials.json`에 있는 무작위 비밀번호로 `root` 계정으로 로그인하세요.
관리 리스너는 루프백에만 바인딩합니다.

자격 증명과 영속 브리지 인증서는 Git에서 무시되는 `conf/orbstack/` 디렉터리에 있습니다.
설정과 트래픽 스냅샷은 `felucca_server-data` 볼륨에 있습니다. 자격 증명 파일에는 초기
비밀번호가 기록되며, 저장된 관리 설정이 부트스트랩 TOML보다 우선합니다. 스크립트를 다시
실행하면 현재 소스를 빌드해 기존 자격 증명, 인증서, 데이터를 유지한 채 컨테이너를
갱신합니다.

수동으로 멈추지 않는 한 컨테이너는 Docker 엔진과 함께 재시작합니다. 초기에는 클라이언트가
설정되어 있지 않습니다. 브리지 TCP와 QUIC은 포트 14028을 사용합니다. Compose는 OrbStack
호스트 네트워킹을 사용하므로, 추가 터널 포트에 개별 Docker 포트 매핑은 필요하지 않습니다.
다른 로컬 프로그램이 이미 쓰는 포트는 피하세요.

```sh
docker --context orbstack compose -f compose.orbstack.yaml logs --tail 100 server
docker --context orbstack compose -f compose.orbstack.yaml restart server
docker --context orbstack compose -f compose.orbstack.yaml down
```

`down`은 데이터 볼륨을 유지합니다. `-v`를 추가하면 영속 설정과 트래픽 데이터가 삭제됩니다.

## 패키징과 CI

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
python3 scripts/package.py --output dist
```

소스 빌드에는 Node.js 22.12 이상(24 권장)이 필요합니다. `apps/admin/`의 React + Vite 관리
UI는 서버 바이너리에 포함되며, 배포에는 Node.js가 필요하지 않습니다. 프런트엔드를 바꾼 뒤에는
다시 빌드하세요. Cargo는 번들이 없거나 오래되면 거부합니다.

아티팩트에는 두 프로그램, 예제, 문서, 바이너리별 다이제스트 매니페스트와 아카이브
다이제스트가 들어 있습니다. 패키징 호스트에서 실행할 수 없는 바이너리는 기본적으로
거부되며, 크로스 플랫폼 CI는 각 대상 플랫폼에서 패키징합니다. GitHub 워크플로는 Linux
amd64/arm64, macOS arm64, Windows amd64에 대해 검사와 테스트, 아카이브를 실행하고,
Rust 1.88 최소 버전과 Docker 빌드를 별도로 검증합니다. 워크플로는 작업 아티팩트만
업로드하며, GitHub 릴리스를 만들거나 이미지를 푸시하지 않습니다.

## 검증 체크리스트

```sh
felucca-server service --name office render --config conf/server.toml
felucca-server --config conf/server.toml --fingerprint
felucca-agent  --config conf/agent.toml  --check
```

`--fingerprint`는 설정이 고정 PEM 파일을 참조해야 합니다. `--check`가 통과했다고 해서
백엔드, DNS, 인증서 파일, 공개 포트가 이미 사용 가능하다는 뜻은 아닙니다.

## 다음 단계

- [felucca-server 모듈](server.md)
- [관리 UI와 중앙 집중 설정](management.md)
- [참조](reference.md)
