import { useRef, useState } from "react";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import { Switch } from "../components/Switch.jsx";
import { Button, DataTable, Empty, Info, PageHeader } from "../components/ui.jsx";
import { Modal } from "../components/Dialogs.jsx";
import { CopyValue } from "../components/Flow.jsx";
import { OneLineConnection } from "../components/OneLineConnection.jsx";
import { NameInput } from "../components/NameInput.jsx";
import { generateName } from "../lib/names.js";
import { Select } from "../components/Select.jsx";
import {
  deviceConfig,
  deviceName,
  deviceOperations,
  newIdentity,
  randomKey,
  serviceName,
  visitConfig,
} from "../features/product-flow.js";

export default function Sharing({ data }) {
  const { t } = useLocale();
  const { mutate, session, navigate, refresh, confirm, api } = useAdmin();
  const existingNames = data.clients.map(deviceName);
  const [opened, setOpened] = useState(false);
  const [name, setName] = useState("");
  const [host, setHost] = useState(window.location.hostname.replace(/^\[|\]$/g, ""));
  const [port, setPort] = useState(18080);
  const [lanEntry, setLanEntry] = useState(false);
  const [packageValue, setPackage] = useState(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [permission, setPermission] = useState(null);
  const identity = useRef(null);
  const saving = useRef(false);
  const services = Object.entries(data.managedByClient).flatMap(([client, managed]) =>
    (managed.tunnel ?? [])
      .filter((tunnel) => tunnel.public === false && ["tcp", "udp", "file"].includes(tunnel.mode))
      .map((tunnel) => ({ client, tunnel })),
  );
  const [selected, setSelected] = useState("");
  const eligible = services.filter((row) => !data.chosenClient || row.client === data.chosenClient);
  const service =
    services.find((row) => `${row.client}/${row.tunnel.id}` === selected) ??
    eligible[0] ??
    services[0];
  const editable = session.principal.role !== "viewer";
  async function create(event) {
    event.preventDefault();
    if (saving.current) return;
    saving.current = true;
    setBusy(true);
    setError("");
    try {
      identity.current ??= {
        device: newIdentity("device"),
        share: newIdentity("share"),
        key: randomKey(),
        secret: randomKey(),
      };
      const ids = identity.current;
      const owner = data.clients.find((row) => row.id === service.client)?.config.user;
      const share = {
        id: ids.share,
        config: {
          client: service.client,
          tunnel: service.tunnel.id,
          secret: ids.secret,
          allow_clients: [ids.device],
          direct: Boolean(data.launch.p2p),
          dynamic: false,
        },
      };
      const visitor = visitConfig(share, service.tunnel, port, lanEntry);
      const config = deviceConfig({
        id: ids.device,
        key: ids.key,
        host,
        launch: data.launch,
        visitor,
      });
      // The receiving machine only consumes this share. It does not expose its own services.
      config.remote.enabled = false;
      const operations = deviceOperations(ids.device, ids.key, name, owner).slice(0, 1);
      operations.push({ method: "PUT", path: `/api/shares/${ids.share}`, value: share.config });
      await mutate("/api/batch", "POST", { operations }, data.revision);
      setPackage(config);
    } catch (error) {
      setError(error.message.startsWith("flow.") ? t(error.message) : error.message);
    } finally {
      saving.current = false;
      setBusy(false);
    }
  }
  const open = () => {
    if (!name.trim() || packageValue)
      setName(generateName(t("names.receiver"), { existingNames, current: name }));
    setOpened(true);
    setPackage(null);
    identity.current = null;
    setError("");
  };
  return (
    <>
      <PageHeader title={t("nav.shares")} description={t("flow.sharingHint")}>
        {editable && services.length > 0 && (
          <Button variant="primary" onClick={open}>
            {t("flow.connectAnother")}
          </Button>
        )}
      </PageHeader>
      <section className="surface">
        {data.shares.length ? (
          <DataTable
            headers={[
              t("flow.serviceName"),
              t("flow.onDevice"),
              t("flow.receivingDevice"),
              t("flow.connectionMode"),
              t("table.actions"),
            ]}
            rows={data.shares.map((share) => {
              const route = services.find(
                (row) =>
                  row.client === share.config.client && row.tunnel.id === share.config.tunnel,
              );
              return {
                id: share.id,
                cells: [
                  route ? serviceName(route.tunnel) : share.id,
                  deviceName(data.clients.find((row) => row.id === share.config.client)) ||
                    share.config.client,
                  share.config.allow_clients.length
                    ? share.config.allow_clients
                        .map((id) => deviceName(data.clients.find((row) => row.id === id)) || id)
                        .join(", ")
                    : t("flow.allAuthenticated"),
                  t(share.config.direct ? "flow.automatic" : "flow.viaServer"),
                  editable && (
                    <div className="row-actions">
                      <Button
                        onClick={() => {
                          setError("");
                          setPermission(structuredClone(share));
                        }}
                      >
                        {t("flow.accessPermissions")}
                      </Button>
                      <Button
                        variant="danger-ghost"
                        onClick={() =>
                          confirm({
                            title: t("flow.revokeShare"),
                            message: t("flow.revokeHint"),
                            action: async () => {
                              await mutate(
                                `/api/shares/${encodeURIComponent(share.id)}`,
                                "DELETE",
                                undefined,
                                data.revision,
                              );
                              refresh(t("notice.deleteSaved"));
                            },
                          })
                        }
                      >
                        {t("flow.revokeShare")}
                      </Button>
                    </div>
                  ),
                ],
              };
            })}
          />
        ) : (
          <Empty
            title={t("flow.shareFirst")}
            description={t(services.length ? "flow.sharingHint" : "flow.privateFirst")}
            action={
              editable && (
                <Button
                  variant="primary"
                  onClick={() =>
                    services.length ? open() : navigate("tunnels", data.chosenClient || "")
                  }
                >
                  {t(services.length ? "flow.connectAnother" : "flow.addService")}
                </Button>
              )
            }
          />
        )}
      </section>
      {permission && (
        <Modal
          id="share-permissions"
          title={t("flow.accessPermissions")}
          onClose={() => setPermission(null)}
          busy={busy}
        >
          <form
            className="form-stack"
            onSubmit={async (event) => {
              event.preventDefault();
              if (saving.current) return;
              saving.current = true;
              setBusy(true);
              setError("");
              try {
                await mutate(
                  `/api/shares/${encodeURIComponent(permission.id)}`,
                  "PUT",
                  permission.config,
                  data.revision,
                );
                setPermission(null);
                refresh(t("notice.saved"));
              } catch (error) {
                setError(error.message);
              } finally {
                saving.current = false;
                setBusy(false);
              }
            }}
          >
            <Info>{t("flow.permissionHint")}</Info>
            <p>
              {t(
                permission.config.allow_clients.length
                  ? "flow.receivingDevice"
                  : "flow.allAuthenticated",
              )}
            </p>
            {data.clients.map((row) => (
              <label className="check-field" key={row.id}>
                <input
                  type="checkbox"
                  checked={permission.config.allow_clients.includes(row.id)}
                  onChange={(event) =>
                    setPermission((old) => ({
                      ...old,
                      config: {
                        ...old.config,
                        allow_clients: event.target.checked
                          ? [...old.config.allow_clients, row.id]
                          : old.config.allow_clients.filter((id) => id !== row.id),
                      },
                    }))
                  }
                />
                {deviceName(row)}
              </label>
            ))}
            <Switch
              label={t("flow.automatic")}
              checked={permission.config.direct}
              disabled={!data.launch.p2p}
              onChange={(direct) =>
                setPermission((old) => ({
                  ...old,
                  config: { ...old.config, direct },
                }))
              }
            />
            <p className="form-note">{t("flow.saveNote")}</p>
            <p className="form-error" role="alert">
              {error}
            </p>
            <Button type="submit" variant="primary" busy={busy}>
              {t("action.save")}
            </Button>
          </form>
        </Modal>
      )}
      {opened && (
        <Modal
          id="share-wizard"
          title={t("flow.connectAnother")}
          onClose={() => {
            setOpened(false);
            if (packageValue) refresh();
          }}
          busy={busy}
          wide
        >
          {!packageValue ? (
            <form className="flow-panel" onSubmit={create}>
              <div className="fields">
                <label>
                  {t("flow.serviceName")}
                  <Select
                    aria-label={t("flow.serviceName")}
                    name="sharedService"
                    value={service ? `${service.client}/${service.tunnel.id}` : ""}
                    onValueChange={setSelected}
                    options={services.map((row) => ({
                      value: `${row.client}/${row.tunnel.id}`,
                      label: `${serviceName(row.tunnel)} · ${deviceName(data.clients.find((client) => client.id === row.client))}`,
                    }))}
                  />
                </label>
                <NameInput
                  label={t("flow.receivingDevice")}
                  prefix={t("names.receiver")}
                  existingNames={existingNames}
                  name="receivingName"
                  required
                  maxLength={80}
                  value={name}
                  disabled={busy}
                  onChange={setName}
                />
                <label>
                  {t("flow.serverHost")}
                  <input required value={host} onChange={(event) => setHost(event.target.value)} />
                  <span className="field-hint">{t("flow.serverHostHint")}</span>
                </label>
                <label>
                  {t("flow.localPort")}
                  <input
                    name="localPort"
                    type="number"
                    min="1"
                    max="65535"
                    required
                    value={port}
                    onChange={(event) => setPort(event.target.value)}
                  />
                </label>
              </div>
              <label className="check-field">
                <input
                  type="checkbox"
                  checked={lanEntry}
                  onChange={(event) => setLanEntry(event.target.checked)}
                />
                {t("flow.visitorLan")}
              </label>
              <span className="field-hint">{t("flow.visitorLanHint")}</span>
              <Info>{t(data.launch.p2p ? "flow.directHint" : "flow.relayHint")}</Info>
              {!data.launch.p2p && session.principal.role === "admin" && (
                <Button onClick={() => navigate("settings")}>{t("flow.enableDirect")}</Button>
              )}
              <p className="form-note">{t("flow.saveNote")}</p>
              <p className="form-error" role="alert">
                {error}
              </p>
              <Button variant="primary" type="submit" busy={busy}>
                {t("flow.prepareConnection")}
              </Button>
            </form>
          ) : (
            <div className="flow-panel">
              <OneLineConnection config={packageValue} name={name} packages={data.packages} />
              <h3>{t("flow.openLocally")}</h3>
              <CopyValue value={`127.0.0.1:${port}`} label={t("flow.accessAddress")} />
              <p>{t("flow.localAccessHint")}</p>
              <Button
                onClick={async () => {
                  try {
                    const result = await api("/api/overview");
                    setError(
                      t(
                        result.clients.some((row) => row.id === packageValue.key_id && row.online)
                          ? "flow.deviceReady"
                          : "flow.waitingDevice",
                      ),
                    );
                  } catch (error) {
                    setError(error.message);
                  }
                }}
              >
                {t("action.refresh")}
              </Button>
              <p role="status">{error}</p>
            </div>
          )}
        </Modal>
      )}
    </>
  );
}
