pub mod audio;
pub mod usb;
pub mod video;

pub enum DeviceScanResult {
    Success(Vec<String>, Vec<(String, String)>, Vec<(String, String)>, Vec<(String, String)>),
    Failure(anyhow::Error),
}