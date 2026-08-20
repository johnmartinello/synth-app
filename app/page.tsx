"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { DEFAULT_ENV, EnvelopePanel, mergeEnv, type EnvParams } from "./envelope";
import { HwButton, HwDragValue, HwKnob, HwMenu, HwStomp, HwToggle, HwToggleGroup, Silk, type HwKnobMidi } from "./ui";

type FilterType = "lp" | "hp" | "bp" | "notch";

function parseFilterType(v: string | undefined): FilterType {
  if (v === "hp" || v === "bp" || v === "notch") return v;
  return "lp";
}

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
  midi_sink?: string;
  sustain: boolean;
  octave?: number;
  arp_enabled?: boolean;
  arp_rate_hz?: number;
  midi_clock?: boolean;
  filter_cutoff?: number;
  filter_type?: FilterType;
  filter_res?: number;
  roll_ms?: number;
  midi_learn?: string | null;
  midi_maps?: Record<string, string>;
  envelope?: EnvParams;
  looper?: LooperStatus;
  metronome?: MetronomeStatus;
};

type LooperStatus = {
  selected: number;
  modes: [number, number];
  muted: [boolean, boolean];
  has_phrase: [boolean, boolean];
  playhead: number;
  levels: [number, number];
  mult: number;
  max_mult: number;
  clock_sync: boolean;
  pending: boolean;
  count_in?: boolean;
  counting?: boolean;
};

type MetronomeStatus = {
  enabled: boolean;
  bpm: number;
  numerator: number;
  denominator: number;
  subdivision: number;
  volume: number;
  midi_sync: boolean;
};

const DEFAULT_METRO: MetronomeStatus = {
  enabled: false,
  bpm: 120,
  numerator: 4,
  denominator: 4,
  subdivision: 1,
  volume: 0.5,
  midi_sync: false,
};

const METERS: ReadonlyArray<readonly [string, string]> = [
  ["2/4", "2/4"],
  ["3/4", "3/4"],
  ["4/4", "4/4"],
  ["5/4", "5/4"],
  ["6/8", "6/8"],
  ["7/8", "7/8"],
];

const GRIDS: ReadonlyArray<readonly [string, string]> = [
  ["1", "1/4"],
  ["2", "1/8"],
  ["3", "1/8T"],
  ["4", "1/16"],
];

