use anyhow::{anyhow, Context, Result};
use std::process::Command;

pub fn reset_usb_device(device_id: &str) -> Result<()> {
    let status = Command::new("pkexec")
        .arg("usbreset")
        .arg(device_id)
        .status()
        .context("Failed to execute 'pkexec usbreset'. Is pkexec installed?")?;

    if status.success() {
        Ok(())
    } else {
        let msg = format!("'pkexec usbreset' failed with status: {}. Check if 'usbreset' is in your PATH.", status);
        tracing::error!("{}", msg);
        Err(anyhow!(msg))
    }
}

pub fn find_usb_devices() -> Result<Vec<(String, String)>> {
    let output = Command::new("lsusb")
        .output()
        .context("Failed to execute 'lsusb'. Is it installed and in your PATH?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("lsusb failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let devices = stdout.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() > 6 && parts[4] == "ID" {
                let id = parts[5].to_string();
                let name = parts[6..].join(" ");
                Some((id, name))
            } else {
                None
            }
        })
        .collect();

    Ok(devices)
}