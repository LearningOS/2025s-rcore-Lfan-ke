//! Process management syscalls
use crate::{
    config::MAX_SYSCALL_NUM,
    task::{
        change_program_brk, exit_current_and_run_next, suspend_current_and_run_next, TaskStatus,
    },
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// Task information
#[allow(dead_code)]
pub struct TaskInfo {
    /// Task status in it's life cycle
    status: TaskStatus,
    /// The numbers of syscall called by task
    syscall_times: [u32; MAX_SYSCALL_NUM],
    /// Total running time of task
    time: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(_ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    // 所以说，`_ts: *mut TimeVal`只是一个vpn，题目意思就是vpn->buf(ppn)，然后写入是吧。。。
    // 如果是两页也好办，先构造一个结构体，后转为&[u8]，然后字节为单位复制过去就over
    use core::mem::size_of;
    use crate::timer::get_time_us;
    use crate::task::current_user_token;
    use crate::mm::translated_byte_buffer;

    let buffers = translated_byte_buffer(current_user_token(), _ts as *mut u8, size_of::<TimeVal>());
    let us  = get_time_us();
    let tmp = TimeVal {
        usec: us % 1_000_000,
        sec : us / 1_000_000,
    };
    unsafe {
        let src:&'static[u8] = core::slice::from_raw_parts(
            &tmp as *const TimeVal as *const u8, size_of::<TimeVal>()
        );
        let mut curr = 0;
        for buffer in buffers {
            let _end = buffer.len().min(size_of::<TimeVal>()-curr);
            buffer.copy_from_slice(&src[curr.._end]);
            curr = _end;
        }
    };
    0
}

/// YOUR JOB: Finish sys_task_info to pass testcases
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TaskInfo`] is splitted by two pages ?
pub fn sys_task_info(_ti: *mut TaskInfo) -> isize {
    trace!("kernel: sys_task_info NOT IMPLEMENTED YET!");
    use core::mem::size_of;
    use crate::task::TASK_MANAGER;
    use crate::timer::get_time_ms;
    use crate::task::current_user_token;
    use crate::mm::translated_byte_buffer;

    let buffers = translated_byte_buffer(current_user_token(), _ti as *mut u8, size_of::<TaskInfo>());

    let inner = TASK_MANAGER.inner.exclusive_access();

    let tmp = TaskInfo {
        status: inner.tasks[inner.current_task].task_status,
        syscall_times: inner.tasks[inner.current_task].syscall_times,
        time: get_time_ms() - inner.tasks[inner.current_task].begin_time,
    };

    unsafe {
        let src:&'static[u8] = core::slice::from_raw_parts(
            &tmp as *const TaskInfo as *const u8, size_of::<TaskInfo>()
        );
        let mut curr = 0;
        for buffer in buffers {
            let _end = buffer.len().min(size_of::<TaskInfo>()-curr);
            buffer.copy_from_slice(&src[curr.._end]);
            curr = _end;
        }
    }

    drop(inner); 0
}

use crate::config::PAGE_SIZE_BITS;

// YOUR JOB: Implement mmap.
pub fn sys_mmap(_start: usize, _len: usize, _port: usize) -> isize {
    trace!("kernel: sys_mmap NOT IMPLEMENTED YET!");
    
    if (_start % 4096)!=0 || (_port & !0x7)!=0 || (_port & 0x7)==0 {
        return -1;
    }

    let start = _start >> PAGE_SIZE_BITS;
    let len = (if _len % 4096 == 0 {0} else {1}) + _len / 4096; // ceil

    use crate::task::TASK_MANAGER;
    let inner = TASK_MANAGER.inner.exclusive_access();
    /*  区间重合的可能(-1)：
     *      要插入的段包含以及插入的段的至少一部分
     * */
    for j in &inner.tasks[inner.current_task].memory_set.areas {
        //println!("new: {:x}-{:x} had: {:x}-{:x}", start, start+len, j.vpn_range.l.0, j.vpn_range.r.0);
        if (start < j.vpn_range.l.0 && start + len > j.vpn_range.l.0) || // start在表中间
           (start >= j.vpn_range.l.0 && start < j.vpn_range.r.0)         // start在前，end>=区间
        {
            return -1;
        }
    }
    let curr__ = inner.current_task;
    drop(inner);
    let mut inner = TASK_MANAGER.inner.exclusive_access();
    inner.tasks[curr__].memory_set.heke_malloc(_start, len, _port)
}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(_start: usize, _len: usize) -> isize {
    trace!("kernel: sys_munmap NOT IMPLEMENTED YET!");
    // 先验证范围，确认存在后注销
    use crate::task::TASK_MANAGER;
    
    if (_start % 4096)!=0 {
        return -1;
    }
   
    let len = (if _len % 4096 == 0 {0} else {1}) + _len / 4096;
    let start = _start >> PAGE_SIZE_BITS;

    'range_true: loop {
    let inner = TASK_MANAGER.inner.exclusive_access();
    for j in &inner.tasks[inner.current_task].memory_set.areas {
        if start+len == j.vpn_range.r.0 && start == j.vpn_range.l.0 {
            break 'range_true;
        }
    } return -1; }
    let mut inner = TASK_MANAGER.inner.exclusive_access();
    let curr__ = inner.current_task;
    inner.tasks[curr__].memory_set.heke_delloc(_start, len)
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
