import { useState } from "react";
import { ArrowUpRight, Plus } from "lucide-react";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import { pages } from "../app/navigation.js";
import {
  Badge,
  Button,
  DataTable,
  Empty,
  PageHeader,
  SearchInput,
  SectionHeader,
} from "../components/ui.jsx";
import { resourceEditor, tunnelEditor } from "../features/resource-editors.js";
import { bytes, encode, download } from "../lib/format.js";

function description(kind, value, t, tag) {
  switch (kind) {
    case "clients":
      return value.remark || value.user || t("overview.unassigned");
    case "shares":
      return t("resources.shareDescription", { client: value.client, tunnel: value.tunnel });
    case "accounts":
      return `${t(`role.${value.role}`)} · ${value.user || t("common.allUsers")}`;
    case "locals":
      return t("resources.tunnelCount", { count: value.tunnel?.length ?? 0 });
    case "platforms":
      return `${value.scope === "full" ? t("resources.fullAccess") : value.user || t("common.specifiedUser")} · ${t(`editor.mode.${value.mode}`)}`;
    case "webhooks":
      return value.delivery.url;
    default:
      return t("resources.quotaDescription", {
        flow:
          value.limits?.quota?.flow_limit == null
            ? t("resources.unlimited")
            : bytes(value.limits.quota.flow_limit, tag),
        connections: value.limits?.max_connections ?? t("resources.unlimited"),
      });
  }
}
function status(kind, row, overview, t) {
  if (kind === "clients") {
    const client = overview.clients.find((client) => client.id === row.id);
    return (
      <Badge tone={!row.config.enabled ? "neutral" : client?.online ? "success" : "warning"}>
        {!row.config.enabled
          ? t("status.disabled")
          : client?.online
            ? t("status.online")
            : t("status.offline")}
      </Badge>
    );
  }
  if (kind === "shares")
    return (
      <div className="badge-group">
        <Badge tone="blue" dot={false}>
          {row.config.direct ? t("resources.directAllowed") : t("resources.relayAccess")}
        </Badge>
        {row.config.dynamic && <Badge dot={false}>{t("resources.dynamicTarget")}</Badge>}
      </div>
    );
  const enabled =
    kind === "accounts"
      ? !row.config.disabled
      : kind === "webhooks"
        ? row.config.delivery.enabled
        : (row.config.enabled ?? true);
  return (
    <Badge tone={enabled ? "enabled" : "neutral"}>
      {enabled ? t("status.enabled") : t("status.disabled")}
    </Badge>
  );
}
export default function Resources({ kind, data }) {
  const { session, api, edit, mutate, confirm, navigate, refresh, run, busy } = useAdmin();
  const { t, tag } = useLocale();
  const [search, setSearch] = useState("");
  const meta = pages[kind];
  const Icon = meta.icon;
  const canEdit =
    session.principal.role !== "viewer" &&
    (!["users", "locals", "accounts", "platforms", "webhooks"].includes(kind) ||
      session.principal.role === "admin");
  const write = (path, method, config) => mutate(path, method, config, data.revision);
  const open = (row) =>
    edit(resourceEditor(kind, row, session.principal, write, !canEdit, t, data.rows));
  const rows = data.rows.filter((row) =>
    `${row.id} ${description(kind, row.config, t, tag)}`
      .toLowerCase()
      .includes(search.toLowerCase()),
  );
  function remove(row) {
    confirm({
      title: t("resources.deleteTitle", { id: row.id }),
      message: t("resources.deleteMessage"),
      action: async () => {
        await write(`/api/${kind}/${encode(row.id)}`, "DELETE");
        refresh(t("notice.deleteSaved"));
      },
    });
  }
  return (
    <>
      <PageHeader title={t(meta.label)} description={t(meta.description)}>
        {canEdit && (
          <Button variant="primary" icon={Plus} onClick={() => open()}>
            {t("action.add")}
          </Button>
        )}
      </PageHeader>
      <section className="surface">
        <SectionHeader
          title={t("resources.allTitle", { kind: t(meta.label) })}
          description={t("resources.recordCount", { count: data.rows.length })}
        >
          <SearchInput value={search} onChange={setSearch} />
        </SectionHeader>
        {rows.length ? (
          <DataTable
            label={t(meta.label)}
            headers={[
              t("table.name"),
              t("resources.column.description"),
              t("resources.column.access"),
              t("table.actions"),
            ]}
            rows={rows.map((row) => ({
              id: row.id,
              cells: [
                <div className="identity-cell">
                  <span className="resource-icon">
                    <Icon size={20} strokeWidth={1.6} />
                  </span>
                  <strong>{row.id}</strong>
                </div>,
                <span className="description-cell">{description(kind, row.config, t, tag)}</span>,
                status(kind, row, data.overview, t),
                <div className="row-actions">
                  <Button onClick={() => open(row)}>
                    {canEdit ? t("action.edit") : t("action.view")}
                  </Button>
                  {kind === "locals" && canEdit && (
                    <Button
                      onClick={() =>
                        edit(
                          tunnelEditor(
                            null,
                            `/api/locals/${encode(row.id)}`,
                            write,
                            t,
                            row.config.tunnel,
                          ),
                        )
                      }
                    >
                      {t("tunnels.newTunnel")}
                    </Button>
                  )}
                  {kind === "accounts" && canEdit && (
                    <>
                      <Button
                        onClick={() =>
                          run(async () => {
                            const issued = await api("/api/token", "POST", {});
                            confirm({
                              title: t("cap.tokenIssue"),
                              message: t("cap.tokenHint"),
                              danger: false,
                              action: async () => {
                                await write(`/api/accounts/${encode(row.id)}`, "PUT", {
                                  ...row.config,
                                  token_hash: issued.token_hash,
                                });
                                download(issued.token, "felucca-token.txt", "text/plain");
                                refresh(t("notice.saved"));
                              },
                            });
                          })
                        }
                      >
                        {t("cap.tokenIssue")}
                      </Button>
                      <Button
                        disabled={!row.config.token_hash}
                        onClick={() =>
                          confirm({
                            title: t("cap.tokenRevoke"),
                            message: t("cap.tokenHint"),
                            action: async () => {
                              await write(`/api/accounts/${encode(row.id)}`, "PUT", {
                                ...row.config,
                                token_hash: null,
                              });
                              refresh(t("notice.saved"));
                            },
                          })
                        }
                      >
                        {t("cap.tokenRevoke")}
                      </Button>
                    </>
                  )}
                  {kind === "clients" && (
                    <Button icon={ArrowUpRight} onClick={() => navigate("tunnels", row.id)}>
                      {t("nav.tunnels")}
                    </Button>
                  )}
                  {canEdit && (
                    <Button
                      variant="danger-ghost"
                      onClick={() => run(() => remove(row))}
                      disabled={busy}
                    >
                      {t("action.delete")}
                    </Button>
                  )}
                </div>,
              ],
            }))}
          />
        ) : (
          <Empty
            title={search ? t("resources.noMatch") : t("common.noRecords")}
            description={
              search
                ? t("resources.noMatchHint")
                : t("resources.emptyHint", { kind: t(meta.label) })
            }
            icon={Icon}
            action={
              !search &&
              canEdit && (
                <Button onClick={() => open()} icon={Plus}>
                  {t("resources.addWithKind", { kind: t(meta.label) })}
                </Button>
              )
            }
          />
        )}
      </section>
    </>
  );
}
