"use client";

import { listen } from "@tauri-apps/api/event";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { HwKnob } from "./ui";

export type EnvParams = {
  delay: number;
  attack: number;
  hold: number;
  decay: number;
  sustain: number;
  release: number;
};

export const DEFAULT_ENV: EnvParams = {
  delay: 0,
  attack: 0.02,
  hold: 0,
  decay: 0.22,
  sustain: 0.85,
  release: 0.28,
};

const TIME_MAX = 32;
const TIME_MIN = 0.001;
const TICK = 0.5;
const VIEW_MIN = 2;
const VIEW_ZOOM_MIN = 0.25;
const GRAPH_W = 640;
const GRAPH_H = 220;

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

type NodeId = "delay" | "attack" | "hold" | "decay" | "release";
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
  pts: Record<NodeId | "start" | "sustain", Pt>;
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

function playheadPoint(pts: GraphGeom["pts"], play: Playhead | null): Pt | null {
  if (!play || play.stage <= 0) return null;
  const t = play.progress;
  switch (play.stage) {
    case 1:
      return lerp(pts.start, pts.delay, t);
    case 2:
      return lerp(pts.delay, pts.attack, t);
    case 3:
      return lerp(pts.attack, pts.hold, t);
    case 4:
      return lerp(pts.hold, pts.decay, t);
    case 5:
      return pts.decay;
    case 6:
      return lerp(pts.sustain, pts.release, t);
    default:
      return null;
  }
}

function traveledPath(pts: GraphGeom["pts"], play: Playhead, head: Pt): string {
  const parts = [`M ${pts.start.x} ${pts.start.y}`];
  const push = (p: Pt) => parts.push(`L ${p.x} ${p.y}`);
  if (play.stage > 1) push(pts.delay);
  if (play.stage > 2) push(pts.attack);
  if (play.stage > 3) push(pts.hold);
  if (play.stage > 4) push(pts.decay);
  if (play.stage > 5) push(pts.sustain);
  push(head);
  return parts.join(" ");
}

export function EnvelopePanel({
  params,
  onChange,
}: {
  params: EnvParams;
  onChange: (next: EnvParams) => void;
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
      <EnvGraph params={params} onChange={onChange} playhead={playhead} />
      <div className="hw-knobs env-knobs">
        <HwKnob
          label="Delay"
          display={formatTime(params.delay)}
          value={timeToNorm(params.delay)}
          min={0}
          max={1}
          step={0.01}
          onChange={(n) => set({ delay: normToTime(n) })}
        />
        <HwKnob
          label="Attack"
          display={formatTime(params.attack)}
          value={timeToNorm(params.attack)}
          min={0}
          max={1}
          step={0.01}
          onChange={(n) => set({ attack: normToTime(n) })}
        />
        <HwKnob
          label="Hold"
          display={formatTime(params.hold)}
          value={timeToNorm(params.hold)}
          min={0}
          max={1}
          step={0.01}
          onChange={(n) => set({ hold: normToTime(n) })}
        />
        <HwKnob
          label="Decay"
          display={formatTime(params.decay)}
          value={timeToNorm(params.decay)}
          min={0}
          max={1}
          step={0.01}
          onChange={(n) => set({ decay: normToTime(n) })}
        />
        <HwKnob
          label="Sustain"
          display={`${Math.round(params.sustain * 100)}`}
          value={params.sustain}
          min={0}
          max={1}
          step={0.01}
          onChange={(n) => set({ sustain: n })}
        />
        <HwKnob
          label="Release"
          display={formatTime(params.release)}
          value={timeToNorm(params.release)}
          min={0}
          max={1}
          step={0.01}
          onChange={(n) => set({ release: normToTime(n) })}
        />
      </div>
    </div>
  );
}

function EnvGraph({
  params,
  onChange,
  playhead,
}: {
  params: EnvParams;
  onChange: (next: EnvParams) => void;
  playhead: Playhead | null;
}) {
  const fit = useMemo(() => fitView(params), [params]);
  const [viewSec, setViewSec] = useState<number | null>(null);
  const totalView = viewSec ?? fit;
  const geom = useMemo(() => geometry(params, totalView), [params, totalView]);
  const svgRef = useRef<SVGSVGElement>(null);
  const viewRef = useRef(totalView);
  viewRef.current = totalView;
  const drag = useRef<{
    id: NodeId;
    x: number;
    y: number;
    params: EnvParams;
    total: number;
    innerW: number;
    innerH: number;
  } | null>(null);
  const { pts, w, h, padL, padR, padT, padB, total } = geom;
  const innerW = w - padL - padR;
  const d = [
    `M ${pts.start.x} ${pts.start.y}`,
    `L ${pts.delay.x} ${pts.delay.y}`,
    `L ${pts.attack.x} ${pts.attack.y}`,
    `L ${pts.hold.x} ${pts.hold.y}`,
    `L ${pts.decay.x} ${pts.decay.y}`,
    `L ${pts.sustain.x} ${pts.sustain.y}`,
    `L ${pts.release.x} ${pts.release.y}`,
  ].join(" ");
  const fill = `${d} L ${pts.release.x} ${h - padB} L ${pts.start.x} ${h - padB} Z`;
  const head = playheadPoint(pts, playhead);
  const live = playhead && head ? traveledPath(pts, playhead, head) : null;
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
    ["release", pts.release],
  ];
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

  const applyDrag = (clientX: number, clientY: number) => {
    const start = drag.current;
    if (!start) return;
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
      onPointerUp={() => {
        drag.current = null;
      }}
      onPointerCancel={() => {
        drag.current = null;
      }}
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
          className="env-node"
          onPointerDown={(e) => {
            e.preventDefault();
            e.stopPropagation();
            e.currentTarget.ownerSVGElement?.setPointerCapture(e.pointerId);
            drag.current = {
              id,
              x: e.clientX,
              y: e.clientY,
              params,
              total,
              innerW,
              innerH: h - padT - padB,
            };
          }}
        />
      ))}
    </svg>
  );
}
