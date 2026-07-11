use crate::video::types::{Resolution, VideoFormat};
use anyhow::{Context, Result};
use std::collections::HashMap;
use v4l::frameinterval::FrameIntervalEnum;
use v4l::framesize::FrameSizeEnum;
use v4l::video::Capture;

pub fn find_video_devices() -> Result<Vec<String>> {
    let mut devices = Vec::new();
    let nodes = v4l::context::enum_devices();
    for node in nodes {
        if let Some(path) = node.path().to_str() {
            devices.push(path.to_string());
        }
    }
    devices.sort();
    Ok(devices)
}

pub fn find_video_formats(device_path: &str) -> Result<Vec<VideoFormat>> {
    let dev = v4l::Device::with_path(device_path).context("Failed to open video device")?;

    let formats_info = dev.enum_formats().unwrap_or_default();
    let mut formats_map = HashMap::new();

    for fmt in formats_info {
        let fourcc = std::str::from_utf8(&fmt.fourcc.repr)
            .unwrap_or("UNKN")
            .to_string();

        let desc = fmt.description.clone();

        let entry = formats_map
            .entry(fourcc.clone())
            .or_insert_with(|| VideoFormat {
                fourcc: fourcc.clone(),
                description: desc,
                resolutions: Vec::new(),
            });

        if let Ok(sizes) = dev.enum_framesizes(fmt.fourcc) {
            for sz in sizes {
                let width;
                let height;

                match sz.size {
                    FrameSizeEnum::Discrete(res) => {
                        width = res.width;
                        height = res.height;
                    }
                    FrameSizeEnum::Stepwise(res) => {
                        width = res.max_width;
                        height = res.max_height;
                    }
                }

                if width > 0 && height > 0 {
                    let mut framerates = Vec::new();
                    if let Ok(intervals) = dev.enum_frameintervals(fmt.fourcc, width, height) {
                        for ival in intervals {
                            match ival.interval {
                                FrameIntervalEnum::Discrete(frac) => {
                                    if frac.numerator > 0 {
                                        let fps = (frac.denominator as f64 / frac.numerator as f64)
                                            .round()
                                            as u32;
                                        if !framerates.contains(&fps) {
                                            framerates.push(fps);
                                        }
                                    }
                                }
                                FrameIntervalEnum::Stepwise(frac) => {
                                    if frac.max.numerator > 0 {
                                        let fps = (frac.max.denominator as f64
                                            / frac.max.numerator as f64)
                                            .round()
                                            as u32;
                                        if !framerates.contains(&fps) {
                                            framerates.push(fps);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    framerates.sort_unstable_by(|a, b| b.cmp(a));

                    if !entry
                        .resolutions
                        .iter()
                        .any(|r| r.width == width && r.height == height)
                    {
                        entry.resolutions.push(Resolution {
                            width,
                            height,
                            framerates,
                        });
                    }
                }
            }
        }

        entry
            .resolutions
            .sort_unstable_by(|a, b| b.width.cmp(&a.width).then_with(|| b.height.cmp(&a.height)));
    }

    let mut result: Vec<VideoFormat> = formats_map.into_values().collect();
    result.sort_by(|a, b| a.description.cmp(&b.description));
    Ok(result)
}
