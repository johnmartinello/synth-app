//! Sample-accurate metronome / musical clock. Lives on the audio thread.

use serde::{Deserialize, Serialize};

pub const BPM_MIN: f32 = 40.0;
pub const BPM_MAX: f32 = 240.0;
pub const MAX_GRID_EVENTS: usize = 32;

const ACCENT_HZ: f32 = 1200.0;
const BEAT_HZ: f32 = 800.0;
const SUB_HZ: f32 = 600.0;
const ACCENT_AMP: f32 = 0.35;
const BEAT_AMP: f32 = 0.22;
const SUB_AMP: f32 = 0.12;

/// Copy-sized command from the host thread onto the audio thread.
#[derive(Debug, Clone, Copy)]
pub enum MetronomeEvent {
    SetEnabled {
        on: bool,
    },
    SetBpm {
        bpm: f32,
    },
    SetMeter {
        numerator: u8,
        denominator: u8,
    },
    SetSubdivision {
        sub: u8,
    },
    SetVolume {
        norm: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridKind {
    Bar,
    Subdivision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subdivision {
    Quarter = 1,
    Eighth = 2,
    EighthTriplet = 3,
    Sixteenth = 4,
}

impl Subdivision {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Quarter,
            2 => Self::Eighth,
            3 => Self::EighthTriplet,
            _ => Self::Sixteenth,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    fn per_quarter(self) -> f64 {
        match self {
            Self::Quarter => 1.0,
            Self::Eighth => 2.0,
            Self::EighthTriplet => 3.0,
            Self::Sixteenth => 4.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MetronomeStatus {
    pub enabled: bool,
    pub bpm: f32,
    pub numerator: u8,
    pub denominator: u8,
    pub subdivision: u8,
    pub volume: f32,
    pub midi_sync: bool,
}

impl Default for MetronomeStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            bpm: 120.0,
            numerator: 4,
            denominator: 4,
            subdivision: 1,
            volume: 0.5,
            midi_sync: false,
        }
    }
}

pub fn sanitize_bpm(bpm: f32) -> f32 {
    bpm.round().clamp(BPM_MIN, BPM_MAX)
}

pub fn sanitize_numerator(n: u8) -> u8 {
    n.clamp(1, 16)
}

pub fn sanitize_denominator(d: u8) -> u8 {
    match d {
        2 | 4 | 8 | 16 => d,
        _ => 4,
    }
}

pub fn sanitize_subdivision(s: u8) -> u8 {
    Subdivision::from_u8(s).as_u8()
}

pub fn bar_frames(sample_rate: f64, bpm: f32, numerator: u8, denominator: u8) -> usize {
    let sr = sample_rate.max(1.0);
    let bpm = sanitize_bpm(bpm) as f64;
    let num = sanitize_numerator(numerator) as f64;
    let den = sanitize_denominator(denominator) as f64;
    let quarters = num * 4.0 / den;
    let seconds = (60.0 / bpm) * quarters;
    (seconds * sr).round().max(1.0) as usize
}

pub struct Metronome {
    sample_rate: f64,
    enabled: bool,
    bpm: f32,
    numerator: u8,
    denominator: u8,
    subdivision: Subdivision,
    volume: f32,
    phase: f64,
    click_env: f32,
    click_phase: f32,
    click_inc: f32,
    click_amp: f32,
    click_coeff: f32,
    count_in_clicks: bool,
}

impl Metronome {
    pub fn new(sample_rate: f64) -> Self {
        let sr = sample_rate.max(1.0);
        let click_coeff = 1.0 - (-1.0 / (0.03 * sr as f32).max(1.0e-4)).exp();
        Self {
            sample_rate: sr,
            enabled: false,
            bpm: 120.0,
            numerator: 4,
            denominator: 4,
            subdivision: Subdivision::Quarter,
            volume: 0.5,
            phase: 0.0,
            click_env: 0.0,
            click_phase: 0.0,
            click_inc: 0.0,
            click_amp: 0.0,
            click_coeff,
            count_in_clicks: false,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn clocking(&self) -> bool {
        self.enabled || self.count_in_clicks
    }

    pub fn start_count_in_clicks(&mut self) {
        if self.enabled {
            return;
        }
        if !self.count_in_clicks {
            self.count_in_clicks = true;
            self.phase = 0.0;
        }
    }

    pub fn stop_count_in_clicks(&mut self) {
        self.count_in_clicks = false;
    }

    pub fn bar_frames(&self) -> usize {
        bar_frames(self.sample_rate, self.bpm, self.numerator, self.denominator)
    }

    pub fn handle(&mut self, ev: MetronomeEvent) {
        match ev {
            MetronomeEvent::SetEnabled { on } => {
                if on && !self.enabled {
                    self.phase = 0.0;
                }
                self.enabled = on;
            }
            MetronomeEvent::SetBpm { bpm } => {
                let frac = self.bar_frac();
                self.bpm = sanitize_bpm(bpm);
                self.phase = frac * self.samples_per_bar();
            }
            MetronomeEvent::SetMeter {
                numerator,
                denominator,
            } => {
                let frac = self.bar_frac();
                self.numerator = sanitize_numerator(numerator);
                self.denominator = sanitize_denominator(denominator);
                self.phase = frac * self.samples_per_bar();
            }
            MetronomeEvent::SetSubdivision { sub } => {
                self.subdivision = Subdivision::from_u8(sub);
            }
            MetronomeEvent::SetVolume { norm } => {
                self.volume = norm.clamp(0.0, 1.0);
            }
        }
    }

    pub fn peek_grid_events(
        &self,
        n_frames: usize,
        out: &mut [(usize, GridKind); MAX_GRID_EVENTS],
    ) -> usize {
        if !self.clocking() || n_frames == 0 {
            return 0;
        }
        let bar = self.samples_per_bar();
        let sub = self.samples_per_sub();
        if bar <= 0.0 || sub <= 0.0 {
            return 0;
        }
        let mut phase = self.phase;
        let mut n = 0usize;
        for i in 0..n_frames {
            let on_bar = on_period(phase, bar);
            let on_sub = on_period(phase, sub);
            if on_bar {
                if n < MAX_GRID_EVENTS {
                    out[n] = (i, GridKind::Bar);
                    n += 1;
                }
            } else if on_sub && n < MAX_GRID_EVENTS {
                out[n] = (i, GridKind::Subdivision);
                n += 1;
            }
            phase += 1.0;
            if phase >= bar {
                phase -= bar;
            }
        }
        n
    }

    pub fn mix_and_advance(&mut self, frames: &mut [f32], channels: usize) {
        let step = channels.max(1);
        if !self.clocking() || frames.len() < step {
            self.advance_phase(frames.len() / step);
            return;
        }
        let bar = self.samples_per_bar();
        let sub = self.samples_per_sub();
        let beat = self.samples_per_beat();
        if bar <= 0.0 {
            return;
        }
        let two_pi = std::f32::consts::TAU;
        let sr = self.sample_rate as f32;
        for frame in frames.chunks_exact_mut(step) {
            if on_period(self.phase, bar) {
                self.trigger_click(ACCENT_HZ, ACCENT_AMP, sr);
            } else if on_period(self.phase, beat) {
                self.trigger_click(BEAT_HZ, BEAT_AMP, sr);
            } else if on_period(self.phase, sub) {
                self.trigger_click(SUB_HZ, SUB_AMP, sr);
            }

            self.click_env += self.click_coeff * (0.0 - self.click_env);
            if self.click_env < 1.0e-4 {
                self.click_env = 0.0;
            }
            if self.click_env > 0.0 {
                let s = (self.click_phase).sin() * self.click_env * self.click_amp * self.volume;
                self.click_phase += self.click_inc;
                if self.click_phase > two_pi {
                    self.click_phase -= two_pi;
                }
                for c in 0..frame.len().min(2) {
                    frame[c] = (frame[c] + s).clamp(-1.0, 1.0);
                }
            }

            self.phase += 1.0;
            if self.phase >= bar {
                self.phase -= bar;
            }
        }
    }

    fn trigger_click(&mut self, hz: f32, amp: f32, sr: f32) {
        self.click_env = 1.0;
        self.click_phase = 0.0;
        self.click_inc = std::f32::consts::TAU * hz / sr.max(1.0);
        self.click_amp = amp;
    }

    fn advance_phase(&mut self, n_frames: usize) {
        let bar = self.samples_per_bar();
        if bar <= 0.0 {
            return;
        }
        self.phase = (self.phase + n_frames as f64) % bar;
    }

    fn bar_frac(&self) -> f64 {
        let bar = self.samples_per_bar();
        if bar <= 0.0 {
            0.0
        } else {
            (self.phase / bar).clamp(0.0, 1.0)
        }
    }

    fn samples_per_quarter(&self) -> f64 {
        self.sample_rate * 60.0 / sanitize_bpm(self.bpm) as f64
    }

    fn samples_per_bar(&self) -> f64 {
        self.samples_per_quarter()
            * (sanitize_numerator(self.numerator) as f64 * 4.0
                / sanitize_denominator(self.denominator) as f64)
    }

    fn samples_per_sub(&self) -> f64 {
        self.samples_per_quarter() / self.subdivision.per_quarter()
    }

    fn samples_per_beat(&self) -> f64 {
        self.samples_per_quarter() * (4.0 / sanitize_denominator(self.denominator) as f64)
    }
}

fn on_period(phase: f64, period: f64) -> bool {
    if period <= 0.0 {
        return false;
    }
    let r = phase % period;
    r < 0.5 || r > period - 0.5
}

pub fn process_looper_with_metro(
    metro: &mut Metronome,
    looper: &mut crate::looper::Looper,
    buf: &mut [f32],
    channels: usize,
) {
    if !metro.clocking() {
        looper.process_interleaved(buf, channels);
        return;
    }
    let step = channels.max(1);
    let n_frames = buf.len() / step;
    let mut events = [(0usize, GridKind::Subdivision); MAX_GRID_EVENTS];
    let n = metro.peek_grid_events(n_frames, &mut events);
    let mut start = 0usize;
    for &(frame, kind) in events.iter().take(n) {
        let frame = frame.min(n_frames);
        if frame > start {
            looper.process_interleaved(&mut buf[start * step..frame * step], channels);
        }
        match kind {
            GridKind::Bar => {
                looper.bar_tick();
                looper.subdivision_tick();
            }
            GridKind::Subdivision => looper.subdivision_tick(),
        }
        start = frame;
    }
    if start < n_frames {
        looper.process_interleaved(&mut buf[start * step..n_frames * step], channels);
    } else if n_frames == 0 {
        looper.process_interleaved(buf, channels);
    }
    metro.mix_and_advance(buf, channels);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_frames_4_4_120_at_48k() {
        assert_eq!(bar_frames(48_000.0, 120.0, 4, 4), 96_000);
    }

    #[test]
    fn bar_frames_6_8_120_at_48k() {
        assert_eq!(bar_frames(48_000.0, 120.0, 6, 8), 72_000);
    }

    #[test]
    fn bar_frames_3_4_120_at_48k() {
        assert_eq!(bar_frames(48_000.0, 120.0, 3, 4), 72_000);
        assert_ne!(bar_frames(48_000.0, 120.0, 3, 4), bar_frames(48_000.0, 120.0, 4, 4));
    }

    #[test]
    fn count_in_clicks_clock_without_metro_enabled() {
        let mut m = Metronome::new(1_000.0);
        m.handle(MetronomeEvent::SetSubdivision { sub: 1 });
        m.handle(MetronomeEvent::SetVolume { norm: 1.0 });
        m.start_count_in_clicks();
        assert!(m.clocking());
        assert!(!m.enabled());
        let mut ev = [(0usize, GridKind::Subdivision); MAX_GRID_EVENTS];
        let n = m.peek_grid_events(200, &mut ev);
        assert!(n >= 1);
        assert_eq!(ev[0], (0, GridKind::Bar));
        let mut buf = vec![0.0f32; 2_000 * 2];
        m.mix_and_advance(&mut buf, 2);
        let head: f32 = buf[..16].iter().map(|s| s.abs()).sum();
        assert!(head > 0.5, "count-in downbeat energy {head}");
        m.stop_count_in_clicks();
        assert!(!m.clocking());
    }

    #[test]
    fn click_energy_on_downbeat() {
        let mut m = Metronome::new(1_000.0);
        m.handle(MetronomeEvent::SetEnabled { on: true });
        m.handle(MetronomeEvent::SetSubdivision { sub: 1 });
        m.handle(MetronomeEvent::SetVolume { norm: 1.0 });
        let mut buf = vec![0.0f32; 2_000 * 2];
        m.mix_and_advance(&mut buf, 2);
        let head: f32 = buf[..16].iter().map(|s| s.abs()).sum();
        let gap: f32 = buf[200..400].iter().map(|s| s.abs()).sum();
        assert!(head > 0.5, "downbeat energy {head}");
        assert!(head > gap * 3.0, "head {head} gap {gap}");
    }

    #[test]
    fn peek_emits_bar_then_subs() {
        let mut m = Metronome::new(1_000.0);
        m.handle(MetronomeEvent::SetEnabled { on: true });
        m.handle(MetronomeEvent::SetSubdivision { sub: 4 });
        let mut ev = [(0usize, GridKind::Subdivision); MAX_GRID_EVENTS];
        let n = m.peek_grid_events(200, &mut ev);
        assert!(n >= 1);
        assert_eq!(ev[0], (0, GridKind::Bar));
        assert!(
            ev.iter().take(n).any(|&(f, k)| f > 0 && k == GridKind::Subdivision),
            "expected a 16th after the downbeat"
        );
    }
}
