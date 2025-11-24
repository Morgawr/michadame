pub mod audio;
pub mod usb;
pub mod video;
use anyhow::Result;

pub type DeviceScanResultData =
    (Vec<String>, Vec<(String, String)>, Vec<(String, String)>, Vec<(String, String)>);
pub type DeviceScanResult = Result<DeviceScanResultData>;