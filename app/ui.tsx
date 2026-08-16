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

function editSeed(value: number, step: number) {
  if (step >= 1) return String(Math.round(value));
  const decimals = Math.min(4, Math.max(0, Math.ceil(-Math.log10(step))));
  return value.toFixed(decimals);
}

function parseTyped(raw: string): number | null {
  const m = raw.trim().replace(",", ".").match(/-?\d*\.?\d+/);
  if (!m) return null;
  const n = Number(m[0]);
  return Number.isFinite(n) ? n : null;
}

export type HwKnobMidi = {
  paramId: string;
  mapped?: string;
  learning: boolean;
  onLearn: () => void;
  onClear: () => void;
};

function midiChipLabel(midi: HwKnobMidi) {
  if (midi.learning) return "Learn";
  if (midi.mapped) return midi.mapped;
  return "MIDI";
}

export function HwMidiChip({
  midi,
  label,
}: {
  midi: HwKnobMidi;
  label?: string;
}) {
  return (
    <button
      type="button"
      className="hw-midi-chip"
      data-on={midi.learning ? "true" : undefined}
      data-mapped={midi.mapped && !midi.learning ? "true" : undefined}
      aria-label={
        midi.learning
          ? `Cancel MIDI learn for ${label ?? midi.paramId}`
          : midi.mapped
            ? `${label ?? midi.paramId} mapped to ${midi.mapped}. Click to relearn, right-click to clear.`
            : `MIDI learn ${label ?? midi.paramId}`
      }
      onClick={(e) => {
        e.preventDefault();
        e.stopPropagation();
        midi.onLearn();
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        e.stopPropagation();
        midi.onClear();
      }}
    >
      {midiChipLabel(midi)}
    </button>
  );
}

export function HwStomp({
  children,
  on,
  mode,
  disabled,
  onClick,
  midi,
  className = "",
}: {
  children: ReactNode;
  on?: boolean;
  mode?: "rec" | "play" | "dub";
  disabled?: boolean;
  onClick: () => void;
  midi?: HwKnobMidi;
  className?: string;
}) {
  return (
    <div className="hw-stomp">
      <HwButton
        on={on}
        disabled={disabled}
        onClick={onClick}
        className={className}
        data-mode={mode}
      >
        {children}
      </HwButton>
      {midi ? <HwMidiChip midi={midi} /> : null}
    </div>
  );
}

type HwKnobProps = {
  label?: string;
  display?: string;
  value: number;
  min: number;
  max: number;
  step: number;
  disabled?: boolean;
  onChange: (v: number) => void;
  onGestureChange?: (dragging: boolean) => void;
  midi?: HwKnobMidi;
};

export function HwKnob({
  label,
  display,
  value,
  min,
  max,
  step,
  disabled = false,
  onChange,
  onGestureChange,
  midi,
}: HwKnobProps) {
  const id = useId();
  const drag = useRef<{ y: number; value: number } | null>(null);
  const editRef = useRef<HTMLSpanElement>(null);
  const skipCommit = useRef(false);
  const [editing, setEditing] = useState(false);
  const span = Math.max(0.0001, max - min);
  const pct = (value - min) / span;
  const angle = -135 + pct * 270;

  const apply = (next: number) => {
    const snapped = snap(next, min, max, step);
    if (snapped !== value) onChange(snapped);
  };

  const beginEdit = () => {
    if (disabled) return;
    skipCommit.current = false;
    setEditing(true);
    onGestureChange?.(true);
  };

  const finishEdit = (commit: boolean) => {
    const raw = editRef.current?.textContent ?? "";
    const parsed = commit ? parseTyped(raw) : null;
    setEditing(false);
    onGestureChange?.(false);
    if (parsed != null) apply(parsed);
  };

  useEffect(() => {
    if (!editing) return;
    const el = editRef.current;
    if (!el) return;
    el.textContent = editSeed(value, step);
    el.focus();
    const range = document.createRange();
    range.selectNodeContents(el);
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.addRange(range);
  }, [editing, step, value]);

  return (
    <div className="hw-knob" data-disabled={disabled ? "true" : undefined}>
      {label ? <Silk>{label}</Silk> : null}
      <div className="hw-knob-dial">
        <div
          id={id}
          className="hw-knob-face"
          role="slider"
          tabIndex={disabled ? -1 : 0}
          aria-disabled={disabled || undefined}
          aria-label={label ?? display ?? "Knob"}
          aria-valuemin={min}
          aria-valuemax={max}
          aria-valuenow={value}
          aria-valuetext={display}
          onPointerDown={(e) => {
            if (disabled) return;
            e.preventDefault();
            e.currentTarget.setPointerCapture(e.pointerId);
            drag.current = { y: e.clientY, value };
            onGestureChange?.(true);
          }}
          onPointerMove={(e) => {
            if (disabled) return;
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
            if (disabled) return;
            e.preventDefault();
            const dir = e.deltaY > 0 ? -1 : 1;
            const fine = e.shiftKey ? 0.25 : 1;
            apply(value + dir * step * fine);
          }}
          onKeyDown={(e) => {
            if (disabled) return;
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
        {midi ? <HwMidiChip midi={midi} label={label} /> : null}
      </div>
      <div className="hw-knob-value-slot">
        {editing ? (
          <span
            ref={editRef}
            className="hw-knob-value hw-knob-value-edit"
            contentEditable
            suppressContentEditableWarning
            role="textbox"
            inputMode="decimal"
            aria-label={`Edit ${label ?? "value"}`}
            onBlur={() => {
              if (skipCommit.current) {
                skipCommit.current = false;
                finishEdit(false);
                return;
              }
              finishEdit(true);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                finishEdit(true);
              } else if (e.key === "Escape") {
                e.preventDefault();
                skipCommit.current = true;
                e.currentTarget.blur();
              }
              e.stopPropagation();
            }}
          />
        ) : (
          <span
            className="hw-knob-value"
            title={disabled ? undefined : "Double-click to type a value"}
            onDoubleClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              beginEdit();
            }}
          >
            {display ?? String(value)}
          </span>
        )}
      </div>
    </div>
  );
}

