use core::{
    any::Any,
    fmt::{Debug, Formatter},
};

use alloc::{
    boxed::Box,
    collections::VecDeque,
    string::{String, ToString},
    sync::{Arc, Weak},
    vec::Vec,
};
use bitmap::{static_bitmap, traits::BitMapOps};
use core::sync::atomic::{AtomicU64, Ordering};
use hashbrown::HashMap;
use log::error;
use system_error::SystemError;
use unified_init::macros::unified_init;
use virtio_drivers::device::blk::{BlkReq, BlkResp, RespStatus, VirtIOBlk, SECTOR_SIZE};
use virtio_drivers::{BufferDirection, Hal, PAGE_SIZE};

use crate::{
    driver::{
        base::{
            block::{
                block_device::{BlockDevice, BlockId, GeneralBlockRange, LBA_SIZE},
                disk_info::Partition,
                manager::{block_dev_manager, BlockDevMeta},
            },
            class::Class,
            device::{
                bus::Bus,
                device_number::Major,
                driver::{Driver, DriverCommonData},
                DevName, Device, DeviceCommonData, DeviceId, DeviceType, IdTable,
            },
            kobject::{KObjType, KObject, KObjectCommonData, KObjectState, LockedKObjectState},
            kset::KSet,
        },
        virtio::{
            sysfs::{virtio_bus, virtio_device_manager, virtio_driver_manager},
            transport::VirtIOTransport,
            virtio_impl::HalImpl,
            VirtIODevice, VirtIODeviceIndex, VirtIODriver, VirtIODriverCommonData, VirtioDeviceId,
            VIRTIO_VENDOR_ID,
        },
    },
    exception::{irqdesc::IrqReturn, IrqNumber},
    filesystem::{
        devfs::{DevFS, DeviceINode, LockedDevFSInode},
        kernfs::KernFSInode,
        mbr::MbrDiskPartionTable,
        vfs::{utils::DName, IndexNode, InodeMode, Metadata},
    },
    init::initcall::INITCALL_POSTCORE,
    libs::{
        mutex::{Mutex, MutexGuard},
        rwsem::{RwSem, RwSemReadGuard, RwSemWriteGuard},
        wait_queue::WaitQueue,
    },
    process::{
        kthread::{KernelThreadClosure, KernelThreadMechanism},
        ProcessFlags,
    },
    time::Duration,
};

const VIRTIO_BLK_BASENAME: &str = "virtio_blk";

static mut VIRTIO_BLK_DRIVER: Option<Arc<VirtIOBlkDriver>> = None;

#[inline(always)]
#[allow(dead_code)]
fn virtio_blk_driver() -> Arc<VirtIOBlkDriver> {
    unsafe { VIRTIO_BLK_DRIVER.as_ref().unwrap().clone() }
}

/// Get the first virtio block device
#[allow(dead_code)]
pub fn virtio_blk_0() -> Option<Arc<VirtIOBlkDevice>> {
    virtio_blk_driver()
        .devices()
        .first()
        .cloned()
        .map(|dev| dev.arc_any().downcast().unwrap())
}

pub fn virtio_blk(
    transport: VirtIOTransport,
    dev_id: Arc<DeviceId>,
    dev_parent: Option<Arc<dyn Device>>,
) {
    let device = VirtIOBlkDevice::new(transport, dev_id);
    if let Some(device) = device {
        if let Some(dev_parent) = dev_parent {
            device.set_dev_parent(Some(Arc::downgrade(&dev_parent)));
        }
        virtio_device_manager()
            .device_add(device.clone() as Arc<dyn VirtIODevice>)
            .expect("Add virtio blk failed");
    }
}

static mut VIRTIOBLK_MANAGER: Option<VirtIOBlkManager> = None;

#[inline]
fn virtioblk_manager() -> &'static VirtIOBlkManager {
    unsafe { VIRTIOBLK_MANAGER.as_ref().unwrap() }
}

#[unified_init(INITCALL_POSTCORE)]
fn virtioblk_manager_init() -> Result<(), SystemError> {
    unsafe {
        VIRTIOBLK_MANAGER = Some(VirtIOBlkManager::new());
    }
    Ok(())
}

pub struct VirtIOBlkManager {
    inner: Mutex<InnerVirtIOBlkManager>,
}

