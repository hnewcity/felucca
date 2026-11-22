import { useId, useState } from "react";
import { useLocale } from "../app/locale.jsx";
import { Choice } from "./Flow.jsx";
import { Select } from "./Select.jsx";
import { Disclosure } from "./Disclosure.jsx";
import {
  parseRules,
  presetOf,
  scopePresets,
  showRules,
  sourceAcl,
  sourcePreset,
  targetPolicy,
  targetRule,
} from "../features/address-scope.js";

// Preset choices plus a one-rule-per-line list for "custom".
// `kind` selects the hints: target, source or trusted.
export function ScopeField({
  kind,
  label,
  presets,
  choice,
  text,
  onChange,
  disabled,
  children,
  required = true,
}) {
  const { t } = useLocale();
  const id = useId();
  const error = choice === "custom" && required && !parseRules(text).length ? t("scope.empty") : "";
  return (
    <div className="scope-field">
      <Choice
        compact
        label={label}
        value={choice}
        disabled={disabled}
        onChange={(next) =>
          onChange(
            next,
            // Starting a custom list from a preset keeps what was already allowed.
            next === "custom" && !text.trim() ? showRules(scopePresets[choice] ?? []) : text,
          )
        }
        options={presets.map((name) => [
          name,
          t(kind === "source" ? `scope.source.${name}Label` : `scope.${name}`),
          t(`scope.${kind}.${name}Hint`),
        ])}
      />
      {choice === "custom" && (
        <div className="scope-rules">
          {children}
          <label htmlFor={id}>
            {t("scope.rules")}
            <textarea
              id={id}
              rows={4}
              spellCheck="false"
              required={required}
              ref={(element) => element?.setCustomValidity(error)}
              aria-invalid={error ? true : undefined}
              aria-describedby={`${id}-hint${error ? ` ${id}-error` : ""}`}
              disabled={disabled}
              value={text}
              placeholder={kind === "target" ? "192.168.1.0/24\nnas.local" : "192.168.1.0/24"}
              onChange={(event) => onChange("custom", event.target.value)}
            />
            <span className="field-hint" id={`${id}-hint`}>
              {t(kind === "target" ? "scope.rulesHint" : "scope.sourceRulesHint")}
            </span>
            {error && (
              <span id={`${id}-error`} className="form-error" role="alert">
                {error}
              </span>
            )}
          </label>
        </div>
      )}
    </div>
  );
}

// Keeps the typed text while the choice is "custom", so formatting survives edits.
function useRuleText(rules) {
  const [text, setText] = useState(() => showRules(rules));
  return [text, setText];
}

const TARGET = ["any", "device", "lan", "custom"];

