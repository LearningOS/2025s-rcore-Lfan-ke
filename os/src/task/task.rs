//! Types related to task management & Functions for completely changing TCB
use super::TaskContext;
use super::{kstack_alloc, pid_alloc, KernelStack, PidHandle};
use crate::fs::{File, Stdin, Stdout};
use crate::config::{TRAP_CONTEXT_BASE, MAX_SYSCALL_NUM};
use crate::mm::{MemorySet, PhysPageNum, VirtAddr, KERNEL_SPACE};
use crate::sync::UPSafeCell;
use crate::trap::{trap_handler, TrapContext};
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefMut;

/// Task control block structure
///
/// Directly save the contents that will not change during running
pub struct TaskControlBlock {
    // Immutable
    /// Process identifier
    pub pid: PidHandle,

    /// Kernel stack corresponding to PID
    pub kernel_stack: KernelStack,

    /// Mutable
    inner: UPSafeCell<TaskControlBlockInner>,
}

impl TaskControlBlock {
    /// Get the mutable reference of the inner TCB
    pub fn inner_exclusive_access(&self) -> RefMut<'_, TaskControlBlockInner> {
        self.inner.exclusive_access()
    }
    /// Get the address of app's page table
    pub fn get_user_token(&self) -> usize {
        let inner = self.inner_exclusive_access();
        inner.memory_set.token()
    }
}

/// heke函数
impl TaskControlBlock {

    /// 设置prio
    pub fn set_priority(&self, p: isize) -> isize {
        self.inner_exclusive_access().priority = p;
        p
    }

    /// 检查[st, st+len)范围的vpn，如果TCB的MS包含任一vpn就返回true
    pub fn contain_any(&self, start: usize, len: usize) -> bool {
        let inner = self.inner_exclusive_access();
        for j in &inner.memory_set.areas {
            if (
                start < j.vpn_range.l.to_va_usize() && start+len > j.vpn_range.l.to_va_usize()
            ) || (  // start在表中间
                start >= j.vpn_range.l.to_va_usize() && start < j.vpn_range.r.to_va_usize()
            )       // start在前，end>=区间
            {
                return true;
            }
        } return false;
    }

    /// 检查是否包含整个[st, st+len)范围的vpn，但是实验要求简单实现，就直接==判断吧...恭敬不如从命
    pub fn contain_all(&self, st: usize, len: usize) -> bool {
        let inner = self.inner_exclusive_access();
        for j in &inner.memory_set.areas {
            if st+len == j.vpn_range.r.to_va_usize() && st == j.vpn_range.l.to_va_usize() {
                return true;
            }
        } return false;
    }

    /// 注册页表和映射虚存
    pub fn malloc(&self, st: usize, len: usize, pt: usize) -> isize {
        use crate::mm::MapPermission;
        let perm: MapPermission = {
            if ((pt >> 0) & 1) == 1 {MapPermission::R} else {MapPermission::empty()}
        } | {
            if ((pt >> 1) & 1) == 1 {MapPermission::W} else {MapPermission::empty()}
        } | {
            if ((pt >> 2) & 1) == 1 {MapPermission::X} else {MapPermission::empty()}
        } | MapPermission::U;
        let mut inner = self.inner_exclusive_access();
        inner.memory_set.insert_framed_area(VirtAddr(st), VirtAddr(st+len), perm);
        0
    }

    /// 注销页表和虚存映射
    pub fn delloc(&self, st: usize, len: usize) -> isize {
        // 因为是简单实现，不考虑交叉、截断区间的情况，所以先不管[st, len, ed]的情况
        use crate::mm::MapArea;
        let mut idx =  0usize; let mut res = -1; let mut tmp: Option<&mut MapArea> = None;
        let mut inner = self.inner_exclusive_access();
        let ref mut memory_set = inner.memory_set;
        for i in &mut memory_set.areas {
            if (i.vpn_range.l == VirtAddr(st).into()) && (i.vpn_range.r == VirtAddr(st+len).into()) {
                tmp = Some(i); res = 0; break;
            } idx += 1;
        }
        if let Some(_t) = tmp {
            _t.unmap(&mut memory_set.page_table);
            memory_set.areas.remove(idx);
        } return res;
    }

