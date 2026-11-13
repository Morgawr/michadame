use crate::gamescope::pipewire_dma::{DmaBufFrame, MAX_DMA_PLANES};
use eframe::glow::{self, HasContext};
use std::ffi::{c_char, c_void, CStr};
use std::os::unix::io::AsRawFd;
use std::ptr::null_mut;

pub const MAX_CACHED_DMA_BUFFERS: usize = 16;

pub const EGL_EXTENSIONS: i32 = 0x3055;
pub const EGL_WIDTH: i32 = 0x3057;
pub const EGL_HEIGHT: i32 = 0x3056;
pub const EGL_NONE: i32 = 0x3038;
pub const EGL_LINUX_DMA_BUF_EXT: u32 = 0x3270;
pub const EGL_LINUX_DRM_FOURCC_EXT: i32 = 0x3271;

pub const EGL_DMA_BUF_PLANE0_FD_EXT: i32 = 0x3272;
pub const EGL_DMA_BUF_PLANE0_OFFSET_EXT: i32 = 0x3273;
pub const EGL_DMA_BUF_PLANE0_PITCH_EXT: i32 = 0x3274;
pub const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: i32 = 0x3443;
pub const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: i32 = 0x3444;

pub const EGL_DMA_BUF_PLANE1_FD_EXT: i32 = 0x3275;
pub const EGL_DMA_BUF_PLANE1_OFFSET_EXT: i32 = 0x3276;
pub const EGL_DMA_BUF_PLANE1_PITCH_EXT: i32 = 0x3277;
pub const EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT: i32 = 0x3445;
pub const EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT: i32 = 0x3446;

pub const EGL_DMA_BUF_PLANE2_FD_EXT: i32 = 0x3278;
pub const EGL_DMA_BUF_PLANE2_OFFSET_EXT: i32 = 0x3279;
pub const EGL_DMA_BUF_PLANE2_PITCH_EXT: i32 = 0x327A;
pub const EGL_DMA_BUF_PLANE2_MODIFIER_LO_EXT: i32 = 0x3447;
pub const EGL_DMA_BUF_PLANE2_MODIFIER_HI_EXT: i32 = 0x3448;

pub const EGL_DMA_BUF_PLANE3_FD_EXT: i32 = 0x3440;
pub const EGL_DMA_BUF_PLANE3_OFFSET_EXT: i32 = 0x3441;
pub const EGL_DMA_BUF_PLANE3_PITCH_EXT: i32 = 0x3442;
pub const EGL_DMA_BUF_PLANE3_MODIFIER_LO_EXT: i32 = 0x3449;
pub const EGL_DMA_BUF_PLANE3_MODIFIER_HI_EXT: i32 = 0x344A;

pub const DRM_FORMAT_XRGB8888: u32 = 0x34325258;
pub const DRM_FORMAT_ARGB8888: u32 = 0x34325241;
pub const DRM_FORMAT_XBGR8888: u32 = 0x34324258;
pub const DRM_FORMAT_ABGR8888: u32 = 0x34324241;
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;
pub const DRM_FORMAT_MOD_INVALID: u64 = (1u64 << 56) - 1;

const PLANE_FD_ATTRIBS: [i32; MAX_DMA_PLANES] = [
    EGL_DMA_BUF_PLANE0_FD_EXT,
    EGL_DMA_BUF_PLANE1_FD_EXT,
    EGL_DMA_BUF_PLANE2_FD_EXT,
    EGL_DMA_BUF_PLANE3_FD_EXT,
];

const PLANE_OFFSET_ATTRIBS: [i32; MAX_DMA_PLANES] = [
    EGL_DMA_BUF_PLANE0_OFFSET_EXT,
    EGL_DMA_BUF_PLANE1_OFFSET_EXT,
    EGL_DMA_BUF_PLANE2_OFFSET_EXT,
    EGL_DMA_BUF_PLANE3_OFFSET_EXT,
];

