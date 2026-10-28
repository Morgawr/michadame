//! Read a controller for embedded Libretro cores without grabbing it. External
//! Steam games read their controllers directly; this module is never loaded there.
use anyhow::{Context, Result};
use libloading::Library;
use std::ffi::{c_char, c_int, c_void, CStr};

type Controller = *mut c_void;

pub struct Gamepad {
    _library: Library,
    controller: Controller,
    initialized: bool,
    num: unsafe extern "C" fn() -> c_int,
    is_controller: unsafe extern "C" fn(c_int) -> c_int,
    name: unsafe extern "C" fn(c_int) -> *const c_char,
    open: unsafe extern "C" fn(c_int) -> Controller,
    close: unsafe extern "C" fn(Controller),
    attached: unsafe extern "C" fn(Controller) -> c_int,
    update: unsafe extern "C" fn(),
    button: unsafe extern "C" fn(Controller, c_int) -> u8,
    axis: unsafe extern "C" fn(Controller, c_int) -> i16,
    quit: unsafe extern "C" fn(u32),
}

impl Gamepad {
    pub fn new() -> Result<Self> {
        // Loading SDL2 only here keeps desktop startup
        // and the build-time dependency set unchanged.
        unsafe {
            let library = Library::new("libSDL2-2.0.so.0")
                .or_else(|_| Library::new("libSDL2.so"))
                .context("SDL2 is required for PC-88/PC-98 gamepad input")?;
            let init: unsafe extern "C" fn(u32) -> c_int = *library.get(b"SDL_InitSubSystem\0")?;
            let hint: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int =
                *library.get(b"SDL_SetHint\0")?;
            let event_state: unsafe extern "C" fn(c_int) -> c_int =
                *library.get(b"SDL_JoystickEventState\0")?;
            let mut pad = Self {
                controller: std::ptr::null_mut(),
                initialized: false,
                num: *library.get(b"SDL_NumJoysticks\0")?,
                is_controller: *library.get(b"SDL_IsGameController\0")?,
                name: *library.get(b"SDL_GameControllerNameForIndex\0")?,
                open: *library.get(b"SDL_GameControllerOpen\0")?,
                close: *library.get(b"SDL_GameControllerClose\0")?,
                attached: *library.get(b"SDL_GameControllerGetAttached\0")?,
                update: *library.get(b"SDL_GameControllerUpdate\0")?,
                button: *library.get(b"SDL_GameControllerGetButton\0")?,
                axis: *library.get(b"SDL_GameControllerGetAxis\0")?,
                quit: *library.get(b"SDL_QuitSubSystem\0")?,
                _library: library,
            };
            hint(
                c"SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS".as_ptr(),
                c"1".as_ptr(),
            );
            // Resolve every symbol before initialization; partial loads need no cleanup.
            if init(0x2000) != 0 {
                anyhow::bail!("Could not initialize SDL2 controller input");
            }
            pad.initialized = true;
            // We poll state ourselves; do not accumulate unconsumed SDL events.
            event_state(0);
            Ok(pad)
        }
    }

    pub fn snapshot(&mut self) -> (u16, [[i16; 2]; 2]) {
        unsafe {
            (self.update)();
            if !self.controller.is_null() && (self.attached)(self.controller) == 0 {
                (self.close)(self.controller);
                self.controller = std::ptr::null_mut();
            }
            if self.controller.is_null() {
                let mut candidates: Vec<_> = (0..(self.num)())
                    .filter(|&index| (self.is_controller)(index) != 0)
                    .collect();
                candidates.sort_by_key(|&index| {
                    let name = (self.name)(index);
                    let virtual_pad = !name.is_null()
                        && CStr::from_ptr(name)
                            .to_string_lossy()
                            .contains("Steam Virtual");
                    !virtual_pad
                });
                for index in candidates {
                    self.controller = (self.open)(index);
                    if !self.controller.is_null() {
                        break;
                    }
                }
            }
            if self.controller.is_null() {
                return (0, [[0; 2]; 2]);
            }
            let buttons = std::array::from_fn(|i| (self.button)(self.controller, i as c_int) != 0);
            let axes = std::array::from_fn(|i| (self.axis)(self.controller, i as c_int));
            map_snapshot(buttons, axes)
        }
    }
}

pub fn map_snapshot(buttons: [bool; 15], axes: [i16; 6]) -> (u16, [[i16; 2]; 2]) {
    let mut mask = 0;
    // SDL's Xbox names -> Libretro's Nintendo positions, preserving physical controls.
    for (sdl, retro) in [
        (0, 0),
        (1, 8),
        (2, 1),
        (3, 9),
        (4, 2),
        (6, 3),
        (7, 14),
        (8, 15),
        (9, 10),
        (10, 11),
        (11, 4),
        (12, 5),
        (13, 6),
        (14, 7),
    ] {
        if buttons[sdl] {
            mask |= 1 << retro;
        }
    }
    if axes[4] > 16_000 {
        mask |= 1 << 12;
    }
    if axes[5] > 16_000 {
        mask |= 1 << 13;
    }
    // Cores without analogue support can still use the left stick as a D-pad.
    if axes[0] < -16_000 {
        mask |= 1 << 6;
    }
    if axes[0] > 16_000 {
        mask |= 1 << 7;
    }
    if axes[1] < -16_000 {
        mask |= 1 << 4;
    }
    if axes[1] > 16_000 {
        mask |= 1 << 5;
    }
    (mask, [[axes[0], axes[1]], [axes[2], axes[3]]])
}

impl Drop for Gamepad {
    fn drop(&mut self) {
        unsafe {
            if !self.controller.is_null() {
                (self.close)(self.controller);
            }
            if self.initialized {
                (self.quit)(0x2000);
            }
        }
    }
}