    /// 返回inner的mut
    pub fn get_inner(&self) -> RefMut<TaskControlBlockInner> {
        self.inner_exclusive_access()
    }
}

pub struct TaskControlBlockInner {
    /// The physical page number of the frame where the trap context is placed
    pub trap_cx_ppn: PhysPageNum,

    /// Application data can only appear in areas
    /// where the application address space is lower than base_size
    pub base_size: usize,

    /// Save task context
    pub task_cx: TaskContext,

    /// Maintain the execution status of the current process
    pub task_status: TaskStatus,

    /// Application address space
    pub memory_set: MemorySet,

    /// Parent process of the current process.
    /// Weak will not affect the reference count of the parent
    pub parent: Option<Weak<TaskControlBlock>>,

    /// A vector containing TCBs of all child processes of the current process
    pub children: Vec<Arc<TaskControlBlock>>,

    /// It is set when active exit or execution error occurs
    pub exit_code: i32,
    pub fd_table: Vec<Option<Arc<dyn File + Send + Sync>>>,

    /// Heap bottom
    pub heap_bottom: usize,

    /// Program break
    pub program_brk: usize,

    /// ch 3,4
    pub begin_time: usize,

    /// ch 3,4
    pub syscall_times: [u32; MAX_SYSCALL_NUM],

    /// ch 5
    pub stride: usize,

    /// ch 5
    pub priority: isize,
}

impl TaskControlBlockInner {
    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        self.trap_cx_ppn.get_mut()
    }
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }
    pub fn get_status(&self) -> TaskStatus {
        self.task_status
    }
    pub fn is_zombie(&self) -> bool {
        self.get_status() == TaskStatus::Zombie
    }
    pub fn alloc_fd(&mut self) -> usize {
        if let Some(fd) = (0..self.fd_table.len()).find(|fd| self.fd_table[*fd].is_none()) {
            fd
        } else {
            self.fd_table.push(None);
            self.fd_table.len() - 1
        }
    }
}

