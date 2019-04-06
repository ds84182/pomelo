use std::fmt;
use std::rc::Rc;
use std::cell::RefCell;

use crate::kernel;
use kernel::object::*;

struct TimerImplData {
    next_ns: i64,
    interval: i64,
}

impl TimerImplData {
    fn valid(&self) -> bool {
        self.next_ns >= 0 && self.interval >= 0
    }
}

pub struct TimerImpl {
    signal: RefCell<kernel::Signal>,
    waker_queue: RefCell<Vec<Rc<kernel::Waker>>>,
    data: RefCell<TimerImplData>,
}

impl TimerImpl {
    pub fn new(reset_type: kernel::ResetType) -> TimerImpl {
        TimerImpl {
            signal: RefCell::new(kernel::Signal::new(reset_type)),
            waker_queue: RefCell::new(vec![]),
            data: RefCell::new(TimerImplData { next_ns: -1, interval: -1 })
        }
    }
}

impl fmt::Debug for TimerImpl {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "TimerImpl")
    }
}

impl KSynchronizationObject for TimerImpl {
    fn wait(&self, this: &kernel::KObjectRef, waker: Rc<kernel::Waker>, ct: &mut kernel::CommonThreadManager) {
        if self.signal.borrow_mut().consume() {
            if waker.wake(this, ct).is_err() {
                self.signal.borrow_mut().unconsume();
            }
        } else {
            // Put into the queue
            self.waker_queue.borrow_mut().push(waker);
        }
    }

    fn remove_wakers_by_opaque_id(&self, id: u64) {
        self.waker_queue.borrow_mut().retain(|waker| !waker.check_opaque_id(id))
    }
}

impl KTimer for TimerImpl {
    fn set(&self, initial_ns: i64, interval: i64) {
        *self.data.borrow_mut() = TimerImplData { next_ns: initial_ns, interval };
    }

    fn cancel(&self) {
        self.set(-1, -1)
    }

    fn tick(&self, this: &kernel::KObjectRef, time_ns: u64, ct: &mut kernel::CommonThreadManager) {
        let mut data = self.data.borrow_mut();
        if data.valid() && (data.next_ns as u64) <= time_ns {
            let mut signal = self.signal.borrow_mut();
            signal.signal();
            if signal.is_pulse() {
                let interval = data.interval;
                *data = TimerImplData { next_ns: (time_ns + (interval as u64)) as i64, interval };
            } else {
                *data = TimerImplData { next_ns: -1, interval: -1 };
            }

            if signal.is_sticky() {
                use std::mem::replace;
                let mut queue = replace(&mut *self.waker_queue.borrow_mut(), vec![]);
                queue.iter().for_each(|waker| {
                    let _ = waker.wake(this, ct);
                });
                queue.clear();
                replace(&mut *self.waker_queue.borrow_mut(), queue); // Small optimization: maintain the allocated vec
            } else {
                // Try to wake up a waker in the queue
                let waker = self.waker_queue.borrow().first().map(|waker| waker.clone());
                if let Some(waker) = waker {
                    waker.wake(this, ct).expect("Failed to wake up thread");
                    signal.reset();
                }
            }
        }
    }
}

impl KObjectData for TimerImpl {}
impl KObjectDebug for TimerImpl {}
