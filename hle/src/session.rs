use std::fmt;
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::VecDeque;

use crate::kernel;
use kernel::object::*;

pub fn make_session_pair(kctx: &mut kernel::Kernel) -> (kernel::KTypedObject<KServerSession>, kernel::KTypedObject<KClientSession>) {
    let server = kernel::KTypedObject::new(&kctx.new_object(ServerSessionHLE {
        waker_queue: RefCell::new(VecDeque::new()),
        request_queue: RefCell::new(VecDeque::new()),
        client: RefCell::new(kernel::KWeakTypedObject::empty()),
    })).unwrap();

    let client = kernel::KTypedObject::new(&kctx.new_object(ClientSessionHLE {
        waker_queue: RefCell::new(VecDeque::new()),
        server: kernel::KTypedObject::downgrade(&server),
    })).unwrap();

    *server.client.borrow_mut() = kernel::KTypedObject::downgrade(&client);

    (kernel::KTypedObject::cast(server).unwrap(), kernel::KTypedObject::cast(client).unwrap())
}

pub struct ServerSessionHLE {
    waker_queue: RefCell<VecDeque<Rc<kernel::Waker>>>,
    request_queue: RefCell<VecDeque<kernel::ThreadIndex>>,
    client: RefCell<kernel::KWeakTypedObject<ClientSessionHLE>>,
}

impl fmt::Debug for ServerSessionHLE {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ServerSessionHLE")
    }
}

impl KSynchronizationObject for ServerSessionHLE {
    fn wait(&self, _: &kernel::KObjectRef, waker: Rc<kernel::Waker>, _: &mut kernel::CommonThreadManager) {
        // Put into the queue
        self.waker_queue.borrow_mut().push_back(waker);
    }

    fn remove_wakers_by_opaque_id(&self, id: u64) {
        self.waker_queue.borrow_mut().retain(|waker| !waker.check_opaque_id(id))
    }
}

impl ServerSessionHLE {
    fn enqueue_sync_request(&self, this: &kernel::KObjectRef, data: kernel::ThreadIndex, ct: &mut kernel::CommonThreadManager) {
        self.request_queue.borrow_mut().push_back(data);

        self.wake_thread(this, ct);
    }

    fn wake_thread(&self, this: &kernel::KObjectRef, ct: &mut kernel::CommonThreadManager) {
        let waker = self.waker_queue.borrow_mut().pop_front();
        if let Some(waker) = waker {
            waker.wake(this, ct).expect("Failed to wake up thread");
        }
    }
}

impl KServerSession for ServerSessionHLE {
    fn receive(&self, this_thread: kernel::ThreadIndex, kctx: &mut kernel::Kernel) -> bool {
        if let Some(request) = self.request_queue.borrow().front() {
            kctx.translate_ipc(request.clone(), this_thread, false);
            true
        } else {
            false
        }
    }

    fn reply(&self, this_thread: kernel::ThreadIndex, kctx: &mut kernel::Kernel) {
        let request_thread = self.request_queue.borrow_mut().pop_front().expect("Server replied to nobody");
        kctx.translate_ipc(this_thread, request_thread, true);

        let client = self.client.borrow().upgrade().expect("Client disconnected before reply");
        client.enqueue_response(client.object(), &mut kctx.threads);
    }

    fn on_endpoint_closed(&self) {
        // We'd want to wake up all of those in the request queue, and also set a flag to make recieve respond with an error
        panic!("TODO: Implement")
    }
}

impl KObjectData for ServerSessionHLE {}
impl KObjectDebug for ServerSessionHLE {}

// Client Session

pub struct ClientSessionHLE {
    waker_queue: RefCell<VecDeque<Rc<kernel::Waker>>>,
    server: kernel::KWeakTypedObject<ServerSessionHLE>,
}

impl fmt::Debug for ClientSessionHLE {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ClientSessionHLE")
    }
}

impl KSynchronizationObject for ClientSessionHLE {
    fn wait(&self, _: &kernel::KObjectRef, waker: Rc<kernel::Waker>, _: &mut kernel::CommonThreadManager) {
        // Put into the queue
        self.waker_queue.borrow_mut().push_back(waker);
    }

    fn remove_wakers_by_opaque_id(&self, id: u64) {
        self.waker_queue.borrow_mut().retain(|waker| !waker.check_opaque_id(id))
    }
}

impl ClientSessionHLE {
    fn enqueue_response(&self, this: &kernel::KObjectRef, ct: &mut kernel::CommonThreadManager) {
        self.wake_thread(this, ct);
    }

    fn wake_thread(&self, this: &kernel::KObjectRef, ct: &mut kernel::CommonThreadManager) {
        let waker = self.waker_queue.borrow_mut().pop_front();
        if let Some(waker) = waker {
            waker.wake(this, ct).expect("Failed to wake up thread");
        }
    }
}

impl KClientSession for ClientSessionHLE {
    fn sync_request(&self, waker: Rc<kernel::Waker>, ct: &mut kernel::CommonThreadManager) -> bool {
        if let Some(server) = self.server.upgrade() {
            let thread_id = waker.thread_id();
            self.waker_queue.borrow_mut().push_back(waker);
            server.enqueue_sync_request(server.object(), thread_id, ct);
            true
        } else {
            false
        }
    }
}

impl KObjectData for ClientSessionHLE {}
impl KObjectDebug for ClientSessionHLE {}

// ServiceWrapper
