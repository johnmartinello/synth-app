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

function snap(value: number, min: number, max: number, step: number) {
  const span = Math.max(0.0001, max - min);
  const raw = Math.min(max, Math.max(min, value));
  if (step <= 0) return raw;
  return Math.min(max, Math.max(min, min + Math.round((raw - min) / step) * step));
}

export function HwKnob({
  label,
  display,
  value,
  min,
  max,
  step,
  onChange,
  onGestureChange,
}: {
  label: string;
  display?: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (v: number) => void;
  onGestureChange?: (dragging: boolean) => void;
}) {
  const id = useId();
  const drag = useRef<{ y: number; value: number } | null>(null);
  const span = Math.max(0.0001, max - min);
  const pct = (value - min) / span;
  const angle = -135 + pct * 270;

  const apply = (next: number) => {
    const snapped = snap(next, min, max, step);
    if (snapped !== value) onChange(snapped);
  };

  return (
    <div className="hw-knob">
      <Silk>{label}</Silk>
      <div
        id={id}
        className="hw-knob-face"
        role="slider"
        tabIndex={0}
        aria-label={label}
        aria-valuemin={min}
        aria-valuemax={max}
        aria-valuenow={value}
        aria-valuetext={display}
        onPointerDown={(e) => {
          e.preventDefault();
          e.currentTarget.setPointerCapture(e.pointerId);
          drag.current = { y: e.clientY, value };
          onGestureChange?.(true);
        }}
        onPointerMove={(e) => {
          const start = drag.current;
          if (!start) return;
          const fine = e.shiftKey ? 0.15 : 1;
          apply(start.value + ((start.y - e.clientY) / 84) * span * fine);
        }}
        onPointerUp={(e) => {
          if (drag.current) {
            drag.current = null;
            onGestureChange?.(false);
          }
          if (e.currentTarget.hasPointerCapture(e.pointerId)) {
            e.currentTarget.releasePointerCapture(e.pointerId);
          }
        }}
        onPointerCancel={() => {
          if (!drag.current) return;
          drag.current = null;
          onGestureChange?.(false);
        }}
        onWheel={(e) => {
          e.preventDefault();
          const dir = e.deltaY > 0 ? -1 : 1;
          const fine = e.shiftKey ? 0.25 : 1;
          apply(value + dir * step * fine);
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowUp" || e.key === "ArrowRight") {
            e.preventDefault();
            apply(value + step);
          } else if (e.key === "ArrowDown" || e.key === "ArrowLeft") {
            e.preventDefault();
            apply(value - step);
          } else if (e.key === "Home") {
            e.preventDefault();
            apply(min);
          } else if (e.key === "End") {
            e.preventDefault();
            apply(max);
          }
        }}
      >
        <i className="hw-knob-tick" style={{ "--angle": `${angle}deg` } as CSSProperties} />
      </div>
      <span className="hw-knob-value">{display ?? String(value)}</span>
    </div>
  );
}
