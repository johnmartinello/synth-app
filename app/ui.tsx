"use client";

import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type CSSProperties,
  type HTMLAttributes,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

export function Silk({
  children,
  className = "",
  ...props
}: HTMLAttributes<HTMLSpanElement> & { children: ReactNode }) {
  return (
    <span className={`silk ${className}`} {...props}>
      {children}
    </span>
  );
}

export function HwButton({
  on,
  className = "",
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { on?: boolean }) {
  const { role, ...rest } = props;
  const pressed =
    role === "radio" || "aria-pressed" in props || "aria-expanded" in props ? undefined : on;
  return (
    <button
      type={type}
      data-on={on ? "true" : undefined}
      aria-pressed={pressed}
      role={role}
      className={`hw-btn ${className}`}
      {...rest}
    />
  );
}

export function HwToggle({
  on,
  children,
  disabled,
  onClick,
  className = "",
  title,
}: {
  on: boolean;
  children: ReactNode;
  disabled?: boolean;
  onClick: () => void;
  className?: string;
  title?: string;
}) {
  return (
    <HwButton on={on} disabled={disabled} onClick={onClick} className={className} title={title}>
      {children}
    </HwButton>
  );
}

export function HwToggleGroup({
  label,
  ariaLabel,
  value,
  options,
  onChange,
  disabledIds = [],
}: {
  label?: string;
  ariaLabel?: string;
  value: string;
  options: ReadonlyArray<readonly [string, string]>;
  onChange: (v: string) => void;
  disabledIds?: readonly string[];
}) {
  const labelId = useId();
  return (
    <div
      className="hw-row"
      role="radiogroup"
      aria-labelledby={label ? labelId : undefined}
      aria-label={!label ? ariaLabel : undefined}
    >
      {label ? <Silk id={labelId}>{label}</Silk> : null}
      <div className="hw-cluster">
        {options.map(([id, name]) => (
          <HwButton
            key={id}
            role="radio"
            aria-checked={value === id}
            on={value === id}
            disabled={disabledIds.includes(id)}
            onClick={() => onChange(id)}
          >
            {name}
          </HwButton>
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
  const [active, setActive] = useState(0);
  const activeRef = useRef(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const labelId = useId();
  const current = options.find(([v]) => v === value)?.[1] ?? value;

  const moveActive = useCallback((next: number) => {
    const clamped = Math.min(options.length - 1, Math.max(0, next));
    activeRef.current = clamped;
    setActive(clamped);
  }, [options.length]);

  const openMenu = () => {
    const selected = options.findIndex(([v]) => v === value);
    moveActive(selected < 0 ? 0 : selected);
    setOpen(true);
  };

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.code === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        setOpen(false);
        return;
      }
      if (e.code === "ArrowDown") {
        e.preventDefault();
        e.stopImmediatePropagation();
        moveActive(activeRef.current + 1);
        return;
      }
      if (e.code === "ArrowUp") {
        e.preventDefault();
        e.stopImmediatePropagation();
        moveActive(activeRef.current - 1);
        return;
      }
      if (e.code === "Home") {
        e.preventDefault();
        e.stopImmediatePropagation();
        moveActive(0);
        return;
      }
      if (e.code === "End") {
        e.preventDefault();
        e.stopImmediatePropagation();
        moveActive(options.length - 1);
        return;
      }
      if (e.code === "Enter" || e.code === "Space") {
        const pick = options[activeRef.current];
        if (!pick) return;
        e.preventDefault();
        e.stopImmediatePropagation();
        onChange(pick[0]);
        setOpen(false);
      }
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open, options, onChange, moveActive]);

  return (
    <div className={`hw-row ${className}`} ref={rootRef}>
      {label ? <Silk id={labelId}>{label}</Silk> : null}
      <div className="hw-menu">
        <HwButton
          on={open}
          aria-expanded={open}
          aria-haspopup="listbox"
          aria-controls={listId}
          aria-labelledby={label ? labelId : undefined}
          onClick={() => (open ? setOpen(false) : openMenu())}
        >
          {current || "—"}
        </HwButton>
        {open && (
          <ul
            id={listId}
            className="hw-menu-list"
            role="listbox"
            aria-activedescendant={`${listId}-${active}`}
          >
            {options.map(([v, n], i) => (
              <li key={v || "empty"} role="none">
                <button
                  type="button"
                  id={`${listId}-${i}`}
                  role="option"
                  aria-selected={v === value}
                  className="hw-menu-item"
                  data-on={v === value ? "true" : undefined}
                  data-active={i === active ? "true" : undefined}
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

export function HwFlyout({
  label,
  summary,
  children,
  className = "",
}: {
  label?: string;
  summary: string;
  children: ReactNode;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const labelId = useId();
  const panelId = useId();

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.code === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        setOpen(false);
      }
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  return (
    <div className={`hw-row ${className}`} ref={rootRef}>
      {label ? <Silk id={labelId}>{label}</Silk> : null}
      <div className="hw-menu">
        <HwButton
          on={open}
          aria-expanded={open}
          aria-haspopup="dialog"
          aria-controls={panelId}
          aria-labelledby={label ? labelId : undefined}
          onClick={() => setOpen((v) => !v)}
        >
          {summary}
        </HwButton>
        {open && (
          <div id={panelId} className="hw-flyout-panel" role="dialog">
            {children}
          </div>
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

function useMidiMenu(midi?: HwKnobMidi, label?: string) {
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);

  const bind = midi
    ? {
        title: midi.mapped
          ? `${label ?? midi.paramId} · ${midi.mapped}. Right-click for MIDI learn`
          : midi.learning
            ? `Learning ${label ?? midi.paramId}. Right-click to cancel`
            : "Right-click for MIDI learn",
        onContextMenu: (e: ReactMouseEvent) => {
          e.preventDefault();
          e.stopPropagation();
          const w = 11 * 16;
          const h = midi.mapped ? 72 : 40;
          setPos({
            x: Math.min(Math.max(8, e.clientX), window.innerWidth - w - 8),
            y: Math.min(Math.max(8, e.clientY), window.innerHeight - h - 8),
          });
        },
      }
    : {};

  const menu =
    midi && pos
      ? createPortal(
          <MidiLearnMenu
            midi={midi}
            label={label}
            x={pos.x}
            y={pos.y}
            onClose={() => setPos(null)}
          />,
          document.body,
        )
      : null;

  return {
    bind,
    menu,
    mapped: Boolean(midi?.mapped && !midi.learning),
    learning: Boolean(midi?.learning),
  };
}

function MidiLearnMenu({
  midi,
  label,
  x,
  y,
  onClose,
}: {
  midi: HwKnobMidi;
  label?: string;
  x: number;
  y: number;
  onClose: () => void;
}) {
  const rootRef = useRef<HTMLUListElement>(null);

  useEffect(() => {
    const close = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.code === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        onClose();
      }
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [onClose]);

  const name = label ?? midi.paramId;
  return (
    <ul
      ref={rootRef}
      className="hw-ctx-menu"
      role="menu"
      style={{ left: x, top: y }}
      aria-label={`MIDI for ${name}`}
    >
      <li role="none">
        <button
          type="button"
          role="menuitem"
          className="hw-ctx-item"
          onClick={() => {
            midi.onLearn();
            onClose();
          }}
        >
          {midi.learning ? "Cancel MIDI learn" : "MIDI learn"}
        </button>
      </li>
      {midi.mapped ? (
        <li role="none">
          <button
            type="button"
            role="menuitem"
            className="hw-ctx-item"
            onClick={() => {
              midi.onClear();
              onClose();
            }}
          >
            Clear {midi.mapped}
          </button>
        </li>
      ) : null}
    </ul>
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
  const { bind, menu, mapped, learning } = useMidiMenu(midi);
  return (
    <div
      className="hw-stomp"
      data-midi-mapped={mapped ? "true" : undefined}
      data-midi-learn={learning ? "true" : undefined}
      {...bind}
    >
      <HwButton
        on={on}
        disabled={disabled}
        onClick={onClick}
        className={className}
        data-mode={mode}
      >
        {children}
      </HwButton>
      {menu}
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
  const { bind, menu, mapped, learning } = useMidiMenu(midi, label);
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
    <div
      className="hw-knob"
      data-disabled={disabled ? "true" : undefined}
      data-midi-mapped={mapped ? "true" : undefined}
      data-midi-learn={learning ? "true" : undefined}
      {...bind}
    >
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
            if (disabled || e.button !== 0) return;
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
      </div>
      {menu}
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
  midi?: HwKnobMidi;
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
  midi,
}: HwDragValueProps) {
  const editRef = useRef<HTMLSpanElement>(null);
  const skipCommit = useRef(false);
  const drag = useRef<{ x: number; value: number; moved: boolean } | null>(null);
  const [editing, setEditing] = useState(false);
  const { bind, menu, mapped, learning } = useMidiMenu(midi, label);

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
    <div
      className="hw-drag"
      data-disabled={disabled ? "true" : undefined}
      data-midi-mapped={mapped ? "true" : undefined}
      data-midi-learn={learning ? "true" : undefined}
      {...bind}
    >
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
      {menu}
    </div>
  );
}
