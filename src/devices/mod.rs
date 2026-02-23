pub mod audio;
pub mod filter_type;
pub mod usb;
pub mod video;
use anyhow::Result;

pub type DeviceScanResultData = (
    Vec<String>,
    Vec<(String, String)>,
    Vec<(String, String)>,
    Vec<(String, String)>,
);
pub type DeviceScanResult = Result<DeviceScanResultData>;

pub fn scan_devices(
    _pulse_source: Option<String>,
    _pulse_sink: Option<String>,
    _tx: crossbeam_channel::Sender<DeviceScanResult>,
) -> DeviceScanResultData {
    let video_devices = video::find_video_devices().unwrap_or_default();
    let (pulse_sources, pulse_sinks) = audio::find_pulse_devices().unwrap_or_default();
    let usb_devices = usb::find_usb_devices().unwrap_or_default();
    (video_devices, pulse_sources, pulse_sinks, usb_devices)
}
