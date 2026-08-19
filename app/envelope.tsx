"use client";

import { listen } from "@tauri-apps/api/event";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { HwKnob, type HwKnobMidi } from "./ui";

export type EnvParams = {
  delay: number;
  attack: number;
  hold: number;
  decay: number;
  sustain: number;
  release: number;
  attackCurve: number;
  decayCurve: number;
  releaseCurve: number;
};

export const DEFAULT_ENV: EnvParams = {
  delay: 0,
  attack: 0.02,
  hold: 0,
  decay: 0.22,
  sustain: 0.85,
  release: 0.28,
  attackCurve: 0,
  decayCurve: 0,
  releaseCurve: 0,
};

export function mergeEnv(p: Partial<EnvParams> | null | undefined): EnvParams {
  return { ...DEFAULT_ENV, ...p };
}

const TIME_MAX = 32;
const TIME_MIN = 0.001;
const TICK = 0.5;
const VIEW_MIN = 2;
const VIEW_ZOOM_MIN = 0.25;
const GRAPH_W = 640;
const GRAPH_H = 220;
const CURVE_MID_MIN = 0.001;
const CURVE_MID_MAX = 0.999;
const CURVE_SAMPLES = 20;
const AMP_EPS = 1e-4;
const TIME_EPS = 1e-4;

type Playhead = { stage: number; progress: number; level: number };

export function normToTime(n: number): number {
  const x = Math.min(1, Math.max(0, n));
  if (x <= 0) return 0;
  return TIME_MIN * (TIME_MAX / TIME_MIN) ** x;
}

export function timeToNorm(t: number): number {
  if (t <= 0) return 0;
  return (
    Math.log(Math.min(TIME_MAX, Math.max(TIME_MIN, t)) / TIME_MIN) /
    Math.log(TIME_MAX / TIME_MIN)
  );
}

export function formatTime(t: number): string {
  if (t < 0.0005) return "0 ms";
  if (t < 1) return `${Math.round(t * 1000)} ms`;
  return `${t < 10 ? t.toFixed(2) : t.toFixed(1)} s`;
}

/** Map normalized time through a monotonic power curve. `0` is linear. */
export function shapeProgress(t: number, curve: number): number {
  const x = Math.min(1, Math.max(0, t));
  if (x <= 0) return 0;
  if (x >= 1) return 1;
  if (Math.abs(curve) < 1e-6) return x;
  const c = Math.min(1, Math.max(-1, curve));
  const mid = Math.min(CURVE_MID_MAX, Math.max(CURVE_MID_MIN, 0.5 + 0.5 * c));
  const power = Math.log(mid) / Math.log(0.5);
  return x ** power;
}

/** Interpolate start → target. Positive curve raises the halfway amplitude. */
export function shapedLevel(start: number, target: number, t: number, curve: number): number {
  if (Math.abs(target - start) < 1e-9) return target;
  const signed = target >= start ? curve : -curve;
  return start + (target - start) * shapeProgress(t, signed);
}

export function curveFromMid(start: number, target: number, mid: number): number {
  const range = target - start;
  if (Math.abs(range) < AMP_EPS) return 0;
  const lo = Math.min(start, target);
  const hi = Math.max(start, target);
  const clamped = Math.min(hi, Math.max(lo, mid));
  const normalized = Math.min(
    CURVE_MID_MAX,
    Math.max(CURVE_MID_MIN, (clamped - start) / range),
  );
  const signed = 2 * normalized - 1;
  const curve = target >= start ? signed : -signed;
  return Math.min(1, Math.max(-1, curve));
}

type NodeId = "delay" | "attack" | "hold" | "decay" | "sustain" | "release";
type CurveId = "attackCurve" | "decayCurve" | "releaseCurve";
type Pt = { x: number; y: number };

type GraphGeom = {
  w: number;
  h: number;
  padL: number;
  padR: number;
  padT: number;
  padB: number;
  total: number;
  shelf: number;
  pts: Record<NodeId | "start", Pt>;
};

