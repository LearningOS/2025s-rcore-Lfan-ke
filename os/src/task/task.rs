//! Types related to task management

use super::TaskContext;

use crate::config::MAX_SYSCALL_NUM;

/// The task control block (TCB) of a task.
#[derive(Copy, Clone)]
pub struct TaskControlBlock {
    /// The task status in it's lifecycle
    pub task_status: TaskStatus,
    /// The task context
    pub task_cx: TaskContext,
    /// 10-30 保存系统调用次数与状态
    pub syscall_times: [u32; MAX_SYSCALL_NUM],// 10-30
    /// 10-30 保存第一次调度的时间
    pub begin_time: usize,// 10-30
}

/// The status of a task
#[derive(Copy, Clone, PartialEq)]
pub enum TaskStatus {
    /// uninitialized
    UnInit,
    /// ready to run
    Ready,
    /// running
    Running,
    /// exited
    Exited,
}