struct InnerVirtIOBlkManager {
    id_bmp: static_bitmap!(VirtIOBlkManager::MAX_DEVICES),
    devname: [Option<DevName>; VirtIOBlkManager::MAX_DEVICES],
}

impl VirtIOBlkManager {
    pub const MAX_DEVICES: usize = 25;

    pub fn new() -> Self {
        Self {
            inner: Mutex::new(InnerVirtIOBlkManager {
                id_bmp: bitmap::StaticBitmap::new(),
                devname: [const { None }; Self::MAX_DEVICES],
            }),
        }
    }

    fn inner(&self) -> MutexGuard<'_, InnerVirtIOBlkManager> {
        self.inner.lock()
    }

    pub fn alloc_id(&self) -> Option<DevName> {
        let mut inner = self.inner();
        let idx = inner.id_bmp.first_false_index()?;
        inner.id_bmp.set(idx, true);
        let name = Self::format_name(idx);
        inner.devname[idx] = Some(name.clone());
        Some(name)
    }

    /// Generate a new block device name like 'vda', 'vdb', etc.
    fn format_name(id: usize) -> DevName {
        let x = (b'a' + id as u8) as char;
        DevName::new(format!("vd{}", x), id)
    }

    #[allow(dead_code)]
    pub fn free_id(&self, id: usize) {
        if id >= Self::MAX_DEVICES {
            return;
        }
        self.inner().id_bmp.set(id, false);
        self.inner().devname[id] = None;
    }
}

/// virtio block device
#[cast_to([sync] VirtIODevice)]
#[cast_to([sync] Device)]
pub struct VirtIOBlkDevice {
    blkdev_meta: BlockDevMeta,
    dev_id: Arc<DeviceId>,
    inner: Mutex<InnerVirtIOBlkDevice>,
    /// virtio-drivers 的 non-blocking API 本身不阻塞；设备对象由 worker 与 IRQ 共享。
    blk: Mutex<VirtIOBlk<HalImpl, VirtIOTransport>>,
    submit_wq: WaitQueue,
    work_seq: AtomicU64,
    pending: Mutex<VecDeque<Arc<crate::driver::block::bio::Bio>>>,
    inflight: Mutex<HashMap<u16, Inflight>>,
    locked_kobj_state: LockedKObjectState,
    self_ref: Weak<Self>,
    parent: RwSem<Weak<LockedDevFSInode>>,
    fs: RwSem<Weak<DevFS>>,
    metadata: Metadata,
}

impl Debug for VirtIOBlkDevice {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VirtIOBlkDevice")
            .field("devname", &self.blkdev_meta.devname)
            .field("dev_id", &self.dev_id.id())
            .finish()
    }
}

unsafe impl Send for VirtIOBlkDevice {}
unsafe impl Sync for VirtIOBlkDevice {}

struct DmaBounce {
    paddr: virtio_drivers::PhysAddr,
    vaddr: core::ptr::NonNull<u8>,
    pages: usize,
    len: usize,
}

impl DmaBounce {
    fn new(len: usize) -> Result<Self, SystemError> {
        let pages = len.div_ceil(PAGE_SIZE).max(1);
        let (paddr, vaddr) = <HalImpl as Hal>::dma_alloc(pages, BufferDirection::Both);
        if paddr == 0 {
            return Err(SystemError::ENOMEM);
        }
        Ok(Self {
            paddr,
            vaddr,
            pages,
            len,
        })
    }

    fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.vaddr.as_ptr(), self.len) }
    }

    fn as_mut_slice(&self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.vaddr.as_ptr(), self.len) }
    }
}

impl Drop for DmaBounce {
    fn drop(&mut self) {
        unsafe {
            let _ = <HalImpl as Hal>::dma_dealloc(self.paddr, self.vaddr, self.pages);
        }
    }
}

