use std::ffi::{c_char, c_void};
use std::ptr::null_mut;
use eframe::glow::{self, HasContext};

type EglCreateImageKHR = unsafe extern "C" fn(
    dpy: *mut c_void,
    ctx: *mut c_void,
    target: u32,
    buffer: *mut c_void,
    attrib_list: *const i32,
) -> *mut c_void;

type EglDestroyImageKHR = unsafe extern "C" fn(
    dpy: *mut c_void,
    image: *mut c_void,
) -> u32;

type GlEGLImageTargetTexture2DOES = unsafe extern "C" fn(
    target: u32,
    image: *mut c_void,
);

type EglGetCurrentDisplay = unsafe extern "C" fn() -> *mut c_void;
type EglGetProcAddress = unsafe extern "C" fn(procname: *const c_char) -> *mut c_void;

#[derive(Clone, Copy, Debug)]
pub struct CachedDmaBuffer {
    pub buffer_id: u32,
    pub texture: glow::Texture,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub drm_fourcc: u32,
    pub modifier: u64,
}

pub struct EglDmaBufImporter {
    egl_display: *mut c_void,
    egl_create_image_khr: EglCreateImageKHR,
    egl_destroy_image_khr: EglDestroyImageKHR,
    gl_egl_image_target_texture2d_oes: GlEGLImageTargetTexture2DOES,
    pub current_texture: Option<glow::Texture>,
    pub cached_buffers: Vec<CachedDmaBuffer>,
    last_size: (u32, u32),
    last_captured_at: i64,
}

unsafe impl Send for EglDmaBufImporter {}
unsafe impl Sync for EglDmaBufImporter {}

impl Drop for EglDmaBufImporter {
    fn drop(&mut self) {
        // Texture cleanup will happen via OpenGL context if needed
    }
}

impl EglDmaBufImporter {
    pub fn new() -> Option<Self> {
        unsafe {
            // Load libEGL.so.1 dynamically
            let lib = libc::dlopen(
                b"libEGL.so.1\0".as_ptr() as *const c_char,
                libc::RTLD_LAZY | libc::RTLD_LOCAL,
            );
            if lib.is_null() {
                tracing::warn!("Failed to dlopen libEGL.so.1");
                return None;
            }

            let get_proc_addr_sym = libc::dlsym(lib, b"eglGetProcAddress\0".as_ptr() as *const c_char);
            let get_current_display_sym = libc::dlsym(lib, b"eglGetCurrentDisplay\0".as_ptr() as *const c_char);

            if get_proc_addr_sym.is_null() || get_current_display_sym.is_null() {
                tracing::warn!("Failed to resolve eglGetProcAddress / eglGetCurrentDisplay");
                return None;
            }

            let get_proc_addr: EglGetProcAddress = std::mem::transmute(get_proc_addr_sym);
            let get_current_display: EglGetCurrentDisplay = std::mem::transmute(get_current_display_sym);

            let mut egl_display = get_current_display();
            if egl_display.is_null() {
                let get_display_sym = libc::dlsym(lib, b"eglGetDisplay\0".as_ptr() as *const c_char);
                let initialize_sym = libc::dlsym(lib, b"eglInitialize\0".as_ptr() as *const c_char);
                if !get_display_sym.is_null() {
                    let get_display: unsafe extern "C" fn(*mut c_void) -> *mut c_void = std::mem::transmute(get_display_sym);
                    egl_display = get_display(null_mut());
                    if !egl_display.is_null() && !initialize_sym.is_null() {
                        let init_fn: unsafe extern "C" fn(*mut c_void, *mut i32, *mut i32) -> u32 = std::mem::transmute(initialize_sym);
                        init_fn(egl_display, null_mut(), null_mut());
                    }
                }
            }

            if egl_display.is_null() {
                tracing::warn!("Failed to obtain EGL display (both eglGetCurrentDisplay and eglGetDisplay returned null)");
                return None;
            }

            let create_image_ptr = get_proc_addr(b"eglCreateImageKHR\0".as_ptr() as *const c_char);
            let destroy_image_ptr = get_proc_addr(b"eglDestroyImageKHR\0".as_ptr() as *const c_char);
            let target_tex_ptr = get_proc_addr(b"glEGLImageTargetTexture2DOES\0".as_ptr() as *const c_char);

            if create_image_ptr.is_null() || destroy_image_ptr.is_null() || target_tex_ptr.is_null() {
                tracing::warn!("Failed to resolve EGL DMA-BUF extensions (eglCreateImageKHR/glEGLImageTargetTexture2DOES)");
                return None;
            }

            let egl_create_image_khr: EglCreateImageKHR = std::mem::transmute(create_image_ptr);
            let egl_destroy_image_khr: EglDestroyImageKHR = std::mem::transmute(destroy_image_ptr);
            let gl_egl_image_target_texture2d_oes: GlEGLImageTargetTexture2DOES = std::mem::transmute(target_tex_ptr);

            tracing::info!("EGL DMA-BUF texture importer successfully initialized (display: {:?})", egl_display);

            Some(Self {
                egl_display,
                egl_create_image_khr,
                egl_destroy_image_khr,
                gl_egl_image_target_texture2d_oes,
                current_texture: None,
                cached_buffers: Vec::new(),
                last_size: (0, 0),
                last_captured_at: -1,
            })
        }
    }