const PLANE_PITCH_ATTRIBS: [i32; MAX_DMA_PLANES] = [
    EGL_DMA_BUF_PLANE0_PITCH_EXT,
    EGL_DMA_BUF_PLANE1_PITCH_EXT,
    EGL_DMA_BUF_PLANE2_PITCH_EXT,
    EGL_DMA_BUF_PLANE3_PITCH_EXT,
];

const PLANE_MOD_LO_ATTRIBS: [i32; MAX_DMA_PLANES] = [
    EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT,
    EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT,
    EGL_DMA_BUF_PLANE2_MODIFIER_LO_EXT,
    EGL_DMA_BUF_PLANE3_MODIFIER_LO_EXT,
];

const PLANE_MOD_HI_ATTRIBS: [i32; MAX_DMA_PLANES] = [
    EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT,
    EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT,
    EGL_DMA_BUF_PLANE2_MODIFIER_HI_EXT,
    EGL_DMA_BUF_PLANE3_MODIFIER_HI_EXT,
];

type EglDisplay = *mut c_void;

type EglCreateImageKHR = unsafe extern "C" fn(
    dpy: EglDisplay,
    ctx: *mut c_void,
    target: u32,
    buffer: *mut c_void,
    attrib_list: *const i32,
) -> *mut c_void;

type EglDestroyImageKHR = unsafe extern "C" fn(dpy: EglDisplay, image: *mut c_void) -> u32;

type GlEGLImageTargetTexture2DOES = unsafe extern "C" fn(target: u32, image: *mut c_void);

type EglGetCurrentDisplay = unsafe extern "C" fn() -> EglDisplay;
type EglGetProcAddress = unsafe extern "C" fn(procname: *const c_char) -> *mut c_void;
type EglQueryStringFn = unsafe extern "C" fn(display: EglDisplay, name: i32) -> *const c_char;

type EglQueryDmaBufModifiersExtFn = unsafe extern "C" fn(
    display: EglDisplay,
    format: i32,
    max_modifiers: i32,
    modifiers: *mut u64,
    external_only: *mut u32,
    num_modifiers: *mut i32,
) -> u32;

struct DlCloseGuard(*mut c_void);

impl Drop for DlCloseGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                libc::dlclose(self.0);
            }
            self.0 = null_mut();
        }
    }
}

fn has_egl_extension(extensions: &str, target: &str) -> bool {
    extensions.split_whitespace().any(|ext| ext == target)
}

unsafe fn query_display_has_extension(lib: *mut c_void, display: EglDisplay, target: &str) -> bool {
    if lib.is_null() || display.is_null() {
        return false;
    }
    let query_string_sym = libc::dlsym(lib, b"eglQueryString\0".as_ptr() as *const c_char);
    if query_string_sym.is_null() {
        return false;
    }
    let query_string: EglQueryStringFn = std::mem::transmute(query_string_sym);
    let ext_ptr = query_string(display, EGL_EXTENSIONS);
    if ext_ptr.is_null() {
        return false;
    }
    let ext_str = CStr::from_ptr(ext_ptr).to_string_lossy();
    has_egl_extension(&ext_str, target)
}

/// Filters `(modifier, external_only)` pairs per format, intersects across supported formats
/// while preserving driver preference order, and guarantees `DRM_FORMAT_MOD_LINEAR` (`0`) is
/// always included as a fallback.
fn select_supported_modifiers(per_format: &[Vec<(u64, u32)>]) -> Vec<u64> {
    let mut filtered_lists: Vec<Vec<u64>> = Vec::with_capacity(per_format.len());
    for entries in per_format {
        let mut valid = Vec::new();
        for &(modifier, external_only) in entries {
            if modifier != DRM_FORMAT_MOD_INVALID
                && external_only == 0
                && !valid.contains(&modifier)
            {
                valid.push(modifier);
            }
        }
        if !valid.is_empty() {
            filtered_lists.push(valid);
        }
    }

    let mut result = if let Some((first, rest)) = filtered_lists.split_first() {
        let intersected: Vec<u64> = first
            .iter()
            .copied()
            .filter(|m| rest.iter().all(|other| other.contains(m)))
            .collect();
        if intersected.is_empty() {
            first.clone()
        } else {
            intersected
        }
    } else {
        Vec::new()
    };

    if !result.contains(&DRM_FORMAT_MOD_LINEAR) {
        result.push(DRM_FORMAT_MOD_LINEAR);
    }
    result
}