struct Inflight {
    bio: Arc<crate::driver::block::bio::Bio>,
    op: crate::driver::block::bio::BioOp,
    bytes: usize,
    req: Box<BlkReq>,
    resp: Box<BlkResp>,
    dma: DmaBounce,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SubmitState {
    Idle,
    Submitted,
    QueueFull,
}

impl VirtIOBlkDevice {
    pub fn new(transport: VirtIOTransport, dev_id: Arc<DeviceId>) -> Option<Arc<Self>> {
        // 设置中断
        if let Err(err) = transport.setup_irq(dev_id.clone()) {
            error!("VirtIOBlkDevice '{dev_id:?}' setup_irq failed: {:?}", err);
            return None;
        }

        let devname = virtioblk_manager().alloc_id()?;
        let irq = Some(transport.irq());
        let device_inner = VirtIOBlk::<HalImpl, VirtIOTransport>::new(transport);
        if let Err(e) = device_inner {
            error!("VirtIOBlkDevice '{dev_id:?}' create failed: {:?}", e);
            return None;
        }

        let mut blk: VirtIOBlk<HalImpl, VirtIOTransport> = device_inner.unwrap();
        blk.enable_interrupts();

        let dev = Arc::new_cyclic(|self_ref| Self {
            blkdev_meta: BlockDevMeta::new(devname, Major::VIRTIO_BLK_MAJOR),
            self_ref: self_ref.clone(),
            dev_id,
            locked_kobj_state: LockedKObjectState::default(),
            inner: Mutex::new(InnerVirtIOBlkDevice {
                name: None,
                virtio_index: None,
                device_common: DeviceCommonData::default(),
                kobject_common: KObjectCommonData::default(),
                irq,
            }),
            blk: Mutex::new(blk),
            submit_wq: WaitQueue::default(),
            work_seq: AtomicU64::new(0),
            pending: Mutex::new(VecDeque::new()),
            inflight: Mutex::new(HashMap::new()),
            parent: RwSem::new(Weak::default()),
            fs: RwSem::new(Weak::default()),
            metadata: Metadata::new(
                crate::filesystem::vfs::FileType::BlockDevice,
                InodeMode::from_bits_truncate(0o755),
            ),
        });

        // 启动 per-device worker（负责 submit + completion）
        let arg = Arc::into_raw(dev.clone()) as usize;
        let closure = KernelThreadClosure::UsizeClosure((
            Box::new(|arg| -> i32 {
                let dev = unsafe { Arc::from_raw(arg as *const VirtIOBlkDevice) };
                dev.worker_loop()
            }),
            arg,
        ));

        log::debug!(
            "create virtio_blk_{}_io thread: preempt_count: {}",
            dev.blkdev_meta.devname.to_string(),
            crate::process::ProcessManager::current_pcb().preempt_count()
        );
        let worker_pcb = KernelThreadMechanism::create_and_run(
            closure,
            format!("virtio_blk_{}_io", dev.blkdev_meta.devname.to_string()),
        );

        // This worker is latency-sensitive for mmap/filemap/readahead IO completion.
        // Mark it so CFS can apply a wakeup-placement boost.
        if let Some(pcb) = worker_pcb.as_ref() {
            pcb.flags().insert(ProcessFlags::IO_WORKER);
        }

        Some(dev)
    }

    pub fn submit_bio(&self, bio: Arc<crate::driver::block::bio::Bio>) {
        self.pending.lock().push_back(bio);
        self.submit_wq.wakeup(None);
    }

    fn worker_loop(&self) -> ! {
        let mut observed = self.work_seq.load(Ordering::Acquire);
        loop {
            self.process_completions();
            let submit_state = self.process_submissions();

            let has_used = {
                let mut blk = self.blk.lock();
                blk.peek_used().is_some()
            };
            if has_used {
                observed = self.work_seq.load(Ordering::Acquire);
                continue;
            }

            let pending_empty = self.pending.lock().is_empty();
            let inflight_empty = self.inflight.lock().is_empty();

            if submit_state == SubmitState::QueueFull {
                let _ = self.submit_wq.wait_event_interruptible_timeout(
                    || {
                        if self.work_seq.load(Ordering::Acquire) != observed {
                            return true;
                        }
                        let mut blk = self.blk.lock();
                        blk.peek_used().is_some()
                    },
                    Some(Duration::from_millis(100)),
                );
            } else if pending_empty && !has_used {
                let timeout = if inflight_empty {
                    None
                } else {
                    Some(Duration::from_millis(100))
                };

                let _ = self.submit_wq.wait_event_interruptible_timeout(
                    || {
                        if self.work_seq.load(Ordering::Acquire) != observed {
                            return true;
                        }
                        if !self.pending.lock().is_empty() {
                            return true;
                        }
                        let mut blk = self.blk.lock();
                        blk.peek_used().is_some()
                    },
                    timeout,
                );
            }

            observed = self.work_seq.load(Ordering::Acquire);
        }
    }

