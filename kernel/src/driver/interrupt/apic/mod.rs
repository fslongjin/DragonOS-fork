use core::sync::atomic::Ordering;

use atomic_enum::atomic_enum;
use x86::apic::Icr;

use crate::{
    driver::interrupt::apic::{ioapic::ioapic_init, x2apic::X2Apic, xapic::XApic},
    kdebug, kinfo,
    libs::once::Once,
    smp::core::smp_get_processor_id,
    syscall::SystemError,
};

use self::{
    apic_timer::ApicTimerMode,
    xapic::{xapic_instances_mut, XApicOffset},
};

pub mod apic_timer;
mod c_adapter;
pub mod ioapic;
pub mod new_timer;
pub mod x2apic;
pub mod xapic;

/// 当前启用的APIC类型
#[atomic_enum]
#[derive(PartialEq, Eq)]
pub enum LocalApicEnableType {
    XApic,
    X2Apic,
}

static LOCAL_APIC_ENABLE_TYPE: AtomicLocalApicEnableType =
    AtomicLocalApicEnableType::new(LocalApicEnableType::XApic);

pub trait LocalAPIC {
    /// @brief 判断当前处理器是否支持这个类型的apic
    ///
    /// @return true 当前处理器支持这个类型的apic
    /// @return false 当前处理器不支持这个类型的apic
    fn support() -> bool;

    /// @brief 为当前处理器初始化local apic
    ///
    /// @return true 初始化成功
    /// @return false 初始化失败
    fn init_current_cpu(&mut self) -> bool;

    /// @brief 发送EOI信号（End of interrupt）
    fn send_eoi(&mut self);

    /// @brief 获取APIC版本号
    fn version(&self) -> u8;

    /// @brief 判断当前处理器是否支持EOI广播抑制
    fn support_eoi_broadcast_suppression(&self) -> bool;

    /// 获取最多支持的LVT寄存器数量
    fn max_lvt_entry(&self) -> u8;

    /// @brief 获取当前处理器的APIC ID
    fn id(&self) -> u32;

    /// @brief 设置LVT寄存器
    ///
    /// @param register 寄存器
    /// @param lvt 要被设置成的值
    fn set_lvt(&mut self, lvt: LVT);

    /// 读取LVT寄存器
    fn read_lvt(&self, reg: LVTRegister) -> LVT;

    fn mask_all_lvt(&mut self);

    /// 写入ICR寄存器
    fn write_icr(&self, icr: Icr);
}

/// @brief 所有LVT寄存器的枚举类型
#[allow(dead_code)]
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum LVTRegister {
    /// CMCI寄存器
    ///
    /// 如果支持CMCI功能，那么，当修正的机器错误超过阈值时，Local APIC通过CMCI寄存器的配置，
    /// 向处理器核心投递中断消息
    CMCI = 0x82f,
    /// 定时器寄存器
    ///
    /// 当APIC定时器产生中断信号时，Local APIC通过定时器寄存器的设置，向处理器投递中断消息
    Timer = 0x832,
    /// 温度传感器寄存器
    ///
    /// 当处理器内部的温度传感器产生中断请求信号时，Local APIC会通过温度传感器寄存器的设置，
    /// 向处理器投递中断消息。
    Thermal = 0x833,
    /// 性能监控计数器寄存器
    ///
    /// 当性能检测计数器寄存器溢出，产生中断请求时，Local APIC将会根据这个寄存器的配置，
    /// 向处理器投递中断消息
    PerformanceMonitor = 0x834,
    /// 当处理器的LINT0引脚接收到中断请求信号时，Local APIC会根据这个寄存器的配置，
    /// 向处理器投递中断消息
    LINT0 = 0x835,
    /// 当处理器的LINT0引脚接收到中断请求信号时，Local APIC会根据这个寄存器的配置，
    /// 向处理器投递中断消息
    LINT1 = 0x836,
    /// 错误寄存器
    ///
    /// 当APIC检测到内部错误而产生中断请求信号时，它将会通过错误寄存器的设置，向处理器投递中断消息
    ErrorReg = 0x837,
}