function fitView(p: EnvParams): number {
  const shaped = p.delay + p.attack + p.hold + p.decay + p.release;
  const needed = shaped + TICK;
  return Math.min(TIME_MAX, Math.max(VIEW_MIN, Math.ceil(needed / TICK) * TICK));
}

function geometry(p: EnvParams, viewSec: number, w = GRAPH_W, h = GRAPH_H): GraphGeom {
  const padL = 8;
  const padR = 8;
  const padT = 8;
  const padB = 18;
  const innerW = w - padL - padR;
  const innerH = h - padT - padB;
  const shelf = TICK;
  const total = Math.max(VIEW_ZOOM_MIN, viewSec);
  const xAt = (t: number) => padL + (t / total) * innerW;
  const yAt = (amp: number) => padT + (1 - amp) * innerH;
  const tDelay = p.delay;
  const tAttack = tDelay + p.attack;
  const tHold = tAttack + p.hold;
  const tDecay = tHold + p.decay;
  const tSustain = tDecay + shelf;
  return {
    w,
    h,
    padL,
    padR,
    padT,
    padB,
    total,
    shelf,
    pts: {
      start: { x: xAt(0), y: yAt(0) },
      delay: { x: xAt(tDelay), y: yAt(0) },
      attack: { x: xAt(tAttack), y: yAt(1) },
      hold: { x: xAt(tHold), y: yAt(1) },
      decay: { x: xAt(tDecay), y: yAt(p.sustain) },
      sustain: { x: xAt(tSustain), y: yAt(p.sustain) },
      release: { x: xAt(tSustain + p.release), y: yAt(0) },
    },
  };
}

function envelopeSpan(p: EnvParams): number {
  return p.delay + p.attack + p.hold + p.decay + TICK + p.release;
}

function snapView(sec: number): number {
  const step = tickStep(Math.max(VIEW_ZOOM_MIN, sec));
  return Math.min(TIME_MAX, Math.max(VIEW_ZOOM_MIN, Math.ceil(sec / step) * step));
}

function tickStep(total: number): number {
  if (total <= 0.5) return 0.05;
  if (total <= 1) return 0.1;
  if (total <= 2) return 0.25;
  return TICK;
}

function ticks(total: number): number[] {
  const step = tickStep(total);
  const out: number[] = [];
  for (let t = step; t < total - step * 0.04; t += step) {
    out.push(Number(t.toFixed(4)));
  }
  return out;
}

function formatTick(t: number): string {
  if (t < 1) return `${Math.round(t * 1000)}ms`;
  return `${t % 1 === 0 ? t.toFixed(0) : t.toFixed(1)}s`;
}

function lerp(a: Pt, b: Pt, t: number): Pt {
  return { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t };
}

function yOfAmp(geom: GraphGeom, amp: number): number {
  const innerH = geom.h - geom.padT - geom.padB;
  return geom.padT + (1 - amp) * innerH;
}

function segmentPts(
  x0: number,
  x1: number,
  startAmp: number,
  endAmp: number,
  curve: number,
  yOf: (amp: number) => number,
  fromT = 0,
  toT = 1,
): Pt[] {
  const span = Math.max(0, toT - fromT);
  const curved =
    Math.abs(curve) >= 1e-4 &&
    Math.abs(endAmp - startAmp) >= AMP_EPS &&
    Math.abs(x1 - x0) >= 0.5;
  const n = curved ? Math.max(2, Math.round(CURVE_SAMPLES * Math.max(span, 1e-6))) : 1;
  const pts: Pt[] = [];
  for (let i = 0; i <= n; i++) {
    const t = fromT + (span * i) / n;
    pts.push({
      x: x0 + (x1 - x0) * t,
      y: yOf(shapedLevel(startAmp, endAmp, t, curve)),
    });
  }
  return pts;
}

function appendPts(parts: string[], seg: Pt[]) {
  for (let i = 1; i < seg.length; i++) {
    parts.push(`L ${seg[i].x} ${seg[i].y}`);
  }
}

