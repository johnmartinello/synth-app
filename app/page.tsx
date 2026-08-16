"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { HwButton, HwKnob, HwMenu, HwToggle, HwToggleGroup, Silk } from "./ui";

type DiscoveryReport = {
  searched_paths: string[];
  clap_files: string[];
  found: boolean;
  path: string | null;
  plugin_id: string | null;
  plugin_name: string | null;
  error: string | null;
};

type HostStatus = {
  surge: DiscoveryReport;
  audio_ok: boolean;
  editor_open: boolean;
  midi_port: string | null;
  test_c_playing: boolean;
  message: string | null;
  state_supported: boolean;
  presets_supported: boolean;
  current_preset: string | null;
  current_preset_path: string | null;
  sustain: boolean;
  octave?: number;
  arp_enabled?: boolean;
  arp_rate_hz?: number;
  midi_clock?: boolean;
  filter_cutoff?: number;
  roll_ms?: number;
};

type PresetInfo = {
  path: string;
  name: string;
  category: string;
  bank: string;
};

type MidiPort = { name: string };

const ROOTS = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const DEGREES = [
  { n: 1, roman: "I" },
  { n: 2, roman: "ii" },
  { n: 3, roman: "iii" },
  { n: 4, roman: "IV" },
  { n: 5, roman: "V" },
  { n: 6, roman: "vi" },
  { n: 7, roman: "vii°" },
];
type ChordType = "tri" | "7" | "9" | "11" | "13";
type Secondary = "none" | "v" | "iv" | "vii";
const CHORD_TYPES: ReadonlyArray<readonly [ChordType, string]> = [
  ["tri", "tri"],
  ["7", "7"],
  ["9", "9"],
  ["11", "11"],
  ["13", "13"],
];
const INVERSIONS: ReadonlyArray<readonly [number, string]> = [
  [0, "root"],
  [1, "1st"],
  [2, "2nd"],
  [3, "3rd"],
];
const SECONDARIES: ReadonlyArray<readonly [Secondary, string]> = [
  ["none", "~"],
  ["v", "/V"],
  ["iv", "/IV"],
  ["vii", "/vii°"],
];
const TONE_COUNT: Record<ChordType, number> = {
  tri: 3,
  "7": 4,
  "9": 5,
  "11": 6,
  "13": 7,
};
const MIDI_PORT_KEY = "surge-chord-midi-port";

function pickDefaultMidiPort(ports: MidiPort[], remembered: string | null) {
  if (remembered && ports.some((p) => p.name === remembered)) return remembered;
  const skip = /through|loop|wavetable|microsoft|mapper|\bbus\b/i;
  return ports.find((p) => !skip.test(p.name))?.name ?? ports[0]?.name ?? null;
}

function readRememberedMidiPort() {
  try {
    return localStorage.getItem(MIDI_PORT_KEY);
  } catch {
    return null;
  }
}

function rememberMidiPort(name: string) {
  try {
    localStorage.setItem(MIDI_PORT_KEY, name);
  } catch {
    /* ignore */
  }
}

function romanFor(degree: number, scale: "major" | "minor") {
  if (scale === "major") return DEGREES[degree - 1]?.roman ?? String(degree);
  return ["i", "ii°", "♭III", "iv", "V", "♭VI", "vii°"][degree - 1] ?? String(degree);
}

