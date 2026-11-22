import { useEffect, useRef, useState } from "react";
import { ConfigFields } from "./ConfigFields.jsx";
import { X } from "lucide-react";
import { useLocale } from "../app/locale.jsx";
import { Button } from "./ui.jsx";
import { NameInput } from "./NameInput.jsx";
import { Select } from "./Select.jsx";
import { TargetPolicyFields } from "./ScopeField.jsx";
import { revealInvalidField } from "../features/config-presentation.js";
import { Switch } from "./Switch.jsx";
import { Disclosure } from "./Disclosure.jsx";

export function Modal({ id, title, children, onClose, busy = false, wide = false }) {
  const ref = useRef(null);
  const { t } = useLocale();
  useEffect(() => {
    const dialog = ref.current;
    dialog.showModal();
    return () => dialog.close();
  }, []);
  return (
    <dialog
      ref={ref}
      id={id}
      aria-labelledby={`${id}-title`}
      onInvalidCapture={revealInvalidField}
      className={wide ? "modal-wide" : ""}
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onClose();
      }}
    >
      <div className="dialog-head">
        <h2 id={`${id}-title`}>{title}</h2>
        <Button
          aria-label={t("action.close")}
          className="icon-button"
          icon={X}
          onClick={onClose}
          disabled={busy}
        />
      </div>
      <div className="dialog-body">{children}</div>
    </dialog>
  );
}

