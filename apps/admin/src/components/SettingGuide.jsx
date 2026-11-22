import { useLocale } from "../app/locale.jsx";
import { Badge } from "./ui.jsx";
import { Disclosure } from "./Disclosure.jsx";

export function SettingGuide({ topic, current, children, action }) {
  const { t } = useLocale();
  return (
    <section className="surface setting-guide">
      <div className="setting-guide-heading">
        <div>
          <h2>{t(`guide.${topic}.title`)}</h2>
          <p>{t(`guide.${topic}.description`)}</p>
        </div>
        {action}
      </div>
      <p className="setting-use-case">
        <strong>{t("guide.when")}</strong>
        {t(`guide.${topic}.when`)}
      </p>
      {current && (
        <div className="setting-current">
          <Badge dot={false}>{t("guide.current")}</Badge>
          <span>{current}</span>
        </div>
      )}
      {children}
    </section>
  );
}

export function SettingDetails({ children, title }) {
  const { t } = useLocale();
  return (
    <Disclosure className="setting-details" label={title ?? t("guide.advanced")}>
      {children}
    </Disclosure>
  );
}