    fn process_completions(&self) {
        loop {
            let token = {
                let mut blk = self.blk.lock();
                // 必须确认（ack）设备中断，否则部分 transport 下中断状态不会清除，
                // 也可能导致后续 used ring 的完成无法被及时观察到，从而让 bio.wait() 卡死。
                // 注意：ack_interrupt 需要在进程上下文执行，避免在硬中断里做重锁/耗时操作。
                let _ = blk.ack_interrupt();
                blk.peek_used()
            };
            let Some(token) = token else { break };

            let Some(mut inflight) = self.inflight.lock().remove(&token) else {
                // 未知 token：尝试 pop 掉以避免 used ring 堵塞
                let mut blk = self.blk.lock();
                let dummy_req = BlkReq::default();
                let mut dummy_resp = BlkResp::default();
                let mut dummy_buf = [0u8; 512];
                unsafe {
                    let _ = blk.complete_read_blocks(
                        token,
                        &dummy_req,
                        &mut dummy_buf,
                        &mut dummy_resp,
                    );
                }
                continue;
            };

            // pop_used + unshare（由 virtio-drivers 完成）
            let r = {
                let mut blk = self.blk.lock();
                match inflight.op {
                    crate::driver::block::bio::BioOp::Read => unsafe {
                        blk.complete_read_blocks(
                            token,
                            &inflight.req,
                            inflight.dma.as_mut_slice(),
                            &mut *inflight.resp,
                        )
                    },
                    crate::driver::block::bio::BioOp::Write => unsafe {
                        blk.complete_write_blocks(
                            token,
                            &inflight.req,
                            inflight.dma.as_slice(),
                            &mut *inflight.resp,
                        )
                    },
                    _ => Err(virtio_drivers::Error::Unsupported),
                }
            };

            match r {
                Ok(_) => {
                    let ok = inflight.resp.status() == RespStatus::OK;
                    if ok && inflight.op == crate::driver::block::bio::BioOp::Read {
                        let _ = inflight.bio.scatter_from(inflight.dma.as_slice());
                    }
                    inflight.bio.complete(if ok {
                        Ok(inflight.bytes)
                    } else {
                        Err(SystemError::EIO)
                    });
                }
                Err(_) => inflight.bio.complete(Err(SystemError::EIO)),
            }
        }
    }

    fn process_submissions(&self) -> SubmitState {
        let mut submitted_any = false;
        loop {
            let bio = match self.pending.lock().pop_front() {
                Some(b) => b,
                None => break,
            };

            let bytes = match bio.count.checked_mul(LBA_SIZE) {
                Some(b) => b,
                None => {
                    bio.complete(Err(SystemError::EOVERFLOW));
                    continue;
                }
            };

            let dma = match DmaBounce::new(bytes) {
                Ok(d) => d,
                Err(e) => {
                    bio.complete(Err(e));
                    continue;
                }
            };

            let op = bio.op;
            if op == crate::driver::block::bio::BioOp::Write {
                if let Err(e) = bio.gather_to(dma.as_mut_slice()) {
                    bio.complete(Err(e));
                    continue;
                }
            }

            let mut req = Box::new(BlkReq::default());
            let mut resp = Box::new(BlkResp::default());

            let token_res = {
                let mut blk = self.blk.lock();
                match bio.op {
                    crate::driver::block::bio::BioOp::Read => unsafe {
                        blk.read_blocks_nb(
                            bio.lba_id_start,
                            &mut *req,
                            dma.as_mut_slice(),
                            &mut *resp,
                        )
                    },
                    crate::driver::block::bio::BioOp::Write => unsafe {
                        blk.write_blocks_nb(bio.lba_id_start, &mut *req, dma.as_slice(), &mut *resp)
                    },
                    _ => Err(virtio_drivers::Error::Unsupported),
                }
            };

            match token_res {
                Ok(token) => {
                    self.inflight.lock().insert(
                        token,
                        Inflight {
                            bio,
                            op,
                            bytes,
                            req,
                            resp,
                            dma,
                        },
                    );
                    submitted_any = true;
                }
                Err(virtio_drivers::Error::QueueFull) => {
                    // 队列满：放回 pending，等待 IRQ/完成后重试
                    self.pending.lock().push_front(bio);
                    return SubmitState::QueueFull;
                }
                Err(_) => bio.complete(Err(SystemError::EIO)),
            }
        }
        if submitted_any {
            SubmitState::Submitted
        } else {
            SubmitState::Idle
        }
    }

