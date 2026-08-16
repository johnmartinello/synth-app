//! Separate native window for the Surge XT editor (not the Tauri webview).
#![allow(unsafe_code)]

use super::gui::Gui;
use clack_extensions::gui::GuiSize;
use clack_host::prelude::PluginMainThreadHandle;
use std::error::Error;

pub struct EditorEvent {
    pub closed: bool,
    pub resize: Option<GuiSize>,
}

pub struct EditorFrame {
    inner: Inner,
}

impl EditorFrame {
    pub fn open(
        gui: &mut Gui,
        plugin: &PluginMainThreadHandle<'_>,
    ) -> Result<Self, Box<dyn Error>> {
        Inner::open(gui, plugin).map(|inner| Self { inner })
    }

    pub fn pump(&mut self) -> EditorEvent {
        self.inner.pump()
    }

    pub fn request_resize(&self, size: GuiSize) {
        self.inner.request_resize(size);
    }

    pub fn close(self, gui: &mut Gui, plugin: &PluginMainThreadHandle<'_>) {
        self.inner.close(gui, plugin);
    }

    pub fn hwnd_raw(&self) -> Option<isize> {
        self.inner.hwnd_raw()
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::mem::MaybeUninit;
    use std::ptr;
    use std::sync::OnceLock;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::UpdateWindow;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetAncestor, GetClientRect, GetForegroundWindow, GetWindowLongPtrW, PeekMessageW,
        RegisterClassW, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage,
        CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GA_ROOT, GWLP_USERDATA, GWL_EXSTYLE,
        GWL_STYLE, MSG, PM_REMOVE, SIZE_MINIMIZED, SWP_NOMOVE, SWP_NOZORDER, SW_SHOW, WM_CLOSE,
        WM_NCCREATE, WM_QUIT, WM_SIZE, WNDCLASSW, WS_CLIPCHILDREN, WS_EX_APPWINDOW,
        WS_OVERLAPPEDWINDOW,
    };

    const CLASS_NAME: &[u16] = &[
        'S' as u16, 'u' as u16, 'r' as u16, 'g' as u16, 'e' as u16, 'C' as u16, 'h' as u16,
        'o' as u16, 'r' as u16, 'd' as u16, 'E' as u16, 'd' as u16, 'i' as u16, 't' as u16,
        'o' as u16, 'r' as u16, 0,
    ];

    struct EditorState {
        close: bool,
        resize: Option<(u32, u32)>,
        ignore_size: bool,
    }

    pub struct Inner {
        hwnd: HWND,
        state: *mut EditorState,
    }

    impl Inner {
        pub fn open(
            gui: &mut Gui,
            plugin: &PluginMainThreadHandle<'_>,
        ) -> Result<Self, Box<dyn Error>> {
            let size = gui.begin_embedded(plugin)?;
            let state = Box::into_raw(Box::new(EditorState {
                close: false,
                resize: None,
                ignore_size: false,
            }));
            let hwnd = match unsafe { create_host_window(size, state) } {
                Ok(hwnd) => hwnd,
                Err(err) => {
                    unsafe { drop(Box::from_raw(state)) };
                    gui.destroy(plugin);
                    return Err(err);
                }
            };
            if let Err(err) = unsafe { gui.attach_win32(plugin, hwnd) } {
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    DestroyWindow(hwnd);
                    drop(Box::from_raw(state));
                }
                gui.destroy(plugin);
                return Err(err);
            }
            unsafe {
                ShowWindow(hwnd, SW_SHOW);
                UpdateWindow(hwnd);
            }
            Ok(Self { hwnd, state })
        }

        pub fn hwnd_raw(&self) -> Option<isize> {
            Some(self.hwnd as isize)
        }

        pub fn pump(&mut self) -> EditorEvent {
            drain_messages();
            let state = unsafe { &mut *self.state };
            EditorEvent {
                closed: state.close,
                resize: state.resize.take().map(|(width, height)| GuiSize { width, height }),
            }
        }

        pub fn request_resize(&self, size: GuiSize) {
            unsafe {
                (*self.state).ignore_size = true;
                set_client_size(self.hwnd, size.width, size.height);
                (*self.state).ignore_size = false;
            }
        }