const DEFAULT_LOOPER: LooperStatus = {
  selected: 0,
  modes: [0, 0],
  muted: [false, false],
  has_phrase: [false, false],
  playhead: 0,
  levels: [1, 1],
  mult: 1,
  max_mult: 4,
  clock_sync: false,
  pending: false,
  count_in: false,
  counting: false,
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
type ChordType = "1" | "tri" | "7" | "9" | "11" | "13";
type Secondary = "none" | "v" | "iv" | "vii";
const CHORD_TYPES: ReadonlyArray<readonly [ChordType, string]> = [
  ["1", "1"],
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
  "1": 1,
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
  const [filterType, setFilterType] = useState<FilterType>("lp");
  const [filterRes, setFilterRes] = useState(0);
  const [rollMs, setRollMs] = useState(0);
  const [strumMode, setStrumMode] = useState(false);
  const [env, setEnv] = useState<EnvParams>(DEFAULT_ENV);
  const [looper, setLooper] = useState<LooperStatus>(DEFAULT_LOOPER);
  const [metro, setMetro] = useState<MetronomeStatus>(DEFAULT_METRO);
  const [engineNotes, setEngineNotes] = useState<number[] | null>(null);
  const [configOpen, setConfigOpen] = useState(false);
  const autoMidiRef = useRef(false);
  const rateGesture = useRef(false);
  const filterGesture = useRef(false);
  const filterResGesture = useRef(false);
  const rollGesture = useRef(false);
  const envGesture = useRef(false);
  const levelGesture = useRef<number | null>(null);
  const metroBpmGesture = useRef(false);
  const metroVolGesture = useRef(false);

  const piano = chordType === "1";

  const previewNotes = useMemo(() => {
    if (piano) {
      return engineNotes ?? [];
    }
    if (held.size > 0) {
      const all = [...held].flatMap((degree) =>
        chordMidi(root, scale, chordType, inversion, secondary, octave, degree),
      );
      return [...new Set(all)].sort((a, b) => a - b);
    }
    if (engineNotes != null && engineNotes.length > 0) return engineNotes;
    return [];
  }, [
    piano,
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
        if (next.filter_type) setFilterType(parseFilterType(next.filter_type));
        if (typeof next.filter_res === "number") setFilterRes(next.filter_res);
        if (typeof next.roll_ms === "number") setRollMs(next.roll_ms);
        if (next.envelope) setEnv(mergeEnv(next.envelope));
        if (next.looper) setLooper(next.looper);
        if (next.metronome) setMetro(next.metronome);
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
          if (next.filter_type) setFilterType(parseFilterType(next.filter_type));
          if (typeof next.filter_res === "number" && !filterResGesture.current) {
            setFilterRes(next.filter_res);
          }
          if (typeof next.roll_ms === "number" && !rollGesture.current) {
            setRollMs(next.roll_ms);
          }
          if (next.envelope && !envGesture.current) {
            setEnv(mergeEnv(next.envelope));
          }
          if (next.looper && levelGesture.current == null) {
            setLooper(next.looper);
          }
          if (next.metronome) {
            setMetro((prev) => ({
              ...next.metronome!,
              bpm: metroBpmGesture.current ? prev.bpm : next.metronome!.bpm,
              volume: metroVolGesture.current ? prev.volume : next.metronome!.volume,
            }));
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
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listen<LooperStatus>("looper-ui", (ev) => {
          const next = ev.payload;
          setLooper((prev) => {
            const g = levelGesture.current;
            if (g == null) return next;
            const levels: [number, number] = [...(next.levels ?? prev.levels)];
            levels[g] = prev.levels[g] ?? levels[g];
            return { ...next, levels };
          });
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

  const pushMetro = (next: MetronomeStatus) => {
    setMetro(next);
    invoke("set_metronome", {
      enabled: next.enabled,
      bpm: next.bpm,
      numerator: next.numerator,
      denominator: next.denominator,
      subdivision: next.subdivision,
      volume: next.volume,
    }).catch(() => undefined);
  };

  const bumpOctave = (next: number) => {
    setOctave(next);
    invoke("set_octave", { octave: next }).catch(() => undefined);
  };

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
    invoke("set_envelope", env).catch(() => undefined);
  }, [env]);

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

  const midiFor = useCallback(
    (paramId: string): HwKnobMidi => {
      const mapped = status?.midi_maps?.[paramId];
      const learning = status?.midi_learn === paramId;
      return {
        paramId,
        mapped,
        learning,
        onLearn: () => {
          if (learning) {
            invoke("cancel_midi_learn").catch(() => undefined);
          } else {
            invoke("start_midi_learn", { paramId }).catch(() => undefined);
          }
        },
        onClear: () => invoke("unmap_midi", { paramId }).catch(() => undefined),
      };
    },
    [status?.midi_learn, status?.midi_maps],
  );

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
      if (configOpen) return;
      const degree = digitDegree(e);
      if (degree != null && !e.repeat && !typingField(e.target)) {
        e.preventDefault();
        padDown(degree);
        return;
      }
      if (e.code === "Escape") {
        invoke("cancel_midi_learn").catch(() => undefined);
        return;
      }
      if (e.code !== "Space" || e.repeat || isTypingTarget(e.target)) return;
      e.preventDefault();
      invoke("set_sustain_hold", { enabled: true }).catch(() => undefined);
    };
    const up = (e: KeyboardEvent) => {
      if (configOpen) return;
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
  }, [padDown, padUp, configOpen]);

  const missing = status && !status.surge.found;

  return (
    <div className="flex min-h-screen flex-col px-5 py-4 md:px-7">
      <header className="mb-3 flex flex-wrap items-center justify-between gap-3">
        <h1 className="text-2xl font-semibold tracking-tight">Surge Chord</h1>
        <div className="flex items-center gap-2">
          {status?.editor_open && (
            <Silk>
              MIDI {status.midi_sink === "surge" ? "→ Surge" : "→ Chord"}
            </Silk>
          )}
          <HwButton
            className="hw-btn-icon"
            aria-label="Config"
            onClick={() => {
              setHeld(new Set());
              setArmed(null);
              setConfigOpen(true);
              refreshPorts();
            }}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" aria-hidden="true">
              <path
                fill="currentColor"
                d="M19.14 12.94c.04-.31.06-.63.06-.94s-.02-.63-.06-.94l2.03-1.58a.5.5 0 0 0 .12-.64l-1.92-3.32a.5.5 0 0 0-.6-.22l-2.39.96c-.5-.38-1.04-.7-1.63-.94l-.36-2.54a.5.5 0 0 0-.5-.42h-3.84a.5.5 0 0 0-.5.42l-.36 2.54c-.59.24-1.13.56-1.63.94l-2.39-.96a.5.5 0 0 0-.6.22L2.71 8.84a.5.5 0 0 0 .12.64l2.03 1.58c-.04.31-.06.63-.06.94s.02.63.06.94L2.83 13.94a.5.5 0 0 0-.12.64l1.92 3.32c.13.23.4.32.64.22l2.39-.96c.5.38 1.04.7 1.63.94l.36 2.54c.05.24.25.42.5.42h3.84c.24 0 .45-.18.5-.42l.36-2.54c.59-.24 1.13-.56 1.63-.94l2.39.96c.24.1.51 0 .64-.22l1.92-3.32a.5.5 0 0 0-.12-.64l-2.03-1.58ZM12 15.6A3.6 3.6 0 1 1 12 8.4a3.6 3.6 0 0 1 0 7.2Z"
              />
            </svg>
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

      <section className="mb-3 flex flex-wrap items-start gap-x-8 gap-y-3">
        <div className="flex min-w-0 flex-col gap-3">
          <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
            <HwToggleGroup
              label="Key"
              value={String(root)}
              options={ROOTS.map((n, i) => [String(i), n] as const)}
              disabledIds={piano ? ROOTS.map((_, i) => String(i)) : []}
              onChange={(v) => setRoot(Number(v))}
            />
            <HwToggleGroup
              label="Scale"
              value={scale}
              options={[
                ["major", "Maj"],
                ["minor", "Min"],
              ]}
              disabledIds={piano ? ["major", "minor"] : []}
              onChange={(v) => setScale(v as "major" | "minor")}
            />
          </div>
          <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
            <div className="hw-row">
              <Silk>Oct</Silk>
              <div className="hw-cluster">
                <HwButton
                  disabled={octave <= 1}
                  onClick={() => bumpOctave(Math.max(1, octave - 1))}
                >
                  −
                </HwButton>
                <HwButton className="min-w-8">{octave}</HwButton>
                <HwButton
                  disabled={octave >= 7}
                  onClick={() => bumpOctave(Math.min(7, octave + 1))}
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
                if (next === "1") {
                  setInversion(0);
                  setSecondary("none");
                  setStrumMode(false);
                } else {
                  setInversion((inv) => Math.min(inv, TONE_COUNT[next] - 1));
                }
              }}
            />
            <HwToggleGroup
              label="Inv"
              value={String(inversion)}
              options={INVERSIONS.map(([id, name]) => [String(id), name] as const)}
              disabledIds={
                piano
                  ? INVERSIONS.map(([id]) => String(id))
                  : chordType === "tri"
                    ? ["3"]
                    : []
              }
              onChange={(v) => setInversion(Number(v))}
            />
            <HwToggleGroup
              label="Sec"
              value={secondary}
              options={SECONDARIES}
              disabledIds={piano ? SECONDARIES.map(([id]) => id) : []}
              onChange={(v) => setSecondary(v as Secondary)}
            />
          </div>
        </div>
        <div className="flex min-w-0 flex-wrap items-center gap-x-6 gap-y-2">
          <Silk>Metronome{metro.midi_sync ? " · sync" : ""}</Silk>
          <HwToggle
            on={metro.enabled}
            onClick={() => pushMetro({ ...metro, enabled: !metro.enabled })}
          >
            On
          </HwToggle>
            <HwMenu
              className="hw-menu-compact"
              label="Meter"
            value={`${metro.numerator}/${metro.denominator}`}
            options={METERS}
            onChange={(v) => {
              const [n, d] = v.split("/").map(Number);
              pushMetro({
                ...metro,
                numerator: n || 4,
                denominator: d || 4,
              });
            }}
          />
            <HwMenu
              className="hw-menu-compact"
              label="Grid"
            value={String(metro.subdivision)}
            options={GRIDS}
            onChange={(v) => pushMetro({ ...metro, subdivision: Number(v) })}
          />
          <HwDragValue
            label="BPM"
            display={`${Math.round(metro.bpm)}${metro.midi_sync ? " sync" : ""}`}
            value={metro.bpm}
            min={40}
            max={240}
            step={1}
            disabled={metro.midi_sync}
            onChange={(v) => pushMetro({ ...metro, bpm: v })}
            onGestureChange={(on) => {
              metroBpmGesture.current = on;
            }}
          />
          <HwDragValue
            label="Level"
            display={`${Math.round(metro.volume * 100)}`}
            value={Math.round(metro.volume * 100)}
            min={0}
            max={100}
            step={1}
            onChange={(v) => pushMetro({ ...metro, volume: v / 100 })}
            onGestureChange={(on) => {
              metroVolGesture.current = on;
            }}
          />
        </div>
      </section>

      <section className="mb-4 grid gap-2 lg:grid-cols-[55fr_45fr] lg:items-stretch">
        <div className="grid grid-cols-7 gap-1.5">
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
                <span className="text-lg font-semibold leading-tight">
                  {chordSymbol(root, scale, chordType, secondary, d.n)}
                </span>
              </button>
            );
          })}
        </div>
        <EnvelopePanel
          params={env}
          onChange={setEnv}
          midi={midiFor}
          onGestureChange={(on) => {
            envGesture.current = on;
          }}
        />
      </section>

      <section className="mb-4 grid items-center gap-x-4 gap-y-2 lg:grid-cols-[minmax(0,1fr)_16rem]">
        <Silk className="max-lg:order-1">Keyboard</Silk>
        <div className="flex items-center justify-between gap-2 max-lg:order-3">
          <Silk>Strum</Silk>
          <HwToggle
            on={strumMode}
            disabled={piano}
            onClick={() => setStrumMode((v) => !v)}
          >
            On
          </HwToggle>
        </div>
        <div className="max-lg:order-2">
          <PianoPreview notes={previewNotes} octave={octave} playable={piano} />
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

      <div className="grid items-start gap-x-8 gap-y-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
      <section>
        <Silk className="mb-2">Arp</Silk>
        <div className="hw-inline">
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
          <HwMenu
            className="hw-menu-compact"
            label="Type"
            value={filterType}
            options={[
              ["lp", "Lowpass"],
              ["hp", "Highpass"],
              ["bp", "Bandpass"],
              ["notch", "Notch"],
            ]}
            onChange={(v) => {
              const mode = v as FilterType;
              setFilterType(mode);
              invoke("set_filter_type", { mode }).catch(() => undefined);
            }}
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
            midi={midiFor("arp_rate")}
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
            midi={midiFor("filter")}
          />
          <HwKnob
            label="Res"
            display={`${Math.round(filterRes * 100)}`}
            value={filterRes}
            min={0}
            max={1}
            step={0.01}
            onChange={(v) => {
              setFilterRes(v);
              invoke("set_filter_res", { value: v }).catch(() => undefined);
            }}
            onGestureChange={(on) => {
              filterResGesture.current = on;
            }}
            midi={midiFor("filter_res")}
          />
          <HwKnob
            label="Roll"
            display={`${Math.round(rollMs)} ms`}
            value={rollMs}
            min={0}
            max={500}
            step={1}
            disabled={
              piano ||
              (held.size === 0 ? TONE_COUNT[chordType] : heldNotes.length) <= 1
            }
            onChange={(v) => {
              setRollMs(v);
              invoke("set_roll", { ms: v }).catch(() => undefined);
            }}
            onGestureChange={(on) => {
              rollGesture.current = on;
            }}
            midi={midiFor("roll")}
          />
        </div>
        </div>
      </section>

      <section>
        <Silk className="mb-2">
          Loop{looper.clock_sync ? " · sync" : ""}
          {looper.counting ? " · count" : looper.pending ? " · wait" : ""}
        </Silk>
        <div className="hw-inline">
        <div>
        <div className="flex flex-wrap items-center gap-3">
          <HwToggleGroup
            label="Track"
            value={String(looper.selected)}
            options={[
              ["0", "1"],
              ["1", "2"],
            ]}
            onChange={(v) => {
              const track = Number(v);
              setLooper((prev) => ({ ...prev, selected: track }));
              invoke("looper_select", { track }).catch(() => undefined);
            }}
          />
          <HwToggle
            on={!!looper.count_in}
            onClick={() => {
              const on = !looper.count_in;
              setLooper((prev) => ({ ...prev, count_in: on }));
              invoke("looper_count_in", { on }).catch(() => undefined);
            }}
          >
            Count
          </HwToggle>
          <HwStomp
            mode={looper.counting ? "rec" : loopRecMode(looper.modes[looper.selected] ?? 0)}
            on={
              looper.counting ||
              looper.modes[looper.selected] === 1 ||
              looper.modes[looper.selected] === 3
            }
            onClick={() => invoke("looper_rec").catch(() => undefined)}
            midi={midiFor("looper_rec")}
          >
            Rec
          </HwStomp>
          <HwStomp
            mode={looper.modes[looper.selected] === 2 ? "play" : undefined}
            on={looper.modes[looper.selected] === 2}
            disabled={(looper.modes[looper.selected] ?? 0) === 0}
            onClick={() => invoke("looper_play").catch(() => undefined)}
            midi={midiFor("looper_play")}
          >
            Play
          </HwStomp>
          <HwStomp
            onClick={() => invoke("looper_stop").catch(() => undefined)}
            midi={midiFor("looper_stop")}
          >
            Stop
          </HwStomp>
          <HwStomp
            onClick={() => invoke("looper_undo").catch(() => undefined)}
            midi={midiFor("looper_undo")}
          >
            Undo
          </HwStomp>
          <HwButton
            disabled={(looper.modes[looper.selected] ?? 0) !== 4 && (looper.modes[looper.selected] ?? 0) !== 0}
            onClick={() => invoke("looper_clear").catch(() => undefined)}
          >
            Clear
          </HwButton>
        </div>
        <div className="mt-2 flex flex-wrap items-center gap-3">
          <HwToggle
            on={looper.muted[0]}
            onClick={() => {
              const on = !looper.muted[0];
              setLooper((prev) => ({ ...prev, muted: [on, prev.muted[1]] }));
              invoke("looper_mute", { track: 0, on }).catch(() => undefined);
            }}
          >
            Mute 1
          </HwToggle>
          <HwToggle
            on={looper.muted[1]}
            onClick={() => {
              const on = !looper.muted[1];
              setLooper((prev) => ({ ...prev, muted: [prev.muted[0], on] }));
              invoke("looper_mute", { track: 1, on }).catch(() => undefined);
            }}
          >
            Mute 2
          </HwToggle>
          <HwStomp
            on={looper.muted[looper.selected] ?? false}
            onClick={() => {
              const track = looper.selected;
              const on = !looper.muted[track];
              setLooper((prev) => {
                const muted: [boolean, boolean] = [...prev.muted];
                muted[track] = on;
                return { ...prev, muted };
              });
              invoke("looper_mute", { track, on }).catch(() => undefined);
            }}
            midi={midiFor("looper_mute")}
          >
            Mute
          </HwStomp>
          <HwToggleGroup
            label="T2"
            value={String(looper.mult)}
            options={[
              ["1", "1×"],
              ["2", "2×"],
              ["4", "4×"],
            ]}
            disabledIds={[
              ...(looper.max_mult < 2 ? ["2"] : []),
              ...(looper.max_mult < 4 ? ["4"] : []),
            ]}
            onChange={(v) => {
              const mult = Number(v);
              setLooper((prev) => ({ ...prev, mult }));
              invoke("looper_mult", { mult }).catch(() => undefined);
            }}
          />
        </div>
        </div>
        <div className="hw-knobs">
          <HwKnob
            label="Lvl 1"
            display={`${Math.round((looper.levels[0] ?? 1) * 100)}`}
            value={looper.levels[0] ?? 1}
            min={0}
            max={1}
            step={0.01}
            onChange={(v) => {
              setLooper((prev) => ({ ...prev, levels: [v, prev.levels[1] ?? 1] }));
              invoke("looper_level", { track: 0, value: v }).catch(() => undefined);
            }}
            onGestureChange={(on) => {
              levelGesture.current = on ? 0 : null;
            }}
            midi={midiFor("looper_level_1")}
          />
          <HwKnob
            label="Lvl 2"
            display={`${Math.round((looper.levels[1] ?? 1) * 100)}`}
            value={looper.levels[1] ?? 1}
            min={0}
            max={1}
            step={0.01}
            onChange={(v) => {
              setLooper((prev) => ({ ...prev, levels: [prev.levels[0] ?? 1, v] }));
              invoke("looper_level", { track: 1, value: v }).catch(() => undefined);
            }}
            onGestureChange={(on) => {
              levelGesture.current = on ? 1 : null;
            }}
            midi={midiFor("looper_level_2")}
          />
        </div>
        </div>
        <div
          className="loop-playhead"
          data-rec={looper.modes[looper.selected] === 1 ? "true" : undefined}
          data-pending={looper.pending || looper.counting ? "true" : undefined}
          style={{ "--pos": String(looper.playhead) } as CSSProperties}
          aria-label="Loop playhead"
        >
          <i />
        </div>
      </section>
      </div>

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

