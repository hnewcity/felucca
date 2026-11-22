# Índice de la documentación de Felucca · Español

[中文](../README.md) · [English](en/README.md) · [日本語](ja/README.md) · [한국어](ko/README.md) · [Français](fr/README.md) · Español · [Русский](ru/README.md)

Esta es la documentación de usuario de Felucca: desde un primer túnel funcional, pasando por cómo
configurar y usar cada módulo, hasta las referencias de comandos, campos, extremos y glosario.
Describe **cómo usar Felucca**, no cómo está implementado.

## Primeros pasos

| Página | Contenido |
| --- | --- |
| [Primeros pasos](get-started.md) | Instalación, la demostración local y un primer túnel paso a paso |
| [Arquitectura y conceptos](architecture.md) | Los componentes, la ruta del tráfico y un glosario |
| [Fuentes de configuración y CLI](configuration.md) | Fuentes de configuración, variables de entorno, varias instancias, `--check` |

## Guías

| Página | Contenido |
| --- | --- |
| [El módulo felucca-server](server.md) | Puente, tabla de clientes, ingreso HTTP, autorización de puertos |
| [El módulo felucca-agent](agent.md) | Conexión, confianza, reconexiones, red de salida, proxy ascendente |
| [Túneles y mapeos de puertos](tunnels.md) | Los ocho modos, `[[mapping]]`, UDP, protocolo PROXY |
| [Back-ends, balanceo y comprobaciones de estado](backends.md) | Campos del back-end, las cinco estrategias, desalojo y recuperación |
| [Proxy inverso HTTP/HTTPS](http-services.md) | Enrutado por dominio y ruta, reescritura, CORS, SNI, HTTP/2, HTTP/3 |
| [Certificados, SNI y ACME](certificates.md) | Certificados del puente, certificados estáticos, emisión y renovación |
| [Proxy directo](forward-proxy.md) | Entradas SOCKS5 / HTTP / mixtas, cuentas, ACL de destino |
| [Servicios de archivos y WebDAV](file-services.md) | Capacidades de directorio, control de solo lectura, Range, métodos WebDAV |
| [Destinos locales del servidor y rutas anidadas](local-services.md) | `[local.*]`, alias de back-end, control de bucles de 16 saltos |
| [Recursos compartidos privados y visitantes](private-shares.md) | Acceso TCP/UDP/archivos/proxy sin un puerto público |
| [Directo P2P y respaldo por relé](p2p.md) | Tickets de un solo uso, perforación de NAT, directo por QUIC, respaldo |
| [Control de acceso, cuotas y codificación de flujo](security.md) | ACL de origen y destino, GeoIP, cuotas por capas, compresión y cifrado |
| [Interfaz de administración y configuración centralizada](management.md) | Interfaz de administración, roles de cuenta, API, envío de configuración |
| [Integración con plataformas externas](platform.md) | Tokens de plataforma, RPC WSS inverso, llamadas de retorno firmadas, importación de tráfico |
| [Servicios, actualizaciones, registro y supervisión](operations.md) | systemd/launchd/SCM, actualizaciones, rotación de registros, métricas |

## Referencia

| Página | Contenido |
| --- | --- |
| [Referencia](reference.md) | CLI, referencia rápida de campos, API de administración, índice de ejemplos, límites numéricos, glosario |

## Documentación relacionada

- [Arquitectura](../../architecture.md) — límites de responsabilidad de los crates
- [Configuración y despliegue](../../configuration.md) — detalle a nivel de campo
- [Estado de implementación](../../implementation-status.md) · [Validación](../../validation.md)

Los documentos de referencia un nivel más arriba se mantienen en chino y en inglés.
