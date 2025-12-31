use crate::driver::base::block::bio::init_bio_pools;
use crate::driver::input::serio::serio_bus_init;
use system_error::SystemError;

use super::{
    class::classes_init,
    cpu::CpuDeviceManager,
    device::{bus::buses_init, init::devices_init},
    firmware::firmware_init,
    hypervisor::hypervisor_init,
    platform::platform_bus_init,
};

/// 初始化设备驱动模型
#[inline(never)]
pub fn driver_init() -> Result<(), SystemError> {
    devices_init()?;
    buses_init()?;
    classes_init()?;
    firmware_init()?;
    hypervisor_init()?;
    platform_bus_init()?;
    serio_bus_init()?;
    CpuDeviceManager::init()?;

    // 初始化 Bio 内存池
    if let Err(e) = init_bio_pools() {
        log::warn!("Failed to initialize Bio segment pools: {:?}", e);
        // 不返回错误，Bio 池是可选的优化
    }

    // 至此，已完成设备驱动模型的初始化
    return Ok(());
}
