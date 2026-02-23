use anyhow::{anyhow, Context, Result};

pub fn find_usb_devices() -> Result<Vec<(String, String)>> {
    let mut result = Vec::new();
    let devices = rusb::devices()?;
    for device in devices.iter() {
        if let Ok(desc) = device.device_descriptor() {
            let vid = desc.vendor_id();
            let pid = desc.product_id();
            let id = format!("{:04x}:{:04x}", vid, pid);
            
            // Try to pull the human readable product descriptor string. This requires system bus permissions.
            let name = match device.open() {
                Ok(timeout_handle) => {
                    match timeout_handle.read_product_string_ascii(&desc) {
                        Ok(n) => n,
                        Err(_) => "Generic Device".to_string(),
                    }
                }
                Err(_) => "Restricted System Device".to_string(),
            };
            
            // deduplicate IDs incase of multi-interfaces etc.
            if !result.iter().any(|(existing_id, _)| existing_id == &id) {
                result.push((id, name));
            }
        }
    }
    result.sort();
    Ok(result)
}

pub fn reset_usb_device(device_id: &str) -> Result<()> {
    let parts: Vec<&str> = device_id.split(':').collect();
    if parts.len() != 2 {
        return Err(anyhow!("Invalid device ID format. Expected VID:PID"));
    }
    
    let vid: u16 = u16::from_str_radix(parts[0], 16)?;
    let pid: u16 = u16::from_str_radix(parts[1], 16)?;
    
    let devices = rusb::devices()?;
    for device in devices.iter() {
        if let Ok(desc) = device.device_descriptor() {
            if desc.vendor_id() == vid && desc.product_id() == pid {
                match device.open() {
                    Ok(handle) => {
                        if let Err(e) = handle.reset() {
                            tracing::warn!("Native reset failed: {}. Falling back to pkexec.", e);
                        } else {
                            tracing::info!("Successfully sent IOCTL port reset to USB bus {:04x}:{:04x}", vid, pid);
                            return Ok(());
                        }
                    },
                    Err(e) => {
                        tracing::warn!("Insufficient privileges to open bus natively: {}. Falling back to pkexec.", e);
                    }
                }
                
                let status = std::process::Command::new("pkexec")
                    .arg("usbreset")
                    .arg(device_id)
                    .status()
                    .context("Failed to execute 'pkexec usbreset'. Is pkexec and usbreset installed?")?;

                if status.success() {
                    return Ok(());
                } else {
                    return Err(anyhow!("'pkexec usbreset' failed with status: {}", status));
                }
            }
        }
    }
    
    Err(anyhow!("Device not currently connected to the bus."))
}
