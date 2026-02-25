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
    _audio_source: Option<String>,
    _audio_sink: Option<String>,
    _tx: crossbeam_channel::Sender<DeviceScanResult>,
) -> DeviceScanResultData {
    let video_devices = video::find_video_devices().unwrap_or_default();
    let (audio_sources, audio_sinks) = audio::find_audio_devices().unwrap_or_default();
    let usb_devices = usb::find_usb_devices().unwrap_or_default();
    (video_devices, audio_sources, audio_sinks, usb_devices)
}
