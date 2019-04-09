use std::fmt;
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::VecDeque;

use crate::kernel;
use kernel::KernelExt;
use kernel::object::*;

pub fn make_port_pair(kctx: &mut kernel::Kernel) -> (kernel::KTypedObject<KServerPort>, kernel::KTypedObject<KClientPort>) {
    let server = kernel::KTypedObject::new(&kctx.new_object(ServerPortHLE {
        waker_queue: RefCell::new(VecDeque::new()),
        session_queue: RefCell::new(VecDeque::new()),
        client: RefCell::new(kernel::KWeakTypedObject::empty()),
    })).unwrap();

    let client = kernel::KTypedObject::new(&kctx.new_object(ClientPortHLE {
        waker_queue: RefCell::new(VecDeque::new()),
        server: kernel::KTypedObject::downgrade(&server),
    })).unwrap();

    *server.client.borrow_mut() = kernel::KTypedObject::downgrade(&client);

    (kernel::KTypedObject::cast(server).unwrap(), kernel::KTypedObject::cast(client).unwrap())
}

// ServerPort

struct ServerPortHLE {
    waker_queue: RefCell<VecDeque<Rc<kernel::Waker>>>,
    session_queue: RefCell<VecDeque<kernel::KTypedObject<KServerSession>>>,
    client: RefCell<kernel::KWeakTypedObject<ClientPortHLE>>,
}

impl fmt::Debug for ServerPortHLE {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ServerPortHLE")
    }
}

impl KSynchronizationObject for ServerPortHLE {
    fn wait(&self, _: &kernel::KObjectRef, waker: Rc<kernel::Waker>, _: &mut kernel::CommonThreadManager) {
        // Put into the queue
        self.waker_queue.borrow_mut().push_back(waker);
    }

    fn remove_wakers_by_opaque_id(&self, id: u64) {
        self.waker_queue.borrow_mut().retain(|waker| !waker.check_opaque_id(id))
    }
}

impl ServerPortHLE {
    fn enqueue_session(&self, server_session: kernel::KTypedObject<KServerSession>, this: &kernel::KObjectRef, ct: &mut kernel::CommonThreadManager) {
        self.session_queue.borrow_mut().push_back(server_session);
        self.wake_thread(this, ct);
    }

    fn wake_thread(&self, this: &kernel::KObjectRef, ct: &mut kernel::CommonThreadManager) {
        let waker = self.waker_queue.borrow_mut().pop_front();
        if let Some(waker) = waker {
            waker.wake(this, ct).expect("Failed to wake up thread");
        }
    }
}

impl KServerPort for ServerPortHLE {
    fn accept_session(&self, ct: &mut kernel::CommonThreadManager) -> Option<kernel::KTypedObject<KServerSession>> {
        let session = self.session_queue.borrow_mut().pop_front();
        if session.is_some() {
            let client = self.client.borrow().upgrade().expect("ClientPortHLE disconnected");
            client.wake_thread(client.object(), ct);
        }
        session
    }
}

impl KObjectData for ServerPortHLE {}
impl KObjectDebug for ServerPortHLE {}

// ClientPort

pub struct ClientPortHLE {
    waker_queue: RefCell<VecDeque<Rc<kernel::Waker>>>,
    server: kernel::KWeakTypedObject<ServerPortHLE>,
}

impl fmt::Debug for ClientPortHLE {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ClientSessionHLE")
    }
}

impl KSynchronizationObject for ClientPortHLE {
    fn wait(&self, _: &kernel::KObjectRef, waker: Rc<kernel::Waker>, _: &mut kernel::CommonThreadManager) {
        // Put into the queue
        self.waker_queue.borrow_mut().push_back(waker);
    }

    fn remove_wakers_by_opaque_id(&self, id: u64) {
        self.waker_queue.borrow_mut().retain(|waker| !waker.check_opaque_id(id))
    }
}

impl ClientPortHLE {
    fn wake_thread(&self, this: &kernel::KObjectRef, ct: &mut kernel::CommonThreadManager) {
        let waker = self.waker_queue.borrow_mut().pop_front();
        if let Some(waker) = waker {
            waker.wake(this, ct).expect("Failed to wake up thread");
        }
    }
}

impl KClientPort for ClientPortHLE {
    fn create_session_to_port(&self, server_session: kernel::KTypedObject<KServerSession>, ct: &mut kernel::CommonThreadManager) {
        let server = self.server.upgrade().expect("ServerPortHLE disconnected");
        server.enqueue_session(server_session, server.object(), ct);
    }
}

impl KObjectData for ClientPortHLE {}
impl KObjectDebug for ClientPortHLE {}
