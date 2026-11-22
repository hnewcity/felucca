import { Disclosure } from "../components/Disclosure.jsx";
import { useState } from "react";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import { Button, Info, PageHeader } from "../components/ui.jsx";
import { pages } from "../app/navigation.js";
import { serverAddress } from "../features/product-flow.js";
import { Choice } from "../components/Flow.jsx";
import { SettingGuide } from "../components/SettingGuide.jsx";
import { settingsGroups } from "../features/settings-model.js";

export default function Settings({ data }) {
  const { t } = useLocale();
  const { navigate, session, mutate, refresh, confirm, run, busy } = useAdmin();
  const [direct, setDirect] = useState(Boolean(data.config?.p2p));
  const [host, setHost] = useState(
    data.config?.p2p?.advertise?.replace(/:\d+$/, "").replace(/^\[|\]$/g, "") ||
      window.location.hostname,
  );
  const [port, setPort] = useState(Number(data.config?.p2p?.bind?.split(":").at(-1)) || 14029);
  return (
    <>
      <PageHeader title={t("flow.settings")} description={t("flow.settingsHint")} />
      <section className="surface setting-shortcuts">
        <div>
          <h2>{t("guide.start.title")}</h2>
          <p>{t("guide.start.description")}</p>
        </div>
        <div className="row-actions">
          <Button onClick={() => navigate("deployment", "")}>{t("flow.connectDevice")}</Button>
          <Button onClick={() => navigate("shares")}>{t("guide.visitor.wizard")}</Button>
        </div>
      </section>
      {session.principal.role === "admin" && (
        <Disclosure label={t("flow.directConnections")}>
          <SettingGuide
            topic="directServer"
            current={t(data.config?.p2p ? "guide.directServer.on" : "guide.directServer.off")}
          >
            <form
              className="form-stack"
              onSubmit={(event) => {
                event.preventDefault();
                run(async () => {
                  const config = structuredClone(data.config);
                  config.p2p = direct
                    ? {
                        ...(config.p2p ?? {}),
                        bind: serverAddress(
                          config.p2p?.bind?.replace(/:\d+$/, "") || "0.0.0.0",
                          port,
                        ),
                        advertise: serverAddress(host, port),
                      }
                    : null;
                  confirm({
                    title: t("flow.directConnections"),
                    message: t(
                      direct ? "guide.directServer.confirmOn" : "guide.directServer.confirmOff",
                    ),
                    danger: false,
                    action: async () => {
                      await mutate("/api/config", "PUT", config, data.revision);
                      refresh(t("notice.saved"));
                    },
                  });
                });
              }}
            >
              <Choice
                label={t("guide.directServer.choice")}
                value={direct ? "direct" : "relay"}
                onChange={(next) => setDirect(next === "direct")}
                options={[
                  ["relay", t("guide.directServer.relay"), t("guide.directServer.relayHint")],
                  ["direct", t("guide.directServer.direct"), t("guide.directServer.directHint")],
                ]}
              />
              {direct && (
                <div className="fields">
                  <label>
                    {t("flow.serverHost")}
                    <input
                      required
                      value={host}
                      onChange={(event) => setHost(event.target.value)}
                    />
                    <span className="field-hint">{t("flow.serverHostHint")}</span>
                  </label>
                  <label>
                    {t("flow.directPort")}
                    <input
                      type="number"
                      min="1"
                      max="65535"
                      required
                      value={port}
                      onChange={(event) => setPort(Number(event.target.value))}
                    />
                    <span className="field-hint">{t("guide.directServer.portHint")}</span>
                  </label>
                </div>
              )}
              {direct && <Info>{t("flow.directServerHint", { port })}</Info>}
              <p className="setting-impact">{t("guide.directServer.impact")}</p>
              <Button variant="primary" busy={busy} type="submit">
                {t("action.save")}
              </Button>
            </form>
          </SettingGuide>
        </Disclosure>
      )}
      {settingsGroups.map((group) => {
        const links = group.pages
          .filter((key) => !pages[key].admin || session.principal.role === "admin")
          .map((key) => {
            const Icon = pages[key].icon;
            return (
              <button className="surface settings-card" key={key} onClick={() => navigate(key)}>
                <Icon size={24} />
                <strong>{t(pages[key].label)}</strong>
                <span>{t(pages[key].description)}</span>
              </button>
            );
          });
        return (
          <Disclosure
            className="settings-group"
            key={group.id}
            label={t(`guide.group.${group.id}`)}
          >
            <p>{t(`guide.group.${group.id}Hint`)}</p>
            <div className="settings-grid">{links}</div>
          </Disclosure>
        );
      })}
    </>
  );
}
