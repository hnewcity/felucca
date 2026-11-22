import { useLocale } from "../app/locale.jsx";
import { Choice } from "./Flow.jsx";
import { SettingDetails, SettingGuide } from "./SettingGuide.jsx";
import { setTimeSource } from "../features/settings-model.js";
import { ConfigFields } from "./ConfigFields.jsx";
import { schema } from "../features/config-model.js";

export function TimeSettings({ config, onChange, disabled = false }) {
  const { t } = useLocale();
  const custom = config.ntp != null;
  return (
    <SettingGuide
      topic="time"
      current={t(custom ? "guide.time.customState" : "guide.time.systemState")}
    >
      <fieldset className="setting-controls" disabled={disabled}>
        <Choice
          label={t("guide.time.source")}
          value={custom ? "custom" : "system"}
          onChange={(next) => onChange(setTimeSource(config, next === "custom"))}
          options={[
            ["system", t("guide.time.system"), t("guide.time.systemHint")],
            ["custom", t("guide.time.custom"), t("guide.time.customHint")],
          ]}
        />
        {custom && (
          <div className="form-stack">
            <label>
              {t("guide.field.timeServer.label")}
              <input
                name="ntp.server"
                value={config.ntp.server ?? ""}
                required
                placeholder="time.example.com:123"
                onChange={(event) =>
                  onChange({ ...config, ntp: { ...config.ntp, server: event.target.value } })
                }
              />
              <span className="field-hint">{t("guide.field.timeServer.hint")}</span>
            </label>
            <SettingDetails title={t("guide.time.tuning")}>
              <ConfigFields
                descriptor={{ type: "object", fields: schema("NtpConfig").fields }}
                context="NtpConfig"
                path="ntp"
                value={config.ntp}
                exclude={["server"]}
                disabled={disabled}
                onChange={(ntp) => onChange({ ...config, ntp })}
              />
            </SettingDetails>
          </div>
        )}
      </fieldset>
      <p className="setting-impact">{t("guide.time.impact")}</p>
    </SettingGuide>
  );
}
