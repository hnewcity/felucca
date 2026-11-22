import { Disclosure } from "../components/Disclosure.jsx";
import { useEffect, useRef, useState } from "react";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import { Badge, Button, Info, PageHeader } from "../components/ui.jsx";
import { Steps } from "../components/Flow.jsx";
import { OneLineConnection } from "../components/OneLineConnection.jsx";
import { ScopeField } from "../components/ScopeField.jsx";
import { NameInput } from "../components/NameInput.jsx";
import { generateName } from "../lib/names.js";
import {
  deviceConfig,
  deviceName,
  deviceOperations,
  newIdentity,
  randomKey,
} from "../features/product-flow.js";

export default function ConnectDevice({ data }) {
  const { t } = useLocale();
  const { api, mutate, session, navigate } = useAdmin();
  const existing = data.clients.find((row) => row.id === data.chosenClient);
  const existingNames = data.clients.map(deviceName);
  const [name, setName] = useState(() =>
    existing ? deviceName(existing) : generateName(t("names.device"), { existingNames }),
  );
  const [host, setHost] = useState(window.location.hostname.replace(/^\[|\]$/g, ""));
  const [scope, setScope] = useState("any");
  const [rules, setRules] = useState("");
  const [folder, setFolder] = useState("");
  const [connection, setConnection] = useState(null);
  const [online, setOnline] = useState(false);
  const [downloaded, setDownloaded] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const identity = useRef(null);
  const saving = useRef(false);
  useEffect(() => {
    if (!connection) return;
    let active = true,
      timer;
    const abort = new AbortController();
    const poll = async () => {
      try {
        const result = await api("/api/overview", "GET", undefined, { signal: abort.signal });
        if (active) {
          setOnline(result.clients.some((row) => row.id === connection.key_id && row.online));
          setError("");
        }
      } catch (error) {
        if (active && error.name !== "AbortError") setError(error.message);
      }
      if (active) timer = setTimeout(poll, 3000);
    };
    poll();
    return () => {
      active = false;
      clearTimeout(timer);
      abort.abort();
    };
  }, [api, connection]);
  async function prepare(event) {
    event.preventDefault();
    if (saving.current) return;
    saving.current = true;
    setBusy(true);
    setError("");
    try {
      identity.current ??= { id: newIdentity("device"), key: randomKey() };
      let { id, key } = identity.current;
      if (existing) {
        if (data.shares.some((share) => share.config.allow_clients.includes(existing.id)))
          throw new Error("flow.receivingReconnect");
        const full = await api("/api/config?secrets=1");
        if (full.revision !== data.revision) throw new Error(t("error.staleRevision"));
        id = existing.id;
        key = full.config.client[id].vkey;
      }
      const config = deviceConfig({
        id,
        key,
        host,
        launch: data.launch,
        scope,
        rules,
        folder,
      });
      if (!existing)
        await mutate(
          "/api/batch",
          "POST",
          {
            operations: deviceOperations(
              id,
              key,
              name,
              session.principal.user,
              config.remote.target_policy,
            ),
          },
          data.revision,
        );
      setConnection(config);
    } catch (error) {
      setError(/^(flow|scope)\./.test(error.message) ? t(error.message) : error.message);
    } finally {
      saving.current = false;
      setBusy(false);
    }
  }
  return (
    <>
      <PageHeader title={t("flow.connectDevice")} description={t("flow.connectHint")}>
        <Button onClick={() => navigate("clients")}>{t("nav.clients")}</Button>
      </PageHeader>
      <Steps
        current={online ? 2 : connection ? 1 : 0}
        labels={[t("flow.nameDevice"), t("flow.startDevice"), t("flow.deviceReady")]}
      />
      {!connection ? (
        <form onSubmit={prepare} className="surface flow-panel">
          <h2>{t(existing ? "flow.reconnectDevice" : "flow.nameDevice")}</h2>
          {existing && <Info>{t("flow.replaceConnection")}</Info>}
          <div className="fields">
            <NameInput
              label={t("flow.deviceName")}
              prefix={t("names.device")}
              existingNames={existingNames}
              name="deviceName"
              required
              maxLength={80}
              value={name}
              disabled={Boolean(existing) || busy}
              hideGenerator={Boolean(existing)}
              placeholder={t("flow.deviceExample")}
              hint={t("flow.deviceExample")}
              onChange={setName}
            />
            <label>
              {t("flow.serverHost")}
              <input
                name="serverHost"
                required
                value={host}
                onChange={(event) => setHost(event.target.value)}
              />
              <span className="field-hint">{t("flow.serverHostHint")}</span>
            </label>
          </div>
          <Disclosure className="advanced" label={t("flow.devicePermissions")}>
            <ScopeField
              kind="target"
              label={t("flow.allowAccess")}
              presets={["any", "device", "lan", "custom"]}
              choice={scope}
              text={rules}
              onChange={(next, text) => {
                setScope(next);
                setRules(text);
              }}
            />
            <label>
              {t("flow.sharedFolder")}
              <input value={folder} onChange={(event) => setFolder(event.target.value)} />
              <span className="field-hint">{t("flow.folderHint")}</span>
            </label>
          </Disclosure>
          <p className="form-note">{t("flow.saveNote")}</p>
          <p className="form-error" role="alert">
            {error}
          </p>
          <Button variant="primary" type="submit" busy={busy}>
            {t("flow.prepareConnection")}
          </Button>
        </form>
      ) : (
        <>
          <OneLineConnection
            config={connection}
            name={name}
            packages={data.packages}
            onDownloaded={() => setDownloaded(true)}
          />
          <section className="surface flow-panel" aria-live="polite">
            <Badge tone={online ? "success" : "warning"}>
              {t(online ? "flow.deviceReady" : "flow.waitingDevice")}
            </Badge>
            <h2>{name}</h2>
            <p>
              {t(
                online ? "flow.readyHint" : downloaded ? "flow.waitingHint" : "connect.runCommand",
              )}
            </p>
            {error && (
              <p role="alert" className="form-error">
                {error}
              </p>
            )}
            {online ? (
              <Button variant="primary" onClick={() => navigate("tunnels", connection.key_id)}>
                {t("flow.addService")}
              </Button>
            ) : (
              <Disclosure label={t("flow.connectionHelp")}>
                <p>
                  {t("flow.connectionHelpText", {
                    port: data.launch.bridge.bind.split(":").at(-1),
                  })}
                </p>
              </Disclosure>
            )}
          </section>
        </>
      )}
    </>
  );
}
