# Felucca 문서 색인 · 한국어

[中文](../README.md) · [English](en/README.md) · [日本語](ja/README.md) · 한국어 · [Français](fr/README.md) · [Español](es/README.md) · [Русский](ru/README.md)

이 문서 모음은 Felucca 사용자 문서입니다. 첫 터널을 만드는 방법부터 모든 모듈의 설정과 사용
방법, 그리고 명령어·필드·엔드포인트·용어 참조까지 다룹니다. 여기서는 **Felucca를 어떻게
사용하는지**를 설명하며, 내부 구현은 다루지 않습니다.

## 시작하기

| 페이지 | 내용 |
| --- | --- |
| [시작하기](get-started.md) | 설치, 로컬 데모, 수동으로 만드는 첫 터널 |
| [아키텍처와 개념](architecture.md) | 구성 요소, 트래픽 경로, 용어집 |
| [설정 소스와 CLI](configuration.md) | 설정 소스, 환경 변수, 다중 인스턴스, `--check` |

## 가이드

| 페이지 | 내용 |
| --- | --- |
| [felucca-server 모듈](server.md) | 브리지, 클라이언트 테이블, HTTP 인그레스, 포트 인가 |
| [felucca-agent 모듈](agent.md) | 연결, 신뢰, 재연결, 이그레스 네트워킹, 업스트림 프록시 |
| [터널과 포트 매핑](tunnels.md) | 여덟 가지 모드, `[[mapping]]`, UDP, PROXY 프로토콜 |
| [백엔드, 밸런싱과 헬스 체크](backends.md) | 백엔드 필드, 다섯 가지 전략, 축출과 복구 |
| [HTTP/HTTPS 리버스 프록시](http-services.md) | 도메인·경로 라우팅, 재작성, CORS, SNI, HTTP/2, HTTP/3 |
| [인증서, SNI와 ACME](certificates.md) | 브리지 인증서, 정적 인증서, 발급과 갱신 |
| [포워드 프록시](forward-proxy.md) | SOCKS5 / HTTP / 혼합 항목, 계정, 대상 ACL |
| [파일 서비스와 WebDAV](file-services.md) | 디렉터리 권한, 읽기 전용 제어, Range, WebDAV 메서드 |
| [서버 로컬 대상과 중첩 라우트](local-services.md) | `[local.*]`, 백엔드 별칭, 16홉 루프 제어 |
| [비공개 공유와 방문자](private-shares.md) | 공개 포트 없이 TCP/UDP/파일/프록시 접근 |
| [P2P 직접 연결과 릴레이 대체](p2p.md) | 일회용 티켓, 홀 펀칭, QUIC 직접 연결, 대체 경로 |
| [접근 제어, 할당량과 스트림 부호화](security.md) | 출발지·대상 ACL, GeoIP, 계층형 할당량, 압축과 암호화 |
| [관리 UI와 중앙 집중 설정](management.md) | 관리 UI, 계정 역할, API, 설정 배포 |
| [외부 플랫폼 연동](platform.md) | 플랫폼 토큰, 역방향 WSS RPC, 서명된 콜백, 트래픽 가져오기 |
| [서비스, 업데이트, 로깅과 모니터링](operations.md) | systemd/launchd/SCM, 업데이트, 로그 로테이션, 메트릭 |

## 참조

| 페이지 | 내용 |
| --- | --- |
| [참조](reference.md) | CLI, 필드 빠른 참조, 관리 API, 예제 목록, 수치 제한, 용어집 |

## 관련 문서

- [아키텍처](../../architecture.md) — 크레이트 소유권 경계
- [설정과 배포](../../configuration.md) — 필드별 상세 설명
- [구현 현황](../../implementation-status.md) · [검증](../../validation.md)

한 단계 위의 참조 문서는 중국어와 영어로 유지됩니다.
