//! 地址段定义

#![deny(missing_docs)]

mod fixed;
mod lazy;

use alloc::sync::Arc;
use lock::Mutex;
use core::slice;

use crate::error::{OSError, OSResult};
use crate::memory::phys_to_virt;

use super::addr::{align_down, align_up, PhysAddr, VirtAddr};
use super::{PTEFlags, PageTable, PageTableEntry};
use super::PAGE_SIZE;

pub use fixed::PmAreaFixed;
pub use lazy::PmAreaLazy;

/// 一段访问权限相同的物理地址。注意物理地址本身不一定连续，只是拥有对应长度的空间
/// 
/// 可实现为 lazy 分配
pub trait PmArea: core::fmt::Debug + Send + Sync {
    /// 地址段总长度
    fn size(&self) -> usize;
    /// 获取 idx 所在页的页帧。
    /// 
    /// 如果有 need_alloc，则会在 idx 所在页未分配时尝试分配
    fn get_frame(&mut self, idx: usize, need_alloc: bool) -> OSResult<Option<PhysAddr>>;
    /// 释放 idx 地址对应的物理页
    fn release_frame(&mut self, idx: usize) -> OSResult;
    /// 读从 offset 开头的一段数据，成功时返回读取长度
    fn read(&mut self, offset: usize, dst: &mut [u8]) -> OSResult<usize>;
    /// 把数据写到从 offset 开头的地址，成功时返回写入长度
    fn write(&mut self, offset: usize, src: &[u8]) -> OSResult<usize>;
}

/// 一段访问权限相同的虚拟地址
#[derive(Debug)]
pub struct VmArea {
    /// 地址段开头，需要对其页
    pub(super) start: VirtAddr,
    /// 地址段结尾，需要对其页
    pub(super) end: VirtAddr,
    /// 访问权限
    pub(super) flags: PTEFlags,
    /// 对应的物理地址段
    pub(super) pma: Arc<Mutex<dyn PmArea>>,
    name: &'static str,
}

impl VmArea {
    /// 新建地址段，成功时返回 VmArea 结构
    pub fn new(
        start: VirtAddr,
        end: VirtAddr,
        flags: PTEFlags,
        pma: Arc<Mutex<dyn PmArea>>,
        name: &'static str,
    ) -> OSResult<Self> {
        if start >= end {
            //println!("invalid memory region: [{:#x?}, {:#x?})", start, end);
            return Err(OSError::VmArea_InvalidRange);
        }
        let start = align_down(start);
        let end = align_up(end);
        if end - start != pma.lock().size() {
            /*
            println!(
                "VmArea size != PmArea size: [{:#x?}, {:#x?}), {:x?}",
                start,
                end,
                pma.lock()
            );
            */
            return Err(OSError::VmArea_VmSizeNotEqualToPmSize);
        }
        Ok(Self {
            start,
            end,
            flags,
            pma,
            name,
        })
    }

    /// 当前地址段是否包含这个地址
    pub fn contains(&self, vaddr: VirtAddr) -> bool {
        self.start <= vaddr && vaddr < self.end
    }

    /// 当前地址段是否包含这一段地址
    pub fn is_overlap_with(&self, start: VirtAddr, end: VirtAddr) -> bool {
        let p0 = self.start;
        let p1 = self.end;
        let p2 = align_down(start);
        let p3 = align_up(end);
        !(p1 <= p2 || p0 >= p3)
    }

    /// 把虚拟地址段和对应的物理地址段的映射写入页表。
    /// 
    /// 如果是 lazy 分配的，或者说还没有对应页帧时，则不分配，等到 page fault 时再分配
    pub fn map_area(&self, pt: &mut PageTable) -> OSResult {
        //println!("create mapping: {:#x?}", self);
        let mut pma = self.pma.lock();
        for vaddr in (self.start..self.end).step_by(PAGE_SIZE) {
            let page = pma.get_frame((vaddr - self.start) / PAGE_SIZE, false)?;
            let res = if let Some(paddr) = page {
                // if vaddr < 0x9000_0000 { println!("create mapping {:x}->{:x} at {:x}", vaddr, paddr, pt.get_root_paddr()); }
                pt.map(vaddr, paddr, self.flags)
            } else {
                pt.map(vaddr, 0, PTEFlags::empty())
            };
            res.map_err(|e| {
                println!(
                    "failed to create mapping: {:#x?} -> {:#x?}, {:?}",
                    vaddr, page, e
                );
                e
            })?;
        }
        Ok(())
    }