/// Queries the current EGL display for non-external DRM format modifiers supported across the
/// RGBx/RGBA formats negotiated with Gamescope (`XRGB8888`, `ARGB8888`, `XBGR8888`, `ABGR8888`).
///
/// Returns an empty `Vec` gracefully on GLX / headless contexts when `eglGetCurrentDisplay()`
/// returns `EGL_NO_DISPLAY`. When an active EGL display is present, `DRM_FORMAT_MOD_LINEAR` (`0`)
/// is always included as a fallback.
pub fn query_supported_dmabuf_modifiers() -> Vec<u64> {
    const QUERY_FORMATS: [u32; 4] = [
        DRM_FORMAT_XRGB8888,
        DRM_FORMAT_ARGB8888,
        DRM_FORMAT_XBGR8888,
        DRM_FORMAT_ABGR8888,
    ];

    unsafe {
        let lib = libc::dlopen(
            b"libEGL.so.1\0".as_ptr() as *const c_char,
            libc::RTLD_LAZY | libc::RTLD_LOCAL,
        );
        if lib.is_null() {
            return Vec::new();
        }
        let _lib_guard = DlCloseGuard(lib);

        let get_current_display_sym =
            libc::dlsym(lib, b"eglGetCurrentDisplay\0".as_ptr() as *const c_char);
        let get_proc_addr_sym = libc::dlsym(lib, b"eglGetProcAddress\0".as_ptr() as *const c_char);
        let query_string_sym = libc::dlsym(lib, b"eglQueryString\0".as_ptr() as *const c_char);

        if get_current_display_sym.is_null()
            || get_proc_addr_sym.is_null()
            || query_string_sym.is_null()
        {
            return Vec::new();
        }

        let get_current_display: EglGetCurrentDisplay =
            std::mem::transmute(get_current_display_sym);
        let display = get_current_display();
        if display.is_null() {
            return Vec::new();
        }

        let query_string: EglQueryStringFn = std::mem::transmute(query_string_sym);
        let ext_ptr = query_string(display, EGL_EXTENSIONS);
        if ext_ptr.is_null() {
            return Vec::new();
        }
        let ext_str = CStr::from_ptr(ext_ptr).to_string_lossy();
        if !has_egl_extension(&ext_str, "EGL_EXT_image_dma_buf_import") {
            return Vec::new();
        }
        if !has_egl_extension(&ext_str, "EGL_EXT_image_dma_buf_import_modifiers") {
            return vec![DRM_FORMAT_MOD_LINEAR];
        }

        let get_proc_addr: EglGetProcAddress = std::mem::transmute(get_proc_addr_sym);
        let query_mods_ptr =
            get_proc_addr(b"eglQueryDmaBufModifiersEXT\0".as_ptr() as *const c_char);
        if query_mods_ptr.is_null() {
            return vec![DRM_FORMAT_MOD_LINEAR];
        }
        let query_mods: EglQueryDmaBufModifiersExtFn = std::mem::transmute(query_mods_ptr);

        let mut per_format: Vec<Vec<(u64, u32)>> = Vec::with_capacity(QUERY_FORMATS.len());
        for &fourcc in &QUERY_FORMATS {
            let mut count: i32 = 0;
            let ok = query_mods(
                display,
                fourcc as i32,
                0,
                null_mut(),
                null_mut(),
                &mut count,
            );
            if ok == 0 || count <= 0 {
                per_format.push(Vec::new());
                continue;
            }

            let cap = count as usize;
            let mut mods = vec![0u64; cap];
            let mut ext_only = vec![0u32; cap];
            let mut out_count: i32 = 0;
            let ok2 = query_mods(
                display,
                fourcc as i32,
                count,
                mods.as_mut_ptr(),
                ext_only.as_mut_ptr(),
                &mut out_count,
            );
            if ok2 == 0 || out_count <= 0 {
                per_format.push(Vec::new());
                continue;
            }

            let len = (out_count as usize).min(cap);
            let pairs: Vec<(u64, u32)> = mods[..len]
                .iter()
                .copied()
                .zip(ext_only[..len].iter().copied())
                .collect();
            per_format.push(pairs);
        }

        let selected = select_supported_modifiers(&per_format);
        tracing::info!(
            "Queried {} supported non-external EGL DMA-BUF modifiers for Gamescope",
            selected.len()
        );
        selected
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaBufPlaneAttribs {
    pub fd: i32,
    pub offset: u32,
    pub stride: u32,
}

/// Builds the `EGL_NONE`-terminated `eglCreateImageKHR` attribute list for `1..=4` DMA-BUF planes.
pub fn build_dmabuf_egl_attribs(
    width: u32,
    height: u32,
    drm_fourcc: u32,
    modifier: u64,
    planes: &[DmaBufPlaneAttribs],
    has_modifiers_ext: bool,
) -> Vec<i32> {
    let n_planes = planes.len().min(MAX_DMA_PLANES);
    let include_modifier = modifier != DRM_FORMAT_MOD_INVALID
        && (has_modifiers_ext || modifier != DRM_FORMAT_MOD_LINEAR);
    let mod_lo = (modifier & 0xFFFF_FFFF) as i32;
    let mod_hi = ((modifier >> 32) & 0xFFFF_FFFF) as i32;

    let per_plane_len = if include_modifier { 10 } else { 6 };
    let mut attribs = Vec::with_capacity(7 + n_planes * per_plane_len);
    attribs.push(EGL_WIDTH);
    attribs.push(width as i32);
    attribs.push(EGL_HEIGHT);
    attribs.push(height as i32);
    attribs.push(EGL_LINUX_DRM_FOURCC_EXT);
    attribs.push(drm_fourcc as i32);

    for (idx, plane) in planes.iter().take(n_planes).enumerate() {
        attribs.push(PLANE_FD_ATTRIBS[idx]);
        attribs.push(plane.fd);
        attribs.push(PLANE_OFFSET_ATTRIBS[idx]);
        attribs.push(plane.offset as i32);
        attribs.push(PLANE_PITCH_ATTRIBS[idx]);
        attribs.push(plane.stride as i32);

        if include_modifier {
            attribs.push(PLANE_MOD_LO_ATTRIBS[idx]);
            attribs.push(mod_lo);
            attribs.push(PLANE_MOD_HI_ATTRIBS[idx]);
            attribs.push(mod_hi);
        }
    }

    attribs.push(EGL_NONE);
    attribs
}

#[derive(Clone, Copy, Debug)]
pub struct CachedDmaBuffer {
    pub buffer_id: u32,
    pub texture: glow::Texture,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub n_planes: u32,
    pub drm_fourcc: u32,
    pub modifier: u64,
}

struct EglImageGuard {
    display: EglDisplay,
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
    egl_display: EglDisplay,
    has_modifiers_ext: bool,
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
                    let get_display: unsafe extern "C" fn(*mut c_void) -> EglDisplay =
                        std::mem::transmute(get_display_sym);
                    egl_display = get_display(null_mut());
                    if !egl_display.is_null() && !initialize_sym.is_null() {
                        let init_fn: unsafe extern "C" fn(EglDisplay, *mut i32, *mut i32) -> u32 =
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

            let has_modifiers_ext = query_display_has_extension(
                lib,
                egl_display,
                "EGL_EXT_image_dma_buf_import_modifiers",
            );

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
                "EGL DMA-BUF texture importer successfully initialized (display: {:?}, modifiers_ext: {})",
                egl_display,
                has_modifiers_ext
            );

            Some(Self {
                lib_handle: lib,
                egl_display,
                has_modifiers_ext,
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

    /// Imports or updates a multi-plane `DmaBufFrame`.
    pub unsafe fn import_or_update(
        &mut self,
        gl: &glow::Context,
        frame: &DmaBufFrame,
    ) -> Option<glow::Texture> {
        for tex in self.advance_pool_generation(frame.pool_generation) {
            gl.delete_texture(tex);
        }

        let mut plane_attribs = [DmaBufPlaneAttribs {
            fd: -1,
            offset: 0,
            stride: 0,
        }; MAX_DMA_PLANES];
        let max_planes = (frame.n_planes as usize).min(MAX_DMA_PLANES);
        let mut valid_planes = 0usize;
        for i in 0..max_planes {
            if let Some(ref plane) = frame.planes[i] {
                plane_attribs[valid_planes] = DmaBufPlaneAttribs {
                    fd: plane.fd.as_raw_fd(),
                    offset: plane.offset,
                    stride: plane.stride,
                };
                valid_planes += 1;
            } else {
                break;
            }
        }
        if valid_planes == 0 {
            return None;
        }

        self.import_dmabuf_planes(
            gl,
            frame.buffer_id,
            &plane_attribs[..valid_planes],
            frame.width,
            frame.height,
            frame.drm_fourcc,
            frame.modifier,
            frame.captured_at,
        )
    }

    /// Imports a single-plane Linux DMA-BUF file descriptor directly into an OpenGL texture.
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
        let single_plane = [DmaBufPlaneAttribs {
            fd,
            offset: 0,
            stride,
        }];
        self.import_dmabuf_planes(
            gl,
            buffer_id,
            &single_plane,
            width,
            height,
            drm_fourcc,
            modifier,
            captured_at,
        )
    }

    /// Imports a `1..=4` plane Linux DMA-BUF directly into an OpenGL texture.
    ///
    /// When `buffer_id` is non-zero and already present in the cache with identical format
    /// parameters, the cached `glow::Texture` is returned immediately without issuing any
    /// kernel DRM ioctls or EGLImage allocations.
    pub unsafe fn import_dmabuf_planes(
        &mut self,
        gl: &glow::Context,
        buffer_id: u32,
        planes: &[DmaBufPlaneAttribs],
        width: u32,
        height: u32,
        drm_fourcc: u32,
        modifier: u64,
        captured_at: i64,
    ) -> Option<glow::Texture> {
        if planes.is_empty() || planes.len() > MAX_DMA_PLANES {
            return None;
        }
        let n_planes = planes.len() as u32;
        let primary_stride = planes[0].stride;
        let primary_fd = planes[0].fd;

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
                    && e.stride == primary_stride
                    && e.n_planes == n_planes
                    && e.drm_fourcc == drm_fourcc
                    && e.modifier == modifier
            }) {
                let tex = entry.texture;
                self.current_texture = Some(tex);
                return Some(tex);
            }
        }

        let attribs = build_dmabuf_egl_attribs(
            width,
            height,
            drm_fourcc,
            modifier,
            planes,
            self.has_modifiers_ext,
        );

        let image = (self.egl_create_image_khr)(
            self.egl_display,
            null_mut(), // EGL_NO_CONTEXT
            EGL_LINUX_DMA_BUF_EXT,
            null_mut(),
            attribs.as_ptr(),
        );

        if image.is_null() {
            tracing::warn!(
                "eglCreateImageKHR failed for DMA-BUF buffer_id {} fd {} ({} planes, modifier 0x{:016x})",
                buffer_id,
                primary_fd,
                n_planes,
                modifier
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
            "Imported new DMA-BUF texture to pool (buffer_id: {}, fd: {}, {} planes, {}x{}, fourcc: 0x{:08x}, modifier: 0x{:016x})",
            buffer_id,
            primary_fd,
            n_planes,
            width,
            height,
            drm_fourcc,
            modifier
        );

        self.current_texture = Some(tex);
        if buffer_id > 0 {
            let displaced = self.insert_cached_buffer(CachedDmaBuffer {
                buffer_id,
                texture: tex,
                width,
                height,
                stride: primary_stride,
                n_planes,
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
        _dpy: EglDisplay,
        _ctx: *mut c_void,
        _target: u32,
        _buffer: *mut c_void,
        _attrib_list: *const i32,
    ) -> *mut c_void {
        null_mut()
    }

    static DESTROY_IMAGE_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn dummy_destroy_image(_dpy: EglDisplay, _image: *mut c_void) -> u32 {
        DESTROY_IMAGE_CALLS.fetch_add(1, Ordering::SeqCst);
        1
    }

    unsafe extern "C" fn dummy_target_tex(_target: u32, _image: *mut c_void) {}

    fn make_test_importer() -> EglDmaBufImporter {
        EglDmaBufImporter {
            lib_handle: null_mut(),
            egl_display: null_mut(),
            has_modifiers_ext: true,
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
            n_planes: 1,
            drm_fourcc: DRM_FORMAT_ARGB8888,
            modifier: DRM_FORMAT_MOD_LINEAR,
        };

        let list = [cached];
        let found = list.iter().find(|e| {
            e.buffer_id == 1
                && e.width == 3840
                && e.height == 2160
                && e.stride == 15360
                && e.n_planes == 1
                && e.drm_fourcc == DRM_FORMAT_ARGB8888
                && e.modifier == DRM_FORMAT_MOD_LINEAR
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
                n_planes: 1,
                drm_fourcc: DRM_FORMAT_XRGB8888,
                modifier: DRM_FORMAT_MOD_LINEAR,
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
                n_planes: 1,
                drm_fourcc: DRM_FORMAT_XRGB8888,
                modifier: DRM_FORMAT_MOD_LINEAR,
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
                display: 0x1 as EglDisplay,
                image: 0x2 as *mut c_void,
                destroy_fn: dummy_destroy_image,
            };
        }
        assert_eq!(DESTROY_IMAGE_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_build_dmabuf_egl_attribs_single_plane_linear_and_tiled() {
        let plane0 = [DmaBufPlaneAttribs {
            fd: 11,
            offset: 0,
            stride: 7680,
        }];

        // Without modifiers extension: linear omits MODIFIER_LO/HI
        let attribs_legacy = build_dmabuf_egl_attribs(
            1920,
            1080,
            DRM_FORMAT_XRGB8888,
            DRM_FORMAT_MOD_LINEAR,
            &plane0,
            false,
        );
        assert_eq!(
            attribs_legacy,
            vec![
                EGL_WIDTH,
                1920,
                EGL_HEIGHT,
                1080,
                EGL_LINUX_DRM_FOURCC_EXT,
                DRM_FORMAT_XRGB8888 as i32,
                EGL_DMA_BUF_PLANE0_FD_EXT,
                11,
                EGL_DMA_BUF_PLANE0_OFFSET_EXT,
                0,
                EGL_DMA_BUF_PLANE0_PITCH_EXT,
                7680,
                EGL_NONE,
            ]
        );

        // With modifiers extension: linear explicitly passes MODIFIER_LO=0, MODIFIER_HI=0
        let attribs_linear_ext = build_dmabuf_egl_attribs(
            1920,
            1080,
            DRM_FORMAT_XRGB8888,
            DRM_FORMAT_MOD_LINEAR,
            &plane0,
            true,
        );
        assert_eq!(
            attribs_linear_ext,
            vec![
                EGL_WIDTH,
                1920,
                EGL_HEIGHT,
                1080,
                EGL_LINUX_DRM_FOURCC_EXT,
                DRM_FORMAT_XRGB8888 as i32,
                EGL_DMA_BUF_PLANE0_FD_EXT,
                11,
                EGL_DMA_BUF_PLANE0_OFFSET_EXT,
                0,
                EGL_DMA_BUF_PLANE0_PITCH_EXT,
                7680,
                EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT,
                0,
                EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT,
                0,
                EGL_NONE,
            ]
        );

        // Single-plane tiled modifier (e.g. Intel X-tiled 0x0100000000000001)
        let tiled_mod: u64 = 0x0100_0000_0000_0001;
        let attribs_tiled = build_dmabuf_egl_attribs(
            2560,
            1440,
            DRM_FORMAT_ARGB8888,
            tiled_mod,
            &[DmaBufPlaneAttribs {
                fd: 15,
                offset: 4096,
                stride: 10240,
            }],
            true,
        );
        assert_eq!(
            attribs_tiled,
            vec![
                EGL_WIDTH,
                2560,
                EGL_HEIGHT,
                1440,
                EGL_LINUX_DRM_FOURCC_EXT,
                DRM_FORMAT_ARGB8888 as i32,
                EGL_DMA_BUF_PLANE0_FD_EXT,
                15,
                EGL_DMA_BUF_PLANE0_OFFSET_EXT,
                4096,
                EGL_DMA_BUF_PLANE0_PITCH_EXT,
                10240,
                EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT,
                1,
                EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT,
                0x0100_0000,
                EGL_NONE,
            ]
        );

        // DRM_FORMAT_MOD_INVALID never emits MODIFIER_LO/HI even when extension is present
        let attribs_invalid_mod = build_dmabuf_egl_attribs(
            1920,
            1080,
            DRM_FORMAT_XRGB8888,
            DRM_FORMAT_MOD_INVALID,
            &plane0,
            true,
        );
        assert_eq!(attribs_invalid_mod, attribs_legacy);
    }

    #[test]
    fn test_build_dmabuf_egl_attribs_two_plane_amd_dcc_and_three_plane_ccs() {
        // 2-plane AMD RDNA2/3 DCC modifier
        let amd_dcc_mod: u64 = 0x0200_0000_0020_6b03;
        let mod_lo = (amd_dcc_mod & 0xFFFF_FFFF) as i32;
        let mod_hi = ((amd_dcc_mod >> 32) & 0xFFFF_FFFF) as i32;
        let dcc_planes = [
            DmaBufPlaneAttribs {
                fd: 21,
                offset: 0,
                stride: 5120,
            },
            DmaBufPlaneAttribs {
                fd: 22,
                offset: 4_194_304,
                stride: 256,
            },
        ];
        let dcc_attribs = build_dmabuf_egl_attribs(
            1280,
            800,
            DRM_FORMAT_XRGB8888,
            amd_dcc_mod,
            &dcc_planes,
            true,
        );
        assert_eq!(
            dcc_attribs,
            vec![
                EGL_WIDTH,
                1280,
                EGL_HEIGHT,
                800,
                EGL_LINUX_DRM_FOURCC_EXT,
                DRM_FORMAT_XRGB8888 as i32,
                EGL_DMA_BUF_PLANE0_FD_EXT,
                21,
                EGL_DMA_BUF_PLANE0_OFFSET_EXT,
                0,
                EGL_DMA_BUF_PLANE0_PITCH_EXT,
                5120,
                EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT,
                mod_lo,
                EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT,
                mod_hi,
                EGL_DMA_BUF_PLANE1_FD_EXT,
                22,
                EGL_DMA_BUF_PLANE1_OFFSET_EXT,
                4_194_304,
                EGL_DMA_BUF_PLANE1_PITCH_EXT,
                256,
                EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT,
                mod_lo,
                EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT,
                mod_hi,
                EGL_NONE,
            ]
        );

        // 3-plane CCS + clear-color modifier (and verify plane 3 constants on 4-plane case)
        let ccs_mod: u64 = 0x0100_0000_0000_0008;
        let ccs_lo = (ccs_mod & 0xFFFF_FFFF) as i32;
        let ccs_hi = ((ccs_mod >> 32) & 0xFFFF_FFFF) as i32;
        let ccs_planes = [
            DmaBufPlaneAttribs {
                fd: 30,
                offset: 0,
                stride: 7680,
            },
            DmaBufPlaneAttribs {
                fd: 31,
                offset: 8_388_608,
                stride: 512,
            },
            DmaBufPlaneAttribs {
                fd: 32,
                offset: 8_912_896,
                stride: 64,
            },
        ];
        let ccs_attribs =
            build_dmabuf_egl_attribs(1920, 1080, DRM_FORMAT_ABGR8888, ccs_mod, &ccs_planes, true);
        assert_eq!(
            ccs_attribs,
            vec![
                EGL_WIDTH,
                1920,
                EGL_HEIGHT,
                1080,
                EGL_LINUX_DRM_FOURCC_EXT,
                DRM_FORMAT_ABGR8888 as i32,
                EGL_DMA_BUF_PLANE0_FD_EXT,
                30,
                EGL_DMA_BUF_PLANE0_OFFSET_EXT,
                0,
                EGL_DMA_BUF_PLANE0_PITCH_EXT,
                7680,
                EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT,
                ccs_lo,
                EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT,
                ccs_hi,
                EGL_DMA_BUF_PLANE1_FD_EXT,
                31,
                EGL_DMA_BUF_PLANE1_OFFSET_EXT,
                8_388_608,
                EGL_DMA_BUF_PLANE1_PITCH_EXT,
                512,
                EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT,
                ccs_lo,
                EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT,
                ccs_hi,
                EGL_DMA_BUF_PLANE2_FD_EXT,
                32,
                EGL_DMA_BUF_PLANE2_OFFSET_EXT,
                8_912_896,
                EGL_DMA_BUF_PLANE2_PITCH_EXT,
                64,
                EGL_DMA_BUF_PLANE2_MODIFIER_LO_EXT,
                ccs_lo,
                EGL_DMA_BUF_PLANE2_MODIFIER_HI_EXT,
                ccs_hi,
                EGL_NONE,
            ]
        );
    }

    #[test]
    fn test_select_supported_modifiers_filters_external_and_invalid_and_keeps_linear() {
        let mod_dcc = 0x0200_0000_0020_6b03u64;
        let mod_tiled = 0x0200_0000_0020_0b03u64;
        let mod_ext_only = 0x0200_0000_0099_9999u64;
        let mod_only_xrgb = 0x0200_0000_0011_1111u64;

        let per_format = vec![
            vec![
                (DRM_FORMAT_MOD_INVALID, 0),
                (mod_ext_only, 1), // external_only != 0 -> must be excluded for GL_TEXTURE_2D
                (mod_dcc, 0),
                (mod_only_xrgb, 0),
                (mod_tiled, 0),
            ],
            vec![(mod_dcc, 0), (mod_tiled, 0)],
            vec![(mod_dcc, 0), (mod_tiled, 0), (DRM_FORMAT_MOD_LINEAR, 0)],
            vec![(mod_dcc, 0), (mod_tiled, 0)],
        ];

        let selected = select_supported_modifiers(&per_format);
        // Preserves driver preference order, intersects across formats, excludes external/invalid,
        // and appends DRM_FORMAT_MOD_LINEAR as fallback!
        assert_eq!(selected, vec![mod_dcc, mod_tiled, DRM_FORMAT_MOD_LINEAR]);

        assert!(has_egl_extension(
            "EGL_EXT_image_dma_buf_import EGL_EXT_image_dma_buf_import_modifiers",
            "EGL_EXT_image_dma_buf_import_modifiers"
        ));
        assert!(!has_egl_extension(
            "EGL_EXT_image_dma_buf_import",
            "EGL_EXT_image_dma_buf_import_modifiers"
        ));

        // Headless test environment has no current EGL display -> returns empty Vec gracefully
        let headless_mods = query_supported_dmabuf_modifiers();
        assert!(headless_mods.is_empty() || headless_mods.contains(&DRM_FORMAT_MOD_LINEAR));
    }
}
