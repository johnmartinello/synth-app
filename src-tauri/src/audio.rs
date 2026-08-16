//! CPAL output stream driving the CLAP audio processor.

mod buffers;
mod config;

pub use config::FullAudioConfig;

use crate::clap_host::CpalHost;
use crate::engine::{AudioEngine, EngineEvent, GeneratedNote};
use crate::looper::{Looper, LooperStatus};
use crate::metronome::{self, Metronome, MetronomeEvent};
use crate::midi;
use buffers::HostAudioBuffers;
use clack_host::prelude::*;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BuildStreamError, Device, FromSample, OutputCallbackInfo, SampleFormat, Stream, StreamConfig,
};
use rtrb::Consumer;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::f32::consts::{FRAC_1_SQRT_2, PI};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// 1.0 = open (IEEE bits of 1.0f32).
static FILTER_BITS: AtomicU32 = AtomicU32::new(0x3f80_0000);
static FILTER_RES_BITS: AtomicU32 = AtomicU32::new(0);
static FILTER_MODE_BITS: AtomicU32 = AtomicU32::new(0);

const FILTER_Q_MIN: f32 = FRAC_1_SQRT_2;
const FILTER_Q_MAX: f32 = 10.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u32)]
pub enum FilterMode {
    #[default]
    Lp = 0,
    Hp = 1,
    Bp = 2,
    Notch = 3,
}

impl FilterMode {
    fn from_bits(v: u32) -> Self {
        match v {
            1 => Self::Hp,
            2 => Self::Bp,
            3 => Self::Notch,
            _ => Self::Lp,
        }
    }
}