// TargetPolicy: allowed destinations, then deny rules and ports as plain text.
export function TargetPolicyFields({ value, onChange, disabled, label }) {
  const { t } = useLocale();
  const policy = value ?? {};
  const [choice, setChoice] = useState(() => presetOf(policy.allow, TARGET));
  const [text, setText] = useRuleText(policy.allow);
  const [deny, setDeny] = useRuleText(policy.deny);
  const [ports, setPorts] = useState(() => (policy.ports ?? []).join(", "));
  const portsId = useId();
  const portList = parseRules(ports).map(Number);
  const portsError = portList.every((port) => Number.isInteger(port) && port > 0 && port <= 65535)
    ? ""
    : t("form.portsInvalid");
  return (
    <div className="form-stack">
      <ScopeField
        kind="target"
        label={label ?? t("scope.label.target")}
        presets={TARGET}
        choice={choice}
        text={text}
        disabled={disabled}
        onChange={(next, nextText) => {
          setChoice(next);
          setText(nextText);
          // An empty custom list is incomplete, not "any"; keep the last valid value.
          if (next !== "custom" || parseRules(nextText).length)
            onChange(targetPolicy(next, nextText, policy));
        }}
      />
      <Disclosure
        label={t("form.destinationExceptions")}
        description={t("form.exceptionsPreview", {
          rules: policy.deny?.length ?? 0,
          ports: policy.ports?.length ?? 0,
        })}
        initiallyOpen={Boolean(policy.deny?.length || policy.ports?.length)}
      >
        <label>
          {t("scope.deny")}
          <textarea
            rows={2}
            spellCheck="false"
            disabled={disabled}
            value={deny}
            onChange={(event) => {
              setDeny(event.target.value);
              onChange({ ...policy, deny: parseRules(event.target.value).map(targetRule) });
            }}
          />
          <span className="field-hint">{t("scope.denyHint")}</span>
        </label>
        <label>
          {t("scope.ports")}
          <input
            inputMode="numeric"
            disabled={disabled}
            value={ports}
            placeholder="80, 443"
            id={portsId}
            ref={(element) => element?.setCustomValidity(portsError)}
            aria-invalid={portsError ? true : undefined}
            aria-describedby={`${portsId}-hint${portsError ? ` ${portsId}-error` : ""}`}
            onChange={(event) => {
              setPorts(event.target.value);
              const list = parseRules(event.target.value).map(Number);
              if (list.every((port) => Number.isInteger(port) && port > 0 && port <= 65535))
                onChange({ ...policy, ports: list });
            }}
          />
          <span className="field-hint" id={`${portsId}-hint`}>
            {t("scope.portsHint")}
          </span>
          {portsError && (
            <span className="form-error" id={`${portsId}-error`} role="alert">
              {portsError}
            </span>
          )}
        </label>
      </Disclosure>
    </div>
  );
}

const SOURCE = ["any", "lan", "private", "device", "custom"];

// acl_mode + acl_rules edited together. Custom keeps the allow/deny choice.
export function SourceScopeFields({ mode, rules, onChange, disabled }) {
  const { t } = useLocale();
  const [choice, setChoice] = useState(() => sourcePreset(mode, rules));
  const [text, setText] = useState(() => parseRules(rules).join("\n"));
  const [custom, setCustom] = useState(() =>
    mode === "deny" ? "deny" : mode === "off" && parseRules(rules).length ? "off" : "allow",
  );
  const emit = (next, nextText, nextMode = custom) => {
    setChoice(next);
    setText(nextText);
    setCustom(nextMode);
    // Allow-only with no rules is rejected by the server; wait for a rule.
    if (next === "custom" && nextMode === "allow" && !parseRules(nextText).length) return;
    onChange(
      next === "custom"
        ? { acl_mode: nextMode, acl_rules: parseRules(nextText).join(", ") }
        : sourceAcl(next, nextText),
    );
  };
  return (
    <ScopeField
      kind="source"
      label={t("scope.label.source")}
      presets={SOURCE}
      choice={choice}
      text={text}
      disabled={disabled}
      required={custom === "allow"}
      onChange={(next, nextText) => emit(next, nextText)}
    >
      <label>
        {t("scope.mode")}
        <Select
          aria-label={t("scope.mode")}
          disabled={disabled}
          value={custom}
          onValueChange={(nextMode) => emit("custom", text, nextMode)}
          options={["allow", "deny", "off"].map((value) => ({
            value,
            label: t(value === "off" ? "option.off" : `scope.mode.${value}`),
          }))}
        />
      </label>
    </ScopeField>
  );
}

const TRUSTED = ["device", "private", "custom"];

export function TrustedProxyField({ value, onChange, disabled }) {
  const { t } = useLocale();
  const [choice, setChoice] = useState(() => presetOf(value, TRUSTED));
  const [text, setText] = useRuleText(value);
  return (
    <ScopeField
      kind="trusted"
      label={t("scope.label.trusted")}
      presets={TRUSTED}
      choice={choice}
      text={text}
      disabled={disabled}
      onChange={(next, nextText) => {
        setChoice(next);
        setText(nextText);
        if (next !== "custom" || parseRules(nextText).length)
          onChange(next === "custom" ? parseRules(nextText) : [...scopePresets[next]]);
      }}
    />
  );
}
