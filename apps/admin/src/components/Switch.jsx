import { useId } from "react";
import { useLocale } from "../app/locale.jsx";

// Keep a native input for keyboard, focus and form semantics.
export function Switch({
  label,
  hint,
  checked,
  onChange,
  id: suppliedId,
  className = "",
  ...props
}) {
  const generatedId = useId();
  const id = suppliedId ?? generatedId;
  const { t } = useLocale();
  return (
    <label className={`switch-field ${className}`} htmlFor={id}>
      <span className="switch-copy">
        <span id={`${id}-label`} className="switch-label">
          {label}
        </span>
        {hint && (
          <span id={`${id}-hint`} className="field-hint">
            {hint}
          </span>
        )}
      </span>
      <span className="switch-state" aria-hidden="true">
        {t(checked ? "form.switchOn" : "form.switchOff")}
      </span>
      <span className="switch-control">
        <input
          {...props}
          id={id}
          type="checkbox"
          role="switch"
          aria-labelledby={`${id}-label`}
          aria-describedby={
            [props["aria-describedby"], hint ? `${id}-hint` : null].filter(Boolean).join(" ") ||
            undefined
          }
          checked={Boolean(checked)}
          onChange={(event) => onChange(event.target.checked)}
        />
        <span className="switch-track" aria-hidden="true" />
      </span>
    </label>
  );
}
