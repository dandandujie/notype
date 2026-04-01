//! Audio input device enumeration.

use cpal::traits::{DeviceTrait, HostTrait};

use crate::{AudioError, Result};

#[derive(Debug, Clone)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
}

/// List all available audio input devices.
pub fn list_input_devices() -> Result<Vec<AudioDeviceInfo>> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();

    let devices = host
        .input_devices()
        .map_err(|e| AudioError::DeviceError(e.to_string()))?;

    let mut result = Vec::new();
    for device in devices {
        if let Ok(name) = device.name() {
            result.push(AudioDeviceInfo {
                is_default: name == default_name,
                name,
            });
        }
    }

    tracing::debug!("Found {} input devices", result.len());
    Ok(result)
}

/// Get the default input device, or a specific device by name.
pub(crate) fn get_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();

    match name {
        Some(name) => {
            let devices = host
                .input_devices()
                .map_err(|e| AudioError::DeviceError(e.to_string()))?;
            for device in devices {
                if device.name().ok().as_deref() == Some(name) {
                    return Ok(device);
                }
            }
            Err(AudioError::DeviceError(format!("device not found: {name}")))
        }
        None => host.default_input_device().ok_or(AudioError::NoInputDevice),
    }
}