export function EditorDialog({ editor, onClose, onSaved }) {
  const { t } = useLocale();
  const [values, setValues] = useState(() =>
    Object.fromEntries(
      editor.fields.map((field) => [
        field.key,
        // A scope field is off when null; "" would read as an enabled, empty scope.
        field.value ?? (field.type === "checkbox" ? false : field.type === "scope" ? null : ""),
      ]),
    ),
  );
  const [extra, setExtra] = useState(() => structuredClone(editor.extra ?? {}));
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const saving = useRef(false);
  async function submit(event) {
    event.preventDefault();
    if (saving.current || editor.readOnly) return;
    saving.current = true;
    setBusy(true);
    setError("");
    try {
      await editor.save(values, editor.extra === undefined ? undefined : extra);
      onSaved();
    } catch (error) {
      if (error.name !== "AbortError") setError(error.message);
    } finally {
      saving.current = false;
      setBusy(false);
    }
  }
  return (
    <Modal id="editor" title={editor.title} onClose={onClose} busy={busy} wide>
      <form id="editor-form" className="form-stack" onSubmit={submit}>
        <div id="fields" className="fields">
          {editor.fields
            .filter((field) => !field.hidden)
            .map((field) => {
              const disabled =
                busy ||
                (editor.readOnly && (field.type === "checkbox" || Boolean(field.options))) ||
                (typeof field.disabled === "function" ? field.disabled(values) : field.disabled);
              const required =
                !editor.readOnly &&
                (typeof field.required === "function" ? field.required(values) : field.required);
              const props = {
                name: field.key,
                disabled,
                required,
                min: field.min,
                max: field.max,
                step: field.step,
                autoComplete: field.autoComplete,
                readOnly: editor.readOnly,
              };
              if (field.type === "scope")
                return (
                  <Disclosure
                    className="advanced field-wide"
                    label={t("flow.devicePermissions")}
                    key={field.key}
                  >
                    <Switch
                      label={field.label}
                      hint={field.hint}
                      disabled={disabled || editor.readOnly}
                      checked={values[field.key] != null}
                      onChange={(checked) =>
                        setValues({
                          ...values,
                          [field.key]: checked ? (field.initial ?? { allow: [] }) : null,
                        })
                      }
                    />
                    {values[field.key] != null && (
                      <TargetPolicyFields
                        label={field.scopeLabel}
                        value={values[field.key]}
                        disabled={disabled}
                        onChange={(next) => setValues({ ...values, [field.key]: next })}
                      />
                    )}
                  </Disclosure>
                );
              if (field.type === "schema")
                return (
                  <div className="field-wide" key={field.key}>
                    {editor.fields.filter((item) => !item.hidden).length > 1 && (
                      <h3>{field.label}</h3>
                    )}
                    <ConfigFields
                      descriptor={field.descriptor}
                      value={values[field.key]}
                      path={field.key === "config" ? "" : field.key}
                      disabled={disabled || editor.readOnly}
                      onChange={(next) => setValues({ ...values, [field.key]: next })}
                    />
                  </div>
                );
              if (field.nameGeneration)
                return (
                  <NameInput
                    {...props}
                    {...field.nameGeneration}
                    key={field.key}
                    label={field.label}
                    hint={field.hint}
                    className={field.wide ? "field-wide" : ""}
                    value={values[field.key]}
                    onChange={(value) => setValues({ ...values, [field.key]: value })}
                  />
                );
              if (field.type === "checkbox")
                return (
                  <Switch
                    key={field.key}
                    name={field.key}
                    label={field.label}
                    hint={field.hint}
                    className="field-wide"
                    disabled={disabled}
                    checked={values[field.key]}
                    onChange={(checked) => setValues({ ...values, [field.key]: checked })}
                  />
                );
              return (
                <label
                  key={field.key}
                  className={field.wide || field.type === "textarea" ? "field-wide" : ""}
                >
                  {field.label}
                  {field.options ? (
                    <Select
                      {...props}
                      aria-label={field.label}
                      value={values[field.key]}
                      onValueChange={(value) => setValues({ ...values, [field.key]: value })}
                      options={field.options.map(([value, label]) => ({ value, label }))}
                    />
                  ) : field.type === "textarea" ? (
                    <textarea
                      {...props}
                      spellCheck="false"
                      className="code-input"
                      value={values[field.key]}
                      onChange={(e) => setValues({ ...values, [field.key]: e.target.value })}
                    />
                  ) : (
                    <input
                      {...props}
                      type={field.type || "text"}
                      value={values[field.key]}
                      onChange={(e) =>
                        setValues({
                          ...values,
                          [field.key]: e.target.value,
                        })
                      }
                    />
                  )}
                  {field.hint && <span className="field-hint">{field.hint}</span>}
                </label>
              );
            })}
          {editor.extraDescriptor && (
            <Disclosure
              className="advanced field-wide"
              label={t("dialog.editor.advanced")}
              description={t("dialog.editor.advancedHint")}
            >
              <ConfigFields
                descriptor={editor.extraDescriptor}
                value={extra}
                disabled={busy || editor.readOnly}
                onChange={setExtra}
              />
            </Disclosure>
          )}
        </div>
        {!editor.readOnly && <p className="form-note">{t("dialog.editor.saveNote")}</p>}
        <div id="editor-error" className="form-error" role="alert">
          {error}
        </div>
        <div className="dialog-actions">
          {!editor.readOnly && <span className="dialog-action-note">{t("form.saveDraft")}</span>}
          <Button onClick={onClose} disabled={busy}>
            {editor.readOnly ? t("action.close") : t("action.cancel")}
          </Button>
          {!editor.readOnly && (
            <Button type="submit" variant="primary" busy={busy}>
              {t("action.save")}
            </Button>
          )}
        </div>
      </form>
    </Modal>
  );
}
export function ConfirmationDialog({ confirmation, onClose }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const running = useRef(false);
  const { t } = useLocale();
  async function confirm(event) {
    event.preventDefault();
    if (running.current) return;
    running.current = true;
    setBusy(true);
    try {
      await confirmation.action();
      onClose();
    } catch (error) {
      if (error.name !== "AbortError") setError(error.message);
    } finally {
      running.current = false;
      setBusy(false);
    }
  }
  return (
    <Modal id="confirmation" title={confirmation.title} onClose={onClose} busy={busy}>
      <form onSubmit={confirm}>
        <p className="confirm-message">{confirmation.message}</p>
        <p className="form-error" role="alert">
          {error}
        </p>
        <div className="dialog-actions">
          <Button disabled={busy} onClick={onClose}>
            {t("action.cancel")}
          </Button>
          <Button
            type="submit"
            variant={confirmation.danger === false ? "primary" : "danger"}
            busy={busy}
            value="confirm"
          >
            {t("action.confirm")}
          </Button>
        </div>
      </form>
    </Modal>
  );
}
