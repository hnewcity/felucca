import { Disclosure } from "./Disclosure.jsx";
import { useRef, useState } from "react";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import { Modal } from "./Dialogs.jsx";
import { Button, Info } from "./ui.jsx";
import { Choice } from "./Flow.jsx";
import {
  buildService,
  deviceName,
  nextServiceId,
  serviceDraft,
  serviceDevices,
  serverAddress,
} from "../features/product-flow.js";
import { ConfigFields } from "./ConfigFields.jsx";
import { schema } from "../features/config-model.js";
import { omit } from "../lib/format.js";
import { NameInput } from "./NameInput.jsx";
import { generateName } from "../lib/names.js";
import { Select } from "./Select.jsx";

export default function ServiceWizard({ data, initial, onClose, onSaved }) {
  const { t } = useLocale();
  const { mutate, api, session } = useAdmin();
  const candidates = serviceDevices(data.clients, data.shares);
  const [clientId, setClient] = useState(
    initial?.client ||
      candidates.find((client) => client.id === data.chosenClient)?.id ||
      candidates[0]?.id ||
      "",
  );
  const original = initial?.tunnel;
  const all = [
    ...Object.values(data.managedByClient).flatMap((entry) => entry.tunnel ?? []),
    ...data.overview.tunnels.map((entry) => entry.spec),
  ];
  const existingNames = all.map((tunnel) => tunnel.remark);
  const [draft, setDraft] = useState(() => ({
    ...serviceDraft(original, all),
    ...(!original ? { name: generateName(t("names.service"), { existingNames }) } : {}),
  }));
  const [advanced, setAdvanced] = useState(() => structuredClone(original ?? {}));
  const [proxyUser, setProxyUser] = useState("");
  const [proxyPassword, setProxyPassword] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const saving = useRef(false);
  const set = (key, value) => setDraft((old) => ({ ...old, [key]: value }));
  const kinds = [
    "website",
    "ssh",
    "rdp",
    "tcp",
    "udp",
    "socks5",
    "httpproxy",
    "mixedproxy",
    "file",
  ];
  const selected = candidates.find((row) => row.id === clientId);
  const domainRoute =
    ["website", "http"].includes(draft.kind) && draft.access === "public" && draft.domain.trim();
  const existingHttpPort = data.launch.http_bind
    ? Number(data.launch.http_bind.split(":").at(-1))
    : null;
  let previewEntry = t("flow.private");
  if (draft.access === "public") {
    try {
      previewEntry = serverAddress(
        domainRoute ? draft.domain.trim() : window.location.hostname,
        !original && domainRoute && existingHttpPort ? existingHttpPort : draft.port,
      );
    } catch {
      previewEntry = t("form.incomplete");
    }
  }
  async function save(event) {
    event.preventDefault();
    if (saving.current) return;
    saving.current = true;
    setBusy(true);
    setError("");
    try {
      if (!selected) throw new Error("flow.connectFirst");
      if (!selected.managed && data.overview.tunnels.some((route) => route.client === clientId))
        throw new Error("flow.existingLocalServices");
      const managed = data.managedByClient[clientId];
      const tunnel = buildService(
        {
          ...draft,
          port: !original && domainRoute && existingHttpPort ? existingHttpPort : draft.port,
        },
        original ? advanced : undefined,
        nextServiceId(managed?.tunnel ?? []),
      );
      if (!original)
        Object.assign(
          tunnel,
          omit(advanced, [
            "id",
            "mode",
            "remark",
            "enabled",
            "public",
            "port",
            "bind",
            "backend",
            "hosts",
            "file",
          ]),
        );
      if (!original && advanced.backend?.length && tunnel.backend.length)
        tunnel.backend = [
          { ...advanced.backend[0], addr: draft.target.trim() },
          ...advanced.backend.slice(1),
        ];
      if (!original && tunnel.mode === "file")
        tunnel.file = { ...tunnel.file, ...advanced.file, root: draft.folder.trim() };
      if (["socks5", "httpproxy", "mixedproxy"].includes(tunnel.mode)) {
        const users = { ...(tunnel.options?.auth?.users ?? {}) };
        if (proxyUser.trim() && proxyPassword) users[proxyUser.trim()] = proxyPassword;
        if (!Object.keys(users).length) throw new Error("flow.proxyAuthRequired");
        tunnel.options = { ...tunnel.options, auth: { ...tunnel.options?.auth, users } };
      }
      const operations = [];
      if (tunnel.mode === "http" && !original && !existingHttpPort) {
        if (session.principal.role !== "admin") throw new Error("flow.websiteAdmin");
        const result = await api("/api/config");
        if (result.revision !== data.revision) throw new Error(t("error.staleRevision"));
        const config = result.config;
        config.http = { ...config.http, bind: `0.0.0.0:${tunnel.port}` };
        operations.push({ method: "PUT", path: "/api/config", value: config });
      }
      if (!selected?.managed)
        operations.push({
          method: "PUT",
          path: `/api/managed/${encodeURIComponent(clientId)}`,
          value: { tunnel: [] },
        });
      operations.push({
        method: "PUT",
        path: `/api/managed/${encodeURIComponent(clientId)}/tunnels/${tunnel.id}`,
        value: tunnel,
      });
      await mutate("/api/batch", "POST", { operations }, data.revision);
      onSaved({ client: clientId, tunnel });
    } catch (error) {
      setError(error.message.startsWith("flow.") ? t(error.message) : error.message);
    } finally {
      saving.current = false;
      setBusy(false);
    }
  }
  const proxy = ["socks5", "httpproxy", "mixedproxy"].includes(draft.kind);
  const fields = omit(schema("TunnelConfig").fields, [
    "id",
    "mode",
    "remark",
    "enabled",
    "public",
    "port",
    "bind",
    "hosts",
    ...(proxy || draft.kind === "file" ? ["backend"] : []),
    ...(draft.kind !== "file" ? ["file"] : []),
  ]);
  return (
    <Modal
      id="service-wizard"
      title={t(original ? "flow.editService" : "flow.addService")}
      onClose={onClose}
      busy={busy}
      wide
    >
      <form className="form-stack" onSubmit={save}>
        <div className="fields">
          <NameInput
            label={t("flow.serviceName")}
            prefix={t("names.service")}
            existingNames={existingNames}
            name="serviceName"
            value={draft.name}
            required
            disabled={busy}
            onChange={(value) => set("name", value)}
            placeholder={t("flow.serviceExample")}
            hint={t("flow.serviceExample")}
          />
          <label>
            {t("flow.onDevice")}
            <Select
              aria-label={t("flow.onDevice")}
              name="serviceDevice"
              value={clientId}
              disabled={Boolean(original)}
              required
              onValueChange={setClient}
              options={[
                { value: "", label: t("cap.select"), disabled: true },
                ...candidates.map((row) => ({ value: row.id, label: deviceName(row) })),
              ]}
            />
          </label>
          <label>
            {t("flow.serviceKind")}
            <Select
              aria-label={t("flow.serviceKind")}
              name="serviceKind"
              value={draft.kind}
              disabled={Boolean(original)}
              onValueChange={(kind) => {
                setDraft((old) => ({
                  ...old,
                  kind,
                  target:
                    kind === "ssh"
                      ? "127.0.0.1:22"
                      : kind === "rdp"
                        ? "127.0.0.1:3389"
                        : old.target,
                  access: ["socks5", "httpproxy", "mixedproxy"].includes(kind)
                    ? "public"
                    : old.access,
                }));
              }}
              options={[...new Set([...kinds, ...(original ? [original.mode] : [])])].map(
                (kind) => ({ value: kind, label: t(`flow.kind.${kind}`) }),
              )}
            />
          </label>
          {!proxy && draft.kind !== "file" && (
            <label>
              {t("flow.targetAddress")}
              <input
                name="serviceTarget"
                value={draft.target}
                required
                onChange={(event) => set("target", event.target.value)}
              />
              <span className="field-hint">{t("flow.targetHint")}</span>
            </label>
          )}
          {draft.kind === "file" && <Info>{t("flow.fileServiceHint")}</Info>}
        </div>
        <Choice
          label={t("flow.whoAccesses")}
          value={draft.access}
          onChange={(value) => set("access", value)}
          options={[
            ["public", t("flow.public"), t("flow.publicHint")],
            ...(!proxy && draft.kind !== "http"
              ? [["private", t("flow.private"), t("flow.privateHint")]]
              : []),
          ]}
        />
        {draft.access === "public" && (
          <div className="fields">
            <label>
              {t("flow.accessPort")}
              <input
                name="servicePort"
                type="number"
                min="1"
                max="65535"
                required
                value={!original && domainRoute && existingHttpPort ? existingHttpPort : draft.port}
                disabled={Boolean(!original && domainRoute && existingHttpPort)}
                onChange={(event) => set("port", event.target.value)}
              />
              <span className="field-hint">{t("form.hint.publicPort")}</span>
            </label>
            {["website", "http"].includes(draft.kind) && (
              <label>
                {t("flow.domain")}
                <input
                  name="serviceDomain"
                  value={draft.domain}
                  required={draft.kind === "http"}
                  onChange={(event) => set("domain", event.target.value)}
                />
                <span className="field-hint">{t("flow.domainHint")}</span>
              </label>
            )}
          </div>
        )}
        {proxy && (
          <>
            <Info>{t("flow.proxyHint")}</Info>
            <div className="fields">
              <NameInput
                label={t("flow.accessUsername")}
                prefix="account"
                existingNames={Object.keys(original?.options?.auth?.users ?? {})}
                name="proxyUser"
                value={proxyUser}
                disabled={busy}
                required={!Object.keys(original?.options?.auth?.users ?? {}).length}
                onChange={setProxyUser}
              />
              <label>
                {t("flow.accessPassword")}
                <input
                  name="proxyPassword"
                  type="password"
                  autoComplete="new-password"
                  value={proxyPassword}
                  required={
                    !Object.keys(original?.options?.auth?.users ?? {}).length || Boolean(proxyUser)
                  }
                  onChange={(event) => setProxyPassword(event.target.value)}
                />
              </label>
            </div>
          </>
        )}
        {!proxy && draft.kind !== "file" && (
          <div className="service-route-preview">
            <h3>{t("form.routePreview")}</h3>
            <ol>
              <li>
                <span>{t("form.entryPoint")}</span>
                <strong>{previewEntry}</strong>
              </li>
              <li>
                <span>{t("flow.onDevice")}</span>
                <strong>{selected ? deviceName(selected) : t("form.incomplete")}</strong>
              </li>
              <li>
                <span>{t("flow.targetAddress")}</span>
                <strong>{draft.target || t("form.incomplete")}</strong>
              </li>
            </ol>
          </div>
        )}
        {!selected?.managed && <Info>{t("flow.takeOverHint")}</Info>}
        <Disclosure className="advanced" label={t("flow.moreServiceOptions")}>
          <ConfigFields
            context="TunnelConfig"
            descriptor={{ type: "object", fields }}
            value={advanced}
            onChange={(next) => {
              if (
                next.backend?.[0]?.addr !== advanced.backend?.[0]?.addr &&
                next.backend?.[0]?.addr
              )
                set("target", next.backend[0].addr);
              if (next.file?.root !== advanced.file?.root && next.file?.root)
                set("folder", next.file.root);
              setAdvanced(next);
            }}
            disabled={busy}
          />
        </Disclosure>
        <p className="form-note">
          {t(
            !original && domainRoute && !existingHttpPort ? "config.applyMessage" : "flow.saveNote",
          )}
        </p>
        <p className="form-error" role="alert">
          {error}
        </p>
        <div className="dialog-actions">
          {original && <span className="dialog-action-note">{t("form.saveDraft")}</span>}
          <Button disabled={busy} onClick={onClose}>
            {t("action.cancel")}
          </Button>
          <Button variant="primary" type="submit" busy={busy}>
            {t(original ? "action.save" : "flow.addService")}
          </Button>
        </div>
      </form>
    </Modal>
  );
}