    fn inner(&self) -> MutexGuard<'_, InnerVirtIOBlkDevice> {
        self.inner.lock()
    }
}

impl IndexNode for VirtIOBlkDevice {
    fn fs(&self) -> Arc<dyn crate::filesystem::vfs::FileSystem> {
        self.fs
            .read()
            .upgrade()
            .expect("VirtIOBlkDevice fs is not set")
    }
    fn as_any_ref(&self) -> &dyn core::any::Any {
        self
    }
    fn read_at(
        &self,
        _offset: usize,
        _len: usize,
        _buf: &mut [u8],
        _data: MutexGuard<crate::filesystem::vfs::FilePrivateData>,
    ) -> Result<usize, SystemError> {
        Err(SystemError::ENOSYS)
    }
    fn write_at(
        &self,
        _offset: usize,
        _len: usize,
        _buf: &[u8],
        _data: MutexGuard<crate::filesystem::vfs::FilePrivateData>,
    ) -> Result<usize, SystemError> {
        Err(SystemError::ENOSYS)
    }
    fn list(&self) -> Result<alloc::vec::Vec<alloc::string::String>, system_error::SystemError> {
        Err(SystemError::ENOSYS)
    }
    fn metadata(&self) -> Result<crate::filesystem::vfs::Metadata, SystemError> {
        Ok(self.metadata.clone())
    }

    fn parent(&self) -> Result<Arc<dyn IndexNode>, SystemError> {
        let parent = self.parent.read();
        if let Some(parent) = parent.upgrade() {
            return Ok(parent as Arc<dyn IndexNode>);
        }
        Err(SystemError::ENOENT)
    }

    fn close(
        &self,
        _data: MutexGuard<crate::filesystem::vfs::FilePrivateData>,
    ) -> Result<(), SystemError> {
        Ok(())
    }

    fn dname(&self) -> Result<DName, SystemError> {
        let dname = DName::from(self.blkdev_meta.devname.clone().as_ref());
        Ok(dname)
    }

    fn open(
        &self,
        _data: MutexGuard<crate::filesystem::vfs::FilePrivateData>,
        _mode: &crate::filesystem::vfs::file::FileFlags,
    ) -> Result<(), SystemError> {
        Ok(())
    }
}

impl DeviceINode for VirtIOBlkDevice {
    fn set_fs(&self, fs: alloc::sync::Weak<crate::filesystem::devfs::DevFS>) {
        *self.fs.write() = fs;
    }

    fn set_parent(&self, parent: Weak<crate::filesystem::devfs::LockedDevFSInode>) {
        *self.parent.write() = parent;
    }
}

impl BlockDevice for VirtIOBlkDevice {
    fn dev_name(&self) -> &DevName {
        &self.blkdev_meta.devname
    }

    fn blkdev_meta(&self) -> &BlockDevMeta {
        &self.blkdev_meta
    }

    fn disk_range(&self) -> GeneralBlockRange {
        let blocks = self.blk.lock().capacity() as usize * SECTOR_SIZE / LBA_SIZE;
        log::debug!(
            "VirtIOBlkDevice '{:?}' disk_range: 0..{}",
            self.dev_name(),
            blocks
        );
        GeneralBlockRange::new(0, blocks).unwrap()
    }

    fn read_at_sync(
        &self,
        lba_id_start: BlockId,
        count: usize,
        buf: &mut [u8],
    ) -> Result<usize, SystemError> {
        if count == 0 {
            return Ok(0);
        }
        let bytes = count.checked_mul(LBA_SIZE).ok_or(SystemError::EOVERFLOW)?;
        if bytes > buf.len() {
            return Err(SystemError::EINVAL);
        }

        // 用 Bio + Completion 走统一 Block IO 层（virtio-blk: async submit + IRQ/worker completion）
        let bio = unsafe {
            crate::driver::block::bio::Bio::new_read_borrowed(
                lba_id_start,
                count,
                &mut buf[..bytes],
            )?
        };
        self.submit_bio(bio.clone());
        bio.wait()
    }

