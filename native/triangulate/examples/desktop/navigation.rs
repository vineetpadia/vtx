//! SolidWorks mouse bindings, shared by the app and Explorer worker.
use std::time::Instant;
use winit::{event::MouseButton, keyboard::ModifiersState};

#[derive(Default)]
pub struct Clicks {
    origin: Option<[f64; 2]>,
    moved: bool,
    previous: Option<(Instant, [f64; 2])>,
}
fn nearby(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() <= 5. && (a[1] - b[1]).abs() <= 5.
}
impl Clicks {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn press(&mut self, p: Option<[f64; 2]>) {
        self.origin = p;
        self.moved = false;
    }
    pub fn motion(&mut self, p: [f64; 2]) {
        if self.origin.is_some_and(|o| !nearby(o, p)) {
            self.moved = true;
        }
    }
    pub fn release(&mut self, now: Instant, p: Option<[f64; 2]>, ms: u32) -> bool {
        let eligible = self.origin.take().zip(p).is_some_and(|(o, p)| nearby(o, p)) && !self.moved;
        if !eligible {
            self.clear();
            return false;
        }
        let p = p.unwrap();
        let previous = self.previous.take();
        if previous.is_some_and(|(t, old)| {
            now.duration_since(t).as_millis() <= u128::from(ms) && nearby(old, p)
        }) {
            true
        } else {
            self.previous = Some((now, p));
            false
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Drag {
    Orbit,
    Pan,
    Zoom,
    Roll,
}
pub fn drag(button: Option<MouseButton>, modifiers: ModifiersState, drawing: bool) -> Option<Drag> {
    if button == Some(MouseButton::Left) && drawing {
        return Some(Drag::Pan);
    }
    if button != Some(MouseButton::Middle) {
        return None;
    }
    Some(if modifiers.control_key() {
        Drag::Pan
    } else if modifiers.shift_key() {
        Drag::Zoom
    } else if modifiers.alt_key() && !drawing {
        Drag::Roll
    } else if drawing {
        Drag::Pan
    } else {
        Drag::Orbit
    })
}

pub struct Settings {
    pub rotate_speed: f32,
    pub reverse_wheel: bool,
    pub zoom_about_center: bool,
    pub double_click_ms: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            rotate_speed: 1.,
            reverse_wheel: false,
            zoom_about_center: false,
            double_click_ms: 500,
        }
    }
}
impl Settings {
    pub fn load() -> Self {
        #[cfg(windows)]
        {
            windows::load()
        }
        #[cfg(not(windows))]
        {
            Self::default()
        }
    }
    pub fn wheel_exponent(&self, lines: f32) -> f32 {
        lines * if self.reverse_wheel { 0.12 } else { -0.12 }
    }
}

#[cfg(windows)]
mod windows {
    use super::Settings;
    type Key = isize;
    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            key: Key,
            path: *const u16,
            options: u32,
            access: u32,
            out: *mut Key,
        ) -> i32;
        fn RegEnumKeyExW(
            key: Key,
            index: u32,
            name: *mut u16,
            len: *mut u32,
            reserved: *mut u32,
            class: *mut u16,
            class_len: *mut u32,
            time: *mut u64,
        ) -> i32;
        fn RegGetValueW(
            key: Key,
            path: *const u16,
            name: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut std::ffi::c_void,
            size: *mut u32,
        ) -> i32;
        fn RegCloseKey(key: Key) -> i32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetDoubleClickTime() -> u32;
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn value(key: Key, name: &str) -> Option<String> {
        let mut data = [0u16; 128];
        let mut size = 256u32;
        let mut kind = 0;
        if unsafe {
            RegGetValueW(
                key,
                std::ptr::null(),
                wide(name).as_ptr(),
                0x12,
                &mut kind,
                data.as_mut_ptr().cast(),
                &mut size,
            )
        } != 0
        {
            return None;
        }
        if kind == 4 {
            Some(u32::from_ne_bytes(unsafe { *(data.as_ptr().cast::<[u8; 4]>()) }).to_string())
        } else {
            Some(String::from_utf16_lossy(
                &data[..(size as usize / 2).saturating_sub(1).min(127)],
            ))
        }
    }
    pub fn load() -> Settings {
        let mut settings = Settings::default();
        settings.double_click_ms = unsafe { GetDoubleClickTime() };
        let mut root = 0;
        if unsafe {
            RegOpenKeyExW(
                0x80000001u32 as i32 as isize,
                wide("Software\\SolidWorks").as_ptr(),
                0,
                0x20019,
                &mut root,
            )
        } != 0
        {
            return settings;
        }
        let mut versions = Vec::new();
        for i in 0..128 {
            let mut name = [0u16; 128];
            let mut len = 128;
            if unsafe {
                RegEnumKeyExW(
                    root,
                    i,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            } != 0
            {
                break;
            }
            let name = String::from_utf16_lossy(&name[..len as usize]);
            if name
                .strip_prefix("SOLIDWORKS ")
                .is_some_and(|v| v.len() == 4 && v.bytes().all(|c| c.is_ascii_digit()))
            {
                versions.push(name);
            }
        }
        versions.sort();
        for version in versions.iter().rev() {
            let mut general = 0;
            if unsafe {
                RegOpenKeyExW(
                    root,
                    wide(&format!("{version}\\General")).as_ptr(),
                    0,
                    0x20019,
                    &mut general,
                )
            } != 0
            {
                continue;
            }
            if let Some(speed) = value(general, "Mouse Rotate Speed")
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|s| s.is_finite() && *s > 0.)
            {
                settings.rotate_speed = speed.clamp(0.05, 20.);
            }
            if let Some(v) = value(general, "Reverse Zoom Direction") {
                settings.reverse_wheel = v == "1";
            }
            if let Some(v) = value(general, "Zoom About Screen Center") {
                settings.zoom_about_center = v == "1";
            }
            unsafe {
                RegCloseKey(general);
            }
            break;
        }
        unsafe {
            RegCloseKey(root);
        }
        settings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn solidworks_bindings_and_drawing_pan() {
        assert_eq!(
            drag(Some(MouseButton::Middle), ModifiersState::empty(), false),
            Some(Drag::Orbit)
        );
        assert_eq!(
            drag(Some(MouseButton::Middle), ModifiersState::CONTROL, false),
            Some(Drag::Pan)
        );
        assert_eq!(
            drag(Some(MouseButton::Middle), ModifiersState::SHIFT, false),
            Some(Drag::Zoom)
        );
        assert_eq!(
            drag(Some(MouseButton::Middle), ModifiersState::ALT, false),
            Some(Drag::Roll)
        );
        assert_eq!(
            drag(Some(MouseButton::Left), ModifiersState::empty(), false),
            None
        );
        assert_eq!(
            drag(Some(MouseButton::Right), ModifiersState::empty(), false),
            None
        );
        assert_eq!(
            drag(Some(MouseButton::Middle), ModifiersState::empty(), true),
            Some(Drag::Pan)
        );
        assert_eq!(
            drag(Some(MouseButton::Left), ModifiersState::empty(), true),
            Some(Drag::Pan)
        );
    }
    #[test]
    fn default_wheel_matches_solidworks_and_reverse_setting() {
        let mut s = Settings::default();
        assert!(s.wheel_exponent(1.) < 0.);
        assert!(s.wheel_exponent(-1.) > 0.);
        s.reverse_wheel = true;
        assert!(s.wheel_exponent(1.) > 0.);
    }
    #[test]
    fn double_click_fit_excludes_drags_even_when_pointer_returns() {
        let now = Instant::now();
        let p = Some([100., 100.]);
        let mut clicks = Clicks::default();
        clicks.press(p);
        assert!(!clicks.release(now, p, 500));
        clicks.press(p);
        assert!(clicks.release(now + std::time::Duration::from_millis(100), p, 500));
        clicks.press(p);
        clicks.motion([130., 100.]);
        clicks.motion([100., 100.]);
        assert!(!clicks.release(now, p, 500));
        clicks.press(p);
        assert!(!clicks.release(now, p, 500));
    }
}
