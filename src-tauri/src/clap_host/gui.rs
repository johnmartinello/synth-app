use crate::clap_host::{CpalHostMainThread, CpalHostShared, MainThreadMessage};
use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, GuiError, GuiSize, HostGuiImpl, PluginGui, Window as ClapWindow,
};
use clack_host::prelude::*;
use std::error::Error;
use winit::dpi::{LogicalSize, PhysicalSize, Size};
use winit::event_loop::EventLoop;
use winit::window::Window;

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
        // Prefer a separate floating native window (not the Tauri webview).
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

    #[allow(dead_code)]
    pub fn gui_size_to_winit_size(&self, size: GuiSize) -> Size {
        let Some(GuiConfiguration { api_type, .. }) = self.configuration else {
            panic!("GUI size on incompatible plugin");
        };
        if api_type.uses_logical_size() {
            LogicalSize {
                width: size.width,
                height: size.height,
            }
            .into()
        } else {
            PhysicalSize {
                width: size.width,
                height: size.height,
            }
            .into()
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

    #[allow(unsafe_code, deprecated)]
    pub fn open_embedded(
        &mut self,
        plugin: &PluginMainThreadHandle<'_>,
        event_loop: &EventLoop<()>,
    ) -> Result<Window, Box<dyn Error>> {
        let gui = self.plugin_gui;
        let Some(configuration) = self.configuration else {
            panic!("open_embedded on incompatible plugin");
        };
        if configuration.is_floating {
            panic!("open_embedded on incompatible plugin");
        }
        gui.create(plugin, configuration)?;
        let initial_size = gui.get_size(plugin).unwrap_or(GuiSize {
            width: 960,
            height: 700,
        });
        self.is_resizeable = gui.can_resize(plugin);
        let window = event_loop.create_window(
            Window::default_attributes()
                .with_title("Surge XT")
                .with_inner_size(PhysicalSize {
                    height: initial_size.height,
                    width: initial_size.width,
                })
                .with_resizable(self.is_resizeable),
        )?;
        unsafe {
            gui.set_parent(plugin, ClapWindow::from_window(&window).unwrap())?;
        }
        let _ = gui.show(plugin);
        self.is_open = true;
        Ok(window)
    }

    #[allow(dead_code)]
    pub fn resize(
        &mut self,
        plugin: &PluginMainThreadHandle<'_>,
        size: PhysicalSize<u32>,
        scale_factor: f64,
    ) -> Size {
        let uses_logical_pixels = self.configuration.unwrap().api_type.uses_logical_size();
        let size = if uses_logical_pixels {
            let size = size.to_logical(scale_factor);
            GuiSize {
                width: size.width,
                height: size.height,
            }
        } else {
            GuiSize {
                width: size.width,
                height: size.height,
            }
        };
        if !self.is_resizeable {
            let forced_size = self.plugin_gui.get_size(plugin).unwrap_or(size);
            return self.gui_size_to_winit_size(forced_size);
        }
        let working_size = self.plugin_gui.adjust_size(plugin, size).unwrap_or(size);
        let _ = self.plugin_gui.set_size(plugin, working_size);
        self.gui_size_to_winit_size(working_size)
    }

    pub fn destroy(&mut self, plugin: &PluginMainThreadHandle<'_>) {
        if self.is_open {
            self.plugin_gui.destroy(plugin);
            self.is_open = false;
        }
    }
}

impl CpalHostMainThread<'_> {}
