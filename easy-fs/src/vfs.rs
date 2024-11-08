use super::{
    block_cache_sync_all, get_block_cache, BlockDevice, DirEntry, DiskInode, DiskInodeType,
    EasyFileSystem, DIRENT_SZ,
};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, MutexGuard};

#[allow(empty)]
use core::mem::size_of;
#[allow(empty)]
use crate::BLOCK_SZ;

pub struct Inode {
    block_id: usize,
    block_offset: usize,
    fs: Arc<Mutex<EasyFileSystem>>,
    block_device: Arc<dyn BlockDevice>,
}

impl Inode {
    /// We should not acquire efs lock here.
    pub fn new(
        block_id: u32,
        block_offset: usize,
        fs: Arc<Mutex<EasyFileSystem>>,
        block_device: Arc<dyn BlockDevice>,
    ) -> Self {
        Self {
            block_id: block_id as usize,
            block_offset,
            fs,
            block_device,
        }
    }

    fn read_disk_inode<V>(&self, f: impl FnOnce(&DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .read(self.block_offset, f)
    }

    fn modify_disk_inode<V>(&self, f: impl FnOnce(&mut DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .modify(self.block_offset, f)
    }

    fn find_inode_id(&self, name: &str, disk_inode: &DiskInode) -> Option<u32> {
        // assert it is a directory
        assert!(disk_inode.is_dir());
        let file_count = (disk_inode.size as usize) / DIRENT_SZ;
        let mut dirent = DirEntry::empty();
        for i in 0..file_count {
            assert_eq!(
                disk_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device,),
                DIRENT_SZ,
            );
            if dirent.name() == name {
                return Some(dirent.inode_id() as u32);
            }
        }
        None
    }

    pub fn find(&self, name: &str) -> Option<Arc<Inode>> {
        let fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            self.find_inode_id(name, disk_inode).map(|inode_id| {
                let (block_id, block_offset) = fs.get_disk_inode_pos(inode_id);
                Arc::new(Self::new(
                    block_id,
                    block_offset,
                    self.fs.clone(),
                    self.block_device.clone(),
                ))
            })
        })
    }

    fn increase_size(
        &self,
        new_size: u32,
        disk_inode: &mut DiskInode,
        fs: &mut MutexGuard<EasyFileSystem>,
    ) {
        if new_size < disk_inode.size {
            return;
        }
        let blocks_needed = disk_inode.blocks_num_needed(new_size);
        let mut v: Vec<u32> = Vec::new();
        for _ in 0..blocks_needed {
            v.push(fs.alloc_data());
        }
        disk_inode.increase_size(new_size, v, &self.block_device);
    }

    pub fn create(&self, name: &str) -> Option<Arc<Inode>> {
        let mut fs = self.fs.lock();
        let op = |root_inode: &mut DiskInode| {
            // assert it is a directory
            assert!(root_inode.is_dir());
            // has the file been created?
            self.find_inode_id(name, root_inode)
        };
        if self.modify_disk_inode(op).is_some() {
            return None;
        }
        // create a new file
        // alloc a inode with an indirect block
        let new_inode_id = fs.alloc_inode();
        // initialize inode
        let (new_inode_block_id, new_inode_block_offset) = fs.get_disk_inode_pos(new_inode_id);
        get_block_cache(new_inode_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(new_inode_block_offset, |new_inode: &mut DiskInode| {
                new_inode.initialize(DiskInodeType::File);
            });
        self.modify_disk_inode(|root_inode| {
            // append file in the dirent
            let file_count = (root_inode.size as usize) / DIRENT_SZ;
            let new_size = (file_count + 1) * DIRENT_SZ;
            // increase size
            self.increase_size(new_size as u32, root_inode, &mut fs);
            // write dirent
            let dirent = DirEntry::new(name, new_inode_id);
            root_inode.write_at(
                file_count * DIRENT_SZ,
                dirent.as_bytes(),
                &self.block_device,
            );
        });

        let (block_id, block_offset) = fs.get_disk_inode_pos(new_inode_id);
        block_cache_sync_all();
        // return inode
        Some(Arc::new(Self::new(
            block_id,
            block_offset,
            self.fs.clone(),
            self.block_device.clone(),
        )))
        // release efs lock automatically by compiler
    }

    pub fn ls(&self) -> Vec<String> {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            let file_count = (disk_inode.size as usize) / DIRENT_SZ;
            let mut v: Vec<String> = Vec::new();
            for i in 0..file_count {
                let mut dirent = DirEntry::empty();
                assert_eq!(
                    disk_inode.read_at(i * DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                v.push(String::from(dirent.name()));
            }
            v
        })
    }

    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| disk_inode.read_at(offset, buf, &self.block_device))
    }

    pub fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        let mut fs = self.fs.lock();
        let size = self.modify_disk_inode(|disk_inode| {
            self.increase_size((offset + buf.len()) as u32, disk_inode, &mut fs);
            disk_inode.write_at(offset, buf, &self.block_device)
        });
        block_cache_sync_all();
        size
    }

    pub fn clear(&self) {
        let mut fs = self.fs.lock();
        self.modify_disk_inode(|disk_inode| {
            let size = disk_inode.size;
            let data_blocks_dealloc = disk_inode.clear_size(&self.block_device);
            assert!(data_blocks_dealloc.len() == DiskInode::total_blocks(size) as usize);
            for data_block in data_blocks_dealloc.into_iter() {
                fs.dealloc_data(data_block);
            }
        });
        block_cache_sync_all();
    }
}

