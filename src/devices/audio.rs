use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use alsa::device_name::HintIter;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;

pub struct AudioStreamHandle {
    _input_stream: cpal::Stream,
    _output_stream: cpal::Stream,
}

#[inline(always)]
fn i16_to_f32(s: i16) -> f32 {
    s as f32 / i16::MAX as f32
}

#[inline(always)]
fn u16_to_f32(s: u16) -> f32 {
    (s as f32 - 32768.0) / 32768.0
}

#[inline(always)]
fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

#[inline(always)]
fn f32_to_u16(s: f32) -> u16 {
    ((s.clamp(-1.0, 1.0) * 32768.0) + 32768.0) as u16
}

/// A no-op error handler to suppress ALSA's internal verbose logging to stderr.
#[cfg(target_os = "linux")]
extern "C" fn alsa_error_handler(
    _file: *const libc::c_char,
    _line: libc::c_int,
    _function: *const libc::c_char,
    _err: libc::c_int,
    _fmt: *const libc::c_char,
) {}


pub fn find_audio_devices() -> Result<(Vec<(String, String)>, Vec<(String, String)>)> {
    #[cfg(target_os = "linux")]
    {
        // Suppress ALSA's noisy stderr logging for missing/unreachable devices during probing.
        unsafe {
            use libc::{c_int, c_void};
            extern "C" {
                fn snd_lib_error_set_handler(handler: *const c_void) -> c_int;
            }
            // We cast our non-variadic handler to a void pointer to skip the variadic type check.
            snd_lib_error_set_handler(alsa_error_handler as *const c_void);
        }
    }

    let host = cpal::default_host();
    let mut sources = Vec::new();
    let mut sinks = Vec::new();

    #[cfg(target_os = "linux")]
    let alsa_hints: HashMap<String, String> = {
        let mut map = HashMap::new();
        if let Ok(hints) = HintIter::new_str(None, "pcm") {
            for hint in hints {
                if let (Some(name), Some(desc)) = (hint.name, hint.desc) {
                    // Skip OSS emulation devices as they are slow to probe and often conflict
                    if name.contains("oss") || desc.contains("OSS") {
                        continue;
                    }
                    map.insert(name, desc.replace("\n", " - "));
                }
            }
        }
        map
    };

    if let Ok(devices) = host.input_devices() {
        for device in devices {
            if let Ok(name) = device.name() {
                #[cfg(target_os = "linux")]
                {
                    if name.contains("oss") { continue; }
                    let readable_name = alsa_hints.get(&name).cloned().unwrap_or_else(|| name.clone());
                    sources.push((readable_name, name));
                }
                #[cfg(not(target_os = "linux"))]
                sources.push((name.clone(), name));
            }
        }
    }

    if let Ok(devices) = host.output_devices() {
        for device in devices {
            if let Ok(name) = device.name() {
                #[cfg(target_os = "linux")]
                {
                    if name.contains("oss") { continue; }
                    let readable_name = alsa_hints.get(&name).cloned().unwrap_or_else(|| name.clone());
                    sinks.push((readable_name, name));
                }
                #[cfg(not(target_os = "linux"))]
                sinks.push((name.clone(), name));
            }
        }
    }

    Ok((sources, sinks))
}

