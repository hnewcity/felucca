import { createContext, useContext, useId, useState } from "react";
import { ChevronDown } from "lucide-react";
import { useLocale } from "../app/locale.jsx";

const DisclosureDepth = createContext(0);

// Deeper groups become ordinary sections instead of another layer to open.
// Children stay mounted so collapsing cannot discard incomplete input.
export function Disclosure({
  label,
  description,
  status,
  children,
  initiallyOpen = false,
  onOpenChange,
  className = "config-disclosure",
}) {
  const depth = useContext(DisclosureDepth);
  const [open, setOpen] = useState(initiallyOpen);
  const id = useId();
  const { t } = useLocale();
  const flat = depth >= 2;
  const heading = (
    <>
      <span className="disclosure-copy">
        <span className="disclosure-title" id={`${id}-label`}>
          {label}
        </span>
        {description && (
          <span className="disclosure-description" id={`${id}-description`}>
            {description}
          </span>
        )}
      </span>
      {status && (
        <span id={`${id}-status`} className={`disclosure-status ${status.tone ?? ""}`}>
          {status.label}
        </span>
      )}
      {!flat && (
        <span className="disclosure-affordance" aria-hidden="true">
          <span>{t(open ? "form.collapse" : "form.expand")}</span>
          <ChevronDown size={18} className="disclosure-chevron" />
        </span>
      )}
    </>
  );
  const content = (
    <DisclosureDepth.Provider value={depth + (flat ? 0 : 1)}>
      <div id={`${id}-content`} className="config-disclosure-body">
        {children}
      </div>
    </DisclosureDepth.Provider>
  );
  return flat ? (
    <section className={`disclosure disclosure-flat ${className}`} aria-labelledby={`${id}-label`}>
      <div className="disclosure-heading">{heading}</div>
      {content}
    </section>
  ) : (
    <details
      className={`disclosure ${className}`}
      open={open}
      onToggle={(event) => {
        // Nested native toggle events must not change the parent's state.
        if (event.target === event.currentTarget) {
          setOpen(event.currentTarget.open);
          onOpenChange?.(event.currentTarget.open);
        }
      }}
    >
      <summary
        aria-labelledby={`${id}-label`}
        aria-describedby={
          [description ? `${id}-description` : null, status ? `${id}-status` : null]
            .filter(Boolean)
            .join(" ") || undefined
        }
        aria-controls={`${id}-content`}
        aria-expanded={open}
      >
        {heading}
      </summary>
      {content}
    </details>
  );
}
