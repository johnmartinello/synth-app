//! Negotiate CPAL output vs plugin audio ports.

use crate::clap_host::CpalHost;
use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfoBuffer, AudioPortType, PluginAudioPorts,
};
use clack_host::prelude::{
    ClapId, PluginAudioConfiguration, PluginInstance, PluginMainThreadHandle,
};
use cpal::traits::DeviceTrait;
use cpal::{
    BufferSize, Device, SampleFormat, SampleRate, StreamConfig, SupportedBufferSize,
    SupportedStreamConfigRange,
};
use std::cmp::Ordering;
use std::error::Error;
use std::fmt::{Display, Formatter};

pub struct FullAudioConfig {
    pub plugin_input_port_config: PluginAudioPortsConfig,
    pub plugin_output_port_config: PluginAudioPortsConfig,
    pub output_channel_count: usize,
    pub min_buffer_size: u32,
    pub max_likely_buffer_size: u32,
    pub sample_rate: u32,
    pub sample_format: SampleFormat,
}

impl FullAudioConfig {
    pub fn find_best_from(
        device: &Device,
        instance: &mut PluginInstance<CpalHost>,
    ) -> Result<Self, Box<dyn Error>> {
        let best_cpal_configs = list_device_configs_ordered(device)?;
        let input_ports = get_config_from_ports(&mut instance.plugin_handle(), true);
        let output_ports = get_config_from_ports(&mut instance.plugin_handle(), false);
        Ok(find_matching_output_config(
            &best_cpal_configs,
            output_ports,
            input_ports,
        ))
    }

    pub fn as_cpal_stream_config(&self) -> StreamConfig {
        StreamConfig {
            channels: self.output_channel_count as u16,
            // WASAPI shared mode often rejects Fixed; Default is more reliable.
            buffer_size: BufferSize::Default,
            sample_rate: SampleRate(self.sample_rate),
        }
    }

    pub fn as_clack_plugin_config(&self) -> PluginAudioConfiguration {
        PluginAudioConfiguration {
            sample_rate: self.sample_rate as f64,
            min_frames_count: self.min_buffer_size,
            max_frames_count: self.max_likely_buffer_size,
        }
    }
}

impl Display for FullAudioConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} channels at {:.1}kHz, buffer {}-{}, plugin port \"{}\" ({})",
            self.output_channel_count,
            self.sample_rate as f64 / 1_000.0,
            self.min_buffer_size,
            self.max_likely_buffer_size,
            self.plugin_output_port_config.main_port().name,
            self.plugin_output_port_config.main_port().port_layout
        )
    }
}

#[derive(Clone, Debug)]
pub struct PluginAudioPortsConfig {
    pub ports: Vec<PluginAudioPortInfo>,
    pub main_port_index: u32,
}

impl PluginAudioPortsConfig {
    fn empty() -> Self {
        Self {
            main_port_index: 0,
            ports: vec![],
        }
    }

    fn default_stereo() -> Self {
        Self {
            main_port_index: 0,
            ports: vec![PluginAudioPortInfo {
                _id: None,
                port_layout: AudioPortLayout::Stereo,
                name: "Default".into(),
            }],
        }
    }

    pub fn main_port(&self) -> &PluginAudioPortInfo {
        &self.ports[self.main_port_index as usize]
    }

    pub fn total_channel_count(&self) -> usize {
        self.ports
            .iter()
            .map(|p| p.port_layout.channel_count() as usize)
            .sum()
    }
}

#[derive(Clone, Debug)]
pub struct PluginAudioPortInfo {
    pub _id: Option<ClapId>,
    pub port_layout: AudioPortLayout,
    pub name: String,
}

#[derive(Eq, PartialEq, Copy, Clone, Debug)]
pub enum AudioPortLayout {
    Mono,
    Stereo,
    Unsupported { channel_count: u16 },
}

impl AudioPortLayout {
    pub fn channel_count(&self) -> u16 {
        match self {
            AudioPortLayout::Mono => 1,
            AudioPortLayout::Stereo => 2,
            AudioPortLayout::Unsupported { channel_count } => *channel_count,
        }
    }
}

impl Display for AudioPortLayout {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioPortLayout::Mono => f.write_str("mono"),
            AudioPortLayout::Stereo => f.write_str("stereo"),
            AudioPortLayout::Unsupported { channel_count } => write!(f, "{channel_count}-channels"),
        }
    }
}

fn list_device_configs_ordered(
    device: &Device,
) -> Result<Vec<SupportedStreamConfigRange>, Box<dyn Error>> {
    let mut output_configs: Vec<_> = device
        .supported_output_configs()?
        .filter(is_device_config_supported)
        .collect();
    output_configs.sort_by(|a, b| compare_devices_configs(a, b).reverse());
    Ok(output_configs)
}

