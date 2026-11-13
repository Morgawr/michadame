use anyhow::{anyhow, Context, Result};

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

#[derive(Default, Debug)]
struct SysfsUsbNames {
    by_bus_addr: HashMap<(u8, u8), String>,
    by_id: HashMap<String, String>,
}

impl SysfsUsbNames {
    fn lookup(&self, bus: u8, addr: u8, id: &str) -> Option<String> {
        self.by_bus_addr
            .get(&(bus, addr))
            .or_else(|| self.by_id.get(id))
            .cloned()
    }
}

fn read_trimmed_sysfs_attr(dir: &Path, attr: &str) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join(attr)).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn format_sysfs_usb_label(manufacturer: Option<&str>, product: Option<&str>) -> Option<String> {
    let prod = product?.trim();
    if prod.is_empty() {
        return None;
    }
    if let Some(mfg) = manufacturer.map(str::trim).filter(|m| !m.is_empty()) {
        if !mfg.starts_with("Linux ")
            && !prod
                .to_ascii_lowercase()
                .starts_with(&mfg.to_ascii_lowercase())
        {
            return Some(format!("{mfg} {prod}"));
        }
    }
    Some(prod.to_string())
}

/// Reads cached USB device strings directly from Linux sysfs (`/sys/bus/usb/devices/*`)
/// without spawning `lsusb` or opening USB device handles for live EP0 descriptor transfers.
fn read_sysfs_usb_names_from(sysfs_root: &Path) -> SysfsUsbNames {
    let mut out = SysfsUsbNames::default();
    let Ok(entries) = std::fs::read_dir(sysfs_root) else {
        return out;
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        if file_name.to_string_lossy().contains(':') {
            // Skip USB interface subdirectories (e.g. `1-2:1.0`).
            continue;
        }
        let dir = entry.path();
        let Some(vid_str) = read_trimmed_sysfs_attr(&dir, "idVendor") else {
            continue;
        };
        let Some(pid_str) = read_trimmed_sysfs_attr(&dir, "idProduct") else {
            continue;
        };
        let Ok(vid) = u16::from_str_radix(&vid_str, 16) else {
            continue;
        };
        let Ok(pid) = u16::from_str_radix(&pid_str, 16) else {
            continue;
        };

        let mfg = read_trimmed_sysfs_attr(&dir, "manufacturer");
        let prod = read_trimmed_sysfs_attr(&dir, "product");
        let Some(label) = format_sysfs_usb_label(mfg.as_deref(), prod.as_deref()) else {
            continue;
        };

        let id = format!("{vid:04x}:{pid:04x}");
        out.by_id.entry(id).or_insert_with(|| label.clone());

        if let (Some(bus_s), Some(dev_s)) = (
            read_trimmed_sysfs_attr(&dir, "busnum"),
            read_trimmed_sysfs_attr(&dir, "devnum"),
        ) {
            if let (Ok(bus), Ok(addr)) = (bus_s.parse::<u8>(), dev_s.parse::<u8>()) {
                out.by_bus_addr.insert((bus, addr), label);
            }
        }
    }

    out
}

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
    let sysfs_names = read_sysfs_usb_names_from(Path::new("/sys/bus/usb/devices"));
    let devices = rusb::devices()?;
    let mut fallback_lsusb: Option<HashMap<String, String>> = None;

    for device in devices.iter() {
        if let Ok(desc) = device.device_descriptor() {
            let vid = desc.vendor_id();
            let pid = desc.product_id();
            let id = format!("{:04x}:{:04x}", vid, pid);

            // Deduplicate IDs early in case of multi-device / hub entries.
            if result.iter().any(|(existing_id, _)| existing_id == &id) {
                continue;
            }

            // 1. Prefer kernel-cached sysfs descriptor strings (zero USB EP0 traffic, zero subprocesses).
            let name = if let Some(sysfs_name) =
                sysfs_names.lookup(device.bus_number(), device.address(), &id)
            {
                sysfs_name
            } else {
                // 2. Fallback to libusb string descriptor read, then lazy `lsusb` lookup.
                match device.open() {
                    Ok(timeout_handle) => match timeout_handle.read_product_string_ascii(&desc) {
                        Ok(n) if !n.trim().is_empty() => n.trim().to_string(),
                        _ => fallback_lsusb
                            .get_or_insert_with(get_lsusb_names)
                            .get(&id)
                            .cloned()
                            .unwrap_or_else(|| "Generic Device".to_string()),
                    },
                    Err(_) => fallback_lsusb
                        .get_or_insert_with(get_lsusb_names)
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| "Restricted System Device".to_string()),
                }
            };

            result.push((id, name));
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

    #[test]
    fn test_read_sysfs_usb_names_from_synthetic_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // 1. Capture card with distinct manufacturer + product
        let dev1 = root.join("4-2");
        std::fs::create_dir_all(&dev1).unwrap();
        std::fs::write(dev1.join("idVendor"), "345f\n").unwrap();
        std::fs::write(dev1.join("idProduct"), "2131\n").unwrap();
        std::fs::write(dev1.join("busnum"), "4\n").unwrap();
        std::fs::write(dev1.join("devnum"), "2\n").unwrap();
        std::fs::write(dev1.join("manufacturer"), "MACROSILICON\n").unwrap();
        std::fs::write(dev1.join("product"), "UGREEN 25773\n").unwrap();

        // 2. Device whose product already starts with manufacturer (no duplicate prefix)
        let dev2 = root.join("1-5");
        std::fs::create_dir_all(&dev2).unwrap();
        std::fs::write(dev2.join("idVendor"), "054C\n").unwrap();
        std::fs::write(dev2.join("idProduct"), "0CE6\n").unwrap();
        std::fs::write(dev2.join("busnum"), "1\n").unwrap();
        std::fs::write(dev2.join("devnum"), "5\n").unwrap();
        std::fs::write(dev2.join("manufacturer"), "Sony\n").unwrap();
        std::fs::write(dev2.join("product"), "Sony DualSense\n").unwrap();

        // 3. Root hub with Linux kernel hcd manufacturer string -> keeps clean product name
        let hub = root.join("usb1");
        std::fs::create_dir_all(&hub).unwrap();
        std::fs::write(hub.join("idVendor"), "1d6b\n").unwrap();
        std::fs::write(hub.join("idProduct"), "0002\n").unwrap();
        std::fs::write(hub.join("busnum"), "1\n").unwrap();
        std::fs::write(hub.join("devnum"), "1\n").unwrap();
        std::fs::write(hub.join("manufacturer"), "Linux 6.12.0 xhci-hcd\n").unwrap();
        std::fs::write(hub.join("product"), "xHCI Host Controller\n").unwrap();

        // 4. Interface directory (`4-2:1.0`) should be ignored
        let iface = root.join("4-2:1.0");
        std::fs::create_dir_all(&iface).unwrap();
        std::fs::write(iface.join("idVendor"), "345f\n").unwrap();
        std::fs::write(iface.join("idProduct"), "2131\n").unwrap();
        std::fs::write(iface.join("product"), "Wrong Interface Name\n").unwrap();

        let sysfs = read_sysfs_usb_names_from(root);
        assert_eq!(
            sysfs.lookup(4, 2, "345f:2131").as_deref(),
            Some("MACROSILICON UGREEN 25773")
        );
        assert_eq!(
            sysfs.lookup(1, 5, "054c:0ce6").as_deref(),
            Some("Sony DualSense")
        );
        assert_eq!(
            sysfs.lookup(1, 1, "1d6b:0002").as_deref(),
            Some("xHCI Host Controller")
        );
        assert_eq!(sysfs.lookup(9, 9, "dead:beef"), None);
    }
}
