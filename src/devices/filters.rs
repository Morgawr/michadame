use crate::devices::filter_type::CrtFilter;

pub fn apply_filter(filter: CrtFilter, frame_data: &mut [u8], width: u32, height: u32) {
    match filter {
        CrtFilter::Off => {}
        CrtFilter::Scanlines => apply_scanlines(frame_data, width, height),
        CrtFilter::Lottes => {} // Lottes is now a GPU-only filter
    }
}

fn apply_scanlines(frame_data: &mut [u8], width: u32, _height: u32) {
    let width = width as usize;
    for y in (0..frame_data.len() / (width * 3)).step_by(2) {
        for x in 0..width {
            let index = (y * width + x) * 3;
            if index + 2 < frame_data.len() {
                frame_data[index] = frame_data[index].saturating_sub(80);
                frame_data[index + 1] = frame_data[index + 1].saturating_sub(80);
                frame_data[index + 2] = frame_data[index + 2].saturating_sub(80);
            }
        }
    }
}