impl Into<u32> for LVTRegister {
    fn into(self) -> u32 {
        self as u32
    }
}

#[derive(Debug)]
pub struct LVT {
    register: LVTRegister,
    data: u32,
}

impl LVT {
    /// 当第16位为1时，表示屏蔽中断
    pub const MASKED: u32 = 1 << 16;

    pub fn new(register: LVTRegister, data: u32) -> Option<Self> {
        // vector: u8, mode: DeliveryMode, status: DeliveryStatus
        let mut result = Self { register, data: 0 };
        result.set_vector((data & 0xFF) as u8);
        match result.register {
            LVTRegister::Timer | LVTRegister::ErrorReg => {}
            _ => {
                result
                    .set_delivery_mode(DeliveryMode::try_from(((data >> 8) & 0b111) as u8).ok()?)
                    .ok()?;
            }
        }

        if let LVTRegister::LINT0 | LVTRegister::LINT1 = result.register {
            result.set_interrupt_input_pin_polarity((data & (1 << 13)) == 0);

            if data & (1 << 15) != 0 {
                result.set_trigger_mode(TriggerMode::Level).ok()?;
            } else {
                result.set_trigger_mode(TriggerMode::Edge).ok()?;
            }
        }
        result.set_mask((data & (1 << 16)) != 0);

        if let LVTRegister::Timer = result.register {
            result
                .set_timer_mode(ApicTimerMode::try_from(((data >> 17) & 0b11) as u8).ok()?)
                .ok()?;
        }

        return Some(result);
    }

    pub fn data(&self) -> u32 {
        return self.data;
    }

    pub fn register(&self) -> LVTRegister {
        return self.register;
    }

    pub fn set_vector(&mut self, vector: u8) {
        self.data &= !((1 << 8) - 1);
        self.data |= vector as u32;
    }

    pub fn vector(&self) -> u8 {
        return (self.data & 0xFF) as u8;
    }

    /// 设置中断投递模式
    ///
    /// Timer、ErrorReg寄存器不支持这个功能
    ///
    /// ## 参数
    ///
    /// - `mode`：投递模式
    pub fn set_delivery_mode(&mut self, mode: DeliveryMode) -> Result<(), SystemError> {
        match self.register {
            LVTRegister::Timer | LVTRegister::ErrorReg => {
                return Err(SystemError::EINVAL);
            }
            _ => {}
        }

        self.data &= 0xFFFF_F8FF;
        self.data |= ((mode as u32) & 0x7) << 8;
        return Ok(());
    }

    /// 获取中断投递模式
    /// Timer、ErrorReg寄存器不支持这个功能
    pub fn delivery_mode(&self) -> Option<DeliveryMode> {
        if let LVTRegister::Timer | LVTRegister::ErrorReg = self.register {
            return None;
        }
        return DeliveryMode::try_from(((self.data >> 8) & 0b111) as u8).ok();
    }

    pub fn delivery_status(&self) -> DeliveryStatus {
        return DeliveryStatus::from(self.data);
    }

    /// 设置中断输入引脚的极性
    ///
    /// ## 参数
    ///
    /// - `high`：true表示高电平有效，false表示低电平有效
    pub fn set_interrupt_input_pin_polarity(&mut self, high: bool) {
        self.data &= 0xFFFF_DFFF;
        // 0表示高电平有效，1表示低电平有效
        if !high {
            self.data |= 1 << 13;
        }
    }

    /// 获取中断输入引脚的极性
    ///
    /// true表示高电平有效，false表示低电平有效
    pub fn interrupt_input_pin_polarity(&self) -> bool {
        return (self.data & (1 << 13)) == 0;
    }

    /// 设置中断输入引脚的触发模式
    ///
    /// 只有LINT0和LINT1寄存器支持这个功能
    ///
    /// ## 参数
    ///
    /// - `trigger_mode`：触发模式
    pub fn set_trigger_mode(&mut self, trigger_mode: TriggerMode) -> Result<(), SystemError> {
        match self.register {
            LVTRegister::LINT0 | LVTRegister::LINT1 => {
                self.data &= 0xFFFF_7FFF;
                if trigger_mode == TriggerMode::Level {
                    self.data |= 1 << 15;
                }
                return Ok(());
            }
            _ => {
                return Err(SystemError::EINVAL);
            }
        }
    }