pub fn set_filter_norm(value: f32) {
    FILTER_BITS.store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

pub fn set_filter_res(value: f32) {
    FILTER_RES_BITS.store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

pub fn set_filter_mode(mode: FilterMode) {
    FILTER_MODE_BITS.store(mode as u32, Ordering::Relaxed);
}

fn filter_norm() -> f32 {
    f32::from_bits(FILTER_BITS.load(Ordering::Relaxed)).clamp(0.0, 1.0)
}

fn filter_res() -> f32 {
    f32::from_bits(FILTER_RES_BITS.load(Ordering::Relaxed)).clamp(0.0, 1.0)
}

fn filter_mode() -> FilterMode {
    FilterMode::from_bits(FILTER_MODE_BITS.load(Ordering::Relaxed))
}

fn apply_env_gain(frames: &mut [f32], channels: usize, start: f32, end: f32) {
    let step = channels.max(1);
    let n = frames.len() / step;
    if n == 0 {
        return;
    }
    let den = n as f32;
    for (i, frame) in frames.chunks_exact_mut(step).enumerate() {
        let g = start + (end - start) * (i as f32 / den);
        for s in frame.iter_mut() {
            *s *= g;
        }
    }
}

/// Stereo TPT SVF on the host output (12 dB/oct). Cytomic / Zavalishin.
struct Svf {
    sample_rate: f32,
    mode: FilterMode,
    lp_open: bool,
    ic1: [f32; 2],
    ic2: [f32; 2],
    g: f32,
    g_target: f32,
    k: f32,
    k_target: f32,
}

impl Svf {
    fn new(sample_rate: f64) -> Self {
        let sr = sample_rate.max(1.0) as f32;
        let g_open = (PI * 0.49).tan();
        let k = 1.0 / FILTER_Q_MIN;
        Self {
            sample_rate: sr,
            mode: FilterMode::Lp,
            lp_open: true,
            ic1: [0.0; 2],
            ic2: [0.0; 2],
            g: g_open,
            g_target: g_open,
            k,
            k_target: k,
        }
    }

    fn set_params(&mut self, norm: f32, res: f32, mode: FilterMode) {
        self.mode = mode;
        let n = norm.clamp(0.0, 1.0);
        self.lp_open = mode == FilterMode::Lp && n >= 0.995;
        let hz = (70.0 * (18_000.0_f32 / 70.0).powf(n)).min(self.sample_rate * 0.49);
        self.g_target = (PI * hz / self.sample_rate).tan();
        let q = FILTER_Q_MIN + res.clamp(0.0, 1.0) * (FILTER_Q_MAX - FILTER_Q_MIN);
        self.k_target = 1.0 / q;
    }

    fn process_interleaved(&mut self, frames: &mut [f32], channels: usize) {
        let ch = channels.max(1).min(2);
        let step = channels.max(1);
        for frame in frames.chunks_exact_mut(step) {
            self.g += 0.12 * (self.g_target - self.g);
            self.k += 0.12 * (self.k_target - self.k);
            if self.lp_open {
                continue;
            }
            let g = self.g;
            let k = self.k;
            let a1 = 1.0 / (1.0 + g * (g + k));
            let a2 = g * a1;
            let a3 = g * a2;
            for c in 0..ch.min(frame.len()) {
                let x = frame[c];
                let v3 = x - self.ic2[c];
                let v1 = a1 * self.ic1[c] + a2 * v3;
                let v2 = self.ic2[c] + a2 * self.ic1[c] + a3 * v3;
                self.ic1[c] = 2.0 * v1 - self.ic1[c];
                self.ic2[c] = 2.0 * v2 - self.ic2[c];
                let lp = v2;
                let bp = v1;
                let hp = x - k * bp - lp;
                frame[c] = match self.mode {
                    FilterMode::Lp => lp,
                    FilterMode::Hp => hp,
                    FilterMode::Bp => bp,
                    FilterMode::Notch => lp + hp,
                };
            }
        }
    }
}

pub fn activate_to_stream(
    instance: &mut PluginInstance<CpalHost>,
    engine_consumer: Consumer<EngineEvent>,
) -> Result<(Stream, f64), Box<dyn Error>> {
    let cpal_host = cpal::default_host();
    let output_device = cpal_host
        .default_output_device()
        .ok_or("No default audio output device")?;

    let config = FullAudioConfig::find_best_from(&output_device, instance)?;
    log::info!("Audio: {config}");

    let note_port = midi::find_main_note_port_index(instance);
    let plugin_audio_processor = instance
        .activate(|_, _| (), config.as_clack_plugin_config())?
        .start_processing()?;

    let sample_format = config.sample_format;
    let cpal_config = config.as_cpal_stream_config();
    let sample_rate = config.sample_rate as f64;
    let audio_processor =
        StreamAudioProcessor::new(plugin_audio_processor, engine_consumer, config, note_port);

    let stream = build_output_stream_for_sample_format(
        &output_device,
        audio_processor,
        &cpal_config,
        sample_format,
    )?;
    stream.play()?;
    Ok((stream, sample_rate))
}

fn build_output_stream_for_sample_format(
    device: &Device,
    processor: StreamAudioProcessor,
    config: &StreamConfig,
    sample_format: SampleFormat,
) -> Result<Stream, BuildStreamError> {
    let err = |e| log::error!("Audio stream error: {e}");
    match sample_format {
        SampleFormat::I8 => {
            device.build_output_stream(config, make_stream_runner::<i8>(processor), err, None)
        }
        SampleFormat::I16 => {
            device.build_output_stream(config, make_stream_runner::<i16>(processor), err, None)
        }
        SampleFormat::I32 => {
            device.build_output_stream(config, make_stream_runner::<i32>(processor), err, None)
        }
        SampleFormat::I64 => {
            device.build_output_stream(config, make_stream_runner::<i64>(processor), err, None)
        }
        SampleFormat::U8 => {
            device.build_output_stream(config, make_stream_runner::<u8>(processor), err, None)
        }
        SampleFormat::U16 => {
            device.build_output_stream(config, make_stream_runner::<u16>(processor), err, None)
        }
        SampleFormat::U32 => {
            device.build_output_stream(config, make_stream_runner::<u32>(processor), err, None)
        }
        SampleFormat::U64 => {
            device.build_output_stream(config, make_stream_runner::<u64>(processor), err, None)
        }
        SampleFormat::F32 => {
            device.build_output_stream(config, make_stream_runner::<f32>(processor), err, None)
        }
        SampleFormat::F64 => {
            device.build_output_stream(config, make_stream_runner::<f64>(processor), err, None)
        }
        f => unimplemented!("Unknown sample format: {f:?}"),
    }
}

fn make_stream_runner<S: FromSample<f32> + cpal::Sample>(
    mut audio_processor: StreamAudioProcessor,
) -> impl FnMut(&mut [S], &OutputCallbackInfo) {
    move |data, _info| audio_processor.process(data)
}

struct StreamAudioProcessor {
    audio_processor: StartedPluginAudioProcessor<CpalHost>,
    buffers: HostAudioBuffers,
    engine_consumer: Consumer<EngineEvent>,
    engine: AudioEngine,
    looper: Looper,
    metro: Metronome,
    midi_clock_on: bool,
    events: EventBuffer,
    generated: Vec<GeneratedNote>,
    note_port: Option<(u16, bool)>,
    steady_counter: u64,
    svf: Svf,
}

impl StreamAudioProcessor {
    fn new(
        plugin_instance: StartedPluginAudioProcessor<CpalHost>,
        engine_consumer: Consumer<EngineEvent>,
        config: FullAudioConfig,
        note_port: Option<(u16, bool)>,
    ) -> Self {
        let sample_rate = config.sample_rate as f64;
        let channels = config.output_channel_count;
        Self {
            audio_processor: plugin_instance,
            buffers: HostAudioBuffers::from_config(config),
            engine_consumer,
            engine: AudioEngine::new(sample_rate),
            looper: Looper::new(sample_rate, channels),
            metro: Metronome::new(sample_rate),
            midi_clock_on: false,
            events: EventBuffer::with_capacity(256),
            generated: Vec::with_capacity(64),
            note_port,
            steady_counter: 0,
            svf: Svf::new(sample_rate),
        }
    }

    fn process<S: FromSample<f32> + cpal::Sample>(&mut self, data: &mut [S]) {
        self.buffers.ensure_buffer_size_matches(data.len());
        let sample_count = self.buffers.cpal_buf_len_to_frame_count(data.len());

        self.generated.clear();
        while let Ok(ev) = self.engine_consumer.pop() {
            match ev {
                EngineEvent::Looper(cmd) => {
                    self.looper.handle(cmd);
                    self.sync_looper_clock();
                }
                EngineEvent::Metronome(cmd) => self.apply_metro(cmd),
                EngineEvent::MidiClockSync { on } => {
                    self.midi_clock_on = on;
                    self.sync_looper_clock();
                    self.engine.handle(ev, 0, &mut self.generated);
                }
                EngineEvent::MidiClockTick => {
                    if !self.metro.clocking() {
                        self.looper.clock_tick();
                    }
                    self.engine.handle(ev, 0, &mut self.generated);
                }
                other => self.engine.handle(other, 0, &mut self.generated),
            }
        }
        let (env_start, env_end) = self
            .engine
            .process_clock(sample_count as u32, &mut self.generated);

        self.events.clear();
        if let Some((port, prefers_midi)) = self.note_port {
            for n in &self.generated {
                midi::push_note(
                    &mut self.events,
                    n.time.min(sample_count.saturating_sub(1) as u32),
                    port,
                    prefers_midi,
                    n.key,
                    n.vel,
                    n.on,
                );
            }
        }

        let (ins, mut outs) = self.buffers.prepare_plugin_buffers(data.len());
        let input_events = self.events.as_input();
        match self.audio_processor.process(
            &ins,
            &mut outs,
            &input_events,
            &mut OutputEvents::void(),
            Some(self.steady_counter),
            None,
        ) {
            Ok(_) => {
                self.svf
                    .set_params(filter_norm(), filter_res(), filter_mode());
                let svf = &mut self.svf;
                let looper = &mut self.looper;
                let metro = &mut self.metro;
                self.buffers.write_to_cpal_buffer(data, |buf, ch| {
                    svf.process_interleaved(buf, ch);
                    apply_env_gain(buf, ch, env_start, env_end);
                    metronome::process_looper_with_metro(metro, looper, buf, ch);
                });
            }
            Err(e) => log::error!("Plugin process error: {e}"),
        }
        self.sync_looper_clock();
        self.steady_counter += sample_count as u64;
        publish_sounding(&self.engine);
        publish_env(&self.engine);
        publish_looper(&self.looper);
    }

    fn apply_metro(&mut self, ev: MetronomeEvent) {
        self.metro.handle(ev);
        self.sync_looper_clock();
    }

    fn sync_looper_clock(&mut self) {
        let want = self.looper.wants_count_in_clock();
        if want && !self.metro.enabled() {
            self.metro.start_count_in_clicks();
        } else {
            self.metro.stop_count_in_clicks();
        }
        if self.metro.enabled() || want {
            self.looper.set_clock_sync(true);
            self.looper.set_bar_frames(self.metro.bar_frames());
        } else {
            self.looper.set_clock_sync(self.midi_clock_on);
            if self.looper.count_in_enabled() {
                self.looper.set_bar_frames(self.metro.bar_frames());
            } else {
                self.looper.set_bar_frames(0);
            }
        }
    }
}

static SOUNDING_LO: AtomicU64 = AtomicU64::new(0);
static SOUNDING_HI: AtomicU64 = AtomicU64::new(0);
static ENV_STAGE: AtomicU32 = AtomicU32::new(0);
static ENV_PROGRESS: AtomicU32 = AtomicU32::new(0);
static ENV_LEVEL: AtomicU32 = AtomicU32::new(0);
static LOOPER_PACKED: AtomicU32 = AtomicU32::new(0);
static LOOPER_PLAYHEAD: AtomicU32 = AtomicU32::new(0);
static LOOPER_LEVELS: AtomicU64 = AtomicU64::new(0x3f80_0000_3f80_0000);

fn publish_sounding(engine: &AudioEngine) {
    let [lo, hi] = engine.sounding_mask();
    SOUNDING_LO.store(lo, Ordering::Relaxed);
    SOUNDING_HI.store(hi, Ordering::Relaxed);
}

fn publish_env(engine: &AudioEngine) {
    let (stage, progress, level) = engine.env_playhead();
    ENV_STAGE.store(stage as u32, Ordering::Relaxed);
    ENV_PROGRESS.store(progress.to_bits(), Ordering::Relaxed);
    ENV_LEVEL.store(level.to_bits(), Ordering::Relaxed);
}

fn publish_looper(looper: &Looper) {
    let snap = looper.snapshot();
    LOOPER_PACKED.store(pack_looper(&snap), Ordering::Relaxed);
    LOOPER_PLAYHEAD.store(snap.playhead.to_bits(), Ordering::Relaxed);
    let lo = snap.levels[0].to_bits() as u64;
    let hi = snap.levels[1].to_bits() as u64;
    LOOPER_LEVELS.store(lo | (hi << 32), Ordering::Relaxed);
}

fn pack_looper(s: &LooperStatus) -> u32 {
    u32::from(s.selected & 3)
        | (u32::from(s.modes[0] & 7) << 2)
        | (u32::from(s.modes[1] & 7) << 5)
        | (u32::from(s.muted[0]) << 8)
        | (u32::from(s.muted[1]) << 9)
        | (u32::from(s.has_phrase[0]) << 10)
        | (u32::from(s.has_phrase[1]) << 11)
        | (u32::from(s.mult & 7) << 12)
        | (u32::from(s.max_mult & 7) << 15)
        | (u32::from(s.clock_sync) << 18)
        | (u32::from(s.pending) << 19)
        | (u32::from(s.count_in) << 20)
        | (u32::from(s.counting) << 21)
}

fn unpack_looper(packed: u32, playhead: f32, levels: [f32; 2]) -> LooperStatus {
    LooperStatus {
        selected: (packed & 3) as u8,
        modes: [((packed >> 2) & 7) as u8, ((packed >> 5) & 7) as u8],
        muted: [(packed >> 8) & 1 != 0, (packed >> 9) & 1 != 0],
        has_phrase: [(packed >> 10) & 1 != 0, (packed >> 11) & 1 != 0],
        playhead: playhead.clamp(0.0, 1.0),
        levels: [levels[0].clamp(0.0, 1.0), levels[1].clamp(0.0, 1.0)],
        mult: match ((packed >> 12) & 7) as u8 {
            2 => 2,
            4 => 4,
            _ => 1,
        },
        max_mult: match ((packed >> 15) & 7) as u8 {
            1 => 1,
            2 => 2,
            _ => 4,
        },
        clock_sync: (packed >> 18) & 1 != 0,
        pending: (packed >> 19) & 1 != 0,
        count_in: (packed >> 20) & 1 != 0,
        counting: (packed >> 21) & 1 != 0,
    }
}

pub fn looper_snapshot() -> LooperStatus {
    let packed_levels = LOOPER_LEVELS.load(Ordering::Relaxed);
    unpack_looper(
        LOOPER_PACKED.load(Ordering::Relaxed),
        f32::from_bits(LOOPER_PLAYHEAD.load(Ordering::Relaxed)),
        [
            f32::from_bits(packed_levels as u32),
            f32::from_bits((packed_levels >> 32) as u32),
        ],
    )
}

pub fn env_playhead() -> (u8, f32, f32) {
    (
        ENV_STAGE.load(Ordering::Relaxed) as u8,
        f32::from_bits(ENV_PROGRESS.load(Ordering::Relaxed)).clamp(0.0, 1.0),
        f32::from_bits(ENV_LEVEL.load(Ordering::Relaxed)).clamp(0.0, 1.0),
    )
}

pub fn sounding_mask() -> [u64; 2] {
    [
        SOUNDING_LO.load(Ordering::Relaxed),
        SOUNDING_HI.load(Ordering::Relaxed),
    ]
}

pub fn sounding_keys_from_mask(mask: [u64; 2]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    for i in 0..64u8 {
        if mask[0] & (1u64 << i) != 0 {
            out.push(i);
        }
        if mask[1] & (1u64 << i) != 0 {
            out.push(i + 64);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{FilterMode, Svf};
    use std::f32::consts::PI;

    const SR: f64 = 48_000.0;

    fn cutoff_norm_for_hz(hz: f32) -> f32 {
        ((hz / 70.0).ln() / (18_000.0_f32 / 70.0).ln()).clamp(0.0, 1.0)
    }

    fn sine_stereo(freq: f32, frames: usize) -> Vec<f32> {
        let mut buf = vec![0.0f32; frames * 2];
        for i in 0..frames {
            let s = (2.0 * PI * freq * i as f32 / SR as f32).sin();
            buf[i * 2] = s;
            buf[i * 2 + 1] = s;
        }
        buf
    }

    fn energy_tail(buf: &[f32]) -> f32 {
        let start = buf.len() / 2;
        buf[start..].iter().map(|s| s * s).sum()
    }

    fn run(mode: FilterMode, norm: f32, res: f32, mut buf: Vec<f32>) -> Vec<f32> {
        let mut svf = Svf::new(SR);
        svf.set_params(norm, res, mode);
        svf.process_interleaved(&mut buf, 2);
        buf
    }

    #[test]
    fn lowpass_darkens_high_frequency() {
        let mut open = Svf::new(SR);
        let mut dark = Svf::new(SR);
        open.set_params(1.0, 0.0, FilterMode::Lp);
        dark.set_params(0.0, 0.0, FilterMode::Lp);
        let mut hi_open = vec![0.0f32; 256];
        let mut hi_dark = vec![0.0f32; 256];
        for i in 0..128 {
            let s = if i % 2 == 0 { 1.0 } else { -1.0 };
            hi_open[i * 2] = s;
            hi_open[i * 2 + 1] = s;
            hi_dark[i * 2] = s;
            hi_dark[i * 2 + 1] = s;
        }
        open.process_interleaved(&mut hi_open, 2);
        dark.process_interleaved(&mut hi_dark, 2);
        let energy = |buf: &[f32]| buf[128..].iter().map(|s| s * s).sum::<f32>();
        assert!(energy(&hi_dark) < energy(&hi_open) * 0.25);
    }

    #[test]
    fn lp_at_one_stays_dry() {
        let src = sine_stereo(440.0, 512);
        let out = run(FilterMode::Lp, 1.0, 0.0, src.clone());
        let err: f32 = src.iter().zip(&out).map(|(a, b)| (a - b).abs()).sum();
        assert!(err < 1e-4, "lp open should be dry, err={err}");
    }

    #[test]
    fn highpass_thins_low_frequency() {
        let src = sine_stereo(120.0, 4096);
        let open = run(FilterMode::Hp, 0.0, 0.0, src.clone());
        let thin = run(FilterMode::Hp, 1.0, 0.0, src);
        assert!(energy_tail(&thin) < energy_tail(&open) * 0.25);
    }

    #[test]
    fn bandpass_rejects_dc_and_prefers_mid() {
        let dc = run(FilterMode::Bp, 0.5, 0.0, vec![1.0f32; 4096 * 2]);
        assert!(energy_tail(&dc) < 0.05);

        let mid_n = cutoff_norm_for_hz(1_000.0);
        let mid = run(FilterMode::Bp, mid_n, 0.35, sine_stereo(1_000.0, 4096));
        let low = run(FilterMode::Bp, mid_n, 0.35, sine_stereo(80.0, 4096));
        let high = run(FilterMode::Bp, mid_n, 0.35, sine_stereo(12_000.0, 4096));
        let mid_e = energy_tail(&mid);
        assert!(mid_e > energy_tail(&low) * 4.0);
        assert!(mid_e > energy_tail(&high) * 4.0);
    }

    #[test]
    fn notch_cuts_mid() {
        let mid_n = cutoff_norm_for_hz(1_000.0);
        let src = sine_stereo(1_000.0, 4096);
        let dry = run(FilterMode::Lp, 1.0, 0.0, src.clone());
        let notched = run(FilterMode::Notch, mid_n, 0.5, src);
        assert!(energy_tail(&notched) < energy_tail(&dry) * 0.5);
    }
}
