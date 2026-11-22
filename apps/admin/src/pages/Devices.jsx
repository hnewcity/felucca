import { useState } from "react";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import { Badge, Button, DataTable, Empty, PageHeader, SearchInput } from "../components/ui.jsx";
import { deviceName } from "../features/product-flow.js";
import { resourceEditor } from "../features/resource-editors.js";
import { encode } from "../lib/format.js";
import { useLiveOverview } from "../app/use-live-overview.js";

export default function Devices({ data }) {
  const { t } = useLocale();
  const { overview, error: statusError } = useLiveOverview(data.overview);
  const { navigate, edit, mutate, session, confirm, refresh } = useAdmin();
  const [search, setSearch] = useState("");
  const editable = session.principal.role !== "viewer";
  const write = (path, method, value) => mutate(path, method, value, data.revision);
  return (
    <>
      {statusError && (
        <p className="form-error" role="alert">
          {t("flow.statusUnavailable")} {statusError}
        </p>
      )}
      <PageHeader title={t("nav.clients")} description={t("flow.devicesHint")}>
        {editable && (
          <Button variant="primary" onClick={() => navigate("deployment", "")}>
            {t("flow.connectDevice")}
          </Button>
        )}
      </PageHeader>
      <section className="surface">
        <div className="section-head">
          <h2>{t("flow.yourDevices")}</h2>
          <SearchInput value={search} onChange={setSearch} />
        </div>
        {data.rows.length ? (
          <DataTable
            headers={[
              t("flow.deviceName"),
              t("table.status"),
              t("nav.tunnels"),
              t("table.actions"),
            ]}
            rows={data.rows
              .filter((row) =>
                `${deviceName(row)} ${row.id}`.toLowerCase().includes(search.toLowerCase()),
              )
              .map((row) => {
                const state = overview.clients.find((client) => client.id === row.id);
                return {
                  id: row.id,
                  cells: [
                    <strong>{deviceName(row)}</strong>,
                    <Badge
                      tone={!row.config.enabled ? "neutral" : state?.online ? "success" : "warning"}
                    >
                      {t(
                        !row.config.enabled
                          ? "status.disabled"
                          : state?.online
                            ? "status.online"
                            : "status.offline",
                      )}
                    </Badge>,
                    overview.tunnels.filter((route) => route.client === row.id).length,
                    <div className="row-actions">
                      <Button onClick={() => navigate("tunnels", row.id)}>
                        {t("flow.viewServices")}
                      </Button>
                      {session.principal.role === "admin" && (
                        <Button onClick={() => navigate("deployment", row.id)}>
                          {t("flow.reconnectDevice")}
                        </Button>
                      )}
                      {editable && (
                        <Button
                          onClick={() =>
                            edit(
                              resourceEditor(
                                "clients",
                                row,
                                session.principal,
                                write,
                                false,
                                t,
                                data.rows,
                                data.managedByClient?.[row.id],
                              ),
                            )
                          }
                        >
                          {t("action.edit")}
                        </Button>
                      )}
                      {editable && (
                        <Button
                          variant="danger-ghost"
                          onClick={() =>
                            confirm({
                              title: t("flow.removeDevice"),
                              message: t("flow.removeDeviceHint"),
                              action: async () => {
                                await write(`/api/clients/${encode(row.id)}`, "DELETE");
                                refresh(t("notice.deleteSaved"));
                              },
                            })
                          }
                        >
                          {t("action.delete")}
                        </Button>
                      )}
                    </div>,
                  ],
                };
              })}
          />
        ) : (
          <Empty
            title={t("flow.connectFirst")}
            description={t("flow.connectHint")}
            action={
              editable && (
                <Button variant="primary" onClick={() => navigate("deployment", "")}>
                  {t("flow.connectDevice")}
                </Button>
              )
            }
          />
        )}
      </section>
    </>
  );
}