function envelopeStroke(pts: GraphGeom["pts"], p: EnvParams, yOf: (amp: number) => number): string {
  const parts = [`M ${pts.start.x} ${pts.start.y}`, `L ${pts.delay.x} ${pts.delay.y}`];
  appendPts(parts, segmentPts(pts.delay.x, pts.attack.x, 0, 1, p.attackCurve, yOf));
  parts.push(`L ${pts.hold.x} ${pts.hold.y}`);
  appendPts(parts, segmentPts(pts.hold.x, pts.decay.x, 1, p.sustain, p.decayCurve, yOf));
  parts.push(`L ${pts.sustain.x} ${pts.sustain.y}`);
  appendPts(parts, segmentPts(pts.sustain.x, pts.release.x, p.sustain, 0, p.releaseCurve, yOf));
  return parts.join(" ");
}

function playheadPoint(
  pts: GraphGeom["pts"],
  play: Playhead | null,
  p: EnvParams,
  yOf: (amp: number) => number,
): Pt | null {
  if (!play || play.stage <= 0) return null;
  const t = play.progress;
  switch (play.stage) {
    case 1:
      return lerp(pts.start, pts.delay, t);
    case 2:
      return {
        x: pts.delay.x + (pts.attack.x - pts.delay.x) * t,
        y: yOf(shapedLevel(0, 1, t, p.attackCurve)),
      };
    case 3:
      return lerp(pts.attack, pts.hold, t);
    case 4:
      return {
        x: pts.hold.x + (pts.decay.x - pts.hold.x) * t,
        y: yOf(shapedLevel(1, p.sustain, t, p.decayCurve)),
      };
    case 5:
      return pts.decay;
    case 6:
      return {
        x: pts.sustain.x + (pts.release.x - pts.sustain.x) * t,
        y: yOf(shapedLevel(p.sustain, 0, t, p.releaseCurve)),
      };
    default:
      return null;
  }
}

function traveledPath(
  pts: GraphGeom["pts"],
  play: Playhead,
  head: Pt,
  p: EnvParams,
  yOf: (amp: number) => number,
): string {
  const parts = [`M ${pts.start.x} ${pts.start.y}`];
  const push = (pt: Pt) => parts.push(`L ${pt.x} ${pt.y}`);
  if (play.stage === 1) {
    push(head);
    return parts.join(" ");
  }
  push(pts.delay);
  if (play.stage === 2) {
    appendPts(
      parts,
      segmentPts(pts.delay.x, pts.attack.x, 0, 1, p.attackCurve, yOf, 0, play.progress),
    );
    return parts.join(" ");
  }
  appendPts(parts, segmentPts(pts.delay.x, pts.attack.x, 0, 1, p.attackCurve, yOf));
  if (play.stage === 3) {
    push(head);
    return parts.join(" ");
  }
  push(pts.hold);
  if (play.stage === 4) {
    appendPts(
      parts,
      segmentPts(pts.hold.x, pts.decay.x, 1, p.sustain, p.decayCurve, yOf, 0, play.progress),
    );
    return parts.join(" ");
  }
  appendPts(parts, segmentPts(pts.hold.x, pts.decay.x, 1, p.sustain, p.decayCurve, yOf));
  if (play.stage === 5) {
    push(head);
    return parts.join(" ");
  }
  push(pts.decay);
  push(pts.sustain);
  appendPts(
    parts,
    segmentPts(pts.sustain.x, pts.release.x, p.sustain, 0, p.releaseCurve, yOf, 0, play.progress),
  );
  return parts.join(" ");
}

function curveHandle(
  id: CurveId,
  pts: GraphGeom["pts"],
  p: EnvParams,
  yOf: (amp: number) => number,
): Pt | null {
  if (id === "attackCurve") {
    if (p.attack < TIME_EPS) return null;
    return {
      x: (pts.delay.x + pts.attack.x) / 2,
      y: yOf(shapedLevel(0, 1, 0.5, p.attackCurve)),
    };
  }
  if (id === "decayCurve") {
    if (p.decay < TIME_EPS || Math.abs(1 - p.sustain) < AMP_EPS) return null;
    return {
      x: (pts.hold.x + pts.decay.x) / 2,
      y: yOf(shapedLevel(1, p.sustain, 0.5, p.decayCurve)),
    };
  }
  if (p.release < TIME_EPS || p.sustain < AMP_EPS) return null;
  return {
    x: (pts.sustain.x + pts.release.x) / 2,
    y: yOf(shapedLevel(p.sustain, 0, 0.5, p.releaseCurve)),
  };
}

