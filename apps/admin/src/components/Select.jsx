import { useId, useRef, useState } from "react";
import * as SelectPrimitive from "@radix-ui/react-select";
import { Check, ChevronDown, ChevronUp } from "lucide-react";
import { useLocale } from "../app/locale.jsx";

export function Select({
  value = "",
  onValueChange,
  options,
  name,
  required = false,
  disabled = false,
  readOnly = false,
  autoComplete,
  form,
  className = "",
  icon: Icon,
  ...props
}) {
  const { t } = useLocale();
  const id = useId();
  const trigger = useRef(null);
  const [container, setContainer] = useState();
  const current = String(value ?? "");
  const empty = options.find((option) => String(option.value) === "");
  let emptyKey = `${id}-empty`;
  while (options.some((option) => String(option.value) === emptyKey)) emptyKey += "-";
  const selected = current || (!required && empty && !empty.disabled ? emptyKey : "");
  const unavailable = disabled || readOnly;
  return (
    <SelectPrimitive.Root
      value={selected}
      onValueChange={(next) => {
        // The hidden native control can emit an empty value while options mount.
        // Intentional empty choices have their own item key.
        if (next) onValueChange(next === emptyKey ? "" : next);
      }}
      disabled={unavailable}
      required={required}
      autoComplete={autoComplete}
      form={form}
      onOpenChange={(open) => {
        // Native dialogs occupy the top layer; their menus must stay inside it.
        if (open) setContainer(trigger.current?.closest("dialog") ?? document.body);
      }}
    >
      <SelectPrimitive.Trigger
        {...props}
        ref={trigger}
        id={props.id ?? id}
        className={`select-trigger ${className}`}
      >
        {Icon && <Icon size={17} className="select-leading-icon" aria-hidden="true" />}
        <SelectPrimitive.Value
          className="select-value"
          placeholder={empty?.label ?? t("cap.select")}
        />
        <SelectPrimitive.Icon className="select-chevron">
          <ChevronDown size={16} aria-hidden="true" />
        </SelectPrimitive.Icon>
      </SelectPrimitive.Trigger>
      <SelectPrimitive.Portal container={container}>
        <SelectPrimitive.Content
          className="select-menu"
          position="popper"
          sideOffset={6}
          collisionPadding={12}
          onEscapeKeyDown={(event) => event.stopPropagation()}
        >
          <SelectPrimitive.ScrollUpButton className="select-scroll">
            <ChevronUp size={16} aria-hidden="true" />
          </SelectPrimitive.ScrollUpButton>
          <SelectPrimitive.Viewport className="select-options">
            {options.map((option) => (
              <SelectPrimitive.Item
                key={String(option.value)}
                value={String(option.value) || emptyKey}
                disabled={option.disabled}
                textValue={option.label}
                className="select-option"
              >
                <SelectPrimitive.ItemText>{option.label}</SelectPrimitive.ItemText>
                <SelectPrimitive.ItemIndicator className="select-check">
                  <Check size={16} aria-hidden="true" />
                </SelectPrimitive.ItemIndicator>
              </SelectPrimitive.Item>
            ))}
          </SelectPrimitive.Viewport>
          <SelectPrimitive.ScrollDownButton className="select-scroll">
            <ChevronDown size={16} aria-hidden="true" />
          </SelectPrimitive.ScrollDownButton>
        </SelectPrimitive.Content>
      </SelectPrimitive.Portal>
      {name && (
        <input type="hidden" name={name} value={current} disabled={unavailable} form={form} />
      )}
    </SelectPrimitive.Root>
  );
}
