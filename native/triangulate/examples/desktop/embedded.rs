//! The Explorer worker creates a real child window, not a reparented top-level
//! app. The native preview owner controls its lifetime and cancels on selection.
use super::Result;
use std::{ffi::c_void, num::NonZeroIsize, path::PathBuf};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle, Win32WindowHandle};
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    window::{Window, WindowAttributes},
};
type Hwnd = *mut c_void;
#[repr(C)]
#[derive(Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[link(name = "user32")]
extern "system" {
    fn IsWindow(window: Hwnd) -> i32;
    fn GetClientRect(window: Hwnd, rect: *mut Rect) -> i32;
    fn GetClassNameW(window: Hwnd, text: *mut u16, count: i32) -> i32;
    fn GetWindowDpiAwarenessContext(window: Hwnd) -> Hwnd;
    fn SetThreadDpiAwarenessContext(context: Hwnd) -> Hwnd;
    fn PostMessageW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> i32;
    fn SetFocus(window: Hwnd) -> Hwnd;
    fn GetFocus() -> Hwnd;
    fn GetForegroundWindow() -> Hwnd;
    fn GetAncestor(window: Hwnd, flags: u32) -> Hwnd;
    fn SetPropW(window: Hwnd, name: *const u16, value: Hwnd) -> i32;
}
pub const READY: u32 = 0x8000 + 71;
pub const CACHED: u32 = 0x8000 + 72;
#[derive(Clone)]
pub struct EmbeddedPreview {
    pub parent: usize,
    pub cache: Option<PathBuf>,
}
impl EmbeddedPreview {
    pub fn new(parent: usize, cache: Option<PathBuf>) -> Result<Self> {
        let this = Self { parent, cache };
        if !this.alive() {
            return Err("Explorer preview owner is no longer available".into());
        }
        let mut name = [0u16; 128];
        let n = unsafe { GetClassNameW(parent as Hwnd, name.as_mut_ptr(), 128) };
        if n <= 0 || String::from_utf16_lossy(&name[..n as usize]) != "VTXExplorerPreview" {
            return Err("Invalid VTX preview owner".into());
        }
        unsafe {
            let context = GetWindowDpiAwarenessContext(parent as Hwnd);
            if !context.is_null() {
                SetThreadDpiAwarenessContext(context);
            }
        }
        Ok(this)
    }
    pub fn alive(&self) -> bool {
        unsafe { IsWindow(self.parent as Hwnd) != 0 }
    }
    pub fn attributes(&self, attributes: WindowAttributes) -> Result<WindowAttributes> {
        let mut rect = Rect::default();
        if unsafe { GetClientRect(self.parent as Hwnd, &mut rect) } == 0 {
            return Err("Unable to size Explorer preview".into());
        }
        let handle = Win32WindowHandle::new(
            NonZeroIsize::new(self.parent as isize).ok_or("Missing preview parent")?,
        );
        // Native owner retains the parent until this worker is cancelled.
        let attributes =
            unsafe { attributes.with_parent_window(Some(RawWindowHandle::Win32(handle))) };
        Ok(attributes
            .with_visible(false)
            .with_active(false)
            .with_decorations(false)
            .with_drag_and_drop(false)
            .with_skip_taskbar(true)
            .with_class_name("VTXLivePreview")
            .with_position(PhysicalPosition::new(0, 48))
            .with_inner_size(PhysicalSize::new(
                rect.right.max(1) as u32,
                (rect.bottom - 48).max(1) as u32,
            )))
    }
    pub fn ready(&self, window: &Window, retain_saved: bool) -> Result<usize> {
        let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
            return Err("Expected Windows preview window".into());
        };
        let child = handle.hwnd.get() as usize;
        unsafe {
            PostMessageW(self.parent as Hwnd, READY, child, retain_saved as isize);
        }
        Ok(child)
    }
    pub fn focus(window: &Window) {
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(handle) = handle.as_raw() {
                unsafe {
                    SetFocus(handle.hwnd.get() as Hwnd);
                }
            }
        }
    }
    pub fn focus_on_hover(window: &Window) {
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(handle) = handle.as_raw() {
                let child = handle.hwnd.get() as Hwnd;
                unsafe {
                    // Wheel messages target the focus window. Only move focus
                    // within the already-active Explorer; never activate a
                    // background Explorer window merely by moving the pointer.
                    if GetAncestor(child, 2) == GetForegroundWindow() && GetFocus() != child {
                        SetFocus(child);
                    }
                }
            }
        }
    }
    pub fn presented_zoom(window: &Window, zoom: f32) {
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(handle) = handle.as_raw() {
                // Native integration diagnostics: this value is published only
                // after a GPU frame was presented, rather than on input receipt.
                static NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
                let name = NAME
                    .get_or_init(|| "VTX.PresentedZoom".encode_utf16().chain(Some(0)).collect());
                unsafe {
                    SetPropW(
                        handle.hwnd.get() as Hwnd,
                        name.as_ptr(),
                        zoom.to_bits() as usize as Hwnd,
                    );
                }
            }
        }
    }
    pub fn cached(parent: usize, child: usize) {
        unsafe {
            PostMessageW(parent as Hwnd, CACHED, child, 0);
        }
    }
}