        pub fn close(self, gui: &mut Gui, plugin: &PluginMainThreadHandle<'_>) {
            gui.destroy(plugin);
            unsafe {
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                DestroyWindow(self.hwnd);
            }
            drain_messages();
            unsafe { drop(Box::from_raw(self.state)) };
            std::thread::sleep(Duration::from_millis(50));
            drain_messages();
        }
    }

    fn drain_messages() {
        unsafe {
            let mut msg = MaybeUninit::<MSG>::uninit();
            while PeekMessageW(msg.as_mut_ptr(), ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                let msg = msg.assume_init();
                if msg.message == WM_QUIT {
                    continue;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn create_host_window(
        size: GuiSize,
        state: *mut EditorState,
    ) -> Result<HWND, Box<dyn Error>> {
        register_class()?;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: size.width as i32,
            bottom: size.height as i32,
        };
        let style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
        let ex_style = WS_EX_APPWINDOW;
        AdjustWindowRectEx(&mut rect, style, 0, ex_style);
        let title = wide("Surge XT");
        let hwnd = CreateWindowExW(
            ex_style,
            CLASS_NAME.as_ptr(),
            title.as_ptr(),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            ptr::null_mut(),
            ptr::null_mut(),
            GetModuleHandleW(ptr::null()),
            state.cast(),
        );
        if hwnd.is_null() {
            Err("Could not create Surge editor window".into())
        } else {
            Ok(hwnd)
        }
    }

    fn register_class() -> Result<(), Box<dyn Error>> {
        static CLASS: OnceLock<Result<(), String>> = OnceLock::new();
        CLASS
            .get_or_init(|| unsafe {
                let wc = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(wnd_proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: GetModuleHandleW(ptr::null()),
                    hIcon: ptr::null_mut(),
                    hCursor: ptr::null_mut(),
                    hbrBackground: ptr::null_mut(),
                    lpszMenuName: ptr::null(),
                    lpszClassName: CLASS_NAME.as_ptr(),
                };
                if RegisterClassW(&wc) == 0 {
                    Err("Could not register Surge editor window class".into())
                } else {
                    Ok(())
                }
            })
            .clone()
            .map_err(|e| e.into())
    }

    unsafe fn set_client_size(hwnd: HWND, width: u32, height: u32) {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        AdjustWindowRectEx(&mut rect, style, 0, ex_style);
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            0,
            0,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOMOVE | SWP_NOZORDER,
        );
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_NCCREATE {
            let create = lparam as *const CREATESTRUCTW;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*create).lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut EditorState;
        if state.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        match msg {
            WM_CLOSE => {
                (*state).close = true;
                0
            }
            WM_SIZE => {
                if wparam != SIZE_MINIMIZED as usize && !(*state).ignore_size {
                    let mut client = RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    if GetClientRect(hwnd, &mut client) != 0 {
                        let width = (client.right - client.left).max(1) as u32;
                        let height = (client.bottom - client.top).max(1) as u32;
                        (*state).resize = Some((width, height));
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    pub fn midi_targets_surge(app_hwnd: Option<isize>, editor_hwnd: Option<isize>, last: bool) -> bool {
        let fg = unsafe { GetForegroundWindow() };
        if fg.is_null() {
            return last;
        }
        let fg_root = window_root(fg);
        if let Some(ed) = editor_hwnd {
            if window_root(ed as HWND) == fg_root {
                return true;
            }
        }
        if let Some(app) = app_hwnd {
            if window_root(app as HWND) == fg_root {
                return false;
            }
        }
        last
    }

    fn window_root(hwnd: HWND) -> HWND {
        unsafe {
            let root = GetAncestor(hwnd, GA_ROOT);
            if root.is_null() {
                hwnd
            } else {
                root
            }
        }
    }
}

#[cfg(windows)]
use win::Inner;

#[cfg(not(windows))]
mod other {
    use super::*;

    pub struct Inner;

    impl Inner {
        pub fn open(
            _gui: &mut Gui,
            _plugin: &PluginMainThreadHandle<'_>,
        ) -> Result<Self, Box<dyn Error>> {
            Err("Plugin editor is only supported on Windows".into())
        }

        pub fn pump(&mut self) -> EditorEvent {
            EditorEvent {
                closed: false,
                resize: None,
            }
        }

        pub fn request_resize(&self, _size: GuiSize) {}

        pub fn close(self, gui: &mut Gui, plugin: &PluginMainThreadHandle<'_>) {
            gui.destroy(plugin);
        }

        pub fn hwnd_raw(&self) -> Option<isize> {
            None
        }
    }
}

#[cfg(not(windows))]
use other::Inner;

/// True when the Surge editor window (or a child of it) is the foreground window.
pub fn midi_targets_surge(app_hwnd: Option<isize>, editor_hwnd: Option<isize>, last: bool) -> bool {
    #[cfg(windows)]
    {
        win::midi_targets_surge(app_hwnd, editor_hwnd, last)
    }
    #[cfg(not(windows))]
    {
        let _ = (app_hwnd, editor_hwnd);
        last
    }
}
