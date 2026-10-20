//! What is read from the devices that are usable, and kept up to date: the
//! facts a device window shows. It is the core that reads, not a window: every
//! window and the device list show the same latest reading, however many are
//! open.

use super::display;
use super::info::{self as di, DeviceInfo};
use crate::core::adb::api::AdbApi;
use crate::core::context::Ctx;
use crate::core::error::CoreError;
use crate::core::log::{LogCode, LogLevel};
use crate::core::model::{DeviceDetails, DeviceInfoUpdate, DeviceState, Event};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the device-reading loop looks at the settings and the device list.
const INFO_TICK: Duration = Duration::from_secs(1);
/// How long to wait before asking a device that has no reading (it did not
/// answer) again.
const INFO_RETRY: Duration = Duration::from_secs(5);

pub struct ReadingService {
    ctx: Arc<Ctx>,
    /// What was last read from each device that is usable.
    infos: Mutex<HashMap<String, DeviceInfo>>,
}

impl ReadingService {
    pub(crate) fn new(ctx: Arc<Ctx>) -> Arc<Self> {
        Arc::new(ReadingService { ctx, infos: Mutex::new(HashMap::new()) })
    }

    /// Reads each usable device again every `device_refresh_seconds` (a setting,
    /// looked at every second, so a change applies at once; 0 reads a device
    /// only when it has not been read yet). Runs for the whole lifetime of the
    /// application.
    pub(crate) async fn run(self: Arc<Self>) {
        let mut last_pass: Option<Instant> = None;
        // When a device without a reading was last tried, so one that does not
        // answer is not asked every second.
        let mut last_try: HashMap<String, Instant> = HashMap::new();
        loop {
            tokio::time::sleep(INFO_TICK).await;
            let (seconds, usable) = self.ctx.with_state(|i| {
                let usable: Vec<(String, String)> = i
                    .devices
                    .iter()
                    .filter_map(|d| match (d.state, &d.adb_serial) {
                        (DeviceState::Online, Some(serial)) => Some((d.id.clone(), serial.clone())),
                        _ => None,
                    })
                    .collect();
                (i.settings.device_refresh_seconds.seconds(), usable)
            });
            // What was read from a device that is no longer usable is dropped.
            self.infos.lock().unwrap().retain(|id, _| usable.iter().any(|(u, _)| u == id));
            last_try.retain(|id, _| usable.iter().any(|(u, _)| u == id));

            let due = seconds > 0 && last_pass.is_none_or(|t| t.elapsed() >= Duration::from_secs(seconds.into()));
            for (id, serial) in &usable {
                let missing = !self.infos.lock().unwrap().contains_key(id);
                let retry = missing && last_try.get(id).is_none_or(|t| t.elapsed() >= INFO_RETRY);
                if due || retry {
                    last_try.insert(id.clone(), Instant::now());
                    self.refresh_info(id, serial).await;
                }
            }
            // Timed from the end of the pass, so slow devices never make them pile up.
            if due {
                last_pass = Some(Instant::now());
            }
        }
    }

    /// Reads a device and, if it answered, keeps and announces what it said.
    async fn refresh_info(&self, device_id: &str, serial: &str) -> Option<DeviceInfo> {
        let info = self.read(serial, device_id).await?;
        self.infos.lock().unwrap().insert(device_id.to_string(), info.clone());
        self.ctx.send(Event::DeviceInfo(Box::new(DeviceInfoUpdate { device: device_id.to_string(), info: info.clone() })));
        Some(info)
    }

    /// What was last read from each usable device.
    pub fn device_infos(&self) -> HashMap<String, DeviceInfo> {
        self.infos.lock().unwrap().clone()
    }

    /// Reads one device now, instead of waiting for the next time.
    pub async fn refresh_device_info(&self, device_id: &str) -> Result<(), CoreError> {
        let (serial, _) = self.ctx.online_device(device_id)?;
        self.refresh_info(device_id, &serial).await;
        Ok(())
    }

    /// What the device window shows. The information read from the device is
    /// only there for a device that is online and answers.
    pub async fn device_details(&self, device_id: &str) -> Result<DeviceDetails, CoreError> {
        let device = self.ctx.device(device_id).ok_or(CoreError::UnknownDevice)?;
        let known = self.ctx.known_device(device_id);
        // The latest reading of the core's own loop; read now only if there is none yet.
        let cached = self.infos.lock().unwrap().get(device_id).cloned();
        let info = match (cached, device.state, &device.adb_serial) {
            (Some(info), DeviceState::Online, _) => Some(info),
            (None, DeviceState::Online, Some(serial)) => self.refresh_info(device_id, serial).await,
            _ => None,
        };
        Ok(DeviceDetails {
            reported_name: known.as_ref().map(|k| k.model.clone()).unwrap_or_else(|| device.name.clone()),
            first_seen: known.as_ref().map_or(0, |k| k.first_seen),
            last_seen: known.as_ref().map_or(0, |k| k.last_seen),
            device,
            info,
        })
    }

    async fn read(&self, serial: &str, device_id: &str) -> Option<DeviceInfo> {
        let adb = self.ctx.adb().ok()?;
        // Only what a command printed on success counts; the rest is "not read".
        let run = |command: &'static str| {
            let adb = &adb;
            async move {
                match adb.shell(serial, command).await {
                    Ok(out) if out.exit == 0 => String::from_utf8_lossy(&out.stdout).into_owned(),
                    _ => String::new(),
                }
            }
        };
        let (props, battery, storage, memory, uptime, size, density, display) = tokio::join!(
            run(di::PROPS_COMMAND),
            run(di::BATTERY_COMMAND),
            run(di::STORAGE_COMMAND),
            run(di::MEMORY_COMMAND),
            run(di::UPTIME_COMMAND),
            run(di::SIZE_COMMAND),
            run(di::DENSITY_COMMAND),
            run(display::DISPLAY_DUMP_COMMAND),
        );
        if props.is_empty() {
            self.ctx.logger.log(LogLevel::Warn, LogCode::ButtonFailed, format!("the device {device_id} did not answer"));
            return None;
        }
        Some(di::assemble(&di::Outputs {
            props: &props,
            battery: &battery,
            storage: &storage,
            memory: &memory,
            uptime: &uptime,
            size: &size,
            density: &density,
            display: &display,
        }))
    }
}
