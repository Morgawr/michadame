use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use alsa::device_name::HintIter;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;
use std::sync::Arc;

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

fn get_alsa_device_name(name: &str) -> String {
    #[cfg(target_os = "linux")]
    {
        // cpal sometimes returns names like "sysdefault:CARD=USB" or "hw:0,0".
        // We can use ALSA's HintIter to find the matching description.
        if let Ok(hints) = HintIter::new_str(None, "pcm") {
            for hint in hints {
                if let Some(hint_name) = &hint.name {
                    if hint_name == name {
                        if let Some(desc) = &hint.desc {
                            return desc.replace("\n", " - ");
                        }
                    }
                }
            }
        }
    }
    name.to_string()
}

pub fn find_audio_devices() -> Result<(Vec<(String, String)>, Vec<(String, String)>)> {
    let host = cpal::default_host();
    let mut sources = Vec::new();
    let mut sinks = Vec::new();

    if let Ok(devices) = host.input_devices() {
        for device in devices {
            if let Ok(name) = device.name() {
                let readable_name = get_alsa_device_name(&name);
                sources.push((readable_name, name));
            }
        }
    }

    if let Ok(devices) = host.output_devices() {
        for device in devices {
            if let Ok(name) = device.name() {
                let readable_name = get_alsa_device_name(&name);
                sinks.push((readable_name, name));
            }
        }
    }

    Ok((sources, sinks))
}

pub fn start_audio_stream(source_name: &str, sink_name: &str) -> Result<AudioStreamHandle> {
    let host = cpal::default_host();

    let input_device = host
        .input_devices()
        .context("Failed to get input devices")?
        .find(|d| d.name().unwrap_or_default() == source_name)
        .ok_or_else(|| anyhow!("Input device '{}' not found", source_name))?;

    let output_device = host
        .output_devices()
        .context("Failed to get output devices")?
        .find(|d| d.name().unwrap_or_default() == sink_name)
        .ok_or_else(|| anyhow!("Output device '{}' not found", sink_name))?;

    let input_supported = input_device
        .default_input_config()
        .context("Failed to get default input config")?;
    let output_supported = output_device
        .default_output_config()
        .context("Failed to get default output config")?;

    let input_config = input_supported.config();
    let output_config = output_supported.config();

    // Size for ~50ms of audio buffer
    let latency_frames = (input_config.sample_rate.0 as f32 * 0.05) as usize;
    let ring_size = latency_frames * input_config.channels as usize * 4;
    let ring = HeapRb::<f32>::new(ring_size);
    let (mut producer, mut consumer) = ring.split();

    let err_fn = |err| tracing::error!("An error occurred on stream: {}", err);

    let input_stream = match input_supported.sample_format() {
        cpal::SampleFormat::F32 => input_device.build_input_stream(
            &input_config,
            move |data: &[f32], _: &_| {
                producer.push_slice(data);
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => input_device.build_input_stream(
            &input_config,
            move |data: &[i16], _: &_| {
                for &sample in data {
                    let f = i16_to_f32(sample);
                    let _ = producer.try_push(f);
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => input_device.build_input_stream(
            &input_config,
            move |data: &[u16], _: &_| {
                for &sample in data {
                     let f = u16_to_f32(sample);
                     let _ = producer.try_push(f);
                }
            },
            err_fn,
            None,
        ),
        _ => return Err(anyhow!("Unsupported input sample format")),
    }?;

    let output_stream = match output_supported.sample_format() {
        cpal::SampleFormat::F32 => output_device.build_output_stream(
            &output_config,
            move |data: &mut [f32], _: &_| {
                for sample in data.iter_mut() {
                    *sample = consumer.try_pop().unwrap_or(0.0);
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => output_device.build_output_stream(
            &output_config,
            move |data: &mut [i16], _: &_| {
                for sample in data.iter_mut() {
                    let f_sample = consumer.try_pop().unwrap_or(0.0);
                    *sample = f32_to_i16(f_sample);
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => output_device.build_output_stream(
            &output_config,
            move |data: &mut [u16], _: &_| {
                for sample in data.iter_mut() {
                    let f_sample = consumer.try_pop().unwrap_or(0.0);
                    *sample = f32_to_u16(f_sample);
                }
            },
            err_fn,
            None,
        ),
        _ => return Err(anyhow!("Unsupported output sample format")),
    }?;

    input_stream.play()?;
    output_stream.play()?;

    Ok(AudioStreamHandle {
        _input_stream: input_stream,
        _output_stream: output_stream,
    })
}
