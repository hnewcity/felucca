# Справочник

Команды, поля, конечные точки, указатель примеров и глоссарий. В руководствах объясняется *почему*;
на этой странице перечислено *что*.

## CLI

Аргументы, общие для обеих программ:

| Аргумент | Переменная окружения | Описание |
| --- | --- | --- |
| `--build-info` | — | Выводит имя программы, версию, протокол и целевую платформу сборки |
| `-c, --config <SRC>` | `FELUCCA_CONFIG` | Файл TOML/JSON, HTTP(S) URL, встроенный JSON, base64 или URI `felucca://`; для агента можно указывать несколько раз |
| `--expand-env` | `FELUCCA_EXPAND_ENV` | Раскрывает `${VAR}` / `${VAR:-default}` |
| `--config-token-env <NAME>` | `FELUCCA_CONFIG_TOKEN_ENV` | Переменная, которая хранит Bearer-токен для удалённой конфигурации |
| `--check` | — | Выполняет проверку и завершает работу без привязки и подключения |
| подкоманда `service` | — | Управление системными службами |
| подкоманда `update` | — | Проверяет и устанавливает отдельный двоичный файл |

`felucca-server` добавляет:

| Аргумент | Переменная окружения | Описание |
| --- | --- | --- |
| `--runtime-config <PATH>` | `FELUCCA_RUNTIME_CONFIG` | Путь к снимку конфигурации времени выполнения |
| `--fingerprint` | — | Выводит SHA-256 сертификата `bridge.tls` |
| `--hash-password` | — | Читает одну строку пароля из stdin и выводит хеш PBKDF2; не менее 12 байт |
| `--generate-token` | — | Выводит Bearer-токен и его `token_hash` |

`felucca-agent` добавляет:

| Аргумент | По умолчанию | Описание |
| --- | --- | --- |
| `--launch` | — | Псевдоним для `--config` |
| `--log-level <LVL>` | `info` | trace/debug/info/warn/error |
| `--log-file <PATH>` | — | Дописывает в файл и выводит также в stderr; родительский каталог должен существовать |
| `--log-json` | — | Структурированные события JSON |
| `--log-max-bytes <N>` | 10 MiB | Требует `--log-file` |
| `--log-backups <N>` | 5 | Требует `--log-file` |
| `--log-rotate-interval <D>` | — | Например `24h`; требует `--log-file` |

Подкоманда `service`:

| Аргумент | Описание |
| --- | --- |
| `--name <NAME>` | Имя экземпляра службы |
| `--scope <user\|system>` | По умолчанию `user`; `system` только в Windows |
| `--dry-run` | Только печатает действия, которые были бы выполнены |
| `render` | Создаёт определение службы; `--platform systemd\|launchd\|windows` |
| `install` / `start` / `stop` / `restart` / `status` / `uninstall` | Жизненный цикл |
| `--workdir <DIR>` | По умолчанию — каталог, в котором лежит конфигурация |
| `--binary <PATH>` | Выбирает программу для установки |
| `-- <ARGS>` | Дополнительные аргументы службы |

Подкоманда `update`:

| Аргумент | Описание |
| --- | --- |
| `--source <PATH_OR_URL>` | Отдельный двоичный файл или архив tar.gz/zip |
| `--sha256 <HEX>` | 64 шестнадцатеричные цифры; основа доверия |
| `--check-only` | Проверяет без замены |
| `--expect-version <V>` | Необязательное утверждение версии |
| `--token-env <NAME>` | Переменная, которая хранит Bearer-токен для частной загрузки |

## Структура конфигурации

| Файл | Содержимое верхнего уровня |
| --- | --- |
| server | `allowed_ports`, `max_tunnels`, `max_connections`, `[bridge]`, `[http]`, `[admin]`, `[state]`, `[geo]`, `[ntp]`, `[log]`, `[p2p]`, `[node]`, `[user.*]`, `[client.*]`, `[local.*]`, `[share.*]`, `[managed.*]` |
| agent | `server`, `key_id`, `vkey`, `transport`, `reconnect_min/max`, `dial_timeout`, `max_connections`, `[trust]`, `[network]`, `[mux]`, `[kcp]`, `[ntp]`, `[remote]`, `[geo]`, `[[tunnel]]`, `[[mapping]]`, `[[visitor]]`, `[p2p]`, `[log]` (агент использует аргументы командной строки) |

Атрибут `allowed_ports` должен находиться до первого заголовка таблицы TOML. Все таблицы включают
параметр `deny_unknown_fields`.

## Краткий справочник по полям туннеля