function loopRecMode(mode: number): "rec" | "dub" | undefined {
  if (mode === 1) return "rec";
  if (mode === 3) return "dub";
  return undefined;
}

function unique(values: string[]) {
  return [...new Set(values)];
}

function folderLabel(category: string) {
  return category || "Root";
}

function splitCategory(category: string) {
  const parts = category.split("/").filter(Boolean);
  if (parts.length <= 1) return { pack: "", leaf: folderLabel(category) };
  return { pack: parts[0], leaf: parts.slice(1).join("/") };
}

function packKey(bank: string, pack: string) {
  return `${bank}::${pack}`;
}

function normalizePatchPath(path: string) {
  return path.replace(/\\/g, "/").toLowerCase();
}

function samePatchPath(a: string | null | undefined, b: string | null | undefined) {
  if (!a || !b) return false;
  return a === b || normalizePatchPath(a) === normalizePatchPath(b);
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
  const [openPacks, setOpenPacks] = useState<Set<string>>(() => new Set());
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
    () => presets.find((p) => samePatchPath(p.path, currentPath)) ?? null,
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
    return banks.map((b) => {
      const categories = unique(
        presets.filter((p) => p.bank === b).map((p) => p.category),
      ).sort((a, c) => folderLabel(a).localeCompare(folderLabel(c)));
      const roots: { category: string; leaf: string }[] = [];
      const packMap = new Map<string, { category: string; leaf: string }[]>();
      for (const cat of categories) {
        const { pack, leaf } = splitCategory(cat);
        const item = { category: cat, leaf };
        if (!pack) {
          roots.push(item);
          continue;
        }
        const list = packMap.get(pack) ?? [];
        list.push(item);
        packMap.set(pack, list);
      }
      const packs = [...packMap.entries()]
        .sort(([a], [c]) => a.localeCompare(c))
        .map(([name, types]) => ({ name, types }));
      return { bank: b, roots, packs };
    });
  }, [presets]);

  useEffect(() => {
    const { pack } = splitCategory(category);
    if (!pack) return;
    const key = packKey(bank, pack);
    setOpenPacks((prev) => {
      if (prev.has(key)) return prev;
      const next = new Set(prev);
      next.add(key);
      return next;
    });
  }, [bank, category]);

  const selectCategory = (nextBank: string, nextCategory: string) => {
    setBank(nextBank);
    setCategory(nextCategory);
    setQuery("");
  };

  const togglePack = (
    bankName: string,
    pack: { name: string; types: { category: string; leaf: string }[] },
  ) => {
    if (pack.types.length === 1) {
      selectCategory(bankName, pack.types[0].category);
      return;
    }
    const key = packKey(bankName, pack.name);
    setOpenPacks((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

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
    setOpen(false);
    setLoading(true);
    invoke("load_preset", { path })
      .catch(() => undefined)
      .finally(() => setLoading(false));
  };

  const step = (dir: -1 | 1) => {
    if (!filtered.length) return;
    const idx = filtered.findIndex((p) => samePatchPath(p.path, currentPath));
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
                {group.roots.length > 0 && (
                  <div className="patch-tree-types">
                    {group.roots.map((t) => (
                      <button
                        key={`${group.bank}/${t.category}`}
                        type="button"
                        className="patch-chip"
                        data-on={
                          !query && bank === group.bank && category === t.category
                            ? "true"
                            : undefined
                        }
                        onClick={() => selectCategory(group.bank, t.category)}
                      >
                        {t.leaf}
                      </button>
                    ))}
                  </div>
                )}
                {group.packs.map((pack) => {
                  const key = packKey(group.bank, pack.name);
                  const expanded = pack.types.length > 1 && openPacks.has(key);
                  const packOn =
                    !query &&
                    bank === group.bank &&
                    splitCategory(category).pack === pack.name;
                  return (
                    <div key={key} className="patch-tree-pack">
                      <button
                        type="button"
                        className="patch-tree-pack-btn"
                        data-on={packOn && pack.types.length === 1 ? "true" : undefined}
                        data-active={packOn ? "true" : undefined}
                        data-open={expanded ? "true" : undefined}
                        onClick={() => togglePack(group.bank, pack)}
                      >
                        {pack.types.length > 1 && (
                          <span className="patch-tree-chevron" aria-hidden="true">
                            {expanded ? "▾" : "▸"}
                          </span>
                        )}
                        {pack.name}
                      </button>
                      {expanded && (
                        <div className="patch-tree-types">
                          {pack.types.map((t) => (
                            <button
                              key={t.category}
                              type="button"
                              className="patch-chip"
                              data-on={
                                !query &&
                                bank === group.bank &&
                                category === t.category
                                  ? "true"
                                  : undefined
                              }
                              onClick={() => selectCategory(group.bank, t.category)}
                            >
                              {t.leaf}
                            </button>
                          ))}
                        </div>
                      )}
                    </div>
                  );
                })}
              </li>
            ))}
          </ul>
          <ul className="patch-list" data-search={query.trim() ? "true" : undefined}>
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
                    data-on={samePatchPath(p.path, currentPath) ? "true" : undefined}
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
    invoke("set_performance_input", { enabled: false }).catch(() => undefined);
    invoke("cancel_midi_learn").catch(() => undefined);
    const onKey = (e: KeyboardEvent) => {
      if (e.code === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      invoke("set_performance_input", { enabled: true }).catch(() => undefined);
    };
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
            The first hardware input is opened on launch. With Type tri / 7 /
            9 / 11 / 13, white keys in any octave = chords 1–7; Oct− / Oct+
            on a controller shifts the chord octave; other notes are
            chromatic. Type 1 is piano: every key plays as written, Oct
            transposes, and arp walks the held notes. Key / Scale / Inv /
            Sec are off. The Env
            sustain knob is the hold level; Space and the sustain pedal
            (CC64) keep the gate open. In strum mode, the mod touchbar
            (CC1) sweeps the strings. Arp button (CC102) toggles arp. Rate
            follows MIDI clock when present. Click MIDI next to a knob,
            then move a controller knob to assign it (right-click to clear).
            Filter is a 12 dB/oct SVF on the output (LP / HP / BP / notch);
            Res emphasizes the cutoff. Roll staggers block chords.
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

function PianoPreview({
  notes,
  octave,
  playable = false,
}: {
  notes: number[];
  octave: number;
  playable?: boolean;
}) {
  const [local, setLocal] = useState<number[]>([]);
  const pointers = useRef(new Map<number, number>());
  const pressed = useMemo(() => new Set([...notes, ...local]), [notes, local]);
  const { start, end } = useMemo(() => pianoRange(notes, octave), [notes, octave]);

  useEffect(() => {
    if (playable) return;
    const held = [...pointers.current.values()];
    pointers.current.clear();
    setLocal([]);
    for (const key of held) {
      invoke("note_off", { key }).catch(() => undefined);
    }
  }, [playable]);

  useEffect(() => {
    return () => {
      for (const key of pointers.current.values()) {
        invoke("note_off", { key }).catch(() => undefined);
      }
      pointers.current.clear();
    };
  }, []);

  const pressKey = (midi: number, pointerId: number, target: HTMLElement) => {
    if (!playable) return;
    if ([...pointers.current.values()].includes(midi)) return;
    pointers.current.set(pointerId, midi);
    setLocal((prev) => (prev.includes(midi) ? prev : [...prev, midi]));
    target.setPointerCapture?.(pointerId);
    invoke("note_on", { key: midi, vel: 100 }).catch(() => undefined);
  };
  const releasePointer = (pointerId: number) => {
    const midi = pointers.current.get(pointerId);
    if (midi == null) return;
    pointers.current.delete(pointerId);
    setLocal((prev) => prev.filter((n) => n !== midi));
    invoke("note_off", { key: midi }).catch(() => undefined);
  };

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
  const keyClass = playable ? "cursor-pointer touch-none" : "";
  return (
    <div className="relative h-40 select-none">
      <div className="flex h-full overflow-hidden rounded-[var(--r)] border border-[var(--line)] bg-[#101010]">
        {whites.map((m) => {
          const on = pressed.has(m);
          return (
            <div
              key={m}
              className={`relative min-w-0 flex-1 border-r border-black/20 last:border-r-0 ${keyClass} ${
                on ? "bg-[var(--ivory-press)]" : "bg-[var(--ivory)]"
              }`}
              onPointerDown={(e) => {
                e.preventDefault();
                pressKey(m, e.pointerId, e.currentTarget);
              }}
              onPointerUp={(e) => releasePointer(e.pointerId)}
              onPointerCancel={(e) => releasePointer(e.pointerId)}
            >
              {m % 12 === 0 && (
                <span
                  className={`pointer-events-none absolute bottom-1.5 left-1/2 -translate-x-1/2 text-[9px] font-medium ${
                    on ? "text-[#1a1208]/80" : "text-[#24180a]/45"
                  }`}
                >
                  C{Math.floor(m / 12) - 1}
                </span>
              )}
              {on && m % 12 !== 0 && (
                <span className="pointer-events-none absolute bottom-1.5 left-1/2 -translate-x-1/2 text-[9px] font-semibold text-[#1a1208]/80">
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
            className={`absolute top-0 z-10 rounded-b-[2px] border border-black/50 ${keyClass} ${
              on ? "bg-[var(--ivory-press)]" : "bg-[#161616]"
            }`}
            style={{
              left: `${((after + 1) / n) * 100}%`,
              width: `${(100 / n) * 0.55}%`,
              height: "62%",
              transform: "translateX(-50%)",
            }}
            onPointerDown={(e) => {
              e.preventDefault();
              e.stopPropagation();
              pressKey(midi, e.pointerId, e.currentTarget);
            }}
            onPointerUp={(e) => releasePointer(e.pointerId)}
            onPointerCancel={(e) => releasePointer(e.pointerId)}
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
  if (type === "1") return name;
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

function stringColor(_index: number, _count: number) {
  return "#f6f2e8";
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
        ctx.shadowColor = "#f6f2e8";
        ctx.shadowBlur = glow > 0.12 || st.amp > 0.12 ? 8 + glow * 14 + st.amp * 10 : 0;
        ctx.stroke();
        ctx.shadowBlur = 0;
        ctx.strokeStyle = "#fffef8";
        ctx.globalAlpha = opacity * 0.42;
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
              className="absolute bottom-2 font-[family-name:var(--font-departure-mono)] text-[10px] font-medium"
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