fn is_device_config_supported(config: &SupportedStreamConfigRange) -> bool {
    if !(1..=2).contains(&config.channels()) {
        return false;
    }
    if config.max_sample_rate().0 < 44_100 {
        return false;
    }
    if sample_type_preference(config.sample_format()) == u8::MAX {
        return false;
    }
    true
}

fn compare_devices_configs(
    first: &SupportedStreamConfigRange,
    second: &SupportedStreamConfigRange,
) -> Ordering {
    match first.channels().cmp(&second.channels()) {
        o @ (Ordering::Less | Ordering::Greater) => return o,
        Ordering::Equal => {}
    }
    match sample_type_preference(first.sample_format())
        .cmp(&sample_type_preference(second.sample_format()))
    {
        o @ (Ordering::Less | Ordering::Greater) => return o.reverse(),
        Ordering::Equal => {}
    }
    match first.min_sample_rate().cmp(&second.min_sample_rate()) {
        o @ (Ordering::Less | Ordering::Greater) => return o.reverse(),
        Ordering::Equal => {}
    }
    first.cmp_default_heuristics(second)
}

fn sample_type_preference(sample_type: SampleFormat) -> u8 {
    match sample_type {
        SampleFormat::F32 => 0,
        SampleFormat::F64 => 1,
        SampleFormat::I64 => 2,
        SampleFormat::U64 => 3,
        SampleFormat::I32 => 4,
        SampleFormat::U32 => 5,
        SampleFormat::I16 => 6,
        SampleFormat::U16 => 7,
        SampleFormat::I8 => 8,
        SampleFormat::U8 => 9,
        _ => u8::MAX,
    }
}

pub fn get_config_from_ports(
    plugin: &mut PluginMainThreadHandle,
    is_input: bool,
) -> PluginAudioPortsConfig {
    let Some(ports) = plugin.get_extension::<PluginAudioPorts>() else {
        return PluginAudioPortsConfig::default_stereo();
    };

    let mut buffer = AudioPortInfoBuffer::new();
    let mut main_port_index = None;
    let mut discovered_ports = vec![];

    for i in 0..ports.count(plugin, is_input) {
        let Some(info) = ports.get(plugin, i, is_input, &mut buffer) else {
            continue;
        };
        let port_type = info
            .port_type
            .or_else(|| AudioPortType::from_channel_count(info.channel_count));
        let port_layout = match port_type {
            Some(l) if l == AudioPortType::MONO => AudioPortLayout::Mono,
            Some(l) if l == AudioPortType::STEREO => AudioPortLayout::Stereo,
            _ => AudioPortLayout::Unsupported {
                channel_count: info.channel_count as u16,
            },
        };
        if info.flags.contains(AudioPortFlags::IS_MAIN) && main_port_index.replace(i).is_some() {
            log::warn!("Plugin defines multiple main audio ports");
        }
        discovered_ports.push(PluginAudioPortInfo {
            _id: Some(info.id),
            port_layout,
            name: String::from_utf8_lossy(info.name).into_owned(),
        });
    }

    if discovered_ports.is_empty() {
        if is_input {
            return PluginAudioPortsConfig::empty();
        }
        return PluginAudioPortsConfig::default_stereo();
    }

    let main_port_index = main_port_index.unwrap_or_else(|| {
        discovered_ports
            .iter()
            .enumerate()
            .find(|(_, p)| p.port_layout == AudioPortLayout::Stereo)
            .or_else(|| {
                discovered_ports
                    .iter()
                    .enumerate()
                    .find(|(_, p)| p.port_layout == AudioPortLayout::Mono)
            })
            .map(|(i, _)| i as u32)
            .unwrap_or(0)
    });

    PluginAudioPortsConfig {
        main_port_index,
        ports: discovered_ports,
    }
}

fn find_matching_output_config(
    ordered_stream_configs: &[SupportedStreamConfigRange],
    plugin_output_port_config: PluginAudioPortsConfig,
    plugin_input_port_config: PluginAudioPortsConfig,
) -> FullAudioConfig {
    let plugin_channel_count = plugin_output_port_config
        .main_port()
        .port_layout
        .channel_count();
    let matching_config = ordered_stream_configs
        .iter()
        .find(|c| c.channels() == plugin_channel_count);
    let best_stream_config = matching_config
        .or_else(|| ordered_stream_configs.first())
        .expect("No config supported by output device");

    let (min_buffer_size, max_buffer_size) = match best_stream_config.buffer_size() {
        SupportedBufferSize::Range { min, max } => (*min, 2048u32.clamp(*min, *max)),
        SupportedBufferSize::Unknown => (1, 2048),
    };

    FullAudioConfig {
        output_channel_count: best_stream_config.channels() as usize,
        min_buffer_size: min_buffer_size.max(1),
        max_likely_buffer_size: max_buffer_size.max(256),
        sample_rate: 44_100.clamp(
            best_stream_config.min_sample_rate().0,
            best_stream_config.max_sample_rate().0,
        ),
        plugin_output_port_config,
        plugin_input_port_config,
        sample_format: best_stream_config.sample_format(),
    }
}
