import { useLocale } from "../app/locale.jsx";
import { schema } from "../features/config-model.js";
import {
  agentConnectionFields,
  agentGuidedFields,
  fieldsFor,
  patchSetting,
  remoteSummary,
} from "../features/settings-model.js";
import { visitorTemplate } from "../features/launch.js";
import { ConfigFields } from "./ConfigFields.jsx";
import { SettingDetails, SettingGuide } from "./SettingGuide.jsx";
import { TimeSettings } from "./TimeSettings.jsx";
import { Button } from "./ui.jsx";
import { Select } from "./Select.jsx";
import { Switch } from "./Switch.jsx";

export function AgentSettings({ config, onChange, data, navigate }) {
  const { t } = useLocale();
  return (
    <>
      <SettingGuide topic="connection">
        <ConfigFields
          descriptor={fieldsFor("AgentConfig", agentConnectionFields)}
          context="AgentConfig"
          value={config}
          onChange={onChange}
        />
      </SettingGuide>
      <SettingGuide topic="remote" current={t(remoteSummary(config.remote))}>
        <Switch
          label={t("guide.field.remoteEnabled.label")}
          checked={Boolean(config.remote?.enabled)}
          onChange={(enabled) =>
            onChange(
              patchSetting(
                config,
                "remote",
                { enabled },
                {
                  console_scope: true,
                  target_policy: { allow: [] },
                  file_root: {},
                },
              ),
            )
          }
        />
        <p className="setting-impact">{t("guide.remote.impact")}</p>
        <SettingDetails title={t("guide.remote.details")}>
          <ConfigFields
            descriptor="RemotePolicy"
            path="remote"
            value={config.remote ?? {}}
            exclude={["enabled"]}
            onChange={(remote) => onChange({ ...config, remote })}
          />
        </SettingDetails>
      </SettingGuide>
      <SettingGuide
        topic="visitor"
        current={t("guide.visitor.current", { count: config.visitor?.length ?? 0 })}
        action={<Button onClick={() => navigate("shares")}>{t("guide.visitor.wizard")}</Button>}
      >
        <ol className="setting-journey">
          {["app", "entry", "service"].map((step) => (
            <li key={step}>{t(`guide.visitor.${step}`)}</li>
          ))}
        </ol>
        <p className="setting-impact">{t("guide.visitor.example")}</p>
        <SettingDetails title={t("guide.visitor.details")}>
          <label>
            {t("guide.visitor.add")}
            <Select
              aria-label={t("guide.visitor.add")}
              value=""
              onValueChange={(id) => {
                const share = data.shares.find((row) => row.id === id);
                if (share)
                  onChange({
                    ...config,
                    visitor: [...(config.visitor ?? []), visitorTemplate(share)],
                  });
              }}
              options={[
                { value: "", label: t("cap.select"), disabled: true },
                ...data.shares.map((row) => ({ value: row.id, label: row.id })),
              ]}
            />
          </label>
          <ConfigFields
            descriptor={schema("AgentConfig").fields.visitor}
            path="visitor"
            value={config.visitor ?? []}
            onChange={(visitor) => onChange({ ...config, visitor })}
          />
        </SettingDetails>
      </SettingGuide>
      <SettingGuide
        topic="directAgent"
        current={t(config.p2p?.enabled ? "guide.directAgent.on" : "guide.directAgent.off")}
      >
        <Switch
          label={t("guide.field.directEnabled.label")}
          checked={Boolean(config.p2p?.enabled)}
          onChange={(enabled) =>
            onChange(patchSetting(config, "p2p", { enabled }, { max_sessions: 64 }))
          }
        />
        <p className="setting-impact">{t("guide.directAgent.impact")}</p>
        <SettingDetails>
          <ConfigFields
            descriptor="P2pAgentConfig"
            path="p2p"
            value={config.p2p ?? { enabled: false, max_sessions: 64 }}
            exclude={["enabled"]}
            onChange={(p2p) => onChange({ ...config, p2p })}
          />
        </SettingDetails>
      </SettingGuide>
      <TimeSettings config={config} onChange={onChange} />
      <SettingGuide topic="network">
        <SettingDetails title={t("guide.network.details")}>
          <ConfigFields
            descriptor={{
              type: "object",
              fields: Object.fromEntries(
                Object.entries(schema("AgentConfig").fields).filter(
                  ([key]) => !agentGuidedFields.includes(key),
                ),
              ),
            }}
            context="AgentConfig"
            value={config}
            onChange={onChange}
          />
        </SettingDetails>
      </SettingGuide>
    </>
  );
}
