import { ArrowDownLeft, ArrowLeftRight, ArrowUpRight, Boxes, Radio } from "lucide-react";
import { useState } from "react";
import {
  Badge,
  Button,
  DataTable,
  Empty,
  PageHeader,
  SearchInput,
  SectionHeader,
  TextLink,
} from "../components/ui.jsx";
import { bytes } from "../lib/format.js";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import { deviceName } from "../features/product-flow.js";
import { useLiveOverview } from "../app/use-live-overview.js";

export default function Overview({ data }) {
  const { navigate, session } = useAdmin();
  const { t, tag } = useLocale();
  const [search, setSearch] = useState("");
  const { overview, error: statusError } = useLiveOverview(data.overview);
  const online = overview.clients.filter((client) => client.online).length;
  const resources = [...overview.clients, ...overview.locals];
  const connections = resources.reduce((n, client) => n + client.connections, 0);
  const traffic = resources.reduce(
    (total, client) => ({
      inlet: total.inlet + (client.traffic?.inlet_bytes || 0),
      outlet: total.outlet + (client.traffic?.export_bytes || 0),
    }),
    { inlet: 0, outlet: 0 },
  );
  const rows = overview.clients.filter((client) =>
    `${client.id} ${client.remark} ${client.user}`.toLowerCase().includes(search.toLowerCase()),
  );
  return (
    <>
      {statusError && (
        <p className="form-error" role="alert">
          {t("flow.statusUnavailable")} {statusError}
        </p>
      )}
      <PageHeader title={t("nav.overview")} description={t("overview.description")}>
        <Badge tone={overview.healthy ? "success" : "danger"}>
          {t(overview.healthy ? "overview.serviceRunning" : "overview.serviceUnavailable")}
        </Badge>
      </PageHeader>
      {session.principal.role !== "viewer" && (
        <section className="surface flow-panel">
          <h2>{t("flow.getStarted")}</h2>
          <div className="start-grid">
            {[
              ["flow.connectDevice", "flow.connectHint", "deployment", online > 0],
              ["flow.addService", "flow.servicesHint", "tunnels", overview.tunnels.length > 0],
              ["flow.connectAnother", "flow.sharingHint", "shares", false],
            ].map(([title, hint, target, complete], index) => (
              <div key={target} className="start-step">
                <span className="step-number">{complete ? "✓" : index + 1}</span>
                <h3>{t(title)}</h3>
                <p>{t(hint)}</p>
                <Button onClick={() => navigate(target, "")}>{t(title)}</Button>
              </div>
            ))}
          </div>
        </section>
      )}
      {!overview.healthy && (
        <div className="info error" role="alert">
          {t("overview.healthCheckBlurb")}
        </div>
      )}
      <div className="stats-grid">
        <article className="stat-card stat-dark card">
          <div className="stat-top">
            <span>{t("overview.onlineNodes")}</span>
            <Boxes size={22} strokeWidth={1.5} />
          </div>
          <div className="stat-number">
            {online}
            <span>/ {overview.clients.length}</span>
          </div>
          <div className="stat-bottom">
            <span>{t("overview.offlineNodes", { count: overview.clients.length - online })}</span>
          </div>
        </article>
        <article className="stat-card card">
          <div className="stat-top">
            <span>{t("overview.publishedTunnels")}</span>
            <ArrowLeftRight size={22} strokeWidth={1.5} />
          </div>
          <div className="stat-number">
            {t("overview.tunnelCount", { count: overview.tunnels.length })}
          </div>
          <div className="stat-bottom">
            <TextLink onClick={() => navigate("tunnels")}>{t("overview.viewTunnels")}</TextLink>
          </div>
        </article>
        <article className="stat-card card">
          <div className="stat-top">
            <span>{t("overview.currentConnections")}</span>
            <Radio size={22} strokeWidth={1.5} />
          </div>
          <div className="stat-number">{t("overview.connectionCount", { count: connections })}</div>
          <div className="stat-bottom">
            <span>{t("overview.connectionsBlurb")}</span>
            <TextLink onClick={() => navigate("monitor")}>{t("overview.viewMonitor")}</TextLink>
          </div>
        </article>
      </div>
      <div className="overview-grid">
        <section className="surface node-surface">
          <SectionHeader
            title={t("overview.yourNodes")}
            description={t("overview.nodeCount", {
              total: overview.clients.length,
              online,
            })}
          >
            <SearchInput
              value={search}
              onChange={setSearch}
              placeholder={t("overview.searchNodes")}
            />
          </SectionHeader>
          {rows.length ? (
            <DataTable
              label={t("overview.nodeTableLabel")}
              headers={[
                t("overview.columnNode"),
                t("overview.columnStatus"),
                t("overview.columnConnections"),
                t("overview.columnTraffic"),
                t("table.actions"),
              ]}
              rows={rows.map((client) => ({
                id: client.id,
                cells: [
                  <div className="identity-cell">
                    <span className="resource-icon">
                      <Boxes size={19} />
                    </span>
                    <div>
                      <strong>{deviceName(client)}</strong>
                      {client.user && <small>{client.user}</small>}
                    </div>
                  </div>,
                  <Badge tone={client.online ? "success" : "neutral"}>
                    {t(client.online ? "status.online" : "status.offline")}
                  </Badge>,
                  <span className="tabular">{client.connections}</span>,
                  <div className="traffic-cell">
                    <span>
                      <ArrowDownLeft size={13} />
                      {bytes(client.traffic?.inlet_bytes, tag)}
                    </span>
                    <span>
                      <ArrowUpRight size={13} />
                      {bytes(client.traffic?.export_bytes, tag)}
                    </span>
                  </div>,
                  <Button
                    className="icon-button"
                    icon={ArrowUpRight}
                    aria-label={t("overview.manageNode", { id: deviceName(client) })}
                    onClick={() => navigate("tunnels", client.id)}
                  />,
                ],
              }))}
            />
          ) : (
            <Empty
              title={t(search ? "overview.noMatch" : "overview.emptyTitle")}
              description={t(search ? "overview.noMatchHint" : "overview.emptyHint")}
              icon={Boxes}
              action={
                !search && (
                  <Button onClick={() => navigate("clients")}>{t("overview.goToNodes")}</Button>
                )
              }
            />
          )}
          <div className="surface-footer">
            <TextLink onClick={() => navigate("clients")}>{t("overview.manageAllNodes")}</TextLink>
          </div>
        </section>
        <aside className="overview-aside">
          <section className="traffic-summary">
            <div className="traffic-summary-heading">
              <h3>{t("overview.totalTraffic")}</h3>
              <p>{t("overview.totalTrafficHint")}</p>
            </div>
            <div>
              <span>
                <ArrowDownLeft size={16} />
                {t("overview.inbound")}
              </span>
              <strong>{bytes(traffic.inlet, tag)}</strong>
            </div>
            <div>
              <span>
                <ArrowUpRight size={16} />
                {t("overview.outbound")}
              </span>
              <strong>{bytes(traffic.outlet, tag)}</strong>
            </div>
          </section>
        </aside>
      </div>
    </>
  );
}