function clientToAmp(
  svg: SVGSVGElement,
  clientY: number,
  h: number,
  padT: number,
  padB: number,
): number {
  const rect = svg.getBoundingClientRect();
  const y = ((clientY - rect.top) / Math.max(1, rect.height)) * h;
  const innerH = h - padT - padB;
  return Math.min(1, Math.max(0, 1 - (y - padT) / innerH));
}

export function EnvelopePanel({
  params,
  onChange,
  midi,
  onGestureChange,
}: {
  params: EnvParams;
  onChange: (next: EnvParams) => void;
  midi?: (id: string) => HwKnobMidi;
  onGestureChange?: (dragging: boolean) => void;
}) {
  const [playhead, setPlayhead] = useState<Playhead | null>(null);
  const set = (patch: Partial<EnvParams>) => onChange({ ...params, ...patch });

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listen<Playhead>("env-ui", (ev) => {
          const next = ev.payload;
          setPlayhead(next.stage === 0 ? null : next);
        });
      } catch {
        /* web preview */
      }
    })();
    return () => {
      unlisten?.();
    };
  }, []);

  return (
    <div className="env-panel">
      <EnvGraph
        params={params}
        onChange={onChange}
        playhead={playhead}
        onGestureChange={onGestureChange}
      />
      <div className="hw-knobs env-knobs">
        <HwKnob
          label="Delay"
          display={formatTime(params.delay)}
          value={timeToNorm(params.delay)}
          min={0}
          max={1}
          step={0.01}
          onChange={(n) => set({ delay: normToTime(n) })}
          onGestureChange={onGestureChange}
          midi={midi?.("env_delay")}
        />
        <HwKnob
          label="Attack"
          display={formatTime(params.attack)}
          value={timeToNorm(params.attack)}
          min={0}
          max={1}
          step={0.01}
          onChange={(n) => set({ attack: normToTime(n) })}
          onGestureChange={onGestureChange}
          midi={midi?.("env_attack")}
        />
        <HwKnob
          label="Hold"
          display={formatTime(params.hold)}
          value={timeToNorm(params.hold)}
          min={0}
          max={1}
          step={0.01}
          onChange={(n) => set({ hold: normToTime(n) })}
          onGestureChange={onGestureChange}
          midi={midi?.("env_hold")}
        />
        <HwKnob
          label="Decay"
          display={formatTime(params.decay)}
          value={timeToNorm(params.decay)}
          min={0}
          max={1}
          step={0.01}
          onChange={(n) => set({ decay: normToTime(n) })}
          onGestureChange={onGestureChange}
          midi={midi?.("env_decay")}
        />
        <HwKnob
          label="Sustain"
          display={`${Math.round(params.sustain * 100)}`}
          value={params.sustain}
          min={0}
          max={1}
          step={0.01}
          onChange={(n) => set({ sustain: n })}
          onGestureChange={onGestureChange}
          midi={midi?.("env_sustain")}
        />
        <HwKnob
          label="Release"
          display={formatTime(params.release)}
          value={timeToNorm(params.release)}
          min={0}
          max={1}
          step={0.01}
          onChange={(n) => set({ release: normToTime(n) })}
          onGestureChange={onGestureChange}
          midi={midi?.("env_release")}
        />
      </div>
    </div>
  );
}

