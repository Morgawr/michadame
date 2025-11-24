use crate::video::types::{Resolution, VideoFormat};
use anyhow::{anyhow, Context, Result};
use std::process::Command;

pub fn find_video_devices() -> Result<Vec<String>> {
    let mut devices = Vec::new();
    for entry in glob::glob("/dev/video*").context("Failed to read glob pattern /dev/video*")? {
        match entry {
            Ok(path) => {
                if let Some(path_str) = path.to_str() {
                    devices.push(path_str.to_string());
                }
            }
            Err(e) => tracing::error!("Glob error: {:?}", e),
        }
    }
    Ok(devices)
}

fn parse_format_line(line: &str) -> Option<VideoFormat> {
    if line.starts_with('[') && line.contains(':') && line.contains('\'') {
        let parts: Vec<&str> = line.split('\'').collect();
        if parts.len() >= 2 {
            let fourcc = parts[1].to_string();
            let description = line.split(|c| c == '(' || c == ')').nth(1).unwrap_or("").to_string();
            return Some(VideoFormat {
                fourcc,
                description,
                resolutions: Vec::new(),
            });
        }
    }
    None
}

fn parse_resolution_line(line: &str) -> Option<Resolution> {
    if line.starts_with("Size: Discrete") {
        let res_parts: Vec<&str> = line.split_whitespace().collect();
        if res_parts.len() >= 3 {
            let res_str = res_parts[2];
            let dim_parts: Vec<&str> = res_str.split('x').collect();
            if dim_parts.len() == 2 {
                if let (Ok(w), Ok(h)) = (dim_parts[0].parse(), dim_parts[1].parse()) {
                    return Some(Resolution { width: w, height: h, framerates: Vec::new() });
                }
            }
        }
    }
    None
}

fn parse_framerate_line(line: &str, resolution: &mut Resolution) {
    if line.starts_with("Interval: Discrete") {
        if let Some(fps_part) = line.split(|c| c == '(' || c == ')').nth(1) {
            if let Some(fps_str) = fps_part.split_whitespace().next() {
                if let Ok(fps_float) = fps_str.parse::<f64>() {
                    let fps = fps_float.round() as u32;
                    if !resolution.framerates.contains(&fps) {
                        resolution.framerates.push(fps);
                    }
                }
            }
        }
    }
}

pub fn find_video_formats(device_path: &str) -> Result<Vec<VideoFormat>> {
    let output = Command::new("v4l2-ctl")
        .arg("--list-formats-ext")
        .arg("-d")
        .arg(device_path)
        .output()
        .context("Failed to execute 'v4l2-ctl'. Is it installed and in your PATH?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("v4l2-ctl failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut formats = Vec::new();
    let mut current_format: Option<VideoFormat> = None;
    let mut current_resolution: Option<Resolution> = None;

    for line in stdout.lines().filter(|l| !l.is_empty()) {
        let line = line.trim();
        if let Some(new_format) = parse_format_line(line) {
            // Finalize the previous format before starting a new one
            if let Some(mut fmt) = current_format.take() {
                if let Some(res) = current_resolution.take() {
                    if !res.framerates.is_empty() {
                        fmt.resolutions.push(res);
                    }
                }
                if !fmt.resolutions.is_empty() {
                    formats.push(fmt);
                }
            }
            current_format = Some(new_format);
        } else if let Some(new_res) = parse_resolution_line(line) {
            if let Some(fmt) = &mut current_format {
                if let Some(res) = current_resolution.take() {
                    if !res.framerates.is_empty() {
                        fmt.resolutions.push(res);
                    }
                }
                current_resolution = Some(new_res);
            }
        } else if let Some(res) = &mut current_resolution {
            parse_framerate_line(line, res);
        }
    }

    if let Some(mut format) = current_format.take() {
        if let Some(res) = current_resolution.take() {
            if !res.framerates.is_empty() {
                format.resolutions.push(res);
            }
        }
        if !format.resolutions.is_empty() {
            formats.push(format);
        }
    }
    Ok(formats)
}