impl TaskControlBlock {
    /// Create a new process
    ///
    /// At present, it is only used for the creation of initproc
    pub fn new(elf_data: &[u8]) -> Self {
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf(elf_data);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();
        // alloc a pid and a kernel stack in kernel space
        let pid_handle = pid_alloc();
        let kernel_stack = kstack_alloc();
        let kernel_stack_top = kernel_stack.get_top();
        // push a task context which goes to trap_return to the top of kernel stack
        let task_control_block = Self {
            pid: pid_handle,
            kernel_stack,
            inner: unsafe {
                UPSafeCell::new(TaskControlBlockInner {
                    trap_cx_ppn,
                    base_size: user_sp,
                    task_cx: TaskContext::goto_trap_return(kernel_stack_top),
                    task_status: TaskStatus::Ready,
                    memory_set,
                    parent: None,
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: vec![
                        // 0 -> stdin
                        Some(Arc::new(Stdin)),
                        // 1 -> stdout
                        Some(Arc::new(Stdout)),
                        // 2 -> stderr
                        Some(Arc::new(Stdout)),
                    ],
                    heap_bottom: user_sp,
                    program_brk: user_sp,
                    begin_time: 0, stride: 0, priority: 2 ,
                    syscall_times: [0u32; MAX_SYSCALL_NUM],
                })
            },
        };
        // prepare TrapContext in user space
        let trap_cx = task_control_block.inner_exclusive_access().get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            kernel_stack_top,
            trap_handler as usize,
        );
        task_control_block
    }

    /// heke
    pub fn set_parent(&self, parent: Option<Weak<TaskControlBlock>>) {
        self.inner_exclusive_access().parent = parent;
    }

    /// heke
    pub fn get_status(&self) -> TaskStatus {
        self.inner_exclusive_access().task_status
    }

    /// Load a new elf to replace the original application address space and start execution
    pub fn exec(&self, elf_data: &[u8]) {
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf(elf_data);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();

        // **** access current TCB exclusively
        let mut inner = self.inner_exclusive_access();
        // substitute memory_set
        inner.memory_set = memory_set;
        // update trap_cx ppn
        inner.trap_cx_ppn = trap_cx_ppn;
        // initialize trap_cx
        let trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            self.kernel_stack.get_top(),
            trap_handler as usize,
        );
        *inner.get_trap_cx() = trap_cx;
        // **** release current PCB
    }

    /// parent process fork the child process
    pub fn fork(self: &Arc<TaskControlBlock>) -> Arc<TaskControlBlock> {
        // ---- hold parent PCB lock
        let mut parent_inner = self.inner_exclusive_access();
        // copy user space(include trap context)
        let memory_set = MemorySet::from_existed_user(&parent_inner.memory_set);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();
        // alloc a pid and a kernel stack in kernel space
        let pid_handle = pid_alloc();
        let kernel_stack = kstack_alloc();
        let kernel_stack_top = kernel_stack.get_top();
        // copy fd table
        let mut new_fd_table: Vec<Option<Arc<dyn File + Send + Sync>>> = Vec::new();
        for fd in parent_inner.fd_table.iter() {
            if let Some(file) = fd {
                new_fd_table.push(Some(file.clone()));
            } else {
                new_fd_table.push(None);
            }
        }
        let task_control_block = Arc::new(TaskControlBlock {
            pid: pid_handle,
            kernel_stack,
            inner: unsafe {
                UPSafeCell::new(TaskControlBlockInner {
                    trap_cx_ppn,
                    base_size: parent_inner.base_size,
                    task_cx: TaskContext::goto_trap_return(kernel_stack_top),
                    task_status: TaskStatus::Ready,
                    memory_set,
                    parent: Some(Arc::downgrade(self)),
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: new_fd_table,
                    heap_bottom: parent_inner.heap_bottom,
                    program_brk: parent_inner.program_brk,
                    begin_time: 0, stride: 0, priority: 2,
                    syscall_times: [0;  MAX_SYSCALL_NUM ],
                })
            },
        });
        // add child
        parent_inner.children.push(task_control_block.clone());
        // modify kernel_sp in trap_cx
        // **** access child PCB exclusively
        let trap_cx = task_control_block.inner_exclusive_access().get_trap_cx();
        trap_cx.kernel_sp = kernel_stack_top;
        // return
        task_control_block
        // **** release child PCB
        // ---- release parent PCB
    }

    /// get pid of process
    pub fn getpid(&self) -> usize {
        self.pid.0
    }

    /// change the location of the program break. return None if failed.
    pub fn change_program_brk(&self, size: i32) -> Option<usize> {
        let mut inner = self.inner_exclusive_access();
        let heap_bottom = inner.heap_bottom;
        let old_break = inner.program_brk;
        let new_brk = inner.program_brk as isize + size as isize;
        if new_brk < heap_bottom as isize {
            return None;
        }
        let result = if size < 0 {
            inner
                .memory_set
                .shrink_to(VirtAddr(heap_bottom), VirtAddr(new_brk as usize))
        } else {
            inner
                .memory_set
                .append_to(VirtAddr(heap_bottom), VirtAddr(new_brk as usize))
        };
        if result {
            inner.program_brk = new_brk as usize;
            Some(old_break)
        } else {
            None
        }
    }
}

#[derive(Copy, Clone, PartialEq)]
/// task status: UnInit, Ready, Running, Exited
pub enum TaskStatus {
    /// uninitialized
    UnInit,
    /// ready to run
    Ready,
    /// running
    Running,
    /// exited
    Zombie,
}
