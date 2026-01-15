use alloc::boxed::Box;
use alloc::vec::Vec;

use system_error::SystemError;

use crate::arch::MMArch;
use crate::libs::lazy_init::Lazy;
use crate::libs::wait_queue::WaitQueue;
use crate::mm::page::{Page, PageFlags};
use crate::mm::MemoryManagementArch;

const PAGE_WAIT_BUCKETS: usize = 256;

struct PageWaitTable {
    buckets: Box<[WaitQueue]>,
}

impl PageWaitTable {
    fn new() -> Self {
        let mut buckets = Vec::with_capacity(PAGE_WAIT_BUCKETS);
        for _ in 0..PAGE_WAIT_BUCKETS {
            buckets.push(WaitQueue::default());
        }
        Self {
            buckets: buckets.into_boxed_slice(),
        }
    }

    fn bucket(&self, idx: usize) -> &WaitQueue {
        &self.buckets[idx & (PAGE_WAIT_BUCKETS - 1)]
    }
}

static PAGE_WAIT_TABLE: Lazy<PageWaitTable> = Lazy::new();

fn page_wait_table() -> &'static PageWaitTable {
    if !PAGE_WAIT_TABLE.initialized() {
        PAGE_WAIT_TABLE.init(PageWaitTable::new());
    }
    PAGE_WAIT_TABLE.get()
}

fn page_wait_queue(page: &Page) -> &WaitQueue {
    let paddr = page.phys_address().data() as usize;
    let hash = paddr >> MMArch::PAGE_SHIFT;
    page_wait_table().bucket(hash)
}

pub fn wait_on_page_locked(page: &Page, interruptible: bool) -> Result<(), SystemError> {
    {
        let mut guard = page.write();
        if !guard.flags().contains(PageFlags::PG_LOCKED) {
            return Ok(());
        }
        guard.add_flags(PageFlags::PG_WAITERS);
    }

    let wq = page_wait_queue(page);
    if interruptible {
        wq.wait_until_interruptible(|| {
            if !page.read().flags().contains(PageFlags::PG_LOCKED) {
                Some(())
            } else {
                None
            }
        })?;
    } else {
        wq.wait_until(|| {
            if !page.read().flags().contains(PageFlags::PG_LOCKED) {
                Some(())
            } else {
                None
            }
        });
    }

    Ok(())
}

pub fn lock_page(page: &Page, interruptible: bool) -> Result<(), SystemError> {
    loop {
        {
            let mut guard = page.write();
            if !guard.flags().contains(PageFlags::PG_LOCKED) {
                guard.add_flags(PageFlags::PG_LOCKED);
                return Ok(());
            }
            guard.add_flags(PageFlags::PG_WAITERS);
        }

        wait_on_page_locked(page, interruptible)?;
    }
}

pub fn unlock_page(page: &Page) {
    let should_wake = {
        let mut guard = page.write();
        if !guard.flags().contains(PageFlags::PG_LOCKED) {
            return;
        }
        guard.remove_flags(PageFlags::PG_LOCKED);
        let waiters = guard.flags().contains(PageFlags::PG_WAITERS);
        if waiters {
            guard.remove_flags(PageFlags::PG_WAITERS);
        }
        waiters
    };

    if should_wake {
        page_wait_queue(page).wakeup_all(None);
    }
}
