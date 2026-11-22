import { Disclosure } from "../components/Disclosure.jsx";
import { ConfigFields } from "../components/ConfigFields.jsx";
import { schema } from "../features/config-model.js";
import { useRef, useState } from "react";
import { Check, Download, Save, Upload } from "lucide-react";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import { Badge, Button, Info, PageHeader, SectionHeader } from "../components/ui.jsx";
import { download, parseObject } from "../lib/format.js";
import { Select } from "../components/Select.jsx";
import { SettingGuide } from "../components/SettingGuide.jsx";
import { TimeSettings } from "../components/TimeSettings.jsx";
import { serverSettings } from "../features/settings-model.js";

export default function Config({ data }) {
  const { api, mutate, confirm, refresh, run, notify, busy } = useAdmin();
  const { t } = useLocale();
  const initial = JSON.stringify(data.config, null, 2);
  const [text, setText] = useState(initial);
  const input = useRef(null);
  const form = useRef(null);
  const [section, setSection] = useState("bridge");
  const [plan, setPlan] = useState(null);
  const [restartPackage, setRestartPackage] = useState(null);
  const [certificate, setCertificate] = useState(null);
  const [privateKey, setPrivateKey] = useState(null);
  const [certificateHost, setCertificateHost] = useState("");
  const [certificateUse, setCertificateUse] = useState("bridge");
  const change = (next) => {
    setText(next);
    setPlan(null);
    setRestartPackage(null);
  };
  function draft() {
    if (!form.current.reportValidity()) throw new Error(t("flow.formInvalid"));
    return parseObject(text, t);
  }
  async function validate() {
    const result = await api("/api/validate", "POST", draft());
    setPlan(result);
    notify(t(result.restart_fields.length ? "cap.restartRequired" : "config.valid"));
    return result;
  }
  function exportRestart() {
    confirm({
      title: t("cap.restartExport"),
      message: t("config.exportMessage"),
      danger: false,
      action: async () => {
        const result = await api("/api/config/export", "POST", draft(), {
          revision: data.revision,
        });
        setRestartPackage(result);
        setPlan(result);
      },
    });
  }
  const dirty = text !== initial;
  return (
    <>
      <PageHeader title={t("config.title")} description={t("config.description")}>
        <Button
          icon={Check}
          disabled={busy}
          onClick={() =>
            run(async () => {
              await validate();
            })
          }
        >
          {t("config.validate")}
        </Button>
        <Button
          variant="primary"
          icon={Save}
          disabled={busy}
          onClick={() =>
            run(async () => {
              const config = parseObject(text, t);
              const result = await validate();
              if (result.restart_fields.length) return;
              confirm({
                title: t("config.applyTitle"),
                message: t(result.activation === "live" ? "flow.saveNote" : "config.applyMessage"),
                danger: false,
                action: async () => {
                  await mutate("/api/config", "PUT", config, data.revision);
                  refresh(t("notice.configSaved"));
                },
              });
            })
          }
        >
          {t("config.save")}
        </Button>
      </PageHeader>
      <Info>{t("config.info")}</Info>
      {plan?.restart_fields?.length > 0 && (
        <Info>
          {t("cap.restartRequired")} {plan.restart_fields.join(", ")}
        </Info>
      )}
      {restartPackage && (
        <section className="surface">
          <SectionHeader title={t("cap.restartExport")} />
          <div className="surface-body form-stack">
            <p>
              {t("cap.restartInstructions", {
                path: restartPackage.runtime_path ?? t("cap.noSnapshot"),
              })}
            </p>
            <div className="row-actions">
              <Button
                onClick={() =>
                  download(
                    JSON.stringify(restartPackage.startup, null, 2),
                    "felucca-server-restart.json",
                  )
                }
              >
                felucca-server-restart.json
              </Button>
              <Button
                onClick={() =>
                  download(
                    JSON.stringify(restartPackage.snapshot, null, 2),
                    "felucca-runtime-restart.json",
                  )
                }
              >
                felucca-runtime-restart.json
              </Button>
              <Button onClick={() => setRestartPackage(null)}>{t("action.close")}</Button>
            </div>
          </div>
        </section>
      )}
      <Disclosure className="surface surface-disclosure" label={t("cap.uploadCertificate")}>
        <div className="surface-body form-stack">
          <p>{t("cap.uploadHint")}</p>
          <div className="fields">
            <label>
              {t("cap.certificates")}
              <input
                aria-label="certificate-pem"
                type="file"
                accept=".pem,.crt"
                onChange={(event) => setCertificate(event.target.files[0])}
              />
            </label>
            <label>
              {t("field.key")}
              <input
                aria-label="private-key-pem"
                type="file"
                accept=".pem,.key"
                onChange={(event) => setPrivateKey(event.target.files[0])}
              />
            </label>
            <label>
              {t("form.certificateUse")}
              <Select
                aria-label={t("form.certificateUse")}
                value={certificateUse}
                onValueChange={setCertificateUse}
                options={[
                  { value: "bridge", label: t("guide.server.bridge.title") },
                  { value: "default", label: t("field.default_tls") },
                  { value: "domain", label: t("field.hosts") },
                ]}
              />
            </label>
            {certificateUse === "domain" && (
              <label>
                {t("field.hosts")}
                <input
                  aria-label={t("field.hosts")}
                  value={certificateHost}
                  placeholder="app.example.com"
                  onChange={(event) => setCertificateHost(event.target.value)}
                />
                <span className="field-hint">
                  {t("form.example", { value: "app.example.com" })}
                </span>
              </label>
            )}
          </div>
          <Button
            disabled={
              !certificate ||
              !privateKey ||
              (certificateUse === "domain" && !certificateHost.trim()) ||
              busy
            }
            onClick={() =>
              run(async () => {
                if (certificate.size + privateKey.size > 1024 * 1024)
                  throw new Error(t("config.importTooLarge"));
                const files = await api(
                  "/api/certificates/upload",
                  "POST",
                  { cert: await certificate.text(), key: await privateKey.text() },
                  { revision: data.revision },
                );
                const next = parseObject(text, t);
                const host = certificateUse === "domain" ? certificateHost.trim() : certificateUse;
                if (host === "bridge") {
                  next.bridge ??= {};
                  next.bridge.tls = files;
                } else {
                  next.http ??= {};
                  if (host === "default") next.http.default_tls = files;
                  else next.http.certificates = { ...next.http.certificates, [host]: files };
                }
                change(JSON.stringify(next, null, 2));
                setCertificate(null);
                setPrivateKey(null);
                notify(t("config.imported"));
              })
            }
          >
            {t("cap.uploadCertificate")}
          </Button>
        </div>
      </Disclosure>
      <section className="surface config-surface">
        <SectionHeader
          title={t("config.section")}
          description={`${t("overview.revision")} ${data.revision}`}
        >
          <Badge tone={dirty ? "warning" : "neutral"} dot={dirty}>
            {dirty ? t("config.dirty") : t("config.loaded")}
          </Badge>
        </SectionHeader>
        <div className="surface-body">
          <Select
            className="config-section-select"
            aria-label={t("guide.chooseSection")}
            value={section}
            onValueChange={setSection}
            options={serverSettings.map((key) => ({
              value: key,
              label: t(`guide.server.${key}.title`),
            }))}
          />
        </div>
        <div className="config-toolbar">
          <span>{t("flow.backup")}</span>
          <div className="row-actions">
            <Button icon={Upload} onClick={() => input.current.click()}>
              {t("config.import")}
            </Button>
            <Button icon={Download} onClick={() => download(text)}>
              {t("config.export")}
            </Button>
            <Button
              onClick={() =>
                confirm({
                  title: t("config.exportTitle"),
                  message: t("config.exportMessage"),
                  danger: false,
                  action: async () => {
                    const result = await api("/api/config?secrets=1");
                    download(JSON.stringify(result.config, null, 2));
                    notify(t("config.exportedSecrets"));
                  },
                })
              }
            >
              {t("config.exportSecrets")}
            </Button>
            <Button onClick={exportRestart}>{t("cap.restartExport")}</Button>
          </div>
        </div>
        <form ref={form} onSubmit={(event) => event.preventDefault()}>
          {section === "ntp" ? (
            <TimeSettings
              config={JSON.parse(text)}
              disabled={busy}
              onChange={(next) => change(JSON.stringify(next, null, 2))}
            />
          ) : (
            <SettingGuide topic={`server.${section}`}>
              <p className="setting-impact">{t(`guide.server.${section}.default`)}</p>
              <ConfigFields
                key={section}
                context="ServerConfig"
                descriptor={{
                  type: "object",
                  // Source rules are chosen together with their mode.
                  fields: Object.fromEntries(
                    (section === "acl_mode" ? ["acl_mode", "acl_rules"] : [section]).map((key) => [
                      key,
                      schema("ServerConfig").fields[key],
                    ]),
                  ),
                }}
                value={JSON.parse(text)}
                disabled={busy}
                onChange={(next) => change(JSON.stringify(next, null, 2))}
              />
            </SettingGuide>
          )}
        </form>
        <input
          ref={input}
          type="file"
          accept=".json,application/json"
          hidden
          onChange={(event) => {
            const file = event.target.files[0];
            event.target.value = "";
            if (file)
              run(async () => {
                if (file.size > 2 * 1024 * 1024) throw new Error(t("config.importTooLarge"));
                const contents = await file.text();
                parseObject(contents, t);
                change(contents);
                notify(t("config.imported"));
              });
          }}
        />
        <div className="surface-footer">
          <span>{t("config.footerHint")}</span>
        </div>
      </section>
    </>
  );
}
