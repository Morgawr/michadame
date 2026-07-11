use anyhow::{anyhow, Context, Result};

use std::collections::HashMap;
use std::process::Command;

fn parse_lsusb_output(out_str: &str) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for line in out_str.lines() {
        if let Some(id_pos) = line.find("ID ") {
            let rest = &line[id_pos + 3..];
            if rest.len() >= 9 && rest.as_bytes()[4] == b':' {
                let id = &rest[0..9];
                let name = &rest[9..].trim();
                names.insert(id.to_string(), name.to_string());
            }
        }
    }
    names
}

fn get_lsusb_names() -> HashMap<String, String> {
    if let Ok(output) = Command::new("lsusb").output() {
        if let Ok(out_str) = String::from_utf8(output.stdout) {
            return parse_lsusb_output(&out_str);
        }
    }
    HashMap::new()
}

pub fn find_usb_devices() -> Result<Vec<(String, String)>> {
    let mut result = Vec::new();
    let devices = rusb::devices()?;
    let fallback_names = get_lsusb_names();

    for device in devices.iter() {
        if let Ok(desc) = device.device_descriptor() {
            let vid = desc.vendor_id();
            let pid = desc.product_id();
            let id = format!("{:04x}:{:04x}", vid, pid);

            // Try to pull the human readable product descriptor string. This requires system bus permissions.
            let name = match device.open() {
                Ok(timeout_handle) => match timeout_handle.read_product_string_ascii(&desc) {
                    Ok(n) => n,
                    Err(_) => fallback_names
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| "Generic Device".to_string()),
                },
                Err(_) => fallback_names
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| "Restricted System Device".to_string()),
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
                            tracing::info!(
                                "Successfully sent IOCTL port reset to USB bus {:04x}:{:04x}",
                                vid,
                                pid
                            );
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Insufficient privileges to open bus natively: {}. Falling back to pkexec.", e);
                    }
                }

                let status = std::process::Command::new("pkexec")
                    .arg("usbreset")
                    .arg(device_id)
                    .status()
                    .context(
                        "Failed to execute 'pkexec usbreset'. Is pkexec and usbreset installed?",
                    )?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lsusb_output() {
        let sample_output = "\
Bus 001 Device 001: ID 1d6b:0002 Linux Foundation 2.0 root hub
Bus 001 Device 005: ID 054c:0ce6 Sony Corp. DualSense wireless controller (PS5)
Bus 001 Device 038: ID 1397:00d4 BEHRINGER International GmbH X18/XR18
Bus 004 Device 002: ID 345f:2131 MACROSILICON UGREEN 25773\n";

        let names = parse_lsusb_output(sample_output);

        assert_eq!(
            names.get("1d6b:0002").unwrap(),
            "Linux Foundation 2.0 root hub"
        );
        assert_eq!(
            names.get("054c:0ce6").unwrap(),
            "Sony Corp. DualSense wireless controller (PS5)"
        );
        assert_eq!(
            names.get("1397:00d4").unwrap(),
            "BEHRINGER International GmbH X18/XR18"
        );
        assert_eq!(names.get("345f:2131").unwrap(), "MACROSILICON UGREEN 25773");
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn test_parse_lsusb_output_invalid_lines() {
        let sample_output = "\
Some random text
Bus 001 Device 001 ID missing colon
Bus 001 Device 005: ID 1234:567
Bus 001 Device 038: ID 1397:00d4 Valid Device\n";

        let names = parse_lsusb_output(sample_output);

        // Should ignore invalid lines and successfully parse the valid one
        assert_eq!(names.get("1397:00d4").unwrap(), "Valid Device");
        assert_eq!(names.len(), 1);
    }
}
