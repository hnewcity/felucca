import { useId, useRef, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { useLocale } from "../app/locale.jsx";
import { schema, initialValue, setField } from "../features/config-model.js";
import { Button } from "./ui.jsx";
import { Select } from "./Select.jsx";
import { fieldGuidance, serverSettings, settingInitialValue } from "../features/settings-model.js";
import {
  collectionLabel,
  collectionPreview,
  fieldGroups,
  fieldActive,
  fieldHelp,
  fieldLabels,
  groupPreview,
  inputError,
  inputRequired,
  optionLabel,
  revealInvalidField,
} from "../features/config-presentation.js";
import { SourceScopeFields, TargetPolicyFields, TrustedProxyField } from "./ScopeField.jsx";
import { Switch } from "./Switch.jsx";
import { Disclosure } from "./Disclosure.jsx";

// Controls own only their field; grouping never changes the saved configuration.
export function ConfigFields({
  descriptor,
  value,
  onChange,
  path = "",
  disabled = false,
  exclude = [],
  context,
  caption,
  descriptionId,
  controlId,
}) {
  const { t } = useLocale();
  const generatedId = useId();
  const id = controlId ?? generatedId;
  const spec = schema(descriptor);
  const type = typeof descriptor === "string" ? descriptor : (descriptor.ref ?? context);
  const name = path.split(".").at(-1) || path;
  const translated = t(`field.${name}`);
  const label = caption ?? (translated === `field.${name}` ? name || t("form.entry") : translated);
  if (type === "TargetPolicy")
    return <TargetPolicyFields value={value} onChange={onChange} disabled={disabled} />;
  if (spec.type === "object") {
    const sourcePair =
      spec.fields.acl_mode &&
      spec.fields.acl_rules &&
      !exclude.includes("acl_mode") &&
      !exclude.includes("acl_rules");
    const groups = fieldGroups(type, spec, exclude)
      .map((group) => ({
        ...group,
        fields: group.fields.filter((key) => fieldActive(type, key, value)),
      }))
      .filter((group) => group.fields.length);
    const fields = (keys) =>
      keys.map((key) => {
        if (sourcePair && key === "acl_rules") return null;
        if (sourcePair && key === "acl_mode")
          return (
            <SourceScopeFields
              key={key}
              mode={value?.acl_mode}
              rules={value?.acl_rules}
              disabled={disabled}
              onChange={(next) =>
                onChange(
                  setField(setField(value, "acl_mode", next.acl_mode), "acl_rules", next.acl_rules),
                )
              }
            />
          );
        return (
          <ConfigField
            key={key}
            type={type}
            name={key}
            descriptor={spec.fields[key]}
            value={value?.[key]}
            path={path ? `${path}.${key}` : key}
            disabled={disabled}
            onChange={(next) => onChange(setField(value, key, next))}
          />
        );
      });
    return (
      <div className="config-fields" onInvalidCapture={revealInvalidField}>
        {groups.map((group) =>
          !group.name ? (
            <div className="config-group-fields" key="fields">
              {fields(group.fields)}
            </div>
          ) : ["access", "advanced", "security", "routing"].includes(group.name) ? (
            <Disclosure
              key={group.name}
              label={t(`form.group.${group.name}`)}
              description={groupPreview(type, group, value, t)}
            >
              <div className="config-group-fields">{fields(group.fields)}</div>
            </Disclosure>
          ) : (
            <fieldset className="config-group" key={group.name}>
              <legend>{t(`form.group.${group.name}`)}</legend>
              <div className="config-group-fields">{fields(group.fields)}</div>
            </fieldset>
          ),
        )}
      </div>
    );
  }
  if (spec.type === "array" || spec.type === "map")
    return <Collection {...{ spec, value, onChange, path, disabled, label, id }} />;
  if (spec.type === "trust")
    return (
      <div className="form-stack">
        <Select
          id={id}
          aria-label={label}
          aria-describedby={descriptionId}
          disabled={disabled}
          value={value?.mode ?? "webpki"}
          onValueChange={(mode) =>
            onChange(mode === "pinned" ? { mode, fingerprint: value?.fingerprint ?? "" } : { mode })
          }
          options={["webpki", "pinned", "insecure"].map((mode) => ({
            value: mode,
            label: t(`flow.trust.${mode}`),
          }))}
        />
        {value?.mode === "pinned" && (
          <label>
            {t("cap.fingerprint")}
            <input
              disabled={disabled}
              required
              value={value.fingerprint ?? ""}
              onChange={(event) => onChange({ ...value, fingerprint: event.target.value })}
            />
          </label>
        )}
      </div>
    );
  if (spec.type === "portRange")
    return (
      <div className="fields">
        {["from", "to"].map((key) => (
          <label key={key}>
            {t(`cap.${key}`)}
            <input
              type="number"
              inputMode="numeric"
              min="1"
              max="65535"
              required
              disabled={disabled}
              value={typeof value === "number" ? value : (value?.[key] ?? "")}
              onChange={(event) =>
                onChange({
                  from: typeof value === "number" ? value : value?.from,
                  to: typeof value === "number" ? value : value?.to,
                  [key]: event.target.value === "" ? "" : Number(event.target.value),
                })
              }
            />
          </label>
        ))}
      </div>
    );
  if (spec.type === "json")
    return (
      <ConfigFields
        descriptor={describeValue(value)}
        {...{ value, onChange, disabled, path, caption }}
      />
    );
  return (
    <ConfigInput {...{ id, spec, type, value, onChange, path, disabled, label, descriptionId }} />
  );
}

function ConfigField({ type, name, descriptor, value, onChange, path, disabled }) {
  const { t } = useLocale();
  const id = useId();
  const spec = schema(descriptor);
  const guidance = fieldGuidance[type]?.[name];
  const translated = t(`field.${name}`);
  const label = fieldLabels[type]?.[name]
    ? t(fieldLabels[type][name])
    : type === "ServerConfig" && serverSettings.includes(name)
      ? t(`guide.server.${name}.title`)
      : guidance
        ? t(`guide.field.${guidance}.label`)
        : translated === `field.${name}`
          ? name
          : translated;
  const help = fieldHelp(type, name, descriptor);
  const hint = guidance
    ? t(`guide.field.${guidance}.hint`)
    : help.hint
      ? t(`form.hint.${help.hint}`)
      : spec.type === "duration"
        ? t("form.durationHint")
        : null;
  const configured = value != null || !descriptor.optional;
  const collection = ["array", "map"].includes(spec.type);
  const complex = ["object", "array", "map", "portRange"].includes(spec.type);
  const showReset =
    descriptor.optional &&
    configured &&
    !collection &&
    !(type === "TunnelConfig" && ["remark", "enabled", "public"].includes(name));
  // Empty collections are edited directly; adding the first row makes them explicit.
  const editable = configured || collection;
  const descriptionId = `${id}-hint`;
  const content =
    type === "ProxyAccept" && name === "trusted" ? (
      <TrustedProxyField value={value} onChange={onChange} disabled={disabled} />
    ) : (
      <ConfigFields
        descriptor={descriptor}
        value={value}
        path={path}
        caption={label}
        controlId={id}
        descriptionId={descriptionId}
        disabled={disabled}
        onChange={onChange}
      />
    );
  const heading = (
    <div className="config-field-heading">
      {complex || spec.type === "boolean" ? (
        <span id={`${id}-label`}>{label}</span>
      ) : (
        <label htmlFor={id}>
          {label}
          {!descriptor.optional && inputRequired(spec, name) && (
            <span className="config-required">{t("form.requiredLabel")}</span>
          )}
        </label>
      )}
      {showReset && (
        <Button
          className="config-reset"
          disabled={disabled}
          aria-label={t("form.resetField", { label })}
          onClick={() => onChange(undefined)}
        >
          {t("cap.unset")}
        </Button>
      )}
    </div>
  );
  const description = (
    <div id={descriptionId} className="config-description">
      {hint && <p className="field-hint">{hint}</p>}
      {help.example && <p className="field-hint">{t("form.example", { value: help.example })}</p>}
    </div>
  );
  return (
    <div className={`config-field ${spec.type === "boolean" ? "config-boolean" : ""}`}>
      {spec.type === "boolean" && editable ? (
        <>
          <Switch
            id={id}
            name={path}
            label={label}
            hint={hint}
            checked={Boolean(value)}
            disabled={disabled}
            onChange={onChange}
          />
          {showReset && (
            <Button
              className="config-reset"
              disabled={disabled}
              aria-label={t("form.resetField", { label })}
              onClick={() => onChange(undefined)}
            >
              {t("cap.unset")}
            </Button>
          )}
        </>
      ) : (
        <>
          {heading}
          {description}
          {!editable ? (
            <div className="config-default-row">
              <span>{t("form.defaultHint")}</span>
              <Button
                className="config-reset"
                disabled={disabled}
                aria-label={t("form.configureField", { label })}
                onClick={() => onChange(settingInitialValue(type, name, descriptor))}
              >
                {t("cap.set")}
              </Button>
            </div>
          ) : spec.type === "object" &&
            !["ManagedConfig", "TargetPolicy"].includes(type) &&
            !guidance ? (
            <div className="config-nested">{content}</div>
          ) : (
            content
          )}
        </>
      )}
    </div>
  );
}

function ConfigInput({ id, spec, type, value, onChange, path, disabled, label, descriptionId }) {
  const { t } = useLocale();
  const [error, setError] = useState("");
  const props = {
    id,
    name: path,
    disabled,
    "aria-label": label,
    "aria-describedby":
      [descriptionId, error ? `${id}-error` : null].filter(Boolean).join(" ") || undefined,
    "aria-invalid": error ? true : undefined,
    onInvalid: (event) => setError(inputError(event.currentTarget, t)),
    onBlur: (event) => setError(inputError(event.currentTarget, t)),
  };
  const help = fieldHelp(type, path.split(".").at(-1), spec);
  return (
    <div
      className={`config-input ${spec.type === "number" || spec.type === "duration" ? "config-input-short" : ""}`}
    >
      {spec.type === "boolean" ? (
        <Switch {...props} label={label} checked={Boolean(value)} onChange={onChange} />
      ) : spec.type === "enum" ? (
        <Select
          {...props}
          required
          value={value ?? ""}
          onValueChange={onChange}
          options={[
            { value: "", label: t("cap.select"), disabled: true },
            ...spec.options.map((option) => ({
              value: option,
              label: optionLabel(type, option, t),
            })),
          ]}
        />
      ) : help.multiline ? (
        <textarea
          {...props}
          rows={4}
          value={value ?? ""}
          spellCheck={false}
          onChange={(event) => onChange(event.target.value)}
        />
      ) : (
        <input
          {...props}
          type={spec.type === "number" ? "number" : spec.type === "secret" ? "password" : "text"}
          autoComplete={spec.type === "secret" ? "new-password" : undefined}
          inputMode={spec.type === "number" ? "numeric" : undefined}
          min={spec.min}
          max={spec.max}
          step={spec.type === "number" ? 1 : undefined}
          required={inputRequired(spec, path.split(".").at(-1))}
          value={value ?? ""}
          onChange={(event) => {
            const raw = event.target.value;
            // Keep incomplete input visible and invalid; never silently save the previous number.
            onChange(
              spec.type === "number" && raw !== "" && Number.isSafeInteger(Number(raw))
                ? Number(raw)
                : raw,
            );
            if (error) setError(inputError(event.currentTarget, t));
          }}
        />
      )}
      {error && (
        <p id={`${id}-error`} className="form-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

function Collection({ spec, value, onChange, path, disabled, label, id }) {
  const { t } = useLocale();
  const [mapKey, setMapKey] = useState("");
  const [newKey, setNewKey] = useState(null);
  const entries =
    spec.type === "array"
      ? (value ?? []).map((entry, index) => [index, entry])
      : Object.entries(value ?? {});
  const nextRow = useRef(entries.length);
  const [rowKeys, setRowKeys] = useState(() => entries.map((_, index) => `${id}-${index}`));
  const visibleKeys = entries.map((_, index) => rowKeys[index] ?? `${id}-external-${index}`);
  const scalar = !["object", "array", "map", "trust", "portRange", "json"].includes(
    schema(spec.item).type,
  );
  const itemLabel = collectionLabel(spec.item, label, t);
  const remove = (key) => {
    if (spec.type === "array") setRowKeys(visibleKeys.filter((_, index) => index !== key));
    onChange(
      spec.type === "array"
        ? value.filter((_, index) => index !== key)
        : setField(value, key, undefined),
    );
  };
  return (
    <div className="config-collection">
      {!entries.length && <p className="config-empty">{t("form.emptyCollection")}</p>}
      {entries.map(([key, entry]) => {
        const title = scalar
          ? spec.type === "map"
            ? String(key)
            : itemLabel
          : entry?.remark ||
            entry?.name ||
            entry?.addr ||
            (spec.type === "map"
              ? String(key)
              : t("form.numberedItem", { label: itemLabel, count: key + 1 }));
        const content = (
          <ConfigFields
            descriptor={spec.item}
            value={entry}
            path={`${path}.${key}`}
            caption={scalar ? title : undefined}
            disabled={disabled}
            onChange={(next) =>
              onChange(
                spec.type === "array"
                  ? value.map((old, index) => (index === key ? next : old))
                  : setField(value, key, next),
              )
            }
          />
        );
        return scalar ? (
          <div className="config-list-row" key={spec.type === "array" ? visibleKeys[key] : key}>
            <label>
              <span className={spec.type === "array" ? "sr-only" : ""}>{title}</span>
              {content}
            </label>
            <Button
              icon={Trash2}
              className="icon-button"
              disabled={disabled}
              aria-label={t("form.removeItem", { label: title })}
              onClick={() => remove(key)}
            />
          </div>
        ) : (
          <Disclosure
            className="config-entry"
            key={spec.type === "array" ? visibleKeys[key] : key}
            label={title}
            {...collectionPreview(spec.item, entry, t)}
            initiallyOpen={entries.length === 1 || key === newKey}
          >
            {content}
            <div className="row-actions config-entry-actions">
              <Button className="config-reset" disabled={disabled} onClick={() => remove(key)}>
                {t("action.delete")}
              </Button>
              {spec.type === "array" && key > 0 && (
                <Button
                  className="config-reset"
                  disabled={disabled}
                  onClick={() => {
                    const next = [...value];
                    [next[key - 1], next[key]] = [next[key], next[key - 1]];
                    const moved = [...visibleKeys];
                    [moved[key - 1], moved[key]] = [moved[key], moved[key - 1]];
                    setRowKeys(moved);
                    onChange(next);
                  }}
                >
                  {t("cap.moveUp")}
                </Button>
              )}
            </div>
          </Disclosure>
        );
      })}
      <div className="config-add-row">
        {spec.type === "map" && (
          <label htmlFor={id}>
            {t("form.entryName")}
            <input
              id={id}
              disabled={disabled}
              value={mapKey}
              onChange={(event) => setMapKey(event.target.value)}
            />
          </label>
        )}
        <Button
          icon={Plus}
          disabled={
            disabled ||
            (spec.type === "map" && (!mapKey.trim() || Object.hasOwn(value ?? {}, mapKey.trim())))
          }
          onClick={() => {
            setNewKey(spec.type === "array" ? entries.length : mapKey.trim());
            if (spec.type === "array") setRowKeys([...visibleKeys, `${id}-${nextRow.current++}`]);
            onChange(
              spec.type === "array"
                ? [...(value ?? []), initialValue(spec.item)]
                : setField(value, mapKey.trim(), initialValue(spec.item)),
            );
            setMapKey("");
          }}
        >
          {t("form.addItem", { label: itemLabel })}
        </Button>
      </div>
    </div>
  );
}

function describeValue(value) {
  if (Array.isArray(value)) return { type: "array", item: describeValue(value[0] ?? "") };
  if (value && typeof value === "object")
    return {
      type: "object",
      fields: Object.fromEntries(
        Object.entries(value).map(([key, child]) => [key, describeValue(child)]),
      ),
    };
  return { type: ["number", "boolean"].includes(typeof value) ? typeof value : "string" };
}
