use crate::devices::filter_type::CrtFilter;

// CPU filters are currently disabled in favor of GPU-based filtering.
// Keeping the module for potential future fallback or CPU-only modes.
pub fn _apply_filter(_filter: CrtFilter, _frame_data: &mut [u8], _width: u32, _height: u32) {
}
