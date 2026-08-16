//! CPAL output stream driving the CLAP audio processor.

mod buffers;
mod config;

pub use config::FullAudioConfig;

use crate::clap_host::CpalHost;
use crate::engine::{AudioEngine, EngineEvent, GeneratedNote};
use crate::midi;
use buffers::HostAudioBuffers;
use clack_host::prelude::*;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BuildStreamError, Device, FromSample, OutputCallbackInfo, SampleFormat, Stream, StreamConfig,
};
use rtrb::Consumer;
use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};

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
    events: EventBuffer,
    generated: Vec<GeneratedNote>,
    note_port: Option<(u16, bool)>,
    steady_counter: u64,
}

impl StreamAudioProcessor {
    fn new(
        plugin_instance: StartedPluginAudioProcessor<CpalHost>,
        engine_consumer: Consumer<EngineEvent>,
        config: FullAudioConfig,
        note_port: Option<(u16, bool)>,
    ) -> Self {
        let sample_rate = config.sample_rate as f64;
        Self {
            audio_processor: plugin_instance,
            buffers: HostAudioBuffers::from_config(config),
            engine_consumer,
            engine: AudioEngine::new(sample_rate),
            events: EventBuffer::with_capacity(256),
            generated: Vec::with_capacity(64),
            note_port,
            steady_counter: 0,
        }
    }

    fn process<S: FromSample<f32> + cpal::Sample>(&mut self, data: &mut [S]) {
        self.buffers.ensure_buffer_size_matches(data.len());
        let sample_count = self.buffers.cpal_buf_len_to_frame_count(data.len());
        let (ins, mut outs) = self.buffers.prepare_plugin_buffers(data.len());

        self.generated.clear();
        while let Ok(ev) = self.engine_consumer.pop() {
            self.engine.handle(ev, 0, &mut self.generated);
        }
        self.engine
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

        let input_events = self.events.as_input();
        match self.audio_processor.process(
            &ins,
            &mut outs,
            &input_events,
            &mut OutputEvents::void(),
            Some(self.steady_counter),
            None,
        ) {
            Ok(_) => self.buffers.write_to_cpal_buffer(data),
            Err(e) => log::error!("Plugin process error: {e}"),
        }
        self.steady_counter += sample_count as u64;
        publish_sounding(&self.engine);
    }
}

static SOUNDING_LO: AtomicU64 = AtomicU64::new(0);
static SOUNDING_HI: AtomicU64 = AtomicU64::new(0);

fn publish_sounding(engine: &AudioEngine) {
    let [lo, hi] = engine.sounding_mask();
    SOUNDING_LO.store(lo, Ordering::Relaxed);
    SOUNDING_HI.store(hi, Ordering::Relaxed);
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
