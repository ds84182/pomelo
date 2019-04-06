use std::fmt;
use std::rc::Rc;
use std::cell::RefCell;

use crate::kernel;
use kernel::object::*;

pub struct EventImpl {
    signal: RefCell<kernel::Signal>,
    waker_queue: RefCell<Vec<Rc<kernel::Waker>>>,
}

impl EventImpl {
    pub fn new(reset_type: kernel::ResetType) -> EventImpl {
        EventImpl {
            signal: RefCell::new(kernel::Signal::new(reset_type)),
            waker_queue: RefCell::new(vec![]),
        }
    }
}

impl fmt::Debug for EventImpl {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "EventImpl")
    }
}

impl KSynchronizationObject for EventImpl {
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

impl KEvent for EventImpl {
    fn signal(&self, this: &kernel::KObjectRef, ct: &mut kernel::CommonThreadManager) {
        let mut signal = self.signal.borrow_mut();
        signal.signal();

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

    fn clear(&self) {
        let mut signal = self.signal.borrow_mut();
        signal.reset();
    }
}

impl KObjectData for EventImpl {}
impl KObjectDebug for EventImpl {}