type HwDragValueProps = {
  label?: string;
  display?: string;
  value: number;
  min: number;
  max: number;
  step: number;
  disabled?: boolean;
  onChange: (v: number) => void;
  onGestureChange?: (dragging: boolean) => void;
};

export function HwDragValue({
  label,
  display,
  value,
  min,
  max,
  step,
  disabled = false,
  onChange,
  onGestureChange,
}: HwDragValueProps) {
  const editRef = useRef<HTMLSpanElement>(null);
  const skipCommit = useRef(false);
  const drag = useRef<{ x: number; value: number; moved: boolean } | null>(null);
  const [editing, setEditing] = useState(false);

  const apply = (next: number) => {
    const snapped = snap(next, min, max, step);
    if (snapped !== value) onChange(snapped);
  };

  const beginEdit = () => {
    if (disabled) return;
    drag.current = null;
    skipCommit.current = false;
    setEditing(true);
    onGestureChange?.(true);
  };

  const finishEdit = (commit: boolean) => {
    const raw = editRef.current?.textContent ?? "";
    const parsed = commit ? parseTyped(raw) : null;
    setEditing(false);
    onGestureChange?.(false);
    if (parsed != null) apply(parsed);
  };

  useEffect(() => {
    if (!editing) return;
    const el = editRef.current;
    if (!el) return;
    el.textContent = editSeed(value, step);
    el.focus();
    const range = document.createRange();
    range.selectNodeContents(el);
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.addRange(range);
  }, [editing, step, value]);

  return (
    <div className="hw-drag" data-disabled={disabled ? "true" : undefined}>
      {label ? <Silk>{label}</Silk> : null}
      {editing ? (
        <span
          ref={editRef}
          className="hw-drag-value hw-drag-value-edit"
          contentEditable
          suppressContentEditableWarning
          role="textbox"
          inputMode="decimal"
          aria-label={`Edit ${label ?? "value"}`}
          onBlur={() => {
            if (skipCommit.current) {
              skipCommit.current = false;
              finishEdit(false);
              return;
            }
            finishEdit(true);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              finishEdit(true);
            } else if (e.key === "Escape") {
              e.preventDefault();
              skipCommit.current = true;
              e.currentTarget.blur();
            }
            e.stopPropagation();
          }}
        />
      ) : (
        <span
          className="hw-drag-value"
          role="slider"
          tabIndex={disabled ? -1 : 0}
          aria-disabled={disabled || undefined}
          aria-label={label ?? "Value"}
          aria-valuemin={min}
          aria-valuemax={max}
          aria-valuenow={value}
          aria-valuetext={display ?? String(value)}
          title={disabled ? undefined : "Drag sideways to change, double-click to type"}
          onPointerDown={(e) => {
            if (disabled || e.button !== 0) return;
            e.preventDefault();
            e.currentTarget.setPointerCapture(e.pointerId);
            drag.current = { x: e.clientX, value, moved: false };
            onGestureChange?.(true);
          }}
          onPointerMove={(e) => {
            if (disabled) return;
            const start = drag.current;
            if (!start) return;
            const dx = e.clientX - start.x;
            if (!start.moved && Math.abs(dx) < 3) return;
            start.moved = true;
            const fine = e.shiftKey ? 0.25 : 1;
            apply(start.value + dx * step * fine);
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
          onDoubleClick={(e) => {
            e.preventDefault();
            e.stopPropagation();
            beginEdit();
          }}
          onWheel={(e) => {
            if (disabled) return;
            e.preventDefault();
            const dir = e.deltaY > 0 ? -1 : 1;
            const fine = e.shiftKey ? 0.25 : 1;
            apply(value + dir * step * fine);
          }}
          onKeyDown={(e) => {
            if (disabled) return;
            if (e.key === "ArrowRight" || e.key === "ArrowUp") {
              e.preventDefault();
              apply(value + step);
            } else if (e.key === "ArrowLeft" || e.key === "ArrowDown") {
              e.preventDefault();
              apply(value - step);
            } else if (e.key === "Home") {
              e.preventDefault();
              apply(min);
            } else if (e.key === "End") {
              e.preventDefault();
              apply(max);
            } else if (e.key === "Enter") {
              e.preventDefault();
              beginEdit();
            }
          }}
        >
          {display ?? String(value)}
        </span>
      )}
    </div>
  );
}
