use crate::gamescope::pipewire_dma::DmaBufFrame;
use eframe::glow::{self, HasContext};
use std::ffi::{c_char, c_void};
use std::os::unix::io::AsRawFd;
use std::ptr::null_mut;

pub const MAX_CACHED_DMA_BUFFERS: usize = 16;

type EglCreateImageKHR = unsafe extern "C" fn(
    dpy: *mut c_void,
    ctx: *mut c_void,
    target: u32,
    buffer: *mut c_void,
    attrib_list: *const i32,
) -> *mut c_void;

type EglDestroyImageKHR = unsafe extern "C" fn(dpy: *mut c_void, image: *mut c_void) -> u32;

type GlEGLImageTargetTexture2DOES = unsafe extern "C" fn(target: u32, image: *mut c_void);

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

struct EglImageGuard {
    display: *mut c_void,
    image: *mut c_void,
    destroy_fn: EglDestroyImageKHR,
}

impl Drop for EglImageGuard {
    fn drop(&mut self) {
        if !self.image.is_null() {
            unsafe {
                (self.destroy_fn)(self.display, self.image);
            }
            self.image = null_mut();
        }
    }
}

static ORPHANED_DMA_TEXTURES: std::sync::Mutex<Vec<glow::Texture>> =
    std::sync::Mutex::new(Vec::new());

static REMOVED_DMA_BUFFER_IDS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// Queues a removed PipeWire buffer ID so the EGL importer evicts its cached texture on the render thread.
pub fn notify_dma_buffer_removed(buffer_id: u32) {
    if buffer_id == 0 {
        return;
    }
    if let Ok(mut list) = REMOVED_DMA_BUFFER_IDS.lock() {
        if !list.contains(&buffer_id) {
            if list.len() >= 128 {
                list.remove(0);
            }
            list.push(buffer_id);
        }
    }
}

fn take_pending_removed_buffer_ids() -> Vec<u32> {
    if let Ok(mut list) = REMOVED_DMA_BUFFER_IDS.lock() {
        std::mem::take(&mut *list)
    } else {
        Vec::new()
    }
}

/// Deletes any OpenGL textures left behind by dropped `EglDmaBufImporter` instances.
pub unsafe fn drain_orphaned_dma_textures(gl: &glow::Context) {
    if let Ok(mut list) = ORPHANED_DMA_TEXTURES.lock() {
        for tex in list.drain(..) {
            gl.delete_texture(tex);
        }
    }
}

pub struct EglDmaBufImporter {
    lib_handle: *mut c_void,
    egl_display: *mut c_void,
    egl_create_image_khr: EglCreateImageKHR,
    egl_destroy_image_khr: EglDestroyImageKHR,
    gl_egl_image_target_texture2d_oes: GlEGLImageTargetTexture2DOES,
    pub current_texture: Option<glow::Texture>,
    pub cached_buffers: Vec<CachedDmaBuffer>,
    last_size: (u32, u32),
    last_pool_generation: u64,
    last_captured_at: i64,
}

unsafe impl Send for EglDmaBufImporter {}
unsafe impl Sync for EglDmaBufImporter {}