    fn write_at_sync(
        &self,
        lba_id_start: BlockId,
        count: usize,
        buf: &[u8],
    ) -> Result<usize, SystemError> {
        if count == 0 {
            return Ok(0);
        }
        let bytes = count.checked_mul(LBA_SIZE).ok_or(SystemError::EOVERFLOW)?;
        if bytes > buf.len() {
            return Err(SystemError::EINVAL);
        }
        let bio = unsafe {
            crate::driver::block::bio::Bio::new_write_borrowed(lba_id_start, count, &buf[..bytes])?
        };
        self.submit_bio(bio.clone());
        bio.wait()
    }

    fn sync(&self) -> Result<(), SystemError> {
        Ok(())
    }

    fn submit_bio(&self, bio: alloc::sync::Arc<crate::driver::block::bio::Bio>) -> Result<(), SystemError> {
        self.submit_bio(bio);
        Ok(())
    }

    fn blk_size_log2(&self) -> u8 {
        9
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn device(&self) -> Arc<dyn Device> {
        self.self_ref.upgrade().unwrap()
    }

    fn block_size(&self) -> usize {
        todo!()
    }

    fn partitions(&self) -> Vec<Arc<Partition>> {
        let device = self.self_ref.upgrade().unwrap() as Arc<dyn BlockDevice>;
        let mbr_table = MbrDiskPartionTable::from_disk(device.clone())
            .expect("Failed to get MBR partition table");
        mbr_table.partitions(Arc::downgrade(&device))
    }
}

struct InnerVirtIOBlkDevice {
    name: Option<String>,
    virtio_index: Option<VirtIODeviceIndex>,
    device_common: DeviceCommonData,
    kobject_common: KObjectCommonData,
    irq: Option<IrqNumber>,
}

impl Debug for InnerVirtIOBlkDevice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InnerVirtIOBlkDevice").finish()
    }
}

impl VirtIODevice for VirtIOBlkDevice {
    fn irq(&self) -> Option<IrqNumber> {
        self.inner().irq
    }

    fn handle_irq(
        &self,
        _irq: crate::exception::IrqNumber,
    ) -> Result<IrqReturn, system_error::SystemError> {
        // 中断处理：唤醒 worker 线程处理完成的 I/O
        // worker 会在进程上下文中处理 ack_interrupt 和 completion
        self.work_seq.fetch_add(1, Ordering::Release);
        self.submit_wq.wakeup(None);
        Ok(crate::exception::irqdesc::IrqReturn::Handled)
    }

    fn dev_id(&self) -> &Arc<DeviceId> {
        &self.dev_id
    }

    fn set_device_name(&self, name: String) {
        self.inner().name = Some(name);
    }

    fn device_name(&self) -> String {
        self.inner()
            .name
            .clone()
            .unwrap_or_else(|| VIRTIO_BLK_BASENAME.to_string())
    }

    fn set_virtio_device_index(&self, index: VirtIODeviceIndex) {
        self.inner().virtio_index = Some(index);
        self.blkdev_meta.inner().dev_idx = index.into();
    }

    fn virtio_device_index(&self) -> Option<VirtIODeviceIndex> {
        self.inner().virtio_index
    }

    fn device_type_id(&self) -> u32 {
        virtio_drivers::transport::DeviceType::Block as u32
    }

    fn vendor(&self) -> u32 {
        VIRTIO_VENDOR_ID.into()
    }
}

impl Device for VirtIOBlkDevice {
    fn dev_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn id_table(&self) -> IdTable {
        IdTable::new(VIRTIO_BLK_BASENAME.to_string(), None)
    }

    fn bus(&self) -> Option<Weak<dyn Bus>> {
        self.inner().device_common.bus.clone()
    }

    fn set_bus(&self, bus: Option<Weak<dyn Bus>>) {
        self.inner().device_common.bus = bus;
    }

    fn class(&self) -> Option<Arc<dyn Class>> {
        let mut guard = self.inner();
        let r = guard.device_common.class.clone()?.upgrade();
        if r.is_none() {
            guard.device_common.class = None;
        }

        return r;
    }