    /// 获取中断输入引脚的触发模式
    ///
    /// 只有LINT0和LINT1寄存器支持这个功能
    pub fn trigger_mode(&self) -> Option<TriggerMode> {
        match self.register {
            LVTRegister::LINT0 | LVTRegister::LINT1 => {
                if self.data & (1 << 15) != 0 {
                    return Some(TriggerMode::Level);
                } else {
                    return Some(TriggerMode::Edge);
                }
            }
            _ => {
                return None;
            }
        }
    }

    /// 设置是否屏蔽中断
    ///
    /// ## 参数
    ///
    /// - `mask`：true表示屏蔽中断，false表示不屏蔽中断
    pub fn set_mask(&mut self, mask: bool) {
        self.data &= 0xFFFE_FFFF;
        if mask {
            self.data |= 1 << 16;
        }
    }

    /// 获取是否屏蔽中断
    pub fn mask(&self) -> bool {
        return (self.data & (1 << 16)) != 0;
    }

    /// 设置定时器模式
    pub fn set_timer_mode(&mut self, mode: ApicTimerMode) -> Result<(), SystemError> {
        match self.register {
            LVTRegister::Timer => {
                self.data &= 0xFFF9_FFFF;
                match mode {
                    ApicTimerMode::OneShot => {
                        self.data |= 0b00 << 17;
                    }
                    ApicTimerMode::Periodic => {
                        self.data |= 0b01 << 17;
                    }
                    ApicTimerMode::TSCDeadline => {
                        self.data |= 0b10 << 17;
                    }
                }
                return Ok(());
            }
            _ => {
                return Err(SystemError::EINVAL);
            }
        }
    }

    pub fn timer_mode(&self) -> Option<ApicTimerMode> {
        if let LVTRegister::Timer = self.register {
            let mode = (self.data >> 17) & 0b11;
            match mode {
                0b00 => {
                    return Some(ApicTimerMode::OneShot);
                }
                0b01 => {
                    return Some(ApicTimerMode::Periodic);
                }
                0b10 => {
                    return Some(ApicTimerMode::TSCDeadline);
                }
                _ => {
                    return None;
                }
            }
        }
        return None;
    }
}

/// @brief
#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub enum DeliveryMode {
    /// 由LVT寄存器的向量号区域指定中断向量号
    Fixed = 0b000,
    /// 通过处理器的SMI信号线，向处理器投递SMI中断请求。
    /// 由于兼容性的原因，使用此投递模式时，LVT的中断向量号区域必须设置为0。
    SMI = 0b010,
    /// 向处理器投递不可屏蔽中断，并忽略向量号区域
    NMI = 0b100,
    /// 向处理器投递INIT中断请求，处理器会执行初始化的过程。
    /// 由于兼容性的原因，使用此投递模式时，LVT的中断向量号区域必须设置为0。
    /// CMCI、温度传感器、性能监控计数器等寄存器均不支持INIT投递模式
    INIT = 0b101,

    /// 向目标处理器投递Start-Up IPI。
    ///
    /// 这个向量通常由多核引导模块调用（请参阅Intel开发手册Volume3 Section 8.4,
    /// Multiple-Processor (MP) Initialization）。
    /// 如果源APIC无法投递这个IPI，它不会自动重发。如果Start-Up IPI未成功投递，
    /// 则交由软件决定是否在必要时重新投递SIPI
    StartUp = 0b110,

    /// ExtINT模式可以将类8259A中断控制器产生的中断请求投递到处理器，并接收类
    /// 8259A中断控制器提供的中断向量号。
    /// CMCI、温度传感器、性能监控计数器等寄存器均不支持ExtINT投递模式
    ExtINT = 0b111,
}

