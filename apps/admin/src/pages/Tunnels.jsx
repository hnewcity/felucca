import { useState } from "react";
import { ArrowLeftRight, Plus, RotateCcw } from "lucide-react";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import {
  Badge,
  Button,
  DataTable,
  Empty,
  Info,
  PageHeader,
  SearchInput,
} from "../components/ui.jsx";
import { jsonEditor, tunnelEditor, structuredEditor } from "../features/resource-editors.js";
import { encode } from "../lib/format.js";
import { Select } from "../components/Select.jsx";

export default function Tunnels({ data }) {
  const { session, edit, mutate, confirm, refresh, selectClient, navigate, run, busy } = useAdmin();
  const { t } = useLocale();
  const [selected, setSelected] = useState(new Set());
  const [search, setSearch] = useState("");
  const { client, clients, overview } = data;
  const canEdit = session.principal.role !== "viewer";
  const path = `/api/managed/${encode(client?.id || "")}`;
  const write = (path, method, config) => mutate(path, method, config, data.revision);
  const tunnels = data.managed?.tunnel ?? [];
  const shown = tunnels.filter((tunnel) =>
    `${tunnel.id} ${tunnel.remark || ""} ${tunnel.mode}`
      .toLowerCase()
      .includes(search.toLowerCase()),
  );
  async function update(tunnel, enabled) {
    await write(`${path}/tunnels/${tunnel.id}`, "PUT", { ...tunnel, enabled });
    refresh(t(enabled ? "notice.tunnelStarted" : "notice.tunnelStopped"));
  }
  async function batch(enabled) {
    await write("/api/batch", "POST", {
      operations: tunnels
        .filter((tunnel) => selected.has(tunnel.id))
        .map((tunnel) => ({
          method: "PUT",
          path: `${path}/tunnels/${tunnel.id}`,
          value: { ...tunnel, enabled },
        })),
    });
    setSelected(new Set());
    refresh(t("notice.batchSaved"));
  }
  const newTunnel = () => edit(tunnelEditor(null, path, write, t, tunnels));
  function switchMode() {
    confirm({
      title: t(client.managed ? "tunnels.restoreLocalTitle" : "tunnels.enableCentralTitle"),
      message: t(client.managed ? "tunnels.restoreLocalMessage" : "tunnels.enableCentralMessage"),
      danger: client.managed,
      action: async () => {
        await write(
          path,
          client.managed ? "DELETE" : "PUT",
          client.managed ? undefined : { tunnel: [] },
        );
        refresh(t("notice.nodeConfigUpdated"));
      },
    });
  }
  return (
    <>
      <PageHeader title={t("nav.tunnels")} description={t("nav.tunnels.description")}>
        {client?.managed && canEdit && (
          <>
            <Button
              onClick={() =>
                edit(
                  structuredEditor(
                    t("cap.bulkPorts"),
                    data.managed,
                    "ManagedConfig",
                    (next) => write(path, "PUT", next),
                    t,
                  ),
                )
              }
            >
              {t("cap.bulkPorts")}
            </Button>
            <Button variant="primary" icon={Plus} onClick={newTunnel}>
              {t("tunnels.newTunnel")}
            </Button>
          </>
        )}
      </PageHeader>
      {!client ? (
        <section className="surface">
          <Empty
            icon={ArrowLeftRight}
            title={t("tunnels.connectFirstTitle")}
            description={t("tunnels.connectFirstDescription")}
            action={<Button onClick={() => navigate("clients")}>{t("overview.goToNodes")}</Button>}
          />
        </section>
      ) : (
        <>
          <div className="tunnel-toolbar">
            <label className="node-select">
              {t("tunnels.currentNode")}
              <Select
                aria-label={t("tunnels.currentNode")}
                value={client.id}
                onValueChange={(value) => {
                  setSelected(new Set());
                  selectClient(value);
                }}
                options={clients.map((client) => ({ value: client.id, label: client.id }))}
              />
            </label>
            <Badge tone={client.managed ? "blue" : "neutral"} dot={false}>
              {client.managed ? t("tunnels.central") : t("tunnels.agentLocal")}
            </Badge>
            {canEdit && (
              <Button className="push-right" icon={RotateCcw} onClick={switchMode}>
                {client.managed ? t("tunnels.restoreAgent") : t("tunnels.enableCentral")}
              </Button>
            )}
          </div>
          {!client.managed ? (
            <>
              <Info>{t("tunnels.localInfo")}</Info>
              <section className="surface">
                {overview.tunnels.some((tunnel) => tunnel.client === client.id) ? (
                  <DataTable
                    headers={[
                      t("table.id"),
                      t("table.type"),
                      t("tunnels.header.port"),
                      t("table.remark"),
                    ]}
                    rows={overview.tunnels
                      .filter((tunnel) => tunnel.client === client.id)
                      .map(({ spec }) => ({
                        id: spec.tunnel_id,
                        cells: [spec.tunnel_id, spec.mode.toUpperCase(), spec.port, spec.remark],
                      }))}
                  />
                ) : (
                  <Empty
                    title={t("tunnels.emptyPublishedTitle")}
                    description={t("tunnels.emptyPublishedDescription")}
                    icon={ArrowLeftRight}
                  />
                )}
              </section>
            </>
          ) : (
            <section className="surface">
              <div className="section-head">
                <div>
                  <h2>
                    {t("tunnels.list")} <span className="count">{tunnels.length}</span>
                  </h2>
                  <p>{t("tunnels.centralConfig", { id: client.id })}</p>
                </div>
                <SearchInput
                  value={search}
                  onChange={setSearch}
                  placeholder={t("tunnels.searchPlaceholder")}
                />
              </div>
              {canEdit && tunnels.length > 0 && (
                <div className="batch-bar">
                  <span>{t("tunnels.selectedCount", { count: selected.size })}</span>
                  <Button disabled={!selected.size || busy} onClick={() => run(() => batch(true))}>
                    {t("tunnels.enableSelected")}
                  </Button>
                  <Button disabled={!selected.size || busy} onClick={() => run(() => batch(false))}>
                    {t("tunnels.disableSelected")}
                  </Button>
                </div>
              )}
              {shown.length ? (
                <DataTable
                  label={t("tunnels.tableLabel")}
                  headers={[
                    <input
                      type="checkbox"
                      aria-label={t("tunnels.selectAll")}
                      disabled={!canEdit}
                      checked={shown.length > 0 && shown.every((tunnel) => selected.has(tunnel.id))}
                      onChange={(event) =>
                        setSelected(
                          event.target.checked
                            ? new Set(shown.map((tunnel) => tunnel.id))
                            : new Set(),
                        )
                      }
                    />,
                    t("tunnels.header.idRemark"),
                    t("table.status"),
                    t("table.type"),
                    t("tunnels.header.publicPort"),
                    t("tunnels.header.backend"),
                    t("table.actions"),
                  ]}
                  rows={shown.map((tunnel) => {
                    const published = overview.tunnels.some(
                      (route) => route.client === client.id && route.spec.tunnel_id === tunnel.id,
                    );
                    return {
                      id: tunnel.id,
                      cells: [
                        <input
                          type="checkbox"
                          aria-label={t("tunnels.selectOne", { id: tunnel.id })}
                          checked={selected.has(tunnel.id)}
                          disabled={!canEdit}
                          onChange={(event) => {
                            const next = new Set(selected);
                            if (event.target.checked) next.add(tunnel.id);
                            else next.delete(tunnel.id);
                            setSelected(next);
                          }}
                        />,
                        <div className="resource-name">
                          <strong>#{tunnel.id}</strong>
                          <small>{tunnel.remark || t("tunnels.noRemark")}</small>
                        </div>,
                        <Badge
                          tone={tunnel.enabled ? (published ? "success" : "warning") : "neutral"}
                        >
                          {tunnel.enabled
                            ? published
                              ? t("tunnels.published")
                              : t("tunnels.waitingForNode")
                            : t("status.disabled")}
                        </Badge>,
                        <span className="protocol-tag">{tunnel.mode.toUpperCase()}</span>,
                        tunnel.public === false ? t("tunnels.private") : tunnel.port,
                        <span className="endpoint">
                          {(tunnel.hosts?.length
                            ? tunnel.hosts
                            : (tunnel.backend ?? []).map((backend) => backend.addr)
                          ).join(", ") || "—"}
                        </span>,
                        canEdit ? (
                          <div className="row-actions">
                            <Button
                              disabled={busy}
                              onClick={() => run(() => update(tunnel, !tunnel.enabled))}
                            >
                              {tunnel.enabled ? t("action.disable") : t("action.enable")}
                            </Button>
                            <Button
                              onClick={() => edit(tunnelEditor(tunnel, path, write, t, tunnels))}
                            >
                              {t("action.edit")}
                            </Button>
                            <Button
                              variant="danger-ghost"
                              onClick={() =>
                                confirm({
                                  title: t("tunnels.deleteTitle", { id: tunnel.id }),
                                  message: t("tunnels.deleteMessage"),
                                  action: async () => {
                                    await write(`${path}/tunnels/${tunnel.id}`, "DELETE");
                                    refresh(t("notice.tunnelDeleted"));
                                  },
                                })
                              }
                            >
                              {t("action.delete")}
                            </Button>
                          </div>
                        ) : (
                          <Button
                            onClick={() =>
                              edit(
                                jsonEditor(
                                  t("tunnels.jsonTitle", { id: tunnel.id }),
                                  tunnel,
                                  () => {},
                                  true,
                                  t,
                                ),
                              )
                            }
                          >
                            {t("action.view")}
                          </Button>
                        ),
                      ],
                    };
                  })}
                />
              ) : (
                <Empty
                  title={search ? t("tunnels.noMatchTitle") : t("tunnels.firstTunnelTitle")}
                  description={search ? t("tunnels.noMatchHint") : t("tunnels.centralEnabledHint")}
                  icon={ArrowLeftRight}
                  action={
                    !search &&
                    canEdit && (
                      <Button onClick={newTunnel} icon={Plus}>
                        {t("tunnels.newTunnel")}
                      </Button>
                    )
                  }
                />
              )}
            </section>
          )}
        </>
      )}
    </>
  );
}