    /// 把虚拟地址段和对应的物理地址段的映射从页表中删除。
    /// 
    /// 如果页表中的描述和 VmArea 的描述不符，则返回 error
    pub fn unmap_area(&self, pt: &mut PageTable) -> OSResult {
        //println!("destory mapping: {:#x?}", self);
        let mut pma = self.pma.lock();
        for vaddr in (self.start..self.end).step_by(PAGE_SIZE) {
            let res = pma.release_frame((vaddr - self.start) / PAGE_SIZE);
            //if vaddr == 0x3fff_f000 { println!("page {:#x?} at {:x}", res, pt.get_root_paddr()); }
            // 如果触发 OSError::PmAreaLazy_ReleaseNotAllocatedPage，
            // 说明这段 area 是 Lazy 分配的，且这一页还没被用到
            // 这种情况下不需要报错，也不需要修改页表
            if res != Err(OSError::PmAreaLazy_ReleaseNotAllocatedPage) {
                if res.is_err() {
                    return res;
                }
                pt.unmap(vaddr).map_err(|e| {
                    println!("failed to unmap VA: {:#x?}, {:?}", vaddr, e);
                    e
                })?;
            }
        }
        Ok(())
    }

    /// 这一段是否是用户态可见的
    pub fn is_user(&self) -> bool {
        self.flags.contains(PTEFlags::USER)
    }

    /// 从已有 VmArea 复制一个新的 VmArea ，其中虚拟地址段和权限相同，但没有实际分配物理页
    pub fn copy_to_new_area_empty(&self) -> OSResult<VmArea> {
        let page_count = (self.end - self.start) / PAGE_SIZE;
        Ok(VmArea {
            start: self.start,
            end: self.end,
            flags: self.flags,
            pma: Arc::new(Mutex::new(PmAreaLazy::new(page_count)?)),
            name: self.name,
        })
    }

    /// 从已有 VmArea 复制一个新的 VmArea ，复制所有的数据，但是用不同的物理地址
    /// 
    /// Todo: 可以改成 Copy on write 的方式
    /// 需要把 WRITE 权限关掉，然后等到写这段内存发生 Page Fault 再实际写入数据。
    /// 但是这需要建立一种映射关系，帮助在之后找到应该映射到同一块数据的所有 VmArea。
    /// 
    /// 而且不同进程中进行 mmap / munmap 等操作时也可能会修改这样的对应关系，
    /// 不是只有写这段内存才需要考虑 Copy on write，所以真正实现可能比想象的要复杂。
    pub fn copy_to_new_area_with_data(&self) -> OSResult<VmArea> {
        let new_area = self.copy_to_new_area_empty()?;
        let mut new_pma = new_area.pma.lock();
        let mut old_pma = self.pma.lock();
        for vaddr in (self.start..self.end).step_by(PAGE_SIZE) {
            // 获取当前 VmArea 的所有页
            let old_page = old_pma.get_frame((vaddr - self.start) / PAGE_SIZE, false)?;
            if let Some(old_paddr) = old_page { // 如果这个页已被分配
                // 在新 VmArea 中分配一个新页
                // 这里不会出现 Ok(None) 的情况，因为 new_area 是刚生成的，所以 new_pma 里面为空。
                // PmAreaLazy::get_frame 里的实现在这种情况下要么返回内存溢出错误，要么返回新获取的帧的物理地址
                let new_paddr = new_pma.get_frame((vaddr - self.start) / PAGE_SIZE, true)?.unwrap();
                // 手动复制这个页的内存。
                // 其实可以利用 trait 的 write/read 接口，但是那样会需要两次内存复制操作
                let src = unsafe { slice::from_raw_parts(phys_to_virt(old_paddr) as *const u8, PAGE_SIZE) };
                let dst = unsafe { slice::from_raw_parts_mut(phys_to_virt(new_paddr) as *mut u8, PAGE_SIZE) };
                dst.copy_from_slice(src);
            }
        }
        drop(new_pma);
        Ok(new_area)
    }

    /// 处理 page fault
    pub fn handle_page_fault(
        &self,
        offset: usize,
        access_flags: PTEFlags,
        pt: &mut PageTable,
    ) -> OSResult {
        debug_assert!(offset < self.end - self.start);
        info!(
            "handle page fault @ offset {:#x?} with access {:?}: {:#x?}",
            offset,
            access_flags,
            self
        );
        let mut pma = self.pma.lock();
        if !self.flags.contains(access_flags) {
            return Err(OSError::PageFaultHandler_AccessDenied);
        }
        let offset = align_down(offset);
        let vaddr = self.start + offset;
        let paddr = pma
            .get_frame(offset / PAGE_SIZE, true)?
            .ok_or(OSError::Memory_RunOutOfMemory)?;
        // println!("paddr {:x}", paddr);
        if let Some(entry) = pt.get_entry(vaddr) {
            unsafe {
                if (*entry).is_valid() {
                    // println!("entry flags {:x}", entry.bits);
                    Err(OSError::PageFaultHandler_TrapAtValidPage)
                } else {
                    (*entry).set_all(paddr, self.flags | PTEFlags::VALID | PTEFlags::ACCESS | PTEFlags::DIRTY);
                    pt.flush_tlb(Some(vaddr));
                    info!("[Handler] Lazy alloc a page for user.");
                    Ok(())
                }
            }
        } else {
            Err(OSError::PageTable_PageNotMapped)
        }
    }
}