    fn set_class(&self, class: Option<Weak<dyn Class>>) {
        self.inner().device_common.class = class;
    }

    fn driver(&self) -> Option<Arc<dyn Driver>> {
        let r = self.inner().device_common.driver.clone()?.upgrade();
        if r.is_none() {
            self.inner().device_common.driver = None;
        }

        return r;
    }

    fn set_driver(&self, driver: Option<Weak<dyn Driver>>) {
        self.inner().device_common.driver = driver;
    }

    fn is_dead(&self) -> bool {
        false
    }

    fn can_match(&self) -> bool {
        self.inner().device_common.can_match
    }

    fn set_can_match(&self, can_match: bool) {
        self.inner().device_common.can_match = can_match;
    }

    fn state_synced(&self) -> bool {
        true
    }

    fn dev_parent(&self) -> Option<Weak<dyn Device>> {
        self.inner().device_common.get_parent_weak_or_clear()
    }

    fn set_dev_parent(&self, parent: Option<Weak<dyn Device>>) {
        self.inner().device_common.parent = parent;
    }
}

impl KObject for VirtIOBlkDevice {
    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn set_inode(&self, inode: Option<Arc<KernFSInode>>) {
        self.inner().kobject_common.kern_inode = inode;
    }

    fn inode(&self) -> Option<Arc<KernFSInode>> {
        self.inner().kobject_common.kern_inode.clone()
    }

    fn parent(&self) -> Option<Weak<dyn KObject>> {
        self.inner().kobject_common.parent.clone()
    }

    fn set_parent(&self, parent: Option<Weak<dyn KObject>>) {
        self.inner().kobject_common.parent = parent;
    }

    fn kset(&self) -> Option<Arc<KSet>> {
        self.inner().kobject_common.kset.clone()
    }

    fn set_kset(&self, kset: Option<Arc<KSet>>) {
        self.inner().kobject_common.kset = kset;
    }

    fn kobj_type(&self) -> Option<&'static dyn KObjType> {
        self.inner().kobject_common.kobj_type
    }

    fn name(&self) -> String {
        self.device_name()
    }

    fn set_name(&self, _name: String) {
        // do nothing
    }

    fn kobj_state(&self) -> RwSemReadGuard<'_, KObjectState> {
        self.locked_kobj_state.read()
    }

    fn kobj_state_mut(&self) -> RwSemWriteGuard<'_, KObjectState> {
        self.locked_kobj_state.write()
    }

    fn set_kobj_state(&self, state: KObjectState) {
        *self.locked_kobj_state.write() = state;
    }

    fn set_kobj_type(&self, ktype: Option<&'static dyn KObjType>) {
        self.inner().kobject_common.kobj_type = ktype;
    }
}

#[unified_init(INITCALL_POSTCORE)]
fn virtio_blk_driver_init() -> Result<(), SystemError> {
    let driver = VirtIOBlkDriver::new();
    virtio_driver_manager()
        .register(driver.clone() as Arc<dyn VirtIODriver>)
        .expect("Add virtio blk driver failed");
    unsafe {
        VIRTIO_BLK_DRIVER = Some(driver);
    }

    return Ok(());
}

#[derive(Debug)]
#[cast_to([sync] VirtIODriver)]
#[cast_to([sync] Driver)]
struct VirtIOBlkDriver {
    inner: Mutex<InnerVirtIOBlkDriver>,
    kobj_state: LockedKObjectState,
}

impl VirtIOBlkDriver {
    pub fn new() -> Arc<Self> {
        let inner = InnerVirtIOBlkDriver {
            virtio_driver_common: VirtIODriverCommonData::default(),
            driver_common: DriverCommonData::default(),
            kobj_common: KObjectCommonData::default(),
        };

        let id_table = VirtioDeviceId::new(
            virtio_drivers::transport::DeviceType::Block as u32,
            VIRTIO_VENDOR_ID.into(),
        );
        let result = VirtIOBlkDriver {
            inner: Mutex::new(inner),
            kobj_state: LockedKObjectState::default(),
        };
        result.add_virtio_id(id_table);

        return Arc::new(result);
    }

    fn inner(&self) -> MutexGuard<'_, InnerVirtIOBlkDriver> {
        return self.inner.lock();
    }
}

