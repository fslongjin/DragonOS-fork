pub mod blk_page_cache_ops;
pub mod block_dev_cache;
pub mod block_device;
pub mod disk_info;
pub mod gendisk;
pub mod manager;
#[derive(Debug)]
#[allow(dead_code)]
pub enum SeekFrom {
    SeekSet(i64),
    SeekCurrent(i64),
    SeekEnd(i64),
    Invalid,
}
