use core::arch::x86_64::_rdtsc;

use crate::{
    arch::mm::LockedFrameAllocator,
    kinfo,
    mm::allocator::page_frame::{
        allocate_page_frames, deallocate_page_frames, PageFrameCount, PhysPageFrame,
    },
};

extern "C" {
    static mut Cpu_tsc_freq: u64;
}
#[no_mangle]
pub extern "C" fn rs_test_mm_1() {
    unsafe {
        kinfo!("Now in rs_test_mm_1(), tsc freq: {}", Cpu_tsc_freq);
    }

    MMTest::run();
}

struct MMTest;

impl MMTest {
    fn run() {
        kinfo!("Start to test MM performance");
        Self::test_alloc_4k_only(32);
        Self::test_alloc_4k_only(64);
        Self::test_alloc_4k_only(128);
        Self::test_alloc_4k_only(256);
        kinfo!("Test MM performance finished");
    }

    /// 测试分配4K内存的性能(分配16M内存)
    fn test_alloc_4k_only(megabytes: usize) {
        kinfo!("Test buddy: alloc {} MB", megabytes);
        let pages_to_alloc = megabytes * 1024 * 1024 / 4096;
        let mut data = vec![None; pages_to_alloc];
        let start_tsc = unsafe { _rdtsc() };
        for i in 0..pages_to_alloc {
            let x = unsafe { allocate_page_frames(PageFrameCount::new(1)) }.expect("alloc failed");
            data[i] = Some(x);
        }
        let end_tsc = unsafe { _rdtsc() };

        let total_tsc = end_tsc - start_tsc;
        let total_time = total_tsc as f64 / unsafe { Cpu_tsc_freq } as f64;
        kinfo!(
            "Test buddy: alloc 4K pages, page num: {}, total_tsc: {}, cpu freq: {}Hz, total time: {}s, average time: {}s, speed: {} frames/sec",
            pages_to_alloc,
            total_tsc,
            unsafe { Cpu_tsc_freq },
            total_time,
            total_time / pages_to_alloc as f64,
            pages_to_alloc as f64 / total_time
        );

        kinfo!("Test buddy: free {} MB", megabytes);
        let start_tsc = unsafe { _rdtsc() };
        for i in 0..pages_to_alloc {
            unsafe {
                deallocate_page_frames(
                    PhysPageFrame::new(data[i].unwrap().0),
                    PageFrameCount::new(1),
                );
            }
        }
        let end_tsc = unsafe { _rdtsc() };

        let total_tsc = end_tsc - start_tsc;
        let total_time = total_tsc as f64 / unsafe { Cpu_tsc_freq } as f64;

        kinfo!(
            "Test buddy: free 4K pages, page num: {}, total_tsc: {}, cpu freq: {}Hz, total time: {}s, average time: {}s, speed: {} frames/sec",
            pages_to_alloc,
            total_tsc,
            unsafe { Cpu_tsc_freq },
            total_time,
            total_time / pages_to_alloc as f64,
            pages_to_alloc as f64 / total_time
        );
    }
}
