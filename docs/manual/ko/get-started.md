# 시작하기

직접 배포한 공개 Server를 사용하거나 운영자에게 연결 명령을 받으세요. 인증서 준비, 설정 파일 편집, agent 사전 설치가 필요하지 않습니다.

## 명령 한 줄로 기기 연결

1. **기기 → 기기 연결**에서 이름을 입력하세요. 기본 설정은 이 기기, LAN, 인터넷 접근을 허용합니다. 제한하려면 **고급 설정: 접근 권한**을 열고 이 기기, LAN 또는 사용자 지정 범위를 선택하세요.
2. 명령을 복사하여 기기의 터미널(macOS/Linux) 또는 PowerShell(Windows)에서 실행하세요. 프로세서를 감지하고 맞는 클라이언트를 다운로드하여 체크섬을 검증합니다.
3. 창을 열어 두세요. 온라인이 되면 **서비스 추가**를 선택하고 로컬 주소를 입력한 다음 공개 주소를 사용하세요. 연결이 끊기면 자동으로 다시 연결합니다.
4. 비공개 접근에서는 수신 기기를 만들고 다른 컴퓨터에서 해당 명령을 실행하세요. 수신자는 관리 콘솔 로그인이 필요하지 않습니다.

명령에는 기기 자격 증명이 포함됩니다. 의도한 사용자에게만 공유하세요. 다시 연결하려면 기존 연결 창을 닫은 후 같은 명령을 실행하세요. 서버에는 기기 OS에 맞는 패키지가 있어야 합니다. ZIP 다운로드, 폴더 권한 및 고급 설정은 선택 사항입니다.

## 직접 Server 실행

배포 패키지의 프로그램으로 쓰기 가능한 디렉터리에서 처음 한 번 실행하세요:

```sh
./felucca-server --init
```

설정을 만들고 임의의 root 암호를 표시한 뒤 서버를 시작합니다. 표시된 관리 주소를 여세요. 업그레이드나 백업 시 `felucca-server.toml` 및 전체 `felucca-server.data/` 디렉터리를 보존하세요. 이후에는 `--init` 없이 시작하세요:

```sh
./felucca-server
```

공개 배포에서는 `--init --admin-url https://console.example.com`을 사용하고 HTTPS 리버스 프록시가 Host를 유지하여 `127.0.0.1:8990`으로 전달하게 하세요. HTTP 주소를 지정하면 해당 포트를 공개합니다. 공개 접근에는 HTTPS를 사용하세요. 기기는 브리지 TCP 9001, 방문자는 공개 서비스 포트에 접근할 수 있어야 합니다. [배포 및 클라이언트 배포](../../management.md)를 참조하세요.

## 사전 준비

| 요구 사항 | 세부 사항 |
| --- | --- |
| Rust 1.88 이상 | 빌드용입니다. 이 리포지터리는 Rust 1.98.1로 검증합니다 |
| Python 3 + OpenSSL | 로컬 데모 스크립트 `scripts/demo-local.py`에서만 필요합니다 |
| 공개 IP가 있는 호스트 | `felucca-server`를 실행합니다 |
| 사설 네트워크의 호스트 | `felucca-agent`를 실행하며, 노출할 서비스가 있어야 합니다 |
| 로컬 TCP/UDP 바인드 권한 | 테스트에 필요합니다. 방화벽 규칙은 배포하는 사람이 관리합니다 |

Felucca는 시스템 서비스를 설치하지 않으며 방화벽 규칙도 바꾸지 않습니다.

## 옵션 1 — 로컬 데모

공개 네트워크 없이 자기 머신에서만 동작하는 모습(관리 페이지와 포트 포워딩)을 보고
싶다면:

```sh
python3 scripts/demo-local.py
```

이 스크립트는 디버그 바이너리를 점진적으로 빌드하고, 인증서와 무작위 자격 증명을
생성하고, 테스트용 웹사이트와 서버, 에이전트를 띄운 뒤 실제 포워딩을 검증하고, 마지막에
데모 페이지를 직접 엽니다. 터미널에 관리 주소, 계정, 비밀번호가 표시됩니다. 모든 데모
서비스를 멈추려면 `Ctrl+C`를 누르세요.

모든 리스너는 `127.0.0.1`에만 바인딩하고, 기본 포트가 이미 사용 중이면 빈 포트를
자동으로 고릅니다. 실행할 때마다 설정, 로그인 정보, 로그를 각자의
`target/local-demo/run-*` 디렉터리에 보관합니다.

| 플래그 | 효과 |
| --- | --- |
| `--no-open` | 브라우저를 자동으로 열지 않습니다 |
| `--no-build` | 바이너리가 이미 빌드되어 있으면 빌드를 건너뜁니다 |

## 옵션 2 — 빌드

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
```

결과는 `target/release/felucca-server`와 `target/release/felucca-agent`입니다.
`scripts/cargo-external.sh`는 Cargo 캐시를 워크스페이스가 있는 디스크에 둡니다. 디스크
여유가 충분하다면 `cargo`를 직접 호출해도 됩니다.

## 옵션 3 — 첫 터널 직접 만들기

### 1. 브리지 인증서 생성

브리지 인증서는 한 번만 생성하며, 개인 키는 보관합니다.

```sh
mkdir -p conf
umask 077
openssl req -x509 -newkey rsa:2048 -nodes -days 365 \
  -subj '/CN=felucca' -keyout conf/bridge.key -out conf/bridge.pem
cp examples/felucca-server.toml conf/server.local.toml
cp examples/felucca-agent.toml conf/agent.local.toml
```

### 2. 공유 키 설정

두 파일의 `vkey`는 같은 무작위 문자열이어야 합니다.

```sh
openssl rand -hex 32
```

이 값을 `conf/server.local.toml`의 `[client.agent-1].vkey`와
`conf/agent.local.toml`의 `vkey`에 기록하세요. 에이전트의 `key_id`는 서버의
`[client.<id>]` 테이블 항목과 일치해야 합니다. 에이전트의 `server`는 서버 호스트의
도달 가능한 주소로 바꾸세요.

### 3. 인증서 지문 읽기

```sh
target/release/felucca-server --config conf/server.local.toml --fingerprint
```

출력된 SHA-256 지문을 에이전트의 `[trust].fingerprint`에 넣으세요. 그런 다음 두 파일을
검사합니다.

```sh
target/release/felucca-server --config conf/server.local.toml --check
target/release/felucca-agent --config conf/agent.local.toml --check
```

### 4. 두 프로그램 시작

두 터미널에서 각각 하나씩 실행합니다.

```sh
target/release/felucca-server --config conf/server.local.toml
```

```sh
target/release/felucca-agent --config conf/agent.local.toml
```

### 5. 확인

예제 설정은 서버의 TCP `8080`을 에이전트 머신의 `127.0.0.1:3000`으로 전달합니다.
에이전트 머신에서 3000번 포트로 리스닝하는 서비스를 띄운 뒤, 서버의 8080 포트를
열어 보세요.

## 다음 단계

- [아키텍처와 개념](architecture.md) — 브리지, 전송, 터널의 의미
- [felucca-server 모듈](server.md) — 클라이언트 테이블, HTTP 리스너, 포트 인가
- [felucca-agent 모듈](agent.md) — 재연결, 이그레스 네트워킹, 다중 인스턴스
- [터널과 포트 매핑](tunnels.md) — TCP/UDP/HTTP/file/proxy 모드
- [참조](reference.md) — 모든 명령어와 필드
