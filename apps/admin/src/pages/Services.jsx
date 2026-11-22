import { Disclosure } from "../components/Disclosure.jsx";
import { useState } from "react";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import { Badge, Button, DataTable, Empty, PageHeader, SearchInput } from "../components/ui.jsx";
import { Modal } from "../components/Dialogs.jsx";
import { CopyValue } from "../components/Flow.jsx";
import ServiceWizard from "../components/ServiceWizard.jsx";
import {
  deviceName,
  serviceEndpoint,
  serviceName,
  serviceDevices,
} from "../features/product-flow.js";
import { structuredEditor } from "../features/resource-editors.js";
import { useLiveOverview } from "../app/use-live-overview.js";
import { Select } from "../components/Select.jsx";

export default function Services({ data }) {
  const { t } = useLocale();
  const { overview, error: statusError } = useLiveOverview(data.overview);
  const { session, navigate, mutate, refresh, confirm, run, busy, api, edit } = useAdmin();
  const [chosen, setChosen] = useState(data.chosenClient || "");
  const [search, setSearch] = useState("");
  const [wizard, setWizard] = useState(null);
  const [detail, setDetail] = useState(null);
  const [health, setHealth] = useState(null);
  const [checked, setChecked] = useState([]);
  const [host, setHost] = useState(window.location.hostname.replace(/^\[|\]$/g, ""));
  const editable = session.principal.role !== "viewer";
  const canPublish = serviceDevices(data.clients, data.shares).length > 0;
  const configured = Object.entries(data.managedByClient).flatMap(([client, config]) =>
    (config.tunnel ?? []).map((tunnel) => ({ client, tunnel, managed: true })),
  );
  const unmanaged = overview.tunnels
    .filter((route) => !data.managedByClient[route.client])
    .map((route) => ({
      client: route.client,
      tunnel: { ...route.spec, enabled: true, id: route.spec.tunnel_id },
      managed: false,
    }));
  const all = [...configured, ...unmanaged];
  const filtered = all.filter(
    (row) =>
      (!chosen || chosen === row.client) &&
      `${serviceName(row.tunnel)} ${row.client}`.toLowerCase().includes(search.toLowerCase()),
  );
  const online = (row) =>
    overview.clients.some((client) => client.id === row.client && client.online);
  const published = (row) =>
    overview.tunnels.some(
      (route) => route.client === row.client && route.spec.tunnel_id === row.tunnel.id,
    );
  const state = (row) =>
    !row.tunnel.enabled
      ? "status.disabled"
      : !online(row)
        ? "flow.deviceOffline"
        : overview.clients.some(
              (client) =>
                client.id === row.client &&
                client.config_rejected?.some((id) => id === 0 || id === row.tunnel.id),
            )
          ? "flow.syncFailed"
          : row.managed &&
              overview.clients.some(
                (client) => client.id === row.client && client.config_applied === false,
              )
            ? "flow.syncPending"
            : published(row)
              ? "tunnels.published"
              : "flow.pendingPublish";
  const path = (row) => `/api/managed/${encodeURIComponent(row.client)}/tunnels/${row.tunnel.id}`;
  const rowKey = (row) => `${row.client}/${row.tunnel.id}`;
  const batch = (enabled) =>
    run(async () => {
      const operations = all
        .filter((row) => row.managed && checked.includes(rowKey(row)))
        .map((row) => ({ method: "PUT", path: path(row), value: { ...row.tunnel, enabled } }));
      if (operations.length) await mutate("/api/batch", "POST", { operations }, data.revision);
      setChecked([]);
      refresh(t("notice.saved"));
    });
  let address = "";
  try {
    if (detail) address = serviceEndpoint(detail.tunnel, host, data.launch) || "";
  } catch {
    /* Invalid draft host has no address. */
  }
  return (
    <>
      {statusError && (
        <p className="form-error" role="alert">
          {t("flow.statusUnavailable")} {statusError}
        </p>
      )}
      <PageHeader title={t("nav.tunnels")} description={t("flow.servicesHint")}>
        {editable && (
          <Button
            variant="primary"
            onClick={() => (canPublish ? setWizard({}) : navigate("deployment", ""))}
          >
            {t("flow.addService")}
          </Button>
        )}
      </PageHeader>
      <div className="filter-toolbar">
        <label className="filter-field">
          {t("flow.onDevice")}
          <Select
            aria-label={t("flow.onDevice")}
            value={chosen}
            onValueChange={setChosen}
            options={[
              { value: "", label: t("flow.allDevices") },
              ...data.clients.map((row) => ({ value: row.id, label: deviceName(row) })),
            ]}
          />
        </label>
        <SearchInput value={search} onChange={setSearch} />
        {editable && checked.length > 0 && (
          <div className="row-actions">
            <Button disabled={busy} onClick={() => batch(true)}>
              {t("action.enable")} ({checked.length})
            </Button>
            <Button disabled={busy} onClick={() => batch(false)}>
              {t("action.disable")} ({checked.length})
            </Button>
          </div>
        )}
      </div>
      <section className="surface">
        {filtered.length ? (
          <DataTable
            headers={[
              t("flow.serviceName"),
              t("flow.onDevice"),
              t("table.status"),
              t("flow.whoAccesses"),
              t("table.actions"),
            ]}
            rows={filtered.map((row) => ({
              id: `${row.client}/${row.tunnel.id}`,
              cells: [
                <label className="check-field">
                  {editable && row.managed && (
                    <input
                      type="checkbox"
                      checked={checked.includes(rowKey(row))}
                      disabled={busy || (!checked.includes(rowKey(row)) && checked.length >= 128)}
                      onChange={(event) =>
                        setChecked((old) =>
                          event.target.checked
                            ? [...old, rowKey(row)]
                            : old.filter((key) => key !== rowKey(row)),
                        )
                      }
                    />
                  )}
                  <strong>{serviceName(row.tunnel)}</strong>
                </label>,
                deviceName(data.clients.find((client) => client.id === row.client)) || row.client,
                <Badge
                  tone={
                    state(row) === "tunnels.published"
                      ? "success"
                      : state(row) === "flow.syncFailed"
                        ? "danger"
                        : "neutral"
                  }
                >
                  {t(state(row))}
                </Badge>,
                t(row.tunnel.public === false ? "flow.private" : "flow.public"),
                <div className="row-actions">
                  <Button
                    onClick={() => {
                      setDetail(row);
                      setHealth(null);
                    }}
                  >
                    {t("flow.accessService")}
                  </Button>
                  {editable && row.managed && (
                    <>
                      <Button onClick={() => setWizard(row)}>{t("action.edit")}</Button>
                      <Button
                        disabled={busy}
                        onClick={() =>
                          run(async () => {
                            await mutate(
                              path(row),
                              "PUT",
                              { ...row.tunnel, enabled: !row.tunnel.enabled },
                              data.revision,
                            );
                            refresh(t("notice.saved"));
                          })
                        }
                      >
                        {t(row.tunnel.enabled ? "action.disable" : "action.enable")}
                      </Button>
                      <Button
                        variant="danger-ghost"
                        onClick={() =>
                          confirm({
                            title: t("flow.removeService"),
                            message: t("flow.removeServiceHint"),
                            action: async () => {
                              await mutate(path(row), "DELETE", undefined, data.revision);
                              refresh(t("notice.deleteSaved"));
                            },
                          })
                        }
                      >
                        {t("action.delete")}
                      </Button>
                    </>
                  )}
                </div>,
              ],
            }))}
          />
        ) : (
          <Empty
            title={t(canPublish ? "flow.firstService" : "flow.connectFirst")}
            description={t(canPublish ? "flow.servicesHint" : "flow.connectHint")}
            action={
              editable && (
                <Button
                  variant="primary"
                  onClick={() => (canPublish ? setWizard({}) : navigate("deployment", ""))}
                >
                  {t(canPublish ? "flow.addService" : "flow.connectDevice")}
                </Button>
              )
            }
          />
        )}
      </section>
      {editable && chosen && data.managedByClient[chosen] && (
        <Disclosure className="advanced" label={t("flow.bulkServices")}>
          <p>{t("flow.bulkServicesHint")}</p>
          <Button
            onClick={() =>
              edit(
                structuredEditor(
                  t("flow.bulkServices"),
                  data.managedByClient[chosen],
                  "ManagedConfig",
                  (value) =>
                    mutate(
                      `/api/managed/${encodeURIComponent(chosen)}`,
                      "PUT",
                      value,
                      data.revision,
                    ),
                  t,
                ),
              )
            }
          >
            {t("action.edit")}
          </Button>
        </Disclosure>
      )}
      {wizard && (
        <ServiceWizard
          data={{ ...data, chosenClient: chosen }}
          initial={wizard}
          onClose={() => setWizard(null)}
          onSaved={() => {
            setWizard(null);
            refresh(t("flow.serviceSaved"));
          }}
        />
      )}
      {detail && (
        <Modal
          id="service-detail"
          title={serviceName(detail.tunnel)}
          onClose={() => setDetail(null)}
          wide
        >
          <div className="flow-panel">
            <div className="flow-status">
              <p>
                <Badge tone={online(detail) ? "success" : "warning"}>
                  {t(online(detail) ? "status.online" : "status.offline")}
                </Badge>{" "}
                {t("nav.clients")}
              </p>
              <p>
                <Badge tone={published(detail) ? "success" : "warning"}>{t(state(detail))}</Badge>{" "}
                {t("nav.tunnels")}
              </p>
            </div>
            <p>{t("flow.publishedHint")}</p>
            {!online(detail) && (
              <Button onClick={() => navigate("deployment", detail.client)}>
                {t("flow.reconnectDevice")}
              </Button>
            )}
            {detail.tunnel.public === false ? (
              <>
                <p>{t("flow.privateHint")}</p>
                <Button variant="primary" onClick={() => navigate("shares", detail.client)}>
                  {t("flow.connectAnother")}
                </Button>
              </>
            ) : (
              <>
                <label>
                  {t("flow.serverHost")}
                  <input value={host} onChange={(event) => setHost(event.target.value)} />
                </label>
                {address && <CopyValue value={address} label={t("flow.accessAddress")} />}
                <p>{t("flow.publicAccessHint", { port: detail.tunnel.port })}</p>
              </>
            )}
            <Button
              disabled={busy || !online(detail)}
              onClick={() =>
                run(async () => {
                  const result = await api(
                    `/api/diagnostics/${encodeURIComponent(detail.client)}/${detail.tunnel.id}`,
                  );
                  setHealth(result.diagnostics);
                })
              }
            >
              {t("flow.checkBackend")}
            </Button>
            {health && (
              <div aria-live="polite">
                <p>
                  {t(
                    !health.available
                      ? "cap.unavailable"
                      : health.report.probing
                        ? "cap.probing"
                        : "cap.notProbing",
                  )}
                </p>
                {health.available &&
                  health.report.backends.map((backend) => (
                    <p key={backend.addr}>
                      {backend.addr}{" "}
                      <Badge
                        tone={
                          !backend.last_probe_ms
                            ? "neutral"
                            : backend.healthy
                              ? "success"
                              : "danger"
                        }
                      >
                        {t(
                          !backend.last_probe_ms
                            ? "connect.notChecked"
                            : backend.healthy
                              ? "cap.healthy"
                              : "cap.unhealthy",
                        )}
                      </Badge>
                    </p>
                  ))}
              </div>
            )}
          </div>
        </Modal>
      )}
    </>
  );
}
