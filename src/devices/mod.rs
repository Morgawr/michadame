pub mod audio;
pub mod filters;
pub mod usb;
pub mod video;
use anyhow::Result;

pub type DeviceScanResultData =
    (Vec<String>, Vec<(String, String)>, Vec<(String, String)>, Vec<(String, String)>);
pub type DeviceScanResult = Result<DeviceScanResultData>;