impl TryFrom<u8> for DeliveryMode {
    type Error = SystemError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0b000 => {
                return Ok(DeliveryMode::Fixed);
            }
            0b010 => {
                return Ok(DeliveryMode::SMI);
            }
            0b100 => {
                return Ok(DeliveryMode::NMI);
            }
            0b101 => {
                return Ok(DeliveryMode::INIT);
            }
            0b110 => {
                return Ok(DeliveryMode::StartUp);
            }
            0b111 => {
                return Ok(DeliveryMode::ExtINT);
            }
            _ => {
                return Err(SystemError::EINVAL);
            }
        }
    }
}

/// @brief 投递状态
#[derive(Debug)]
#[allow(dead_code)]
pub enum DeliveryStatus {
    /// 空闲态。
    /// 此状态表明，当前中断源未产生中断，或者产生的中断已经投递到处理器，并被处理器处理。
    Idle = 0,
    /// 发送挂起状态。
    /// 此状态表明，中断源产生的请求已经投递至处理器，但尚未被处理器处理。
    SendPending = 1,
}

impl DeliveryStatus {
    pub fn from(data: u32) -> Self {
        if data & (1 << 12) == 0 {
            return DeliveryStatus::Idle;
        } else {
            return DeliveryStatus::SendPending;
        }
    }
}

/// IPI Trigger Mode
#[derive(Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum TriggerMode {
    Edge = 0,
    Level = 1,
}

/// 初始化bsp处理器的apic
#[no_mangle]
pub extern "C" fn rs_apic_init_bsp() -> i32 {
    let r = apic_init();
    if r.is_ok() {
        return 0;
    } else {
        return r.unwrap_err();
    }
}

/// @brief 初始化apic
pub fn apic_init() -> Result<(), i32> {
    static INIT: Once = Once::new();
    assert!(!INIT.is_completed());

    INIT.call_once(|| {
        kdebug!("Support xAPIC?.. {}", XApic::support());
        kdebug!("Support x2APIC?.. {}", X2Apic::support());

        if X2Apic::support() && X2Apic.init_current_cpu() {
            LOCAL_APIC_ENABLE_TYPE.store(LocalApicEnableType::X2Apic, Ordering::SeqCst);
            kinfo!("x2APIC initialized for bsp");
        } else {
            todo!("init xAPIC for bsp");
            LOCAL_APIC_ENABLE_TYPE.store(LocalApicEnableType::XApic, Ordering::SeqCst);
        }

        ioapic_init();
        kinfo!("Apic initialized.");
    });

    return Ok(());
}

#[no_mangle]
pub extern "C" fn rs_apic_init_ap() -> i32 {
    let r = apic_init_ap_core()
        .map(|_| 0)
        .unwrap_or_else(|e| e.to_posix_errno());
    return r;
}

/// 初始化ap核心的local apic
pub fn apic_init_ap_core() -> Result<(), SystemError> {
    if X2Apic::support() && X2Apic.init_current_cpu() {
        kinfo!("x2APIC initialized for cpu {}", smp_get_processor_id());
    } else {
        todo!("init xApic for ap core {}", smp_get_processor_id());
    }

    return Ok(());
}

#[derive(Debug)]
pub struct CurrentApic;

impl CurrentApic {
    /// x2apic是否启用
    pub fn x2apic_enabled(&self) -> bool {
        return LOCAL_APIC_ENABLE_TYPE.load(Ordering::SeqCst) == LocalApicEnableType::X2Apic;
    }

    pub(self) unsafe fn write_xapic_register(&self, reg: XApicOffset, value: u32) {
        xapic_instances_mut()
            .get_mut()
            .borrow_mut()
            .as_mut()
            .map(|xapic| {
                xapic.write(reg, value);
            });
    }
}

impl LocalAPIC for CurrentApic {
    fn support() -> bool {
        true
    }

    fn init_current_cpu(&mut self) -> bool {
        let cpu_id = smp_get_processor_id();
        if X2Apic::support() && X2Apic.init_current_cpu() {
            if cpu_id == 0 {
                LOCAL_APIC_ENABLE_TYPE.store(LocalApicEnableType::X2Apic, Ordering::SeqCst);
            }
            kinfo!("x2APIC initialized for cpu {}", cpu_id);
        } else {
            todo!("init xApic for core {}", smp_get_processor_id());
            if cpu_id == 0 {
                LOCAL_APIC_ENABLE_TYPE.store(LocalApicEnableType::XApic, Ordering::SeqCst);
            }
        }
        if cpu_id == 0 {
            ioapic_init();
        }
        kinfo!("Apic initialized.");
        return true;
    }

    fn send_eoi(&mut self) {
        if LOCAL_APIC_ENABLE_TYPE.load(Ordering::SeqCst) == LocalApicEnableType::X2Apic {
            X2Apic.send_eoi();
        } else {
            xapic_instances_mut()
                .get_mut()
                .borrow_mut()
                .as_mut()
                .map(|xapic| {
                    xapic.send_eoi();
                });
        }
    }

    fn version(&self) -> u8 {
        if LOCAL_APIC_ENABLE_TYPE.load(Ordering::SeqCst) == LocalApicEnableType::X2Apic {
            return X2Apic.version();
        } else {
            return xapic_instances_mut()
                .get()
                .borrow()
                .as_ref()
                .map(|xapic| xapic.version())
                .unwrap_or(0);
        }
    }

    fn support_eoi_broadcast_suppression(&self) -> bool {
        if LOCAL_APIC_ENABLE_TYPE.load(Ordering::SeqCst) == LocalApicEnableType::X2Apic {
            return X2Apic.support_eoi_broadcast_suppression();
        } else {
            return xapic_instances_mut()
                .get()
                .borrow()
                .as_ref()
                .map(|xapic| xapic.support_eoi_broadcast_suppression())
                .unwrap_or(false);
        }
    }

    fn max_lvt_entry(&self) -> u8 {
        if LOCAL_APIC_ENABLE_TYPE.load(Ordering::SeqCst) == LocalApicEnableType::X2Apic {
            return X2Apic.max_lvt_entry();
        } else {
            return xapic_instances_mut()
                .get()
                .borrow()
                .as_ref()
                .map(|xapic| xapic.max_lvt_entry())
                .unwrap_or(0);
        }
    }

    fn id(&self) -> u32 {
        if LOCAL_APIC_ENABLE_TYPE.load(Ordering::SeqCst) == LocalApicEnableType::X2Apic {
            return X2Apic.id();
        } else {
            return xapic_instances_mut()
                .get()
                .borrow()
                .as_ref()
                .map(|xapic| xapic.id())
                .unwrap_or(0);
        }
    }

    fn set_lvt(&mut self, lvt: LVT) {
        if LOCAL_APIC_ENABLE_TYPE.load(Ordering::SeqCst) == LocalApicEnableType::X2Apic {
            X2Apic.set_lvt(lvt);
        } else {
            xapic_instances_mut()
                .get_mut()
                .borrow_mut()
                .as_mut()
                .map(|xapic| {
                    xapic.set_lvt(lvt);
                });
        }
    }

    fn read_lvt(&self, reg: LVTRegister) -> LVT {
        if LOCAL_APIC_ENABLE_TYPE.load(Ordering::SeqCst) == LocalApicEnableType::X2Apic {
            return X2Apic.read_lvt(reg);
        } else {
            return xapic_instances_mut()
                .get()
                .borrow()
                .as_ref()
                .map(|xapic| xapic.read_lvt(reg))
                .unwrap_or(LVT {
                    register: reg,
                    data: 0,
                });
        }
    }

    fn mask_all_lvt(&mut self) {
        if LOCAL_APIC_ENABLE_TYPE.load(Ordering::SeqCst) == LocalApicEnableType::X2Apic {
            X2Apic.mask_all_lvt();
        } else {
            xapic_instances_mut()
                .get_mut()
                .borrow_mut()
                .as_mut()
                .map(|xapic| {
                    xapic.mask_all_lvt();
                });
        }
    }

    fn write_icr(&self, icr: Icr) {
        if LOCAL_APIC_ENABLE_TYPE.load(Ordering::SeqCst) == LocalApicEnableType::X2Apic {
            X2Apic.write_icr(icr);
        } else {
            xapic_instances_mut()
                .get_mut()
                .borrow_mut()
                .as_mut()
                .map(|xapic| {
                    xapic.write_icr(icr);
                });
        }
    }
}