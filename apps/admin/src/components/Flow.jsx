import { Disclosure } from "./Disclosure.jsx";
import { Check, Copy, Download } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { useLocale } from "../app/locale.jsx";
import { Button } from "./ui.jsx";
import { launchCommand, newIdentity } from "../features/product-flow.js";
import {
  connectionPackage,
  packageScripts,
  packageTargets,
} from "../features/connection-package.js";
import { useAdmin } from "../app/context.jsx";
import { Select } from "./Select.jsx";

export function Steps({ current, labels }) {
  return (
    <ol className="flow-steps" aria-label={labels.join(" · ")}>
      {labels.map((label, index) => (
        <li
          key={label}
          aria-current={index === current ? "step" : undefined}
          className={index <= current ? "reached" : ""}
        >
          <span>{index < current ? <Check size={15} /> : index + 1}</span>
          {label}
        </li>
      ))}
    </ol>
  );
}
export function Choice({ value, onChange, options, label, disabled = false, compact = false }) {
  const name = useId();
  return (
    <fieldset className={`flow-choices${compact ? " choice-list" : ""}`} disabled={disabled}>
      <legend>{label}</legend>
      {options.map(([id, title, hint]) => (
        <label key={id} className={value === id ? "chosen" : ""}>
          <input
            type="radio"
            name={name}
            value={id}
            checked={value === id}
            onChange={() => onChange(id)}
          />
          <span>
            <strong>{title}</strong>
            {hint && <small>{hint}</small>}
          </span>
        </label>
      ))}
    </fieldset>
  );
}
export function CopyValue({ value, label }) {
  const { t } = useLocale();
  const [copied, setCopied] = useState(false);
  const [failed, setFailed] = useState(false);
  return (
    <div className="copy-value">
      <input aria-label={label} value={value} readOnly onFocus={(event) => event.target.select()} />
      <Button
        icon={copied ? Check : Copy}
        onClick={async () => {
          try {
            await navigator.clipboard.writeText(value);
            setCopied(true);
            setFailed(false);
          } catch {
            setFailed(true);
          }
        }}
      >
        {t(copied ? "flow.copied" : "flow.copy")}
      </Button>
      {failed && <small role="status">{t("flow.copyManual")}</small>}
    </div>
  );
}
export function PackageTarget({ packages, value, onChange }) {
  const { t } = useLocale();
  const selected = packages.find((item) => item.target === value);
  return (
    <label>
      {t("flow.deviceSystem")}
      <Select
        aria-label={t("flow.deviceSystem")}
        required
        value={value}
        onValueChange={onChange}
        options={[
          {
            value: "",
            label: t(packages.length ? "flow.choosePackageTarget" : "flow.packageUnavailable"),
          },
          ...packages.map((item) => ({
            value: item.target,
            label: t(packageTargets[item.target]),
          })),
        ]}
      />
      <span className="field-hint">{t("flow.packageTargetHint")}</span>
      {selected?.target.includes("apple") && !selected.notarized && (
        <span className="form-error">{t("flow.packageMacUnverified")}</span>
      )}
    </label>
  );
}
export function ConnectionPackage({
  config,
  target,
  name,
  notarized,
  onDownloaded,
  offerCommand = false,
}) {
  const { t, locale } = useLocale();
  const { api } = useAdmin();
  const [id] = useState(() => newIdentity("felucca"));
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [href, setHref] = useState("");
  const scripts = packageScripts(target);
  const command = offerCommand ? launchCommand(config, target) : null;
  useEffect(() => {
    const abort = new AbortController();
    let active = true;
    let url = "";
    setBusy(true);
    setError("");
    setHref("");
    (async () => {
      try {
        const response = await api(`/api/agent-packages/${target}`, "GET", undefined, {
          raw: true,
          signal: abort.signal,
        });
        const binary = new Uint8Array(await response.arrayBuffer());
        if (!active) return;
        url = URL.createObjectURL(
          connectionPackage({ config, binary, target, id, name, locale, t }),
        );
        setHref(url);
      } catch (error) {
        if (active && error.name !== "AbortError") setError(error.message);
      } finally {
        if (active) setBusy(false);
      }
    })();
    return () => {
      active = false;
      abort.abort();
      if (url) URL.revokeObjectURL(url);
    };
  }, [api, attempt, config, id, locale, name, t, target]);
  return (
    <section className="surface flow-panel">
      <h2>{t("flow.startDevice")}</h2>
      {name && <strong>{name}</strong>}
      {target.includes("apple") && !notarized && (
        <p className="form-error">{t("flow.packageMacUnverified")}</p>
      )}
      <p>
        {t(
          target.includes("linux") ? "flow.packageLinuxInstructions" : "flow.packageInstructions",
          { connect: target.includes("linux") ? `sh ${scripts.connect}` : scripts.connect },
        )}
      </p>
      {href ? (
        <a
          className="button button-primary"
          href={href}
          download={`${id}.zip`}
          onClick={() => onDownloaded?.()}
        >
          <Download size={17} />
          {t("flow.downloadPackage")}
        </a>
      ) : (
        <Button
          icon={Download}
          variant="primary"
          busy={busy}
          onClick={() => setAttempt((value) => value + 1)}
        >
          {t("flow.downloadPackage")}
        </Button>
      )}
      {error && (
        <p role="alert" className="form-error">
          {error}
        </p>
      )}
      {target.includes("linux") && (
        <CopyValue value={`sh ${scripts.connect}`} label={t("flow.startCommand")} />
      )}
      {command && (
        <Disclosure className="advanced" label={t("flow.launchCommand")}>
          <p>{t("flow.launchCommandHint")}</p>
          <CopyValue value={command} label={t("flow.launchCommand")} />
        </Disclosure>
      )}
      <Disclosure className="advanced" label={t("flow.packageAutoStart")}>
        <p>{t("flow.packageBackground", { install: scripts.install })}</p>
      </Disclosure>
      <p className="field-hint">{t("flow.packageSecret")}</p>
    </section>
  );
}