impl Drop for EglDmaBufImporter {
    fn drop(&mut self) {
        if !self.cached_buffers.is_empty() {
            if let Ok(mut orphaned) = ORPHANED_DMA_TEXTURES.lock() {
                for entry in self.cached_buffers.drain(..) {
                    orphaned.push(entry.texture);
                }
            }
        }
        self.current_texture = None;
        if !self.lib_handle.is_null() {
            unsafe {
                libc::dlclose(self.lib_handle);
            }
            self.lib_handle = null_mut();
        }
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

            let get_proc_addr_sym =
                libc::dlsym(lib, b"eglGetProcAddress\0".as_ptr() as *const c_char);
            let get_current_display_sym =
                libc::dlsym(lib, b"eglGetCurrentDisplay\0".as_ptr() as *const c_char);

            if get_proc_addr_sym.is_null() || get_current_display_sym.is_null() {
                tracing::warn!("Failed to resolve eglGetProcAddress / eglGetCurrentDisplay");
                libc::dlclose(lib);
                return None;
            }

            let get_proc_addr: EglGetProcAddress = std::mem::transmute(get_proc_addr_sym);
            let get_current_display: EglGetCurrentDisplay =
                std::mem::transmute(get_current_display_sym);

            let mut egl_display = get_current_display();
            if egl_display.is_null() {
                let get_display_sym =
                    libc::dlsym(lib, b"eglGetDisplay\0".as_ptr() as *const c_char);
                let initialize_sym = libc::dlsym(lib, b"eglInitialize\0".as_ptr() as *const c_char);
                if !get_display_sym.is_null() {
                    let get_display: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
                        std::mem::transmute(get_display_sym);
                    egl_display = get_display(null_mut());
                    if !egl_display.is_null() && !initialize_sym.is_null() {
                        let init_fn: unsafe extern "C" fn(*mut c_void, *mut i32, *mut i32) -> u32 =
                            std::mem::transmute(initialize_sym);
                        init_fn(egl_display, null_mut(), null_mut());
                    }
                }
            }

            if egl_display.is_null() {
                tracing::warn!("Failed to obtain EGL display (both eglGetCurrentDisplay and eglGetDisplay returned null)");
                libc::dlclose(lib);
                return None;
            }

            let create_image_ptr = get_proc_addr(b"eglCreateImageKHR\0".as_ptr() as *const c_char);
            let destroy_image_ptr =
                get_proc_addr(b"eglDestroyImageKHR\0".as_ptr() as *const c_char);
            let target_tex_ptr =
                get_proc_addr(b"glEGLImageTargetTexture2DOES\0".as_ptr() as *const c_char);

            if create_image_ptr.is_null() || destroy_image_ptr.is_null() || target_tex_ptr.is_null()
            {
                tracing::warn!("Failed to resolve EGL DMA-BUF extensions (eglCreateImageKHR/glEGLImageTargetTexture2DOES)");
                libc::dlclose(lib);
                return None;
            }

            let egl_create_image_khr: EglCreateImageKHR = std::mem::transmute(create_image_ptr);
            let egl_destroy_image_khr: EglDestroyImageKHR = std::mem::transmute(destroy_image_ptr);
            let gl_egl_image_target_texture2d_oes: GlEGLImageTargetTexture2DOES =
                std::mem::transmute(target_tex_ptr);

            tracing::info!(
                "EGL DMA-BUF texture importer successfully initialized (display: {:?})",
                egl_display
            );

            Some(Self {
                lib_handle: lib,
                egl_display,
                egl_create_image_khr,
                egl_destroy_image_khr,
                gl_egl_image_target_texture2d_oes,
                current_texture: None,
                cached_buffers: Vec::new(),
                last_size: (0, 0),
                last_pool_generation: 0,
                last_captured_at: -1,
            })
        }
    }

    /// Removes cache entries matching `buffer_ids` and returns their textures for deletion.
    pub fn evict_cached_buffer_entries(&mut self, buffer_ids: &[u32]) -> Vec<glow::Texture> {
        let mut evicted = Vec::new();
        if buffer_ids.is_empty() {
            return evicted;
        }
        let mut idx = 0;
        while idx < self.cached_buffers.len() {
            if buffer_ids.contains(&self.cached_buffers[idx].buffer_id) {
                let entry = self.cached_buffers.remove(idx);
                if self.current_texture == Some(entry.texture) {
                    self.current_texture = None;
                }
                evicted.push(entry.texture);
            } else {
                idx += 1;
            }
        }
        evicted
    }

    /// Advances the tracked PipeWire buffer pool generation, returning all stale cached textures
    /// if the generation changed.
    pub fn advance_pool_generation(&mut self, pool_generation: u64) -> Vec<glow::Texture> {
        if pool_generation == 0 {
            return Vec::new();
        }
        if self.last_pool_generation != 0 && self.last_pool_generation != pool_generation {
            self.last_pool_generation = pool_generation;
            self.current_texture = None;
            return self.cached_buffers.drain(..).map(|e| e.texture).collect();
        }
        self.last_pool_generation = pool_generation;
        Vec::new()
    }

    /// Inserts a newly imported buffer into the bounded cache, returning any displaced textures
    /// that must be deleted by the caller.
    pub fn insert_cached_buffer(&mut self, entry: CachedDmaBuffer) -> Vec<glow::Texture> {
        let mut displaced = Vec::new();
        if entry.buffer_id == 0 {
            return displaced;
        }
        let mut idx = 0;
        while idx < self.cached_buffers.len() {
            if self.cached_buffers[idx].buffer_id == entry.buffer_id {
                let old = self.cached_buffers.remove(idx);
                if old.texture != entry.texture {
                    displaced.push(old.texture);
                }
            } else {
                idx += 1;
            }
        }
        while self.cached_buffers.len() >= MAX_CACHED_DMA_BUFFERS {
            let oldest = self.cached_buffers.remove(0);
            if oldest.texture != entry.texture {
                displaced.push(oldest.texture);
            }
        }
        self.cached_buffers.push(entry);
        displaced
    }

    /// Evicts any cached textures corresponding to removed PipeWire `buffer_ids`.
    pub unsafe fn evict_buffers(&mut self, gl: &glow::Context, buffer_ids: &[u32]) {
        for tex in self.evict_cached_buffer_entries(buffer_ids) {
            gl.delete_texture(tex);
        }
    }

    /// Imports or updates a `DmaBufFrame`.
    pub unsafe fn import_or_update(
        &mut self,
        gl: &glow::Context,
        frame: &DmaBufFrame,
    ) -> Option<glow::Texture> {
        for tex in self.advance_pool_generation(frame.pool_generation) {
            gl.delete_texture(tex);
        }
        self.import_dmabuf(
            gl,
            frame.buffer_id,
            frame.fd.as_raw_fd(),
            frame.width,
            frame.height,
            frame.stride,
            frame.drm_fourcc,
            frame.modifier,
            frame.captured_at,
        )
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
        drain_orphaned_dma_textures(gl);
        let removed_ids = take_pending_removed_buffer_ids();
        if !removed_ids.is_empty() {
            self.evict_buffers(gl, &removed_ids);
        }

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
                let tex = entry.texture;
                self.current_texture = Some(tex);
                return Some(tex);
            }
        }

        let mut attribs: Vec<i32> = vec![
            0x3057,
            width as i32, // EGL_WIDTH
            0x3056,
            height as i32, // EGL_HEIGHT
            0x3271,
            drm_fourcc as i32, // EGL_LINUX_DRM_FOURCC_EXT
            0x3272,
            fd, // EGL_DMA_BUF_PLANE0_FD_EXT
            0x3273,
            0, // EGL_DMA_BUF_PLANE0_OFFSET_EXT
            0x3274,
            stride as i32, // EGL_DMA_BUF_PLANE0_PITCH_EXT
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
            tracing::warn!(
                "eglCreateImageKHR failed for DMA-BUF buffer_id {} fd {}",
                buffer_id,
                fd
            );
            crate::gamescope::pipewire_dma::notify_active_import_failure(
                "eglCreateImageKHR failed",
            );
            return None;
        }

        // Ensure the EGLImageKHR is always destroyed even if GL texture creation fails.
        let image_guard = EglImageGuard {
            display: self.egl_display,
            image,
            destroy_fn: self.egl_destroy_image_khr,
        };

        let tex = match gl.create_texture() {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!("Failed to allocate GL texture for DMA-BUF: {err}");
                crate::gamescope::pipewire_dma::notify_active_import_failure(
                    "gl.create_texture failed",
                );
                return None;
            }
        };

        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        (self.gl_egl_image_target_texture2d_oes)(glow::TEXTURE_2D, image_guard.image);

        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.bind_texture(glow::TEXTURE_2D, None);

        // Once bound to the GL texture target, the EGLImage wrapper is destroyed by image_guard.
        drop(image_guard);

        tracing::info!(
            "Imported new DMA-BUF texture to pool (buffer_id: {}, fd: {}, {}x{}, fourcc: 0x{:08x})",
            buffer_id,
            fd,
            width,
            height,
            drm_fourcc
        );

        self.current_texture = Some(tex);
        if buffer_id > 0 {
            let displaced = self.insert_cached_buffer(CachedDmaBuffer {
                buffer_id,
                texture: tex,
                width,
                height,
                stride,
                drm_fourcc,
                modifier,
            });
            for old_tex in displaced {
                gl.delete_texture(old_tex);
            }
        }

        crate::gamescope::pipewire_dma::notify_active_import_success();
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    unsafe extern "C" fn dummy_create_image(
        _dpy: *mut c_void,
        _ctx: *mut c_void,
        _target: u32,
        _buffer: *mut c_void,
        _attrib_list: *const i32,
    ) -> *mut c_void {
        null_mut()
    }

    static DESTROY_IMAGE_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn dummy_destroy_image(_dpy: *mut c_void, _image: *mut c_void) -> u32 {
        DESTROY_IMAGE_CALLS.fetch_add(1, Ordering::SeqCst);
        1
    }

    unsafe extern "C" fn dummy_target_tex(_target: u32, _image: *mut c_void) {}

    fn make_test_importer() -> EglDmaBufImporter {
        EglDmaBufImporter {
            lib_handle: null_mut(),
            egl_display: null_mut(),
            egl_create_image_khr: dummy_create_image,
            egl_destroy_image_khr: dummy_destroy_image,
            gl_egl_image_target_texture2d_oes: dummy_target_tex,
            current_texture: None,
            cached_buffers: Vec::new(),
            last_size: (0, 0),
            last_pool_generation: 0,
            last_captured_at: -1,
        }
    }

    fn tex_id(id: u32) -> glow::Texture {
        glow::NativeTexture(std::num::NonZeroU32::new(id).unwrap())
    }

    #[test]
    fn test_cached_dma_buffer_lookup() {
        let dummy_tex = tex_id(42);
        let cached = CachedDmaBuffer {
            buffer_id: 1,
            texture: dummy_tex,
            width: 3840,
            height: 2160,
            stride: 15360,
            drm_fourcc: 0x34325241,
            modifier: 0,
        };

        let list = [cached];
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

    #[test]
    fn test_evict_cached_buffer_entries_and_pool_generation_change() {
        let mut importer = make_test_importer();
        for id in 1..=4 {
            let displaced = importer.insert_cached_buffer(CachedDmaBuffer {
                buffer_id: id,
                texture: tex_id(100 + id),
                width: 1920,
                height: 1080,
                stride: 7680,
                drm_fourcc: 0x34325258,
                modifier: 0,
            });
            assert!(displaced.is_empty());
        }
        importer.current_texture = Some(tex_id(102));

        // Evict buffer 2 and 4
        let evicted = importer.evict_cached_buffer_entries(&[2, 4]);
        assert_eq!(evicted, vec![tex_id(102), tex_id(104)]);
        assert_eq!(importer.cached_buffers.len(), 2);
        assert!(importer.current_texture.is_none());

        // Pool generation 1 -> 1 keeps remaining entries
        assert!(importer.advance_pool_generation(1).is_empty());
        assert_eq!(importer.cached_buffers.len(), 2);

        // Pool generation 1 -> 2 evicts all entries from old pool
        let gen_evicted = importer.advance_pool_generation(2);
        assert_eq!(gen_evicted, vec![tex_id(101), tex_id(103)]);
        assert!(importer.cached_buffers.is_empty());

        // Clear before drop so ORPHANED_DMA_TEXTURES isn't polluted by synthetic IDs
        importer.cached_buffers.clear();
    }

    #[test]
    fn test_cache_cap_evicts_oldest_entries() {
        let mut importer = make_test_importer();
        for id in 1..=(MAX_CACHED_DMA_BUFFERS as u32 + 2) {
            let displaced = importer.insert_cached_buffer(CachedDmaBuffer {
                buffer_id: id,
                texture: tex_id(200 + id),
                width: 1280,
                height: 720,
                stride: 5120,
                drm_fourcc: 0x34325258,
                modifier: 0,
            });
            if id <= MAX_CACHED_DMA_BUFFERS as u32 {
                assert!(displaced.is_empty());
            } else {
                assert_eq!(displaced.len(), 1);
            }
        }
        assert_eq!(importer.cached_buffers.len(), MAX_CACHED_DMA_BUFFERS);
        assert_eq!(importer.cached_buffers[0].buffer_id, 3);
        importer.cached_buffers.clear();
    }

    #[test]
    fn test_raii_egl_image_guard_destroys_image_on_drop() {
        DESTROY_IMAGE_CALLS.store(0, Ordering::SeqCst);
        {
            let _guard = EglImageGuard {
                display: 0x1 as *mut c_void,
                image: 0x2 as *mut c_void,
                destroy_fn: dummy_destroy_image,
            };
        }
        assert_eq!(DESTROY_IMAGE_CALLS.load(Ordering::SeqCst), 1);
    }
}