function EnvGraph({
  params,
  onChange,
  playhead,
  onGestureChange,
}: {
  params: EnvParams;
  onChange: (next: EnvParams) => void;
  playhead: Playhead | null;
  onGestureChange?: (dragging: boolean) => void;
}) {
  const fit = useMemo(() => fitView(params), [params]);
  const [viewSec, setViewSec] = useState<number | null>(null);
  const totalView = viewSec ?? fit;
  const geom = useMemo(() => geometry(params, totalView), [params, totalView]);
  const svgRef = useRef<SVGSVGElement>(null);
  const viewRef = useRef(totalView);
  useEffect(() => {
    viewRef.current = totalView;
  }, [totalView]);
  const drag = useRef<
    | {
        kind: "node";
        id: NodeId;
        x: number;
        y: number;
        params: EnvParams;
        total: number;
        innerW: number;
        innerH: number;
      }
    | { kind: "curve"; id: CurveId; params: EnvParams }
    | null
  >(null);
  const { pts, w, h, padL, padR, padT, padB, total } = geom;
  const innerW = w - padL - padR;
  const yOf = (amp: number) => yOfAmp(geom, amp);
  const d = envelopeStroke(pts, params, yOf);
  const fill = `${d} L ${pts.release.x} ${h - padB} L ${pts.start.x} ${h - padB} Z`;
  const head = playheadPoint(pts, playhead, params, yOf);
  const live = playhead && head ? traveledPath(pts, playhead, head, params, yOf) : null;
  const liveFill = live
    ? `${live} L ${head!.x} ${h - padB} L ${pts.start.x} ${h - padB} Z`
    : null;
  const gradId = useId().replace(/:/g, "");
  const fillGrad = `${gradId}-fill`;
  const strokeGrad = `${gradId}-stroke`;
  const nodes: Array<[NodeId, Pt]> = [
    ["delay", pts.delay],
    ["attack", pts.attack],
    ["hold", pts.hold],
    ["decay", pts.decay],
    ["sustain", pts.sustain],
    ["release", pts.release],
  ];
  const curves: Array<[CurveId, Pt]> = (
    ["attackCurve", "decayCurve", "releaseCurve"] as CurveId[]
  )
    .map((id) => [id, curveHandle(id, pts, params, yOf)] as const)
    .filter((entry): entry is [CurveId, Pt] => entry[1] !== null);
  const step = tickStep(total);
  const labelEvery = total > 16 ? 2 : total > 8 ? 1 : step;

  useEffect(() => {
    const el = svgRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const factor = Math.exp(e.deltaY * 0.0015);
      const next = Math.min(
        TIME_MAX,
        Math.max(VIEW_ZOOM_MIN, viewRef.current * factor),
      );
      setViewSec(next);
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  const endDrag = () => {
    if (!drag.current) return;
    drag.current = null;
    onGestureChange?.(false);
  };

  const applyDrag = (clientX: number, clientY: number) => {
    const start = drag.current;
    if (!start) return;
    if (start.kind === "curve") {
      const svg = svgRef.current;
      if (!svg) return;
      const amp = clientToAmp(svg, clientY, h, padT, padB);
      const next = { ...start.params };
      if (start.id === "attackCurve") next.attackCurve = curveFromMid(0, 1, amp);
      if (start.id === "decayCurve") next.decayCurve = curveFromMid(1, start.params.sustain, amp);
      if (start.id === "releaseCurve") next.releaseCurve = curveFromMid(start.params.sustain, 0, amp);
      onChange(next);
      return;
    }
    const dt = ((clientX - start.x) / start.innerW) * start.total;
    const damp = -((clientY - start.y) / start.innerH);
    const next = { ...start.params };
    const clampT = (v: number) => Math.min(TIME_MAX, Math.max(0, v));
    if (start.id === "delay") next.delay = clampT(start.params.delay + dt);
    if (start.id === "attack") next.attack = clampT(start.params.attack + dt);
    if (start.id === "hold") next.hold = clampT(start.params.hold + dt);
    if (start.id === "decay") {
      next.decay = clampT(start.params.decay + dt);
      next.sustain = Math.min(1, Math.max(0, start.params.sustain + damp));
    }
    if (start.id === "sustain") {
      next.sustain = Math.min(1, Math.max(0, start.params.sustain + damp));
    }
    if (start.id === "release") next.release = clampT(start.params.release + dt);
    onChange(next);
    const span = envelopeSpan(next);
    if (span > viewRef.current) {
      setViewSec(snapView(span));
    }
  };

  return (
    <svg
      ref={svgRef}
      className="env-graph"
      viewBox={`0 0 ${w} ${h}`}
      preserveAspectRatio="none"
      role="img"
      aria-label="DAHDSR envelope"
      onDoubleClick={() => setViewSec(null)}
      onPointerMove={(e) => applyDrag(e.clientX, e.clientY)}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onLostPointerCapture={endDrag}
    >
      {ticks(total).map((t) => {
        const x = padL + (t / total) * innerW;
        const labeled = Math.abs((t / labelEvery) % 1) < 0.001 || Math.abs(t % labelEvery) < 0.001;
        return (
          <g key={t}>
            <line x1={x} x2={x} y1={padT} y2={h - padB} className="env-grid" />
            {labeled && (
              <text x={x} y={h - 4} className="env-tick">
                {formatTick(t)}
              </text>
            )}
          </g>
        );
      })}
      {head && (
        <defs>
          <linearGradient
            id={fillGrad}
            gradientUnits="userSpaceOnUse"
            x1={pts.start.x}
            y1={0}
            x2={head.x}
            y2={0}
          >
            <stop offset="0%" stopColor="var(--ivory)" stopOpacity="0" />
            <stop offset="55%" stopColor="var(--ivory)" stopOpacity="0.18" />
            <stop offset="100%" stopColor="var(--ivory)" stopOpacity="0.72" />
          </linearGradient>
          <linearGradient
            id={strokeGrad}
            gradientUnits="userSpaceOnUse"
            x1={pts.start.x}
            y1={0}
            x2={head.x}
            y2={0}
          >
            <stop offset="0%" stopColor="var(--ivory)" stopOpacity="0.12" />
            <stop offset="65%" stopColor="var(--ivory)" stopOpacity="0.55" />
            <stop offset="100%" stopColor="var(--ivory)" stopOpacity="1" />
          </linearGradient>
        </defs>
      )}
      <path d={fill} className="env-fill" />
      {liveFill && (
        <path d={liveFill} className="env-fill-live" style={{ fill: `url(#${fillGrad})` }} />
      )}
      <path d={d} className="env-stroke" />
      {live && (
        <path
          d={live}
          className="env-stroke-live"
          style={{ stroke: `url(#${strokeGrad})` }}
        />
      )}
      {head && (
        <>
          <line
            x1={head.x}
            x2={head.x}
            y1={padT}
            y2={h - padB}
            className="env-playhead-line"
          />
          <circle cx={head.x} cy={head.y} r={6} className="env-playhead" />
        </>
      )}
      {nodes.map(([id, p]) => (
        <circle
          key={id}
          cx={p.x}
          cy={p.y}
          r={5}
          className={id === "sustain" ? "env-node env-node-y" : "env-node"}
          onPointerDown={(e) => {
            e.preventDefault();
            e.stopPropagation();
            e.currentTarget.ownerSVGElement?.setPointerCapture(e.pointerId);
            drag.current = {
              kind: "node",
              id,
              x: e.clientX,
              y: e.clientY,
              params,
              total,
              innerW,
              innerH: h - padT - padB,
            };
            onGestureChange?.(true);
          }}
        />
      ))}
      {curves.map(([id, p]) => (
        <rect
          key={id}
          x={p.x - 3.5}
          y={p.y - 3.5}
          width={7}
          height={7}
          transform={`rotate(45 ${p.x} ${p.y})`}
          className="env-curve"
          onPointerDown={(e) => {
            e.preventDefault();
            e.stopPropagation();
            e.currentTarget.ownerSVGElement?.setPointerCapture(e.pointerId);
            drag.current = { kind: "curve", id, params };
            onGestureChange?.(true);
          }}
        />
      ))}
    </svg>
  );
}