export default function Home() {
  const [status, setStatus] = useState<HostStatus | null>(null);
  const [ports, setPorts] = useState<MidiPort[]>([]);
  const [root, setRoot] = useState(0);
  const [scale, setScale] = useState<"major" | "minor">("major");
  const [chordType, setChordType] = useState<ChordType>("tri");
  const [inversion, setInversion] = useState(0);
  const [secondary, setSecondary] = useState<Secondary>("none");
  const [octave, setOctave] = useState(4);
  const [held, setHeld] = useState<Set<number>>(new Set());
  const [armed, setArmed] = useState<number | null>(null);
  const [arpOn, setArpOn] = useState(false);
  const [arpMode, setArpMode] = useState<"up" | "down" | "up-down">("up");
  const [arpRate, setArpRate] = useState(8);
  const [arpLatch, setArpLatch] = useState(false);
  const [filter, setFilter] = useState(1);
  const [rollMs, setRollMs] = useState(0);
  const [strumMode, setStrumMode] = useState(false);
  const [sustain, setSustain] = useState(false);
  const [engineNotes, setEngineNotes] = useState<number[] | null>(null);
  const [configOpen, setConfigOpen] = useState(false);
  const autoMidiRef = useRef(false);
  const rateGesture = useRef(false);
  const filterGesture = useRef(false);
  const rollGesture = useRef(false);

  const previewNotes = useMemo(() => {
    if (held.size > 0) {
      const all = [...held].flatMap((degree) =>
        chordMidi(root, scale, chordType, inversion, secondary, octave, degree),
      );
      return [...new Set(all)].sort((a, b) => a - b);
    }
    if (engineNotes != null && engineNotes.length > 0) return engineNotes;
    return [];
  }, [
    engineNotes,
    held,
    root,
    scale,
    chordType,
    inversion,
    secondary,
    octave,
  ]);

  const heldNotes = useMemo(() => {
    if (held.size === 0) return [];
    const all = [...held].flatMap((degree) =>
      chordMidi(root, scale, chordType, inversion, secondary, octave, degree),
    );
    return [...new Set(all)].sort((a, b) => a - b);
  }, [held, root, scale, chordType, inversion, secondary, octave]);

  const refreshPorts = useCallback(async () => {
    try {
      setPorts(await invoke<MidiPort[]>("list_midi_ports"));
    } catch {
      setPorts([]);
    }
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        const next = await invoke<HostStatus>("get_status");
        setStatus(next);
        if (typeof next.octave === "number") setOctave(next.octave);
        if (typeof next.arp_enabled === "boolean") setArpOn(next.arp_enabled);
        if (typeof next.arp_rate_hz === "number") setArpRate(next.arp_rate_hz);
        if (typeof next.filter_cutoff === "number") setFilter(next.filter_cutoff);
        if (typeof next.roll_ms === "number") setRollMs(next.roll_ms);
      } catch {
        /* web preview without Tauri */
      }
      try {
        unlisten = await listen<HostStatus>("host-status", (ev) => {
          const next = ev.payload;
          setStatus(next);
          if (typeof next.octave === "number") setOctave(next.octave);
          if (typeof next.arp_enabled === "boolean") setArpOn(next.arp_enabled);
          if (typeof next.arp_rate_hz === "number" && !rateGesture.current) {
            setArpRate(next.arp_rate_hz);
          }
          if (typeof next.filter_cutoff === "number" && !filterGesture.current) {
            setFilter(next.filter_cutoff);
          }
          if (typeof next.roll_ms === "number" && !rollGesture.current) {
            setRollMs(next.roll_ms);
          }
        });
      } catch {
        /* not in Tauri */
      }
      await refreshPorts();
    })();
    return () => {
      unlisten?.();
    };
  }, [refreshPorts]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listen<{ notes: number[] }>("notes-ui", (ev) => {
          setEngineNotes(ev.payload.notes);
        });
      } catch {
        /* not in Tauri */
      }
    })();
    return () => {
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listen<{ held: number[] }>("pads-ui", (ev) => {
          const next = ev.payload.held;
          setHeld(new Set(next));
          setArmed(next.at(-1) ?? null);
        });
      } catch {
        /* not in Tauri */
      }
    })();
    return () => {
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    invoke("set_key", { root, scale }).catch(() => undefined);
  }, [root, scale]);

  useEffect(() => {
    invoke("set_chord_shape", { chordType, inversion, secondary }).catch(() => undefined);
  }, [chordType, inversion, secondary]);

  useEffect(() => {
    invoke("set_octave", { octave }).catch(() => undefined);
  }, [octave]);

  useEffect(() => {
    invoke("set_arp", {
      enabled: arpOn,
      mode: arpMode,
      rateHz: arpRate,
      latch: arpLatch,
    }).catch(() => undefined);
  }, [arpOn, arpMode, arpRate, arpLatch]);

  useEffect(() => {
    invoke("set_strum_mode", { enabled: strumMode }).catch(() => undefined);
  }, [strumMode]);

  useEffect(() => {
    invoke("set_sustain", { enabled: sustain }).catch(() => undefined);
  }, [sustain]);

  useEffect(() => {
    invoke("set_roll", { ms: rollMs }).catch(() => undefined);
  }, [rollMs]);

  useEffect(() => {
    if (autoMidiRef.current || !ports.length || status?.midi_port) {
      if (status?.midi_port) autoMidiRef.current = true;
      return;
    }
    const name = pickDefaultMidiPort(ports, readRememberedMidiPort());
    if (!name) return;
    autoMidiRef.current = true;
    rememberMidiPort(name);
    invoke("open_midi_port", { portName: name }).catch(() => undefined);
  }, [ports, status?.midi_port]);

  const padDown = useCallback((degree: number) => {
    setArmed(degree);
    setHeld((prev) => new Set(prev).add(degree));
    invoke("pad_down", { degree }).catch(() => undefined);
  }, []);
  const padUp = useCallback((degree: number) => {
    setHeld((prev) => {
      const next = new Set(prev);
      next.delete(degree);
      return next;
    });
    setArmed((current) => (current === degree ? null : current));
    invoke("pad_up", { degree }).catch(() => undefined);
  }, []);

  useEffect(() => {
    const isTypingTarget = (el: EventTarget | null) => {
      if (!(el instanceof HTMLElement)) return false;
      const tag = el.tagName;
      return (
        tag === "INPUT" ||
        tag === "SELECT" ||
        tag === "TEXTAREA" ||
        tag === "BUTTON" ||
        el.isContentEditable
      );
    };
    const typingField = (el: EventTarget | null) => {
      if (!(el instanceof HTMLElement)) return false;
      const tag = el.tagName;
      return tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA" || el.isContentEditable;
    };
    const digitDegree = (e: KeyboardEvent) => {
      const map: Record<string, number> = {
        Digit1: 1,
        Digit2: 2,
        Digit3: 3,
        Digit4: 4,
        Digit5: 5,
        Digit6: 6,
        Digit7: 7,
        Numpad1: 1,
        Numpad2: 2,
        Numpad3: 3,
        Numpad4: 4,
        Numpad5: 5,
        Numpad6: 6,
        Numpad7: 7,
      };
      return map[e.code] ?? null;
    };
    const down = (e: KeyboardEvent) => {
      const degree = digitDegree(e);
      if (degree != null && !e.repeat && !typingField(e.target)) {
        e.preventDefault();
        padDown(degree);
        return;
      }
      if (e.code !== "Space" || e.repeat || isTypingTarget(e.target)) return;
      e.preventDefault();
      invoke("set_sustain_hold", { enabled: true }).catch(() => undefined);
    };
    const up = (e: KeyboardEvent) => {
      const degree = digitDegree(e);
      if (degree != null && !typingField(e.target)) {
        padUp(degree);
        return;
      }
      if (e.code !== "Space") return;
      invoke("set_sustain_hold", { enabled: false }).catch(() => undefined);
    };
    const clear = () => {
      invoke("set_sustain_hold", { enabled: false }).catch(() => undefined);
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", clear);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", clear);
      clear();
    };
  }, [padDown, padUp]);

  const missing = status && !status.surge.found;
  const subtitle = useMemo(() => {
    if (!status) return "Starting host…";
    if (status.message) return status.message;
    if (missing) return status.surge.error ?? "Surge XT not found";
    if (status.audio_ok) return "Rust audio host is live — pads send chords into Surge.";
    return "Host running";
  }, [status, missing]);

  return (
    <div className="flex min-h-screen flex-col px-5 py-4 md:px-7">
      <header className="mb-3 flex flex-wrap items-center justify-between gap-3">
        <div>
          <Silk className="silk-accent">Chord instrument</Silk>
          <h1 className="mt-1 text-2xl font-semibold tracking-tight">Surge Chord</h1>
          <p className="mt-1 max-w-xl text-xs text-[var(--muted)]">{subtitle}</p>
        </div>
        <div className="flex items-center gap-2">
          <HwButton
            onClick={() => {
              setConfigOpen(true);
              refreshPorts();
            }}
          >
            Config
          </HwButton>
          <StatusLed status={status} />
        </div>
      </header>

      {missing && (
        <div className="mb-3 border border-[var(--danger)] bg-[#1c1210] px-3 py-2 text-sm">
          <p className="font-medium text-[var(--danger-ink)]">Surge XT CLAP not loaded</p>
          <p className="mt-1 text-[var(--muted)]">{status?.surge.error}</p>
          <p className="mt-1 text-xs text-[var(--muted)]">
            Searched: {status?.surge.searched_paths.join(" · ") || "—"}
          </p>
        </div>
      )}

      <PatchBar
        currentPath={status?.current_preset_path ?? null}
        currentLabel={status?.current_preset ?? null}
        supported={!!status?.presets_supported}
      />

      <hr className="hw-rule my-3" />

      <section className="mb-3 flex flex-wrap items-center gap-x-6 gap-y-2">
        <HwToggleGroup
          label="Key"
          value={String(root)}
          options={ROOTS.map((n, i) => [String(i), n] as const)}
          onChange={(v) => setRoot(Number(v))}
        />
        <HwToggleGroup
          label="Scale"
          value={scale}
          options={[
            ["major", "Maj"],
            ["minor", "Min"],
          ]}
          onChange={(v) => setScale(v as "major" | "minor")}
        />
        <HwToggle on={sustain || !!status?.sustain} onClick={() => setSustain((v) => !v)}>
          Sustain
        </HwToggle>
      </section>

      <section className="mb-3 flex flex-wrap items-center gap-x-6 gap-y-2">
        <div className="hw-row">
          <Silk>Oct</Silk>
          <div className="hw-cluster">
            <HwButton
              disabled={octave <= 1}
              onClick={() => setOctave((n) => Math.max(1, n - 1))}
            >
              −
            </HwButton>
            <HwButton className="min-w-8">{octave}</HwButton>
            <HwButton
              disabled={octave >= 7}
              onClick={() => setOctave((n) => Math.min(7, n + 1))}
            >
              +
            </HwButton>
          </div>
        </div>
        <HwToggleGroup
          label="Type"
          value={chordType}
          options={CHORD_TYPES}
          onChange={(v) => {
            const next = v as ChordType;
            setChordType(next);
            setInversion((inv) => Math.min(inv, TONE_COUNT[next] - 1));
          }}
        />
        <HwToggleGroup
          label="Inv"
          value={String(inversion)}
          options={INVERSIONS.map(([id, name]) => [String(id), name] as const)}
          disabledIds={chordType === "tri" ? ["3"] : []}
          onChange={(v) => setInversion(Number(v))}
        />
        <HwToggleGroup
          label="Sec"
          value={secondary}
          options={SECONDARIES}
          onChange={(v) => setSecondary(v as Secondary)}
        />
      </section>

      <section className="mb-4 grid flex-1 grid-cols-7 gap-1.5">
        {DEGREES.map((d) => {
          const on = held.has(d.n);
          return (
            <button
              key={d.n}
              type="button"
              className="hw-pad"
              data-on={on ? "true" : undefined}
              data-armed={armed === d.n ? "true" : undefined}
              onPointerDown={(e) => {
                e.currentTarget.setPointerCapture(e.pointerId);
                padDown(d.n);
              }}
              onPointerUp={() => padUp(d.n)}
              onPointerCancel={() => padUp(d.n)}
            >
              <span className="silk opacity-70">
                {d.n}/{padRoman(d.n, scale, secondary)}
              </span>
              <span className="text-3xl font-semibold">
                {chordSymbol(root, scale, chordType, secondary, d.n)}
              </span>
            </button>
          );
        })}
      </section>

      <section className="mb-4 grid items-center gap-x-4 gap-y-2 lg:grid-cols-[minmax(0,1fr)_16rem]">
        <Silk className="max-lg:order-1">Keyboard</Silk>
        <div className="flex items-center justify-between gap-2 max-lg:order-3">
          <Silk>Strum</Silk>
          <HwToggle on={strumMode} onClick={() => setStrumMode((v) => !v)}>
            On
          </HwToggle>
        </div>
        <div className="max-lg:order-2">
          <PianoPreview notes={previewNotes} octave={octave} />
        </div>
        <div className="max-lg:order-4">
          <StrumPlate
            notes={heldNotes}
            sounding={previewNotes}
            armed={held.size > 0}
            enabled={strumMode}
          />
        </div>
      </section>

      <section className="grid gap-4 md:grid-cols-[1fr_1.2fr]">
        <div>
          <Silk className="mb-2">Arp</Silk>
          <div className="flex flex-wrap items-center gap-3">
            <HwToggle on={arpOn} onClick={() => setArpOn((v) => !v)}>
              On
            </HwToggle>
            <HwToggle on={arpLatch} onClick={() => setArpLatch((v) => !v)}>
              Latch
            </HwToggle>
            <HwToggleGroup
              value={arpMode}
              options={[
                ["up", "Up"],
                ["down", "Dn"],
                ["up-down", "U/D"],
              ]}
              onChange={(v) => setArpMode(v as "up" | "down" | "up-down")}
            />
          </div>
          <div className="hw-knobs">
            <HwKnob
              label="Rate"
              display={`${arpRate.toFixed(1)} Hz${status?.midi_clock ? " sync" : ""}`}
              value={arpRate}
              min={1}
              max={16}
              step={0.5}
              onChange={setArpRate}
              onGestureChange={(on) => {
                rateGesture.current = on;
              }}
            />
            <HwKnob
              label="Filter"
              display={`${Math.round(filter * 100)}`}
              value={filter}
              min={0}
              max={1}
              step={0.01}
              onChange={(v) => {
                setFilter(v);
                invoke("set_filter", { value: v }).catch(() => undefined);
              }}
              onGestureChange={(on) => {
                filterGesture.current = on;
              }}
            />
            <HwKnob
              label="Roll"
              display={`${Math.round(rollMs)} ms`}
              value={rollMs}
              min={0}
              max={150}
              step={1}
              onChange={setRollMs}
              onGestureChange={(on) => {
                rollGesture.current = on;
              }}
            />
          </div>
        </div>
        <div>
          <Silk className="mb-2">Chromatic</Silk>
          <MiniKeys />
        </div>
      </section>

      {configOpen && (
        <ConfigModal
          status={status}
          ports={ports}
          onClose={() => setConfigOpen(false)}
          onRefreshPorts={refreshPorts}
        />
      )}
    </div>
  );
}

