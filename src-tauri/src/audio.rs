//! CPAL output stream driving the CLAP audio processor.

mod buffers;
mod config;

pub use config::FullAudioConfig;

use crate::clap_host::CpalHost;
use crate::engine::{AudioEngine, EngineEvent, GeneratedNote};
use crate::looper::{Looper, LooperStatus};
use crate::midi;
use buffers::HostAudioBuffers;
use clack_host::prelude::*;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BuildStreamError, Device, FromSample, OutputCallbackInfo, SampleFormat, Stream, StreamConfig,
};
use rtrb::Consumer;
use std::error::Error;
use std::f32::consts::PI;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// 1.0 = open (IEEE bits of 1.0f32).
static FILTER_BITS: AtomicU32 = AtomicU32::new(0x3f80_0000);

pub fn set_filter_norm(value: f32) {
    FILTER_BITS.store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

fn filter_norm() -> f32 {
    f32::from_bits(FILTER_BITS.load(Ordering::Relaxed)).clamp(0.0, 1.0)
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

/// Two cascaded one-pole low-passes on the host output (12 dB/oct).
struct Lowpass {
    sample_rate: f32,
    z1: [f32; 2],
    z2: [f32; 2],
    coeff: f32,
    coeff_target: f32,
}

impl Lowpass {
    fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate: sample_rate.max(1.0) as f32,
            z1: [0.0; 2],
            z2: [0.0; 2],
            coeff: 1.0,
            coeff_target: 1.0,
        }
    }

    fn set_norm(&mut self, norm: f32) {
        let n = norm.clamp(0.0, 1.0);
        self.coeff_target = if n >= 0.995 {
            1.0
        } else {
            let hz = (70.0 * (18_000.0_f32 / 70.0).powf(n)).min(self.sample_rate * 0.49);
            1.0 - (-2.0 * PI * hz / self.sample_rate).exp()
        };
    }

    fn process_interleaved(&mut self, frames: &mut [f32], channels: usize) {
        let ch = channels.max(1).min(2);
        let step = channels.max(1);
        for frame in frames.chunks_exact_mut(step) {
            self.coeff += 0.12 * (self.coeff_target - self.coeff);
            let a = self.coeff;
            for c in 0..ch.min(frame.len()) {
                let x = frame[c];
                let y1 = self.z1[c] + a * (x - self.z1[c]);
                self.z1[c] = y1;
                let y2 = self.z2[c] + a * (y1 - self.z2[c]);
                self.z2[c] = y2;
                frame[c] = y2;
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
    events: EventBuffer,
    generated: Vec<GeneratedNote>,
    note_port: Option<(u16, bool)>,
    steady_counter: u64,
    lowpass: Lowpass,
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
            events: EventBuffer::with_capacity(256),
            generated: Vec::with_capacity(64),
            note_port,
            steady_counter: 0,
            lowpass: Lowpass::new(sample_rate),
        }
    }

    fn process<S: FromSample<f32> + cpal::Sample>(&mut self, data: &mut [S]) {
        self.buffers.ensure_buffer_size_matches(data.len());
        let sample_count = self.buffers.cpal_buf_len_to_frame_count(data.len());

        self.generated.clear();
        while let Ok(ev) = self.engine_consumer.pop() {
            match ev {
                EngineEvent::Looper(cmd) => self.looper.handle(cmd),
                EngineEvent::MidiClockSync { on } => {
                    self.looper.set_clock_sync(on);
                    self.engine.handle(ev, 0, &mut self.generated);
                }
                EngineEvent::MidiClockTick => {
                    self.looper.clock_tick();
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
                self.lowpass.set_norm(filter_norm());
                let lp = &mut self.lowpass;
                let looper = &mut self.looper;
                self.buffers.write_to_cpal_buffer(data, |buf, ch| {
                    lp.process_interleaved(buf, ch);
                    apply_env_gain(buf, ch, env_start, env_end);
                    looper.process_interleaved(buf, ch);
                });
            }
            Err(e) => log::error!("Plugin process error: {e}"),
        }
        self.steady_counter += sample_count as u64;
        publish_sounding(&self.engine);
        publish_env(&self.engine);
        publish_looper(&self.looper);
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
    use super::Lowpass;

    #[test]
    fn lowpass_darkens_high_frequency() {
        let mut open = Lowpass::new(48_000.0);
        let mut dark = Lowpass::new(48_000.0);
        open.set_norm(1.0);
        dark.set_norm(0.0);
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
}