impl Inode {
    /// heke - link inode_id to name 并插入目录项，如果存在同名的返回-1
    pub fn push_dirent(&self, name: &str, dsinode_number: u32) -> i32 {
        let mut curr_size = 0;
        self.read_disk_inode(|disk_inode| {
            assert!(disk_inode.is_dir());
            curr_size = disk_inode.size;
        });
        let dirent = DirEntry::new(name, dsinode_number);
        self.write_at(curr_size as usize, dirent.as_bytes());
        let finded = self.find(name).unwrap();
        let curr_link = finded.get_nlink();
        finded.set_nlink(curr_link+1);
        return 0;
    }

    /// 硬链接数
    pub fn get_nlink(&self) -> u32 {
        self.read_disk_inode(|disk_inode| {disk_inode.nlink})
    }

    /// 修改硬链接数
    pub fn set_nlink(&self, n: u32) -> u32 {
        self.modify_disk_inode(|disk_inode| {
            disk_inode.nlink = n;
            disk_inode.nlink
        })
    }

    /// 删除目录项，找不到返回-1
    pub fn remove_dirent(&self, name: &str) -> i32 {
        let mut curr_size = 0;
        self.read_disk_inode(|disk_inode| {
            assert!(disk_inode.is_dir());
            curr_size = disk_inode.size;
        });
        let finded = self.find(name);
        if finded.is_none() { return -1; }
        let finded = finded.unwrap();
        if finded.get_nlink() == 1 {  // 这个时候就该删除了
            /*let mut fs = self.fs.lock();
            let dnid = finded.get_inode_id();
            fs.dealloc_inode(dnid);  // 回收索引bitmap*/
            finded.clear();  // 清空数据块，回收数据bitmap
        } else {
            finded.set_nlink(finded.get_nlink()-1);
        } drop(finded);     // 回写块设备
        // 这里开始处理本目录所包含的目录项
        let mut tmp = Vec::new();
        let file_count = curr_size as usize / DIRENT_SZ;
        if file_count == 1 { self.clear(); return 0; }  // 只有一个目录项，所以清空就行
        self.read_disk_inode(|disk_inode| {
            for i in 0..file_count {
                let mut dirent = DirEntry::empty();
                assert_eq!(
                    disk_inode.read_at(
                        DIRENT_SZ * i as usize,
                        dirent.as_bytes_mut(),
                        &self.block_device,
                    ), DIRENT_SZ,
                );
                if dirent.name() == name {
                    continue;
                } else {
                    let buffer = dirent.as_bytes().to_vec();
                    tmp.push(buffer);
                }
            }
        });
        self.clear();
        for i in 0..tmp.len() {
            self.write_at((i as usize)*size_of::<DirEntry>(), &tmp[i]);
        }
        0
    }

    /// 233
    pub fn get_inode_id(&self) -> usize {
        self.block_id * (BLOCK_SZ / size_of::<DiskInode>()) + self.block_offset / size_of::<DiskInode>()
    }

    ///
    pub fn is_file(&self) -> bool {
        self.read_disk_inode(|disk_inode| disk_inode.is_file())
    }

    ///
    pub fn is_dir (&self) -> bool {
        self.read_disk_inode(|disk_inode| disk_inode.is_dir ())
    }
}