function unique(values: string[]) {
  return [...new Set(values)];
}

function folderLabel(category: string) {
  return category || "Root";
}

function PatchBar({
  currentPath,
  currentLabel,
  supported,
}: {
  currentPath: string | null;
  currentLabel: string | null;
  supported: boolean;
}) {
  const [presets, setPresets] = useState<PresetInfo[]>([]);
  const [bank, setBank] = useState("Factory");
  const [category, setCategory] = useState("Pads");
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  const refresh = useCallback(async () => {
    try {
      setPresets(await invoke<PresetInfo[]>("list_presets"));
    } catch {
      setPresets([]);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const current = useMemo(
    () => presets.find((p) => p.path === currentPath) ?? null,
    [presets, currentPath],
  );

  useEffect(() => {
    if (!current) return;
    setBank(current.bank);
    setCategory(current.category);
  }, [current]);

  const tree = useMemo(() => {
    const found = unique(presets.map((p) => p.bank));
    const order = ["Factory", "Third party", "User"];
    const banks = order
      .filter((b) => found.includes(b))
      .concat(found.filter((b) => !order.includes(b)));
    return banks.map((b) => ({
      bank: b,
      categories: unique(
        presets.filter((p) => p.bank === b).map((p) => p.category),
      ).sort((a, c) => folderLabel(a).localeCompare(folderLabel(c))),
    }));
  }, [presets]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (q) {
      return presets.filter(
        (p) =>
          p.name.toLowerCase().includes(q) ||
          folderLabel(p.category).toLowerCase().includes(q) ||
          p.bank.toLowerCase().includes(q),
      );
    }
    return presets.filter((p) => p.bank === bank && p.category === category);
  }, [presets, bank, category, query]);

  const load = (path: string) => {
    if (!path || !supported) return;
    setLoading(true);
    invoke("load_preset", { path })
      .catch(() => undefined)
      .finally(() => setLoading(false));
  };

  const step = (dir: -1 | 1) => {
    if (!filtered.length) return;
    const idx = filtered.findIndex((p) => p.path === currentPath);
    const next =
      idx < 0
        ? dir > 0
          ? 0
          : filtered.length - 1
        : (idx + dir + filtered.length) % filtered.length;
    load(filtered[next].path);
  };

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

  const disabled = !supported || loading;
  const barLabel = current
    ? `${current.bank} / ${folderLabel(current.category)} / ${current.name}`
    : currentLabel || "Init";

  return (
    <div className="relative" ref={rootRef}>
      <div className="flex items-stretch gap-1">
        <HwButton disabled={disabled || !filtered.length} onClick={() => step(-1)}>
          ◀
        </HwButton>
        <HwButton
          className="hw-btn-wide"
          on={open}
          disabled={!presets.length && !open}
          onClick={() => setOpen((v) => !v)}
        >
          {barLabel}
        </HwButton>
        <HwButton disabled={disabled || !filtered.length} onClick={() => step(1)}>
          ▶
        </HwButton>
      </div>
      {open && (
        <div className="patch-pop">
          <div className="patch-pop-head">
            <input
              className="hw-field"
              value={query}
              placeholder="Search patches"
              onChange={(e) => setQuery(e.target.value)}
            />
            <HwButton onClick={refresh}>Refresh</HwButton>
          </div>
          <ul className="patch-tree">
            {tree.map((group) => (
              <li key={group.bank}>
                <p className="patch-tree-bank">{group.bank}</p>
                {group.categories.map((cat) => (
                  <button
                    key={`${group.bank}/${cat}`}
                    type="button"
                    className="patch-item"
                    data-on={
                      !query && bank === group.bank && category === cat ? "true" : undefined
                    }
                    onClick={() => {
                      setBank(group.bank);
                      setCategory(cat);
                      setQuery("");
                    }}
                  >
                    {folderLabel(cat)}
                  </button>
                ))}
              </li>
            ))}
          </ul>
          <ul className="patch-list">
            {filtered.length === 0 ? (
              <li className="px-2 py-2 text-xs text-[var(--muted)]">
                {!supported
                  ? "Surge is not offering CLAP preset-load yet — open the editor to change patches."
                  : presets.length
                    ? "No matching patches."
                    : "No .fxp patches found under ProgramData or Documents/Surge XT."}
              </li>
            ) : (
              filtered.map((p) => (
                <li key={p.path}>
                  <button
                    type="button"
                    className="patch-item"
                    data-on={p.path === currentPath ? "true" : undefined}
                    disabled={disabled}
                    onClick={() => load(p.path)}
                  >
                    {query ? `${p.bank} / ${folderLabel(p.category)} / ${p.name}` : p.name}
                  </button>
                </li>
              ))
            )}
          </ul>
        </div>
      )}
    </div>
  );
}

function ConfigModal({
  status,
  ports,
  onClose,
  onRefreshPorts,
}: {
  status: HostStatus | null;
  ports: MidiPort[];
  onClose: () => void;
  onRefreshPorts: () => void;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.code === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const selectPort = (name: string) => {
    if (!name) {
      invoke("close_midi_port").catch(() => undefined);
      return;
    }
    rememberMidiPort(name);
    invoke("open_midi_port", { portName: name }).catch(() => undefined);
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4"
      onClick={onClose}
    >
      <div className="hw-dialog flex flex-col gap-4" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between gap-3">
          <Silk>Config</Silk>
          <HwButton onClick={onClose}>Close</HwButton>
        </div>
        <div>
          <Silk className="mb-2">MIDI in</Silk>
          <div className="flex flex-wrap items-center gap-2">
            <HwMenu
              className="min-w-0 flex-1"
              value={status?.midi_port ?? ""}
              onChange={selectPort}
              options={[
                ["", "No device"],
                ...ports.map((p) => [p.name, p.name] as const),
              ]}
            />
            <HwButton onClick={onRefreshPorts}>Refresh</HwButton>
          </div>
          <p className="mt-2 text-[11px] leading-4 text-[var(--muted)]">
            The first hardware input is opened on launch. White keys in any
            octave = chords 1–7; Oct− / Oct+ on a controller shifts the chord
            octave. Other notes are chromatic. Sustain pedal (CC64) and Space
            hold notes. In strum mode, the mod touchbar (CC1) sweeps the
            strings. Arp button (CC102) toggles arp. Rate follows MIDI clock.
            Filter (CC74) is a low-pass on the output. Roll staggers block chords.
          </p>
        </div>
        <div>
          <Silk className="mb-2">Surge</Silk>
          <div className="flex flex-wrap gap-2">
            <HwButton
              on={!!status?.editor_open}
              onClick={() =>
                invoke(status?.editor_open ? "close_editor" : "open_editor")
              }
            >
              {status?.editor_open ? "Close editor" : "Open editor"}
            </HwButton>
            {status?.state_supported && (
              <>
                <HwButton onClick={() => invoke("save_plugin_state")}>
                  Save state
                </HwButton>
                <HwButton onClick={() => invoke("load_plugin_state")}>
                  Restore
                </HwButton>
              </>
            )}
          </div>
          <p className="mt-2 truncate text-[11px] text-[var(--muted)]">
            {status?.current_preset
              ? status.current_preset
              : (status?.surge.path ?? "Not hosting a .clap file")}
          </p>
        </div>
      </div>
    </div>
  );
}

function isBlackPc(pc: number) {
  return pc === 1 || pc === 3 || pc === 6 || pc === 8 || pc === 10;
}

function pianoRange(notes: number[], _octave: number) {
  let start = 36; // C2
  let end = 96; // C7
  if (notes.length) {
    const min = Math.min(...notes);
    const max = Math.max(...notes);
    start = Math.min(start, Math.floor(min / 12) * 12);
    end = Math.max(end, Math.ceil((max + 1) / 12) * 12);
  }
  start = Math.max(24, start);
  end = Math.min(108, end);
  return { start, end };
}

function PianoPreview({ notes, octave }: { notes: number[]; octave: number }) {
  const pressed = useMemo(() => new Set(notes), [notes]);
  const { start, end } = useMemo(() => pianoRange(notes, octave), [notes, octave]);
  const whites: number[] = [];
  const blacks: { midi: number; after: number }[] = [];
  for (let m = start; m < end; m += 1) {
    if (isBlackPc(m % 12)) {
      blacks.push({ midi: m, after: whites.length - 1 });
    } else {
      whites.push(m);
    }
  }
  const n = Math.max(1, whites.length);
  return (
    <div className="relative h-40 select-none">
      <div className="flex h-full overflow-hidden rounded-[var(--r)] border border-[var(--line)] bg-[#101010]">
        {whites.map((m) => {
          const on = pressed.has(m);
          return (
            <div
              key={m}
              className={`relative min-w-0 flex-1 border-r border-black/20 last:border-r-0 ${
                on ? "bg-[var(--ivory-press)]" : "bg-[var(--ivory)]"
              }`}
            >
              {m % 12 === 0 && (
                <span
                  className={`absolute bottom-1.5 left-1/2 -translate-x-1/2 text-[9px] font-medium ${
                    on ? "text-[#1a1208]/80" : "text-[#24180a]/45"
                  }`}
                >
                  C{Math.floor(m / 12) - 1}
                </span>
              )}
              {on && m % 12 !== 0 && (
                <span className="absolute bottom-1.5 left-1/2 -translate-x-1/2 text-[9px] font-semibold text-[#1a1208]/80">
                  {NOTE_NAMES[m % 12]}
                </span>
              )}
            </div>
          );
        })}
      </div>
      {blacks.map(({ midi, after }) => {
        if (after < 0) return null;
        const on = pressed.has(midi);
        return (
          <div
            key={midi}
            className={`absolute top-0 z-10 rounded-b-[2px] border border-black/50 ${
              on ? "bg-[var(--ivory-press)]" : "bg-[#161616]"
            }`}
            style={{
              left: `${((after + 1) / n) * 100}%`,
              width: `${(100 / n) * 0.55}%`,
              height: "62%",
              transform: "translateX(-50%)",
            }}
          />
        );
      })}
    </div>
  );
}

function StatusLed({ status }: { status: HostStatus | null }) {
  const ok = !!status?.surge.found && !!status.audio_ok;
  return (
    <div className="hw-led" data-ok={ok ? "true" : undefined}>
      <i />
      {ok ? "Live" : "Silent"}
    </div>
  );
}

function MiniKeys() {
  const whites = [60, 62, 64, 65, 67, 69, 71, 72];
  return (
    <div className="flex gap-1">
      {whites.map((key, i) => (
        <button
          key={key}
          type="button"
          className="hw-key"
          onPointerDown={(e) => {
            e.currentTarget.setPointerCapture(e.pointerId);
            invoke("note_on", { key, vel: 100 });
          }}
          onPointerUp={() => invoke("note_off", { key })}
        >
          {["C", "D", "E", "F", "G", "A", "B", "C"][i]}
        </button>
      ))}
    </div>
  );
}

const NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

function midiToName(midi: number) {
  return `${NOTE_NAMES[midi % 12]}${Math.floor(midi / 12) - 1}`;
}

function padRoman(degree: number, scale: "major" | "minor", secondary: Secondary) {
  const base = romanFor(degree, scale);
  if (secondary === "none") return base;
  if (secondary === "v") return `V/${base}`;
  if (secondary === "iv") return `IV/${base}`;
  return `vii°/${base}`;
}

function scaleIntervals(scale: "major" | "minor") {
  return scale === "major" ? [0, 2, 4, 5, 7, 9, 11] : [0, 2, 3, 5, 7, 8, 11];
}

function diatonicStack(scale: "major" | "minor", degree: number, n: number) {
  const ivs = scaleIntervals(scale);
  const d0 = Math.max(1, Math.min(7, degree)) - 1;
  const rootPc = ivs[d0];
  return Array.from({ length: n }, (_, i) => {
    const step = d0 + i * 2;
    const octaves = Math.floor(step / 7);
    const pc = ivs[step % 7];
    return pc + 12 * octaves - rootPc;
  });
}

function appliedStack(degreeRoot: number, secondary: Secondary, n: number) {
  const offset = secondary === "v" ? 7 : secondary === "iv" ? 5 : 11;
  const ivs =
    secondary === "v"
      ? [0, 4, 7, 10, 14, 17, 21]
      : secondary === "iv"
        ? [0, 4, 7, 11, 14, 17, 21]
        : [0, 3, 6, 9, 14, 17, 21];
  const root = Math.min(127, degreeRoot + offset);
  return ivs.slice(0, n).map((iv) => Math.min(127, root + iv));
}

function harmonicNotes(
  root: number,
  scale: "major" | "minor",
  type: ChordType,
  secondary: Secondary,
  octave: number,
  degree: number,
) {
  const n = TONE_COUNT[type];
  const scaleIv = scaleIntervals(scale);
  const rootMidi = (Math.max(1, Math.min(7, octave)) + 1) * 12 + root;
  const degreeRoot = Math.max(0, Math.min(127, rootMidi + scaleIv[degree - 1]));
  if (secondary === "none") {
    return diatonicStack(scale, degree, n).map((iv) => Math.min(127, degreeRoot + iv));
  }
  return appliedStack(degreeRoot, secondary, n);
}

function invertNotes(notes: number[], inversions: number) {
  const out = [...notes];
  for (let i = 0; i < inversions; i += 1) {
    const n = out.shift();
    if (n == null) break;
    out.push(Math.min(127, n + 12));
  }
  return out;
}

function chordMidi(
  root: number,
  scale: "major" | "minor",
  type: ChordType,
  inversion: number,
  secondary: Secondary,
  octave: number,
  degree: number,
) {
  const notes = harmonicNotes(root, scale, type, secondary, octave, degree);
  const maxInv = Math.max(0, notes.length - 1);
  return invertNotes(notes, Math.min(inversion, maxInv));
}

function chordSymbol(
  root: number,
  scale: "major" | "minor",
  type: ChordType,
  secondary: Secondary,
  degree: number,
) {
  const notes = harmonicNotes(root, scale, type, secondary, 4, degree);
  if (!notes.length) return "—";
  const name = NOTE_NAMES[notes[0] % 12];
  const pcs = new Set(notes.map((n) => ((n - notes[0]) % 12 + 12) % 12));
  const min3 = pcs.has(3);
  const dim5 = pcs.has(6);
  const maj7 = pcs.has(11);
  const dim7 = pcs.has(9) && dim5 && min3;
  const ext = type === "tri" ? "" : type;
  if (type === "tri") {
    if (min3 && dim5) return `${name}dim`;
    if (min3) return `${name}m`;
    return name;
  }
  if (min3 && dim5) {
    if (dim7) return `${name}dim${ext}`;
    return `${name}ø${ext}`;
  }
  if (maj7) {
    if (min3) return `${name}mM${ext}`;
    return `${name}maj${ext}`;
  }
  if (min3) return `${name}m${ext}`;
  return `${name}${ext}`;
}

function stringColor(index: number, count: number) {
  const t = count <= 1 ? 0.5 : index / (count - 1);
  const hue = 328 - t * 48;
  return `hsl(${hue} 78% 64%)`;
}

function stringCenterX(index: number, count: number) {
  return (index + 0.5) / Math.max(1, count);
}

function midiVibHz(midi: number) {
  const t = (Math.min(96, Math.max(36, midi)) - 36) / 60;
  return 7.5 + t * 15;
}

function stringDeflect(
  u: number,
  phase: number,
  age: number,
  amp: number,
  push: number,
  idle: number,
) {
  const standing = Math.sin(Math.PI * u);
  const h2 = Math.sin(2 * Math.PI * u) * 0.24 * Math.exp(-age * 2.2);
  const h3 = Math.sin(3 * Math.PI * u) * 0.09 * Math.exp(-age * 3.8);
  const trans = Math.exp(-age * 10);
  const peak = u <= 0.5 ? u / 0.5 : (1 - u) / 0.5;
  const osc =
    (1 - trans) *
      (standing * Math.sin(phase) +
        h2 * Math.sin(phase * 2.03) +
        h3 * Math.sin(phase * 3.07)) +
    trans * peak * Math.sin(phase);
  return amp * osc + (push + idle) * standing;
}

type StringVib = { amp: number; phase: number; ringing: boolean; age: number };

function StrumPlate({
  notes,
  sounding,
  armed,
  enabled,
}: {
  notes: number[];
  sounding: number[];
  armed: boolean;
  enabled: boolean;
}) {
  const plateRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const strummingRef = useRef(false);
  const [pick, setPick] = useState<number | null>(null);
  const enabledRef = useRef(enabled);
  const armedRef = useRef(armed);
  const notesRef = useRef(notes);
  const soundingRef = useRef(sounding);
  const pickRef = useRef<number | null>(null);
  const originRef = useRef<number | null>(null);
  const lastPickRef = useRef<number | null>(null);
  const vibRef = useRef<StringVib[]>([]);
  enabledRef.current = enabled;
  armedRef.current = armed;
  notesRef.current = notes;
  soundingRef.current = sounding;

  const beginSweep = (position: number, restart: boolean) => {
    if (restart || originRef.current == null) {
      originRef.current = position;
      lastPickRef.current = position;
    }
    pickRef.current = position;
    setPick(position);
  };

  const sendStrum = (clientX: number, el: HTMLDivElement, restart: boolean) => {
    if (!armedRef.current) return;
    const rect = el.getBoundingClientRect();
    const position = Math.min(
      1,
      Math.max(0, (clientX - rect.left) / Math.max(1, rect.width)),
    );
    beginSweep(position, restart);
    invoke("strum", { position, active: true, restart }).catch(() => undefined);
  };

  const endStrum = () => {
    if (!strummingRef.current) return;
    strummingRef.current = false;
    originRef.current = null;
    lastPickRef.current = null;
    pickRef.current = null;
    setPick(null);
    invoke("strum", { position: 0, active: false, restart: false }).catch(() => undefined);
  };

  useEffect(() => {
    if (!enabled) endStrum();
  }, [enabled]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listen<{ position: number; active: boolean }>(
          "strum-ui",
          (ev) => {
            if (!enabledRef.current || strummingRef.current) return;
            const { position, active } = ev.payload;
            if (!active) {
              originRef.current = null;
              lastPickRef.current = null;
              pickRef.current = null;
              setPick(null);
              return;
            }
            beginSweep(position, originRef.current == null);
          },
        );
      } catch {
        /* not in Tauri */
      }
    })();
    return () => {
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    const plate = plateRef.current;
    const canvas = canvasRef.current;
    if (!plate || !canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    let raf = 0;
    let lastT = performance.now();
    let cssW = plate.clientWidth;
    let cssH = plate.clientHeight;

    const fit = () => {
      const dpr = window.devicePixelRatio || 1;
      cssW = plate.clientWidth;
      cssH = plate.clientHeight;
      canvas.width = Math.max(1, Math.round(cssW * dpr));
      canvas.height = Math.max(1, Math.round(cssH * dpr));
      canvas.style.width = `${cssW}px`;
      canvas.style.height = `${cssH}px`;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(plate);

    const tick = (now: number) => {
      const dt = Math.min(0.05, (now - lastT) / 1000);
      lastT = now;
      const chord = notesRef.current;
      const n = chord.length;
      const vib = vibRef.current;
      while (vib.length < n) vib.push({ amp: 0, phase: 0, ringing: false, age: 8 });
      vib.length = n;

      const ringingNotes = soundingRef.current;
      const pickX = pickRef.current;
      const lastPick = lastPickRef.current;
      const pluckAt = (idx: number, side: number, strength: number) => {
        const st = vib[idx];
        st.phase = side * (Math.PI / 2);
        st.amp = Math.min(1, Math.max(st.amp, strength));
        st.age = 0;
      };

      ctx.clearRect(0, 0, cssW, cssH);
      const y0 = 10;
      const y1 = Math.max(y0 + 16, cssH - 20);
      const len = y1 - y0;
      const segments = 40;

      for (let i = 0; i < n; i += 1) {
        const midi = chord[i];
        const xN = stringCenterX(i, n);
        const x = xN * cssW;
        const st = vib[i];
        st.age = (st.age ?? 8) + dt;
        st.phase += midiVibHz(midi) * Math.PI * 2 * dt;

        if (pickX != null && lastPick != null) {
          const a = lastPick - xN;
          const b = pickX - xN;
          if (a * b < 0) {
            const delta = Math.abs(pickX - lastPick);
            const speed = delta / Math.max(dt, 0.001);
            pluckAt(i, a > 0 ? 1 : -1, Math.min(1, 0.22 + delta * 4.5 + speed * 0.06));
          }
        }

        const ringing = ringingNotes.includes(midi);
        st.ringing = ringing;
        st.amp *= Math.exp(-(ringing ? 0.55 : 2.6) * dt);

        const dist = pickX == null ? 0 : pickX - xN;
        const glow =
          pickX == null ? 0 : Math.max(0, 1 - Math.abs(dist) / 0.16);
        const t = n <= 1 ? 0.5 : i / (n - 1);
        const color = stringColor(i, n);
        const push = reduceMotion ? 0 : glow * len * 0.045 * Math.sign(dist || 1);
        const ampPx = reduceMotion ? 0 : st.amp * len * (0.1 - t * 0.035);
        const opacity = 0.38 + glow * 0.5 + st.amp * 0.28;
        const width = 2.15 - t * 0.85 + glow * 1.4 + st.amp * 0.6;

        ctx.beginPath();
        for (let s = 0; s <= segments; s += 1) {
          const u = s / segments;
          const y = y0 + u * len;
          const dx = stringDeflect(u, st.phase, st.age, ampPx, push, 0);
          if (s === 0) ctx.moveTo(x + dx, y);
          else ctx.lineTo(x + dx, y);
        }
        ctx.strokeStyle = color;
        ctx.globalAlpha = opacity;
        ctx.lineWidth = width;
        ctx.lineCap = "round";
        ctx.lineJoin = "round";
        ctx.shadowColor = color;
        ctx.shadowBlur = glow > 0.12 || st.amp > 0.12 ? 8 + glow * 14 + st.amp * 10 : 0;
        ctx.stroke();
        ctx.shadowBlur = 0;
        ctx.strokeStyle = "#fff8f0";
        ctx.globalAlpha = opacity * 0.35;
        ctx.lineWidth = Math.max(0.55, width * 0.32);
        ctx.stroke();
      }

      ctx.globalAlpha = 1;
      ctx.shadowBlur = 0;
      lastPickRef.current = pickX;
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  }, []);

  return (
    <div
      ref={plateRef}
      className={`relative h-40 touch-none select-none overflow-hidden rounded-[var(--r)] border border-[var(--line)] bg-[var(--face)] ${
        !enabled ? "opacity-45" : ""
      }`}
      style={{
        cursor: enabled ? "ew-resize" : "default",
      }}
      onPointerDown={(e) => {
        if (!enabled || !armedRef.current || e.button !== 0) return;
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        strummingRef.current = true;
        sendStrum(e.clientX, e.currentTarget, true);
      }}
      onPointerMove={(e) => {
        if (!enabled || !armedRef.current || !strummingRef.current || e.buttons === 0) return;
        sendStrum(e.clientX, e.currentTarget, false);
      }}
      onPointerUp={endStrum}
      onPointerCancel={endStrum}
      onLostPointerCapture={endStrum}
    >
      <canvas
        ref={canvasRef}
        className="pointer-events-none absolute inset-0 h-full w-full"
      />
      <div className="pointer-events-none absolute inset-0">
        {notes.map((midi, i) => {
          const color = stringColor(i, notes.length);
          const x = stringCenterX(i, notes.length);
          return (
            <span
              key={`${midi}-${i}`}
              className="absolute bottom-2 font-[family-name:var(--font-geist-mono)] text-[10px] font-medium"
              style={{
                left: `${x * 100}%`,
                transform: "translateX(-50%)",
                color,
                opacity: 0.55,
              }}
            >
              {midiToName(midi)}
            </span>
          );
        })}
      </div>
      {pick != null && (
        <div
          className="pointer-events-none absolute z-10"
          style={{
            left: `${pick * 100}%`,
            top: "50%",
            transform: "translate(-50%, -50%)",
          }}
        >
          <div className="h-8 w-px bg-[var(--ink)]/80" />
          <div className="absolute left-1/2 top-1/2 size-2 -translate-x-1/2 -translate-y-1/2 bg-[var(--cream)]" />
        </div>
      )}
      {!enabled ? (
        <p className="pointer-events-none absolute inset-x-3 top-2 z-10 text-[11px] leading-4 text-[var(--muted)]">
          Off — pads play chords
        </p>
      ) : !armed ? (
        <p className="pointer-events-none absolute inset-x-3 top-2 z-10 text-[11px] leading-4 text-[var(--muted)]">
          Arm a pad, then strum left or right
        </p>
      ) : null}
    </div>
  );
}