| Поле | Применяется к режимам | Описание |
| --- | --- | --- |
| `id` | все | Стабильный идентификатор; закрепите его после включения снимков трафика |
| `mode` | все | tcp/udp/http/socks5/httpproxy/mixedproxy/file/transparent |
| `port` | все, кроме приватных | Порт сервера; для `http` он должен совпадать с HTTP-слушателем |
| `bind` | tcp/udp/proxy/file | Только IP-адрес |
| `public` | все | `false` означает приватный общий доступ |
| `hosts` | http, file | Список доменов; поддерживается маска `*.example.com` |
| `balance` | фиксированные бэкенды | round_robin/weighted/least_connections/source_hash/failover |
| `backend` | фиксированные бэкенды | `addr`, `weight`, `tls`, `trust` |
| `health` | фиксированные бэкенды | `type`, `path`, `interval`, `timeout`, `failures_to_evict`, `successes_to_restore`, `expect_status` |
| `acl_mode`/`acl_rules` | все | ACL источника: off/allow/deny |
| `target_policy` | фиксированные бэкенды и прямые прокси | `allow`/`deny`/`ports` |
| `options.auth.users` | прямые прокси, HTTP | Таблица учётных записей |
| `options.http.*` | http | Путь, перезапись, заголовки, редиректы, CORS, страницы ошибок, tls_mode, backend_http2 |
| `options.payload.*` | все | `compress`, `encrypt` |
| `options.limits.*` | все | `max_connections`, `rate_limit`, `quota` |
| `options.proxy_protocol` | tcp/udp/http/tls | off/v1/v2 |
| `options.proxy_accept` | входы прокси | Принимает заголовки PROXY |
| `file` | file | `root`, `prefix`, `read_only`, `auto_index`, `auth` |

## Сводка по API управления

Аутентификация выполняется по Cookie с Origin и CSRF либо по заголовку
`Authorization: Bearer <token>`. Записи требуют заголовок `If-Match: "<revision>"` и уникальный
`Idempotency-Key`. Предельный размер тела запроса — 2 MiB.

| Группа | Пути |
| --- | --- |
| Сеанс | `/api/login`, `/api/session`, `/api/logout` |
| Ресурсы | `/api/users`, `/api/clients`, `/api/locals`, `/api/shares`, `/api/accounts`, каждый со своим `<id>` |
| Централизованная конфигурация | `/api/managed/<client>`, `/api/managed/<client>/tunnels/<id>` |
| Туннели локальных сервисов сервера | `/api/locals/<group>/tunnels/<id>` |
| Вся конфигурация | `/api/config`, `/api/config?secrets=1`, `/api/validate`, `/api/batch` |
| Реальное время | `/api/events` (SSE), `/metrics`, `/api/metrics`, `/api/profile` |
| Внешняя платформа | `/api/node/*`, `/api/platforms/<name>`, `/api/webhooks/<name>`, `/api/ws`, `/api/realtime/subscriptions` |

Полное описание полей см. в разделах [Интерфейс управления и централизованная
конфигурация](management.md) и [Интеграция с внешними платформами](platform.md).

## Указатель примеров конфигурации

| Файл | Содержимое |
| --- | --- |
| [felucca-server.toml](../../../examples/felucca-server.toml) | Минимальный пример сервера |
| [felucca-agent.toml](../../../examples/felucca-agent.toml) | Минимальный пример агента |
| [features-server.toml](../../../examples/features-server.toml) | Мост QUIC + HTTP/HTTPS + сертификаты |
| [features-agent.toml](../../../examples/features-agent.toml) | Все режимы прокси, балансировка нагрузки, проверки работоспособности |
| [shared-server.toml](../../../examples/shared-server.toml) | Общие порты TCP/UDP + HTTP/3 |
| [shared-agent.toml](../../../examples/shared-agent.toml) | Сторона агента и пакетное сопоставление портов |
| [acme-server.toml](../../../examples/acme-server.toml) | Сервер ACME |
| [zerossl-server.toml](../../../examples/zerossl-server.toml) | ZeroSSL и TLS-ALPN-01 |
| [policy-server.toml](../../../examples/policy-server.toml) / [policy-agent.toml](../../../examples/policy-agent.toml) | Многоуровневые ACL, квоты, сохранение |
| [file-server.toml](../../../examples/file-server.toml) / [file-agent.toml](../../../examples/file-agent.toml) | Файловые сервисы |
| [network-agent.toml](../../../examples/network-agent.toml) | Вышестоящий прокси, собственный DNS, NTP |
| [payload-agent.toml](../../../examples/payload-agent.toml) | Snappy и независимое шифрование потока |
| [kcp-server.toml](../../../examples/kcp-server.toml) / [kcp-agent.toml](../../../examples/kcp-agent.toml) | Мост KCP |
| [local-server.toml](../../../examples/local-server.toml) | Локальные цели сервера и вложенная пересылка |
| [visitor-server.toml](../../../examples/visitor-server.toml) / [private-agent.toml](../../../examples/private-agent.toml) / [visitor-agent.toml](../../../examples/visitor-agent.toml) | Приватный поставщик и посетитель |
| [p2p-server.toml](../../../examples/p2p-server.toml) / [p2p-provider.toml](../../../examples/p2p-provider.toml) / [p2p-visitor.toml](../../../examples/p2p-visitor.toml) | P2P и откат |
| [admin-server.toml](../../../examples/admin-server.toml) | Сервер управления |
| [managed-agent.toml](../../../examples/managed-agent.toml) | Централизованно управляемый агент |
| [node-server.toml](../../../examples/node-server.toml) | Внешняя платформа и управление узлами |

