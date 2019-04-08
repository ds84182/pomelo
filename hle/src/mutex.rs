use std::fmt;
use std::rc::Rc;
use std::cell::{RefCell, Cell};

use crate::kernel;
use kernel::object::*;

pub struct MutexImpl {
    acquired: Cell<bool>,
    waker_queue: RefCell<Vec<Rc<kernel::Waker>>>,
}

impl MutexImpl {
    pub fn new(acquired: bool) -> MutexImpl {
        MutexImpl {
            acquired: Cell::new(acquired),
            waker_queue: RefCell::new(vec![]),
        }
    }
}

impl fmt::Debug for MutexImpl {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "MutexImpl")
    }
}

impl KSynchronizationObject for MutexImpl {
    fn wait(&self, this: &kernel::KObjectRef, waker: Rc<kernel::Waker>, ct: &mut kernel::CommonThreadManager) {
        if !self.acquired.replace(true) {
            if waker.wake(this, ct).is_err() {
                self.acquired.set(false);
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

impl KMutex for MutexImpl {
    fn release(&self, this: &kernel::KObjectRef, ct: &mut kernel::CommonThreadManager) {
        // Try to wake up a waker in the queue
        let waker = self.waker_queue.borrow().first().map(|waker| waker.clone());
        if let Some(waker) = waker {
            waker.wake(this, ct).expect("Failed to wake up thread");
        } else {
            self.acquired.set(false);
        }
    }
}

impl KObjectData for MutexImpl {}
impl KObjectDebug for MutexImpl {}
