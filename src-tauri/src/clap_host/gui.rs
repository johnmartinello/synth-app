use crate::clap_host::{CpalHostMainThread, CpalHostShared, MainThreadMessage};
use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, GuiError, GuiSize, HostGuiImpl, PluginGui, Window as ClapWindow,
};
use clack_host::prelude::*;
use std::error::Error;

impl HostGuiImpl for CpalHostShared {
    fn resize_hints_changed(&self) {}

    fn request_resize(&self, new_size: GuiSize) -> Result<(), HostError> {
        self.sender
            .send(MainThreadMessage::GuiRequestResized { new_size })
            .map_err(|_| HostError::Message("gui channel closed"))?;
        Ok(())
    }

    fn request_show(&self) -> Result<(), HostError> {
        Ok(())
    }

    fn request_hide(&self) -> Result<(), HostError> {
        Ok(())
    }

    fn closed(&self, _was_destroyed: bool) {
        let _ = self.sender.send(MainThreadMessage::GuiClosed);
    }
}

pub struct Gui {
    plugin_gui: PluginGui,
    pub configuration: Option<GuiConfiguration<'static>>,
    is_open: bool,
    is_resizeable: bool,
}

impl Gui {
    pub fn new(plugin_gui: PluginGui, instance: &PluginMainThreadHandle<'_>) -> Self {
        Self {
            plugin_gui,
            configuration: Self::negotiate_configuration(&plugin_gui, instance),
            is_open: false,
            is_resizeable: false,
        }
    }

    fn negotiate_configuration(
        gui: &PluginGui,
        plugin: &PluginMainThreadHandle<'_>,
    ) -> Option<GuiConfiguration<'static>> {
        let api_type = GuiApiType::default_for_current_platform()?;
        let mut config = GuiConfiguration {
            api_type,
            is_floating: true,
        };
        if gui.is_api_supported(plugin, config) {
            return Some(config);
        }
        config.is_floating = false;
        if gui.is_api_supported(plugin, config) {
            Some(config)
        } else {
            None
        }
    }

    pub fn needs_floating(&self) -> Option<bool> {
        self.configuration
            .map(|GuiConfiguration { is_floating, .. }| is_floating)
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn open_floating(&mut self, plugin: &PluginMainThreadHandle<'_>) -> Result<(), GuiError> {
        let Some(configuration) = self.configuration else {
            panic!("open_floating on incompatible plugin");
        };
        if !configuration.is_floating {
            panic!("open_floating on incompatible plugin");
        }
        self.plugin_gui.create(plugin, configuration)?;
        self.plugin_gui.suggest_title(plugin, c"Surge XT");
        self.plugin_gui.show(plugin)?;
        self.is_open = true;
        Ok(())
    }

    pub fn begin_embedded(
        &mut self,
        plugin: &PluginMainThreadHandle<'_>,
    ) -> Result<GuiSize, Box<dyn Error>> {
        let Some(configuration) = self.configuration else {
            return Err("Plugin has no GUI".into());
        };
        if configuration.is_floating {
            return Err("Plugin GUI is floating, not embedded".into());
        }
        self.plugin_gui.create(plugin, configuration)?;
        self.is_open = true;
        let initial_size = self.plugin_gui.get_size(plugin).unwrap_or(GuiSize {
            width: 960,
            height: 700,
        });
        self.is_resizeable = self.plugin_gui.can_resize(plugin);
        Ok(initial_size)
    }

    #[allow(unsafe_code)]
    #[cfg(windows)]
    pub unsafe fn attach_win32(
        &mut self,
        plugin: &PluginMainThreadHandle<'_>,
        hwnd: *mut std::ffi::c_void,
    ) -> Result<(), Box<dyn Error>> {
        unsafe {
            self.plugin_gui
                .set_parent(plugin, ClapWindow::from_win32_hwnd(hwnd))?;
        }
        let _ = self.plugin_gui.show(plugin);
        self.is_open = true;
        Ok(())
    }

    pub fn apply_host_resize(&mut self, plugin: &PluginMainThreadHandle<'_>, size: GuiSize) {
        if !self.is_resizeable {
            return;
        }
        let working_size = self.plugin_gui.adjust_size(plugin, size).unwrap_or(size);
        let _ = self.plugin_gui.set_size(plugin, working_size);
    }

    pub fn destroy(&mut self, plugin: &PluginMainThreadHandle<'_>) {
        if self.is_open {
            let _ = self.plugin_gui.hide(plugin);
            self.plugin_gui.destroy(plugin);
            self.is_open = false;
        }
    }
}

impl CpalHostMainThread<'_> {}
