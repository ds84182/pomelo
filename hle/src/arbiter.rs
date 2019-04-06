use std::fmt;
use std::rc::Rc;
use std::cell::RefCell;

use crate::kernel;
use kernel::object::*;

pub struct ArbiterImpl {
    waker_queue: RefCell<Vec<(Rc<kernel::Waker>, u32)>>,
}

impl ArbiterImpl {
    pub fn new() -> ArbiterImpl {
        ArbiterImpl {
            waker_queue: RefCell::new(vec![]),
        }
    }
}

impl fmt::Debug for ArbiterImpl {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ArbiterImpl")
    }
}

impl KAddressArbiter for ArbiterImpl {
    fn wait(&self, waker: Rc<kernel::Waker>, addr: u32) {
        // Put into the queue
        self.waker_queue.borrow_mut().push((waker, addr));
    }

    fn signal(&self, this: &kernel::KObjectRef, amount: i32, addr: u32, ct: &mut kernel::CommonThreadManager) -> bool {
        let mut signal_count = if amount < 0 {
            // Signal all threads
            0xFFFFFFFF
        } else {
            // Signal only [amount] threads
            amount as u32
        };

        let mut has_waken = false;

        let drained: Vec<_> = self.waker_queue.borrow_mut().drain_filter(|(_, waker_addr)| {
            if *waker_addr == addr && signal_count > 0 {
                if signal_count != 0xFFFFFFFF {
                    signal_count -= 1;
                }
                has_waken = true;
                true
            } else {
                false
            }
        }).map(|(waker, _)| waker).collect();

        println!("Waking {} threads", drained.len());

        for waker in drained {
            waker.wake(this, ct).expect("Failed to wake thread waiting on arbitration");
        }

        has_waken
    }

    fn remove_wakers_by_opaque_id(&self, id: u64) {
        self.waker_queue.borrow_mut().retain(|(waker, _)| !waker.check_opaque_id(id))
    }
}

impl KObjectData for ArbiterImpl {}
impl KObjectDebug for ArbiterImpl {}
