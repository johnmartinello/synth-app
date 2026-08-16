"use client";

import {
  useEffect,
  useId,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type CSSProperties,
  type ReactNode,
} from "react";

export function Silk({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <span className={`silk ${className}`}>{children}</span>;
}

export function HwButton({
  on,
  className = "",
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { on?: boolean }) {
  return (
    <button
      type={type}
      data-on={on ? "true" : undefined}
      className={`hw-btn ${className}`}
      {...props}
    />
  );
}

export function HwToggle({
  on,
  children,
  disabled,
  onClick,
  className = "",
}: {
  on: boolean;
  children: ReactNode;
  disabled?: boolean;
  onClick: () => void;
  className?: string;
}) {
  return (
    <HwButton on={on} disabled={disabled} onClick={onClick} className={className}>
      {children}
    </HwButton>
  );
}

export function HwToggleGroup({
  label,
  value,
  options,
  onChange,
  disabledIds = [],
}: {
  label?: string;
  value: string;
  options: ReadonlyArray<readonly [string, string]>;
  onChange: (v: string) => void;
  disabledIds?: readonly string[];
}) {
  return (
    <div className="hw-row">
      {label ? <Silk>{label}</Silk> : null}
      <div className="hw-cluster">
        {options.map(([id, name]) => (
          <HwToggle
            key={id}
            on={value === id}
            disabled={disabledIds.includes(id)}
            onClick={() => onChange(id)}
          >
            {name}
          </HwToggle>
        ))}
      </div>
    </div>
  );
}

export function HwMenu({
  label,
  value,
  options,
  onChange,
  className = "",
}: {
  label?: string;
  value: string;
  options: ReadonlyArray<readonly [string, string]>;
  onChange: (v: string) => void;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const current = options.find(([v]) => v === value)?.[1] ?? value;

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.code === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div className={`hw-row ${className}`} ref={rootRef}>
      {label ? <Silk>{label}</Silk> : null}
      <div className="hw-menu">
        <HwButton on={open} onClick={() => setOpen((v) => !v)}>
          {current || "—"}
        </HwButton>
        {open && (
          <ul className="hw-menu-list" role="listbox">
            {options.map(([v, n]) => (
              <li key={v || "empty"}>
                <button
                  type="button"
                  className="hw-menu-item"
                  data-on={v === value ? "true" : undefined}
                  onClick={() => {
                    onChange(v);
                    setOpen(false);
                  }}
                >
                  {n}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

export function HwSlider({
  label,
  value,
  min,
  max,
  step,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (v: number) => void;
}) {
  const id = useId();
  const pct = ((value - min) / Math.max(0.0001, max - min)) * 100;
  return (
    <label className="hw-slider" htmlFor={id}>
      <Silk>{label}</Silk>
      <input
        id={id}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        style={{ "--pct": `${pct}%` } as CSSProperties}
      />
    </label>
  );
}
