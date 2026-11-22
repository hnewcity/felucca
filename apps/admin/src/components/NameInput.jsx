import { useId } from "react";
import { Shuffle } from "lucide-react";
import { useLocale } from "../app/locale.jsx";
import { generateName } from "../lib/names.js";
import { Button } from "./ui.jsx";

export function NameInput({
  label,
  prefix,
  existingNames = [],
  value,
  onChange,
  className = "",
  hint,
  hideGenerator = false,
  ...props
}) {
  const id = useId();
  const { t } = useLocale();
  const action = t("names.generateFor", { label });
  return (
    <div className={`name-field ${className}`}>
      <label htmlFor={id}>{label}</label>
      <div className="name-input-row">
        <input
          {...props}
          id={id}
          value={value}
          aria-describedby={hint ? `${id}-hint` : undefined}
          onChange={(event) => onChange(event.target.value)}
        />
        {!hideGenerator && !props.readOnly && (
          <Button
            className="name-generate"
            icon={Shuffle}
            disabled={props.disabled}
            aria-label={action}
            title={action}
            onClick={() => onChange(generateName(prefix, { existingNames, current: value }))}
          >
            {t("names.generate")}
          </Button>
        )}
      </div>
      {hint && (
        <span id={`${id}-hint`} className="field-hint">
          {hint}
        </span>
      )}
    </div>
  );
}
