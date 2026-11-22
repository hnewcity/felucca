import { useState } from "react";
import { useLocale } from "../app/locale.jsx";
import { Choice, ConnectionPackage, CopyValue, PackageTarget } from "./Flow.jsx";
import { Disclosure } from "./Disclosure.jsx";
import { preferredTarget, packageTargets } from "../features/connection-package.js";
import { connectionCommand } from "../features/connection-command.js";
import { launchLink } from "../features/product-flow.js";

export function OneLineConnection({ config, name, packages, onDownloaded }) {
  const { t, locale } = useLocale();
  const [system, setSystem] = useState(/win/i.test(navigator.platform) ? "windows" : "unix");
  const [target, setTarget] = useState(() => preferredTarget(packages));
  const [showPackage, setShowPackage] = useState(false);
  const command = connectionCommand(config, window.location.origin, system, locale);
  const desktop = launchLink(config);
  return (
    <section className="surface flow-panel">
      <h2>{t("connect.commandTitle")}</h2>
      <p>{t("connect.commandHint")}</p>
      <Choice
        label={t("flow.deviceSystem")}
        value={system}
        onChange={setSystem}
        options={[
          ["unix", t("connect.unix"), t("connect.terminal")],
          ["windows", t("connect.windows"), t("connect.powershell")],
        ]}
      />
      <CopyValue value={command} label={t("connect.commandTitle")} />
      {desktop && (
        <p className="field-hint">
          {t("connect.desktopHint")}{" "}
          <a className="button button-outline" href={desktop}>
            {t("connect.desktopOpen")}
          </a>
        </p>
      )}
      <p className="field-hint">{t("connect.keepRunning")}</p>
      <p className="field-hint">{t("connect.shareCommand")}</p>
      <p className="field-hint">
        {packages.length
          ? t("connect.available", {
              systems: packages.map((item) => t(packageTargets[item.target])).join(" · "),
            })
          : t("flow.packageUnavailable")}
      </p>
      <Disclosure
        label={t("connect.packageAlternative")}
        onOpenChange={(open) => {
          if (open) setShowPackage(true);
        }}
      >
        {showPackage && (
          <>
            <PackageTarget packages={packages} value={target} onChange={setTarget} />
            {target && (
              <ConnectionPackage
                config={config}
                name={name}
                target={target}
                notarized={packages.find((item) => item.target === target)?.notarized}
                onDownloaded={onDownloaded}
              />
            )}
          </>
        )}
      </Disclosure>
    </section>
  );
}