Пути конфигурации указываются относительно рабочего каталога процесса. Все 26 примеров проходят
проверку конфигурации.

## Числовые ограничения

| Элемент | Ограничение |
| --- | --- |
| Одна конфигурация | 4 MiB |
| Снимок конфигурации времени выполнения | 16 MiB |
| Экземпляры агента | 32 |
| Прыжки вложенной маршрутизации | 16 |
| Операции в пакете | 128 элементов |
| Импорт трафика | 128 элементов на запрос |
| Хранимые записи идемпотентности | 256 (по 256 для действий с обратными вызовами и импорта трафика) |
| Очередь обратных вызовов | `queue_max` на цель, 4096 в пределах узла |
| Прямые управляющие WebSocket | 16 |
| Параллелизм WS RPC | 2 на соединение / 16 в пределах узла |
| Подписки реального времени | 32 элемента на соединение, 32 KiB на элемент |
| Пул соединений с бэкендами HTTP | 8 простаивающих на источник, 30 секунд простоя, 256 записей во внешней карте |
| HTTP/3 | 128 потоков на соединение, окно 1 MiB на поток / 16 MiB на соединение, заголовки 32 KiB |
| UDP | 65507 байт на источник; 256 ассоциаций на туннель |
| SOCKS5 UDP | 64 назначения на сеанс, по 8 пакетов в очереди для каждого |
| Страницы ошибок | 400..599, 64 KiB на страницу |
| Сопоставление доменов в HTTP | Точные домены имеют приоритет над масками |

## Окружение и сборка

| Элемент | Значение |
| --- | --- |
| Минимальная версия Rust | 1.88 |
| Проверенное окружение | macOS Intel, Rust/Cargo 1.98.1 |
| Тесты рабочего пространства | 581 пройден / 0 с ошибкой / 0 пропущено (2026-09-17) |
| Двоичные файлы | `target/release/felucca-server`, `target/release/felucca-agent` |

```sh
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
sh scripts/cargo-external.sh test  --workspace --locked
sh scripts/cargo-external.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo-external.sh fmt --all --check
```

Одновременно запускайте только одну команду Cargo против одного и того же каталога target.
Завершайте сборку, тесты и rustdoc перед тем, как изменять исходный код и встроенные страницы. Тестам
нужно разрешение на прослушивание локальных портов TCP/UDP.

## Глоссарий

| Термин | Значение |
| --- | --- |
| агент | Процесс `felucca-agent`, который работает внутри частной сети и устанавливает исходящие соединения |
| мост | Аутентифицированное долгоживущее соединение между сервером и агентом |
| клиент / `[client.<id>]` | Таблица личностей сервера для одного агента; `key_id` — имя таблицы |
| key_id | Имя агента на сервере |
| vkey | Секрет моста, общий для агента и сервера |
| транспорт | Транспорт моста: tcp/tls/ws/wss/quic/kcp |
| доверие | Способ проверки агентом сертификата сервера: webpki/pinned/insecure |
| туннель | Сопоставление публичной точки входа с набором бэкендов |
| бэкенд | Цель, к которой агент подключается фактически |
| сопоставление | Пакетное сопоставление портов один к одному |
| локальная группа | Группа собственных сервисов сервера, которой агент не нужен |
| общий доступ | Сервер разрешает гостю использовать один туннель |
| посетитель | Агент, который держит секрет общего доступа и слушает локально |
| поставщик | Сторона, которая предоставляет сервис в приватном общем доступе или в схеме P2P |
| управляемая конфигурация | Конфигурация, которую сервер рассылает агенту |
| mux | Ограниченное мультиплексирование поверх транспортов типа TCP |
| FEC | Упреждающее исправление ошибок KCP, по умолчанию 10 данных + 3 проверочных |
| квота / rate_limit | Квоты трафика и ограничения скорости в байтах в секунду |
| снимок | Одно атомарное сохранение трафика или конфигурации времени выполнения |

## Связанная документация

- [Архитектура](../../architecture.md) — границы владения модулями
- [Конфигурация и развёртывание](../../configuration.md) — подробное описание полей
- [P2P](../../p2p.md), [Приватные туннели и общий доступ](../../visitors.md),
  [Управление и централизованная конфигурация](../../management.md),
  [Узлы и внешние платформы](../../node.md), [Эксплуатация и распространение](../../operations.md),
  [Прозрачный TCP](../../transparent.md)
