import { useState } from "react";
import { Upload } from "lucide-react";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import {
  Badge,
  Button,
  DataTable,
  Empty,
  Info,
  PageHeader,
  SectionHeader,
} from "../components/ui.jsx";
import { structuredEditor } from "../features/resource-editors.js";

export default function Platform({ data, offset }) {
  const { session, api, edit, mutate, confirm, refresh, setOffset, run, busy } = useAdmin();
  const { t, dateTime } = useLocale();
  const [selected, setSelected] = useState(new Set());
  const { status, callbacks } = data;
  const [changes, setChanges] = useState(data.changes);
  const [cursors, setCursors] = useState([null]);
  async function history(next) {
    try {
      const cursor = next.at(-1);
      const page = await api(
        `/api/node/changes?limit=100${cursor ? `&after=${encodeURIComponent(cursor)}` : ""}`,
      );
      setChanges(page);
      setCursors(next);
    } catch (error) {
      if (error.status !== 410) throw error;
      const page = await api("/api/node/changes");
      setChanges(page);
      setCursors([null]);
    }
  }
  const canEdit = session.principal.role !== "viewer";
  const write = (path, method, config) => mutate(path, method, config, data.revision);
  const pending = callbacks.queues.flatMap((queue) =>
    queue.items.map((item) => ({ ...item, target: queue.target })),
  );
  async function callbackAction(action) {
    await write(`/api/node/callbacks/${action}`, "POST", { ids: [...selected] });
    setSelected(new Set());
    refresh(action === "replay" ? t("notice.callbacksReplayed") : t("notice.callbacksDeleted"));
  }
  return (
    <>
      <PageHeader title={t("nav.node")} description={t("nav.node.description")}>
        {session.principal.role === "admin" && (
          <Button
            variant="primary"
            onClick={() =>
              confirm({
                title: t("platform.resyncTitle"),
                message: t("platform.resyncMessage"),
                danger: false,
                action: async () => {
                  await write("/api/node/sync", "POST", {});
                  refresh(t("notice.resynced"));
                },
              })
            }
          >
            {t("platform.resync")}
          </Button>
        )}
      </PageHeader>
      {status.journal_error && <Info tone="error">{t("platform.journalError")}</Info>}
      <div className="platform-toolbar">
        <div>
          <Badge tone="blue" dot={false}>
            {status.node}
          </Badge>
          <span>
            {t("platform.revisionLine", {
              revision: status.revision,
              sequence: status.current,
            })}
          </span>
        </div>
        {canEdit && (
          <div className="row-actions">
            <Button
              icon={Upload}
              onClick={() =>
                edit(
                  structuredEditor(
                    t("platform.importTrafficTitle"),
                    { items: [{ client: "", inlet_bytes: 0, export_bytes: 0 }] },
                    {
                      type: "object",
                      fields: {
                        items: {
                          type: "array",
                          item: {
                            type: "object",
                            fields: {
                              client: { type: "string" },
                              inlet_bytes: { type: "number", min: 0 },
                              export_bytes: { type: "number", min: 0 },
                            },
                          },
                        },
                      },
                    },
                    (value) => write("/api/node/traffic", "POST", value),
                    t,
                  ),
                )
              }
            >
              {t("platform.importTraffic")}
            </Button>
            <Button
              variant="danger-ghost"
              onClick={() =>
                edit(
                  structuredEditor(
                    t("platform.disableNodeTitle"),
                    { ids: [] },
                    {
                      type: "object",
                      fields: { ids: { type: "array", item: { type: "string" } } },
                    },
                    (value) => write("/api/node/kick", "POST", value),
                    t,
                  ),
                )
              }
            >
              {t("platform.disableNode")}
            </Button>
          </div>
        )}
      </div>
      <section className="surface">
        <SectionHeader
          title={t("nav.node")}
          description={t("platform.platformCount", { count: status.platforms.length })}
        />
        {status.platforms.length ? (
          <DataTable
            headers={[
              t("platform.column.platform"),
              t("platform.column.scope"),
              t("platform.column.mode"),
              t("platform.column.reverse"),
              t("platform.column.requests"),
            ]}
            rows={status.platforms.map((platform) => ({
              id: platform.id,
              cells: [
                <strong>{platform.id}</strong>,
                platform.user || t("common.allUsers"),
                t(`editor.mode.${platform.mode}`),
                platform.mode === "direct" && platform.enabled ? (
                  "—"
                ) : (
                  <Badge
                    tone={
                      !platform.enabled
                        ? "neutral"
                        : platform.reverse?.connected
                          ? "success"
                          : "warning"
                    }
                  >
                    {platform.enabled
                      ? platform.reverse?.connected
                        ? t("platform.connected")
                        : t("platform.waitingConnection")
                      : t("status.disabled")}
                  </Badge>
                ),
                platform.mode === "direct"
                  ? "—"
                  : `${platform.reverse?.requests ?? 0} / ${platform.reverse?.responses ?? 0}`,
              ],
            }))}
          />
        ) : (
          <Empty title={t("platform.emptyTitle")} description={t("platform.emptyDescription")} />
        )}
      </section>
      <section className="surface">
        <SectionHeader
          title={t("platform.callbacks")}
          description={t("platform.callbacksDescription")}
        />
        {callbacks.queues.length > 0 && (
          <DataTable
            headers={[
              t("platform.column.subscription"),
              t("table.status"),
              t("platform.column.pending"),
              t("platform.column.delivered"),
              t("platform.column.failed"),
              t("platform.column.dropped"),
            ]}
            rows={callbacks.queues.map((queue) => ({
              id: queue.target,
              cells: [
                queue.target,
                <Badge tone={queue.enabled ? "enabled" : "neutral"}>
                  {queue.enabled ? t("status.enabled") : t("status.disabled")}
                </Badge>,
                queue.total,
                queue.counters.delivered,
                queue.counters.failed,
                queue.counters.dropped,
              ],
            }))}
          />
        )}
        {pending.length ? (
          <DataTable
            label={t("platform.pendingTableLabel")}
            headers={[
              "",
              t("platform.column.subscription"),
              t("platform.column.event"),
              t("table.status"),
              t("platform.column.attempts"),
              "HTTP",
              t("action.details"),
            ]}
            rows={pending.map((item) => ({
              id: `${item.target}-${item.id}`,
              cells: [
                <input
                  type="checkbox"
                  aria-label={t("platform.selectCallback", { id: item.id })}
                  disabled={!canEdit}
                  checked={selected.has(item.id)}
                  onChange={(event) => {
                    const next = new Set(selected);
                    if (event.target.checked) next.add(item.id);
                    else next.delete(item.id);
                    setSelected(next);
                  }}
                />,
                item.target,
                item.event.kind,
                <Badge tone={item.failed ? "danger" : "warning"}>
                  {item.failed ? t("platform.failed") : t("platform.pending")}
                </Badge>,
                item.attempts,
                item.last_status ?? "—",
                <span className="description-cell">{item.last_error ?? "—"}</span>,
              ],
            }))}
          />
        ) : (
          <Empty
            title={t("platform.noPendingTitle")}
            description={t("platform.noPendingDescription")}
          />
        )}
        <div className="surface-footer pagination">
          <div className="row-actions">
            {canEdit && (
              <>
                <Button
                  disabled={!selected.size || busy}
                  onClick={() => run(() => callbackAction("replay"))}
                >
                  {t("platform.replaySelected")}
                </Button>
                <Button
                  disabled={!selected.size || busy}
                  variant="danger-ghost"
                  onClick={() =>
                    confirm({
                      title: t("platform.deleteCallbacksTitle"),
                      message: t("platform.deleteCallbacksMessage"),
                      action: () => callbackAction("delete"),
                    })
                  }
                >
                  {t("platform.deleteSelected")}
                </Button>
              </>
            )}
          </div>
          <div className="row-actions">
            <Button disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - 100))}>
              {t("action.previous")}
            </Button>
            <span>{t("platform.page", { page: Math.floor(offset / 100) + 1 })}</span>
            <Button
              disabled={callbacks.next_offset == null}
              onClick={() => setOffset(callbacks.next_offset)}
            >
              {t("action.next")}
            </Button>
          </div>
        </div>
      </section>
      <section className="surface">
        <SectionHeader
          title={t("platform.changesTitle")}
          description={t("platform.changesDescription")}
        />
        {changes.events?.length ? (
          <DataTable
            headers={[
              t("platform.column.sequence"),
              t("platform.column.revision"),
              t("platform.column.event"),
              t("platform.column.time"),
            ]}
            rows={changes.events.map((event) => ({
              id: event.sequence,
              cells: [
                event.sequence,
                event.revision,
                event.kind,
                dateTime(new Date(event.timestamp_ms)),
              ],
            }))}
          />
        ) : (
          <Empty title={t("platform.noChanges")} />
        )}
        <div className="surface-footer row-actions">
          <Button
            disabled={busy || cursors.length < 2}
            onClick={() => run(() => history(cursors.slice(0, -1)))}
          >
            {t("action.previous")}
          </Button>
          <Button
            disabled={busy || Number(changes.cursor?.split(":").at(-1)) >= changes.current}
            onClick={() => run(() => history([...cursors, changes.cursor]))}
          >
            {t("action.next")}
          </Button>
        </div>
      </section>
    </>
  );
}