pub fn start_audio_stream(source_name: &str, sink_name: Option<&str>) -> Result<AudioStreamHandle> {
    let host = cpal::default_host();

    let input_device = host
        .input_devices()
        .context("Failed to get input devices")?
        .find(|d| d.name().unwrap_or_default() == source_name)
        .ok_or_else(|| anyhow!("Input device '{}' not found", source_name))?;

    let output_device = match sink_name {
        Some(name) => host
            .output_devices()
            .context("Failed to get output devices")?
            .find(|d| d.name().unwrap_or_default() == name)
            .ok_or_else(|| anyhow!("Output device '{}' not found", name))?,
        None => host
            .default_output_device()
            .context("No default output device found")?,
    };

    let input_supported = input_device
        .default_input_config()
        .context("Failed to get default input config")?;
    let output_supported = output_device
        .default_output_config()
        .context("Failed to get default output config")?;

    let input_config = input_supported.config();
    let output_config = output_supported.config();

    let input_sample_rate = input_config.sample_rate.0 as f64;
    let output_sample_rate = output_config.sample_rate.0 as f64;
    let input_channels = input_config.channels as usize;
    let output_channels = output_config.channels as usize;

    // Fixed-size buffer for raw input PCM samples. Size for ~100ms.
    let ring_size = (input_sample_rate * 0.1) as usize * input_channels;
    let ring = HeapRb::<f32>::new(ring_size);
    let (mut producer, mut consumer) = ring.split();

    let err_fn = |err| tracing::error!("An error occurred on stream: {}", err);

    let input_stream = match input_supported.sample_format() {
        cpal::SampleFormat::F32 => input_device.build_input_stream(
            &input_config,
            move |data: &[f32], _: &_| {
                let _ = producer.push_slice(data);
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => input_device.build_input_stream(
            &input_config,
            move |data: &[i16], _: &_| {
                for &sample in data {
                    let _ = producer.try_push(i16_to_f32(sample));
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => input_device.build_input_stream(
            &input_config,
            move |data: &[u16], _: &_| {
                for &sample in data {
                     let _ = producer.try_push(u16_to_f32(sample));
                }
            },
            err_fn,
            None,
        ),
        _ => return Err(anyhow!("Unsupported input sample format")),
    }?;

    // Resampling state
    let resample_ratio = input_sample_rate / output_sample_rate;
    let mut input_cursor = 0.0;
    
    // We'll use this to track the mono-summed input stream.
    let mut current_input_sample = 0.0;

    let mut callback = move |data: &mut [f32]| {
        for frame in data.chunks_mut(output_channels) {
            // "Consume" input samples according to the resample ratio.
            // If input_cursor < 1.0, we need more input samples to determine the next output sample.
            while input_cursor < 1.0 {
                let mut sum = 0.0;
                let mut count = 0;
                for _ in 0..input_channels {
                    if let Some(s) = consumer.try_pop() {
                        sum += s;
                        count += 1;
                    }
                }
                if count > 0 {
                    current_input_sample = sum / count as f32;
                }
                input_cursor += 1.0;
            }
            
            // Nearest-neighbor resampling (can be refined later to linear if needed)
            let output_sample = current_input_sample;
            
            for sample in frame.iter_mut() {
                *sample = output_sample;
            }
            
            input_cursor -= resample_ratio;
        }
    };

    let output_stream = match output_supported.sample_format() {
        cpal::SampleFormat::F32 => output_device.build_output_stream(
            &output_config,
            move |data: &mut [f32], _: &_| callback(data),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => {
            let mut f32_buf = Vec::new();
            output_device.build_output_stream(
                &output_config,
                move |data: &mut [i16], _: &_| {
                    f32_buf.resize(data.len(), 0.0);
                    callback(&mut f32_buf);
                    for (i, &s) in f32_buf.iter().enumerate() {
                        data[i] = f32_to_i16(s);
                    }
                },
                err_fn,
                None,
            )
        },
        cpal::SampleFormat::U16 => {
            let mut f32_buf = Vec::new();
            output_device.build_output_stream(
                &output_config,
                move |data: &mut [u16], _: &_| {
                    f32_buf.resize(data.len(), 0.0);
                    callback(&mut f32_buf);
                    for (i, &s) in f32_buf.iter().enumerate() {
                        data[i] = f32_to_u16(s);
                    }
                },
                err_fn,
                None,
            )
        },
        _ => return Err(anyhow!("Unsupported output sample format")),
    }?;

    input_stream.play()?;
    output_stream.play()?;

    Ok(AudioStreamHandle {
        _input_stream: input_stream,
        _output_stream: output_stream,
    })
}