#[derive(Debug)]
struct InnerVirtIOBlkDriver {
    virtio_driver_common: VirtIODriverCommonData,
    driver_common: DriverCommonData,
    kobj_common: KObjectCommonData,
}

impl VirtIODriver for VirtIOBlkDriver {
    fn probe(&self, device: &Arc<dyn VirtIODevice>) -> Result<(), SystemError> {
        let dev = device
            .clone()
            .arc_any()
            .downcast::<VirtIOBlkDevice>()
            .map_err(|_| {
                error!(
                "VirtIOBlkDriver::probe() failed: device is not a VirtIO block device. Device: '{:?}'",
                device.name()
            );
                SystemError::EINVAL
            })?;

        block_dev_manager().register(dev as Arc<dyn BlockDevice>)?;
        return Ok(());
    }

    fn virtio_id_table(&self) -> Vec<crate::driver::virtio::VirtioDeviceId> {
        self.inner().virtio_driver_common.id_table.clone()
    }

    fn add_virtio_id(&self, id: VirtioDeviceId) {
        self.inner().virtio_driver_common.id_table.push(id);
    }
}

impl Driver for VirtIOBlkDriver {
    fn id_table(&self) -> Option<IdTable> {
        Some(IdTable::new(VIRTIO_BLK_BASENAME.to_string(), None))
    }

    fn add_device(&self, device: Arc<dyn Device>) {
        let iface = device
            .arc_any()
            .downcast::<VirtIOBlkDevice>()
            .expect("VirtIOBlkDriver::add_device() failed: device is not a VirtIOBlkDevice");

        self.inner()
            .driver_common
            .devices
            .push(iface as Arc<dyn Device>);
    }

    fn delete_device(&self, device: &Arc<dyn Device>) {
        let _iface = device
            .clone()
            .arc_any()
            .downcast::<VirtIOBlkDevice>()
            .expect("VirtIOBlkDriver::delete_device() failed: device is not a VirtIOBlkDevice");

        let mut guard = self.inner();
        let index = guard
            .driver_common
            .devices
            .iter()
            .position(|dev| Arc::ptr_eq(device, dev))
            .expect("VirtIOBlkDriver::delete_device() failed: device not found");

        guard.driver_common.devices.remove(index);
    }

    fn devices(&self) -> Vec<Arc<dyn Device>> {
        self.inner().driver_common.devices.clone()
    }

    fn bus(&self) -> Option<Weak<dyn Bus>> {
        Some(Arc::downgrade(&virtio_bus()) as Weak<dyn Bus>)
    }

    fn set_bus(&self, _bus: Option<Weak<dyn Bus>>) {
        // do nothing
    }
}

impl KObject for VirtIOBlkDriver {
    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn set_inode(&self, inode: Option<Arc<KernFSInode>>) {
        self.inner().kobj_common.kern_inode = inode;
    }

    fn inode(&self) -> Option<Arc<KernFSInode>> {
        self.inner().kobj_common.kern_inode.clone()
    }

    fn parent(&self) -> Option<Weak<dyn KObject>> {
        self.inner().kobj_common.parent.clone()
    }

    fn set_parent(&self, parent: Option<Weak<dyn KObject>>) {
        self.inner().kobj_common.parent = parent;
    }

    fn kset(&self) -> Option<Arc<KSet>> {
        self.inner().kobj_common.kset.clone()
    }

    fn set_kset(&self, kset: Option<Arc<KSet>>) {
        self.inner().kobj_common.kset = kset;
    }

    fn kobj_type(&self) -> Option<&'static dyn KObjType> {
        self.inner().kobj_common.kobj_type
    }

    fn set_kobj_type(&self, ktype: Option<&'static dyn KObjType>) {
        self.inner().kobj_common.kobj_type = ktype;
    }

    fn name(&self) -> String {
        VIRTIO_BLK_BASENAME.to_string()
    }

    fn set_name(&self, _name: String) {
        // do nothing
    }

    fn kobj_state(&self) -> RwSemReadGuard<'_, KObjectState> {
        self.kobj_state.read()
    }

    fn kobj_state_mut(&self) -> RwSemWriteGuard<'_, KObjectState> {
        self.kobj_state.write()
    }

    fn set_kobj_state(&self, state: KObjectState) {
        *self.kobj_state.write() = state;
    }
}
