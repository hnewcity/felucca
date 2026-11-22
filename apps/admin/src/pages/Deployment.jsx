import { Disclosure } from "../components/Disclosure.jsx";
import { useRef, useState } from "react";
import { useAdmin } from "../app/context.jsx";
import { useLocale } from "../app/locale.jsx";
import { Button, Info, PageHeader, SectionHeader } from "../components/ui.jsx";
import { agentTemplate, hasMissingCredentials } from "../features/launch.js";
import { download } from "../lib/format.js";
import { deviceName } from "../features/product-flow.js";
import { Select } from "../components/Select.jsx";
import { AgentSettings } from "../components/AgentSettings.jsx";

export default function Deployment({ data }) {
  const { t } = useLocale();
  const { session, api, confirm, run, navigate } = useAdmin();
  const [chosen, setChosen] = useState(data.clients[0]?.id ?? "");
  const [config, setConfig] = useState(() =>
    data.clients[0] ? agentTemplate(data.clients[0], data.launch, window.location.hostname) : null,
  );
  const [platform, setPlatform] = useState("systemd");
  const form = useRef(null);
  function choose(id) {
    setChosen(id);
    setConfig(
      agentTemplate(
        data.clients.find((row) => row.id === id),
        data.launch,
        window.location.hostname,
      ),
    );
  }
  return (
    <>
      <PageHeader title={t("flow.advancedClient")} description={t("flow.advancedClientHint")} />
      <Info>{t("cap.localAuthority")}</Info>
      {!config ? (
        <Info>{t("tunnels.connectFirstDescription")}</Info>
      ) : (
        <>
          <section className="surface">
            <SectionHeader title={t("cap.agentConfig")}>
              <Select
                aria-label={t("tunnels.currentNode")}
                value={chosen}
                onValueChange={choose}
                options={data.clients.map((row) => ({ value: row.id, label: deviceName(row) }))}
              />
              {session.principal.role === "admin" && (
                <Button
                  onClick={() =>
                    confirm({
                      title: t("config.exportTitle"),
                      message: t("config.exportMessage"),
                      danger: false,
                      action: async () => {
                        const full = await api("/api/config?secrets=1");
                        setConfig({
                          ...config,
                          vkey: full.config.client[chosen].vkey,
                          visitor: (config.visitor ?? []).map((visitor) => ({
                            ...visitor,
                            secret: full.config.share[visitor.share]?.secret ?? visitor.secret,
                          })),
                        });
                      },
                    })
                  }
                >
                  {t("cap.fillCredentials")}
                </Button>
              )}
              <Button
                onClick={() =>
                  run(async () => {
                    if (!form.current.reportValidity()) return;
                    if (hasMissingCredentials(config))
                      throw new Error(t("cap.credentialsRequired"));
                    download(JSON.stringify(config, null, 2), "felucca-agent.json");
                  })
                }
              >
                {t("config.export")}
              </Button>
            </SectionHeader>
            <div className="surface-body">
              <p>{t("guide.agentApply")}</p>
            </div>
          </section>
          <form ref={form} className="form-stack" onSubmit={(event) => event.preventDefault()}>
            <AgentSettings
              key={chosen}
              config={config}
              onChange={setConfig}
              data={data}
              navigate={navigate}
            />
          </form>
          <Disclosure className="surface surface-disclosure" label={t("cap.hostOperations")}>
            <div className="surface-body form-stack">
              <Info>{t("cap.hostOperationsHint")}</Info>
              <Select
                aria-label={t("cap.platform")}
                value={platform}
                onValueChange={setPlatform}
                options={["systemd", "launchd", "windows"].map((name) => ({
                  value: name,
                  label: name,
                }))}
              />
              <pre className="log-tail">
                {[
                  "felucca-agent --config felucca-agent.json --check",
                  "felucca-agent --config felucca-agent.json",
                  `felucca-agent service render --platform ${platform} --config felucca-agent.json`,
                  "felucca-agent service install --config felucca-agent.json",
                  "felucca-agent service start",
                  "felucca-agent service status",
                  "felucca-agent service restart",
                  "felucca-agent service stop",
                  "felucca-agent service uninstall",
                  "felucca-agent --config first.json --config second.json",
                  "felucca-agent update --source ./verified-felucca-agent --sha256 SHA256 --check-only",
                  "felucca-agent update --source ./verified-felucca-agent --sha256 SHA256",
                  "docker compose -f compose.orbstack.yaml up --build -d",
                  "docker compose -f compose.orbstack.yaml logs --tail 100",
                  "docker compose -f compose.orbstack.yaml down",
                ].join("\n")}
              </pre>
              <p>{t("cap.operationsManual")}</p>
            </div>
          </Disclosure>
        </>
      )}
    </>
  );
}
