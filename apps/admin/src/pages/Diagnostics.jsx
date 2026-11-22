import { useState } from "react";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import { Badge, Button, DataTable, Info, PageHeader, SectionHeader } from "../components/ui.jsx";
import { bytes, download, encode } from "../lib/format.js";

export default function Diagnostics({ data }) {
  const { api, session, run, refresh, busy } = useAdmin();
  const { t, tag, dateTime } = useLocale();
  const [backend, setBackend] = useState(null);
  const [logs, setLogs] = useState(null);
  const diagnostics = data.diagnostics;
  const certificates = data.certificates;
  return (
    <>
      <PageHeader title={t("cap.diagnostics")} description={t("cap.diagnosticsHint")}>
        <Button onClick={() => refresh()}>{t("action.refresh")}</Button>
        <Button
          onClick={() =>
            run(async () => {
              const result = await api("/api/metrics", "GET", undefined, { raw: true });
              download(await result.text(), "felucca-metrics.txt", "text/plain");
            })
          }
        >
          {t("cap.metrics")}
        </Button>
      </PageHeader>
      <section className="surface">
        <SectionHeader title={t("tunnels.list")} />
        <DataTable
          headers={[
            t("nav.clients"),
            t("table.id"),
            t("overview.inbound"),
            t("overview.outbound"),
            t("table.actions"),
          ]}
          rows={diagnostics.tunnels.map((row) => ({
            id: `${row.client}/${row.id}`,
            cells: [
              row.client,
              row.id,
              bytes(row.traffic.inlet_bytes, tag),
              bytes(row.traffic.export_bytes, tag),
              <Button
                disabled={busy}
                onClick={() =>
                  run(async () => {
                    const result = await api(`/api/diagnostics/${encode(row.client)}/${row.id}`);
                    setBackend({ ...result.diagnostics, client: row.client, id: row.id });
                  })
                }
              >
                {t("cap.backends")}
              </Button>,
            ],
          }))}
        />
      </section>
      {backend && (
        <section className="surface">
          <SectionHeader
            title={`${backend.client} / ${backend.id}`}
            description={t("cap.backendHint")}
          />
          {!backend.available ? (
            <Info>{t("cap.unavailable")}</Info>
          ) : (
            <>
              <Info>{t(backend.report.probing ? "cap.probing" : "cap.notProbing")}</Info>
              <DataTable
                headers={[
                  t("cap.address"),
                  t("table.status"),
                  t("overview.currentConnections"),
                  t("cap.lastProbe"),
                  t("cap.failures"),
                ]}
                rows={backend.report.backends.map((row, index) => ({
                  id: index,
                  cells: [
                    row.addr,
                    <Badge tone={row.healthy ? "success" : "danger"}>
                      {t(row.healthy ? "cap.healthy" : "cap.unhealthy")}
                    </Badge>,
                    row.connections,
                    row.last_probe_ms ? dateTime(new Date(row.last_probe_ms)) : t("cap.noProbe"),
                    row.failures,
                  ],
                }))}
              />
              {backend.report.total > backend.report.backends.length && (
                <Info>{t("cap.truncated")}</Info>
              )}
            </>
          )}
        </section>
      )}
      <section className="surface">
        <SectionHeader title={t("cap.peers")} description={t("cap.peersHint")} />
        {diagnostics.peers.truncated && <Info>{t("cap.truncated")}</Info>}
        <DataTable
          headers={[t("nav.clients"), t("table.id"), t("cap.visitor"), t("table.status")]}
          rows={diagnostics.peers.active.map((row) => ({
            id: row.id,
            cells: [row.client, row.tunnel, row.visitor, t("peer.direct")],
          }))}
        />
        <DataTable
          headers={[
            t("nav.clients"),
            t("table.id"),
            t("cap.visitor"),
            t("table.status"),
            t("platform.column.time"),
          ]}
          rows={[...diagnostics.peers.events].reverse().map((row, index) => ({
            id: index,
            cells: [
              row.client,
              row.tunnel,
              row.visitor,
              t(`peer.${row.state}`),
              dateTime(new Date(row.timestamp_ms)),
            ],
          }))}
        />
      </section>
      {session.principal.role === "admin" && (
        <>
          <section className="surface">
            <SectionHeader title={t("cap.certificates")}>
              <Button
                disabled={busy || !certificates?.acme}
                onClick={() =>
                  run(async () => {
                    await api("/api/certificates/retry", "POST", {}, { revision: data.revision });
                    refresh();
                  })
                }
              >
                {t("cap.retryCertificates")}
              </Button>
            </SectionHeader>
            <DataTable
              headers={[t("table.name"), t("cap.fingerprint"), t("cap.expires")]}
              rows={(certificates?.certificates ?? []).map((row, index) => ({
                id: index,
                cells: [
                  row.name,
                  <code className="break-anywhere">{row.fingerprint}</code>,
                  row.validity
                    ? dateTime(new Date(row.validity.not_after * 1000))
                    : t("cap.unavailable"),
                ],
              }))}
            />
            <DataTable
              headers={[t("table.name"), t("table.status"), t("cap.retryAt")]}
              rows={(certificates?.events ?? []).map((row) => ({
                id: row.domain,
                cells: [
                  row.domain,
                  t(`cert.${row.state}`),
                  row.next_attempt ? dateTime(new Date(row.next_attempt * 1000)) : "—",
                ],
              }))}
            />
          </section>
          <section className="surface">
            <SectionHeader title={t("cap.logs")}>
              <Button onClick={() => run(async () => setLogs(await api("/api/logs")))}>
                {t("action.refresh")}
              </Button>
              {logs?.configured && (
                <Button onClick={() => download(logs.text, "felucca-log.txt", "text/plain")}>
                  {t("config.export")}
                </Button>
              )}
            </SectionHeader>
            {logs &&
              (!logs.configured ? (
                <Info>{t("cap.noLogs")}</Info>
              ) : (
                <>
                  {logs.truncated && <Info>{t("cap.truncated")}</Info>}
                  <pre className="log-tail">{logs.text}</pre>
                </>
              ))}
          </section>
        </>
      )}
    </>
  );
}
