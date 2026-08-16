//! MIDI input devices and conversion into CLAP note/MIDI events.

use clack_extensions::note_ports::{NoteDialects, NotePortInfoBuffer, PluginNotePorts};
use clack_host::events::event_types::{MidiEvent, NoteChokeEvent, NoteOffEvent, NoteOnEvent};
use clack_host::events::{EventFlags, Match};
use clack_host::prelude::*;
use crossbeam_channel::Sender;
use midir::{Ignore, MidiInput, MidiInputConnection};
use serde::Serialize;
use std::error::Error;
use wmidi::Velocity;

use crate::clap_host::CpalHost;

/// Packed MIDI bytes from the midir callback (no SysEx).
#[derive(Clone, Copy, Debug)]
pub struct MidiBytes {
    pub data: [u8; 3],
    pub len: u8,
}

#[derive(Clone, Serialize)]
pub struct MidiPortInfo {
    pub name: String,
}

pub fn list_ports() -> Result<Vec<MidiPortInfo>, String> {
    let mut input = MidiInput::new("Surge Chord").map_err(|e| e.to_string())?;
    input.ignore(Ignore::Sysex | Ignore::ActiveSense);
    let mut out = Vec::new();
    for port in input.ports() {
        if let Ok(name) = input.port_name(&port) {
            out.push(MidiPortInfo { name });
        }
    }
    Ok(out)
}

/// Open a named MIDI input and forward bytes to the CLAP host thread (not the webview).
pub fn connect_port(
    port_name: &str,
    tx: Sender<MidiBytes>,
) -> Result<MidiInputConnection<()>, Box<dyn Error>> {
    let mut input = MidiInput::new("Surge Chord")?;
    input.ignore(Ignore::Sysex | Ignore::ActiveSense);
    let ports = input.ports();
    let selected = ports
        .iter()
        .find(|p| input.port_name(p).ok().as_deref() == Some(port_name))
        .ok_or_else(|| format!("MIDI port not found: {port_name}"))?;

    let conn = input.connect(
        selected,
        "Surge Chord MIDI",
        move |_stamp, data, ()| {
            if data.is_empty() {
                return;
            }
            let mut packed = MidiBytes {
                data: [0; 3],
                len: data.len().min(3) as u8,
            };
            packed.data[..packed.len as usize].copy_from_slice(&data[..packed.len as usize]);
            let _ = tx.send(packed);
        },
        (),
    )?;
    Ok(conn)
}

pub fn find_main_note_port_index(instance: &mut PluginInstance<CpalHost>) -> Option<(u16, bool)> {
    let handle = instance.plugin_handle();
    let plugin_note_ports = handle.get_extension::<PluginNotePorts>()?;
    let mut buffer = NotePortInfoBuffer::new();
    let ports_count = plugin_note_ports.count(&handle, true).min(u16::MAX as u32);

    for i in 0..ports_count {
        let Some(port_info) = plugin_note_ports.get(&handle, i, true, &mut buffer) else {
            continue;
        };
        if !port_info
            .supported_dialects
            .intersects(NoteDialects::CLAP | NoteDialects::MIDI)
        {
            continue;
        }
        let prefers_midi = !port_info.supported_dialects.intersects(NoteDialects::CLAP);
        return Some((i as u16, prefers_midi));
    }
    None
}

/// Push a generated note as CLAP events.
pub fn push_note(
    buffer: &mut EventBuffer,
    time: u32,
    port_index: u16,
    prefers_midi: bool,
    key: u8,
    vel: u8,
    on: bool,
) {
    if prefers_midi {
        let status = if on { 0x90 } else { 0x80 };
        buffer.push(
            &MidiEvent::new(
                time,
                port_index,
                [status, key, if on { vel.max(1) } else { 0 }],
            )
            .with_flags(EventFlags::IS_LIVE),
        );
        return;
    }

    let velocity = vel as f64 / (u8::from(Velocity::MAX) as f64);
    if on {
        buffer.push(
            &NoteOnEvent::new(
                time,
                Pckn::new(port_index, 0u16, key as u16, Match::All),
                velocity,
            )
            .with_flags(EventFlags::IS_LIVE),
        );
    } else {
        buffer.push(
            &NoteOffEvent::new(
                time,
                Pckn::new(port_index, 0u16, key as u16, Match::All),
                velocity,
            )
            .with_flags(EventFlags::IS_LIVE),
        );
    }
}

#[allow(dead_code)]
pub fn push_choke_all(buffer: &mut EventBuffer) {
    buffer.push(&NoteChokeEvent::new(0, Pckn::match_all()).with_flags(EventFlags::IS_LIVE));
}
