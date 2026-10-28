//! Opt-in Mesa software OpenGL context for renderer readback regression tests.
use std::ffi::{c_void, CString};

pub struct SoftwareGl {
    library: *mut c_void,
    display: *mut c_void,
    surface: *mut c_void,
    context: *mut c_void,
}

impl SoftwareGl {
    unsafe fn symbol<T: Copy>(&self, name: &str) -> T {
        let name = CString::new(name).unwrap();
        let address = libc::dlsym(self.library, name.as_ptr());
        assert!(!address.is_null(), "Missing EGL entry point {name:?}");
        std::mem::transmute_copy(&address)
    }

    pub fn new(width: i32, height: i32) -> Self {
        assert_eq!(std::env::var("LIBGL_ALWAYS_SOFTWARE").as_deref(), Ok("1"));
        assert_eq!(std::env::var("EGL_PLATFORM").as_deref(), Ok("surfaceless"));
        unsafe {
            let library = libc::dlopen(c"libEGL.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
            assert!(
                !library.is_null(),
                "Mesa EGL is required for this opt-in test"
            );
            let mut egl = Self {
                library,
                display: std::ptr::null_mut(),
                surface: std::ptr::null_mut(),
                context: std::ptr::null_mut(),
            };
            let get_display: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
                egl.symbol("eglGetDisplay");
            egl.display = get_display(std::ptr::null_mut());
            let initialize: unsafe extern "C" fn(*mut c_void, *mut i32, *mut i32) -> u32 =
                egl.symbol("eglInitialize");
            assert_ne!(
                initialize(egl.display, std::ptr::null_mut(), std::ptr::null_mut()),
                0
            );
            let bind_api: unsafe extern "C" fn(u32) -> u32 = egl.symbol("eglBindAPI");
            assert_ne!(bind_api(0x30a2), 0); // EGL_OPENGL_API
            let choose: unsafe extern "C" fn(
                *mut c_void,
                *const i32,
                *mut *mut c_void,
                i32,
                *mut i32,
            ) -> u32 = egl.symbol("eglChooseConfig");
            let attributes = [
                0x3033, 1, 0x3040, 8, 0x3024, 8, 0x3023, 8, 0x3022, 8, 0x3021, 8, 0x3038,
            ];
            let mut config = std::ptr::null_mut();
            let mut count = 0;
            assert_ne!(
                choose(egl.display, attributes.as_ptr(), &mut config, 1, &mut count),
                0
            );
            assert_eq!(count, 1);
            let pbuffer: unsafe extern "C" fn(*mut c_void, *mut c_void, *const i32) -> *mut c_void =
                egl.symbol("eglCreatePbufferSurface");
            egl.surface = pbuffer(
                egl.display,
                config,
                [0x3057, width, 0x3056, height, 0x3038].as_ptr(),
            );
            assert!(!egl.surface.is_null());
            let create: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *const i32,
            ) -> *mut c_void = egl.symbol("eglCreateContext");
            egl.context = create(
                egl.display,
                config,
                std::ptr::null_mut(),
                [0x3098, 4, 0x30fb, 5, 0x3038].as_ptr(),
            );
            assert!(
                !egl.context.is_null(),
                "Software OpenGL 4.5 context required"
            );
            let make_current: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *mut c_void,
            ) -> u32 = egl.symbol("eglMakeCurrent");
            assert_ne!(
                make_current(egl.display, egl.surface, egl.surface, egl.context),
                0
            );
            egl
        }
    }

    pub fn context(&self) -> eframe::glow::Context {
        unsafe {
            let address: unsafe extern "C" fn(*const libc::c_char) -> *const c_void =
                self.symbol("eglGetProcAddress");
            eframe::glow::Context::from_loader_function(|name| {
                address(CString::new(name).unwrap().as_ptr())
            })
        }
    }
}

impl Drop for SoftwareGl {
    fn drop(&mut self) {
        unsafe {
            let make_current: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *mut c_void,
            ) -> u32 = self.symbol("eglMakeCurrent");
            make_current(
                self.display,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            let destroy_context: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32 =
                self.symbol("eglDestroyContext");
            if !self.context.is_null() {
                destroy_context(self.display, self.context);
            }
            let destroy_surface: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32 =
                self.symbol("eglDestroySurface");
            if !self.surface.is_null() {
                destroy_surface(self.display, self.surface);
            }
            let terminate: unsafe extern "C" fn(*mut c_void) -> u32 = self.symbol("eglTerminate");
            if !self.display.is_null() {
                terminate(self.display);
            }
            libc::dlclose(self.library);
        }
    }
}
