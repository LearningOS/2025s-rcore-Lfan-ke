//!Implementation of [`TaskManager`]
use super::TaskControlBlock;
use crate::sync::UPSafeCell;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use lazy_static::*;
///A array of `TaskControlBlock` that is thread-safe
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

#[allow(unused)]
const BIG_STRIDE: usize = 0x233333;

/// A simple FIFO scheduler.
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
    /// Add process back to ready queue
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    /// Take a process out of the ready queue
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        // self.ready_queue.pop_front()
        // 说好的简单起见昂！
        let mut res = self.ready_queue.pop_front();
        let mut len = self.ready_queue.len();
        if     res.is_none() { return res; }
        if     len == 0      { return res; }
        while len > 0 {
            let mut tmp = self.ready_queue.pop_front();
            let (_re, _tm) = (res.unwrap(), tmp.unwrap());
            let (_rs, _ts) = (_re.get_inner().stride, _tm.get_inner().stride);
            if _rs > _ts {
                (res, tmp) = (Some(_tm), Some(_re));
            } else {
                (res, tmp) = (Some(_re), Some(_tm));
            }
            self.ready_queue.push_back(tmp.unwrap());
            len -= 1;
        }
        let resunwrp = res.unwrap();
        let mut resinner = resunwrp.get_inner();
        let priority = resinner.priority as usize;
        resinner.stride +=  BIG_STRIDE / priority;
        drop(resinner);
        return Some(resunwrp);
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}