    /// Imports a Linux DMA-BUF file descriptor directly into an OpenGL texture.
    ///
    /// When `buffer_id` is non-zero and already present in the cache with identical format
    /// parameters, the cached `glow::Texture` is returned immediately without issuing any
    /// kernel DRM ioctls or EGLImage allocations.
    ///
    /// The import is 100% zero-copy in GPU VRAM.
    pub unsafe fn import_dmabuf(
        &mut self,
        gl: &glow::Context,
        buffer_id: u32,
        fd: i32,
        width: u32,
        height: u32,
        stride: u32,
        drm_fourcc: u32,
        modifier: u64,
        captured_at: i64,
    ) -> Option<glow::Texture> {
        // If stream resolution changed, invalidate and delete all cached textures
        if self.last_size != (0, 0) && self.last_size != (width, height) {
            for entry in self.cached_buffers.drain(..) {
                gl.delete_texture(entry.texture);
            }
            self.current_texture = None;
        }
        self.last_size = (width, height);
        self.last_captured_at = captured_at;

        // Check cache for existing imported buffer
        if buffer_id > 0 {
            if let Some(entry) = self.cached_buffers.iter().find(|e| {
                e.buffer_id == buffer_id
                    && e.width == width
                    && e.height == height
                    && e.stride == stride
                    && e.drm_fourcc == drm_fourcc
                    && e.modifier == modifier
            }) {
                self.current_texture = Some(entry.texture);
                return Some(entry.texture);
            }
        }

        let mut attribs: Vec<i32> = vec![
            0x3057, width as i32,      // EGL_WIDTH
            0x3056, height as i32,     // EGL_HEIGHT
            0x3271, drm_fourcc as i32, // EGL_LINUX_DRM_FOURCC_EXT
            0x3272, fd,                // EGL_DMA_BUF_PLANE0_FD_EXT
            0x3273, 0,                 // EGL_DMA_BUF_PLANE0_OFFSET_EXT
            0x3274, stride as i32,     // EGL_DMA_BUF_PLANE0_PITCH_EXT
        ];

        // If a valid DRM modifier is provided, pass low and high 32-bit parts
        const DRM_FORMAT_MOD_INVALID: u64 = (1u64 << 56) - 1;
        if modifier != 0 && modifier != DRM_FORMAT_MOD_INVALID {
            attribs.push(0x3443); // EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT
            attribs.push((modifier & 0xFFFFFFFF) as i32);
            attribs.push(0x3444); // EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT
            attribs.push(((modifier >> 32) & 0xFFFFFFFF) as i32);
        }

        attribs.push(0x3038); // EGL_NONE

        // Target: EGL_LINUX_DMA_BUF_EXT (0x3270)
        let image = (self.egl_create_image_khr)(
            self.egl_display,
            null_mut(), // EGL_NO_CONTEXT
            0x3270,     // EGL_LINUX_DMA_BUF_EXT
            null_mut(),
            attribs.as_ptr(),
        );

        if image.is_null() {
            tracing::warn!("eglCreateImageKHR failed for DMA-BUF buffer_id {} fd {}", buffer_id, fd);
            return None;
        }

        let tex = gl.create_texture().ok()?;

        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        (self.gl_egl_image_target_texture2d_oes)(glow::TEXTURE_2D, image);

        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);

        // Once bound to the GL texture target, the EGLImage wrapper can be destroyed
        (self.egl_destroy_image_khr)(self.egl_display, image);

        tracing::info!(
            "Imported new DMA-BUF texture to pool (buffer_id: {}, fd: {}, {}x{}, fourcc: 0x{:08x})",
            buffer_id, fd, width, height, drm_fourcc
        );

        self.current_texture = Some(tex);
        if buffer_id > 0 {
            self.cached_buffers.push(CachedDmaBuffer {
                buffer_id,
                texture: tex,
                width,
                height,
                stride,
                drm_fourcc,
                modifier,
            });
        }

        Some(tex)
    }

    /// Deletes all cached OpenGL textures.
    pub unsafe fn destroy(&mut self, gl: &glow::Context) {
        for entry in self.cached_buffers.drain(..) {
            gl.delete_texture(entry.texture);
        }
        self.current_texture = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cached_dma_buffer_lookup() {
        let dummy_tex = glow::NativeTexture(std::num::NonZeroU32::new(42).unwrap());
        let cached = CachedDmaBuffer {
            buffer_id: 1,
            texture: dummy_tex,
            width: 3840,
            height: 2160,
            stride: 15360,
            drm_fourcc: 0x34325241,
            modifier: 0,
        };

        let list = vec![cached];
        let found = list.iter().find(|e| {
            e.buffer_id == 1
                && e.width == 3840
                && e.height == 2160
                && e.stride == 15360
                && e.drm_fourcc == 0x34325241
                && e.modifier == 0
        });
        assert!(found.is_some());
        assert_eq!(found.unwrap().texture, dummy_tex);

        // Mismatched buffer_id should not find
        let not_found = list.iter().find(|e| e.buffer_id == 2);
        assert!(not_found.is_none());

        // Mismatched resolution should not find
        let mismatch_res = list.iter().find(|e| e.buffer_id == 1 && e.width == 1920);
        assert!(mismatch_res.is_none());
    }
}
