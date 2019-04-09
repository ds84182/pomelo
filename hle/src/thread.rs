use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::fmt;

use std::future::Future;
use std::pin::Pin;
use std::task::Waker as LocalWaker;
use std::task::Poll;

use crate::{kernel, svc};

use kernel::KernelExt;

pub struct ProcessHLE;

impl kernel::Process for ProcessHLE {}

struct SharedHLEContext {
    resume: kernel::ThreadResume,
}

// This type should not be moved outside the lifetime of the future spawning closure it is given to.
pub struct HLEContext {
    kernel: NonNull<kernel::Kernel + 'static>,
    shared: NonNull<SharedHLEContext>,
}

impl HLEContext {
    pub fn kernel(&mut self) -> &mut kernel::Kernel {
        // This pointer is always valid as long as the kernel is living.
        // The kernel will not move or drop during execution of a thread because a mutable reference is taken.
        // Outside of thread execution, the kernel is allowed to drop, but not allowed to move.
        unsafe { self.kernel.as_mut() }
    }

    fn shared(&mut self) -> &mut SharedHLEContext {
        // The SharedHLEContext is stored in a Box and is alive as long as the thread is alive.
        // The thread is guaranteed alive as long as the HLEContext is not moved outside of the future spawning closure it is given to.
        unsafe { self.shared.as_mut() }
    }

    pub fn suspend<'a>(&'a mut self) -> impl Future<Output=kernel::ThreadResume> + 'a {
        HLESuspendFuture(false, self)
    }

    // Re-exports of various services
    pub async fn wait_sync_1<'a>(&'a mut self, handle: &'a kernel::Handle, timeout: Option<i64>) -> kernel::KResult<()> {
        svc::wait_sync_1(self.kernel(), Some(handle.clone()), timeout.unwrap_or(-1))?;
        let resume = await!(self.suspend());
        match resume {
            kernel::ThreadResume::Normal => Ok(()),
            kernel::ThreadResume::TimeoutReached => Err(kernel::errors::TIMEOUT_REACHED),
            _ => unreachable!("{:?}", resume)
        }
    }

    pub async fn reply_and_receive<'a>(&'a mut self, handles: &'a [Option<kernel::Handle>], reply_target: Option<kernel::Handle>, ipc_data: &'a mut kernel::IPCData) -> (Option<usize>, kernel::KResult<()>) {
        self.kernel().set_thread_ipc_data(ipc_data); // Place IPC data into slot
        let resume = match svc::reply_and_receive_pre(self.kernel(), handles, reply_target) {
            Ok(Some(..)) => {
                await!(self.suspend())
            },
            Ok(None) => {
                unreachable!()
            },
            Err(err) => return (None, Err(err)),
        };
        let (index, res) = svc::reply_and_receive_post(self.kernel(), handles, &resume);
        self.kernel().get_thread_ipc_data(ipc_data); // Take IPC data from slot
        (Some(index), res)
    }

    pub async fn send_sync_request<'a>(&'a mut self, handle: &'a kernel::Handle, ipc_data: &'a mut kernel::IPCData) -> kernel::KResult<()> {
        self.kernel().set_thread_ipc_data(ipc_data); // Place IPC data into slot
        svc::send_sync_request(self.kernel(), Some(handle.clone()))?;
        let resume = await!(self.suspend());
        match resume {
            kernel::ThreadResume::Normal => {
                self.kernel().get_thread_ipc_data(ipc_data); // Take IPC data from slot
                Ok(())
            },
            _ => unreachable!("{:?}", resume)
        }
    }

    pub async fn create_session_to_port<'a>(&'a mut self, handle: &'a kernel::Handle) -> kernel::KResult<kernel::Handle> {
        let (reschedule, handle) = svc::create_session_to_port(self.kernel(), Some(handle.clone()))?;
        if reschedule.is_some() {
            let resume = await!(self.suspend());
            match resume {
                kernel::ThreadResume::Normal => (),
                _ => unreachable!("{:?}", resume)
            }
        }
        Ok(handle)
    }

    pub async fn accept_session<'a>(&'a mut self, handle: &'a kernel::Handle) -> kernel::KResult<kernel::Handle> {
        loop {
            let res = svc::accept_session(self.kernel(), Some(handle.clone()))?;
            match res {
                Ok(handle) => return Ok(handle),
                Err(..) => {
                    let resume = await!(self.suspend());
                    match resume {
                        kernel::ThreadResume::Normal => (),
                        _ => unreachable!("{:?}", resume)
                    }
                }
            }
        }
    }
}

struct HLESuspendFuture<'a>(bool, &'a mut HLEContext);

impl<'a> Future for HLESuspendFuture<'a> {
    type Output = kernel::ThreadResume;

    fn poll(mut self: Pin<&mut Self>, _: &LocalWaker) -> Poll<Self::Output> {
        if self.0 {
            // Already suspended to scheduler, so take resume from HLE context
            use std::mem;
            Poll::Ready(mem::replace(&mut self.1.shared().resume, kernel::ThreadResume::Normal))
        } else {
            self.0 = true;
            Poll::Pending
        }
    }
}

enum ThreadRuntime {
    Init(Box<dyn FnOnce(HLEContext) -> Box<dyn Future<Output=()>>>),
    Running(Pin<Box<dyn Future<Output=()>>>),
    Indeterminate,
}

pub struct ThreadHLE {
    runtime: RefCell<ThreadRuntime>,
    shared: RefCell<Box<SharedHLEContext>>,
    ipc_data: RefCell<kernel::IPCData>,
}

impl ThreadHLE {
    pub fn new<F: Future<Output=()> + 'static>(func: impl FnOnce(HLEContext) -> F + 'static) -> ThreadHLE {
        ThreadHLE {
            runtime: RefCell::new(
                ThreadRuntime::Init(
                    Box::new(move |ctx| Box::new(func(ctx)))
                )
            ),
            shared: RefCell::new(Box::new(SharedHLEContext {
                resume: kernel::ThreadResume::Normal,
            })),
            ipc_data: RefCell::new([0u8; 256]),
        }
    }
}

impl kernel::Thread for ThreadHLE {
    fn resume(&self, payload: kernel::ThreadResume, kctx: &mut kernel::Kernel) {
        let mut shared = self.shared.borrow_mut();

        let mut runtime = self.runtime.borrow_mut();
        use std::mem;
        let current_runtime = mem::replace(&mut *runtime, ThreadRuntime::Indeterminate);

        let kctx_static = unsafe { std::mem::transmute::<_, &mut (dyn kernel::Kernel + 'static)>(kctx) };

        let current_runtime = if let ThreadRuntime::Init(func) = current_runtime {
            let future = func(HLEContext {
                // As long as the kernel given to this function is the same across all future invocations, this is sound.
                kernel: unsafe { NonNull::new_unchecked(kctx_static as *mut _) },
                // We will not drop the Box associated with this until the end of the thread's lifetime, so this is sound.
                shared: unsafe { NonNull::new_unchecked(shared.as_mut() as *mut _) },
            });

            // It's not possible to move or replace the insides of a `Pin<Box<T>>`
            // when `T: !Unpin`,  so it's safe to pin it directly without any
            // additional requirements.
            ThreadRuntime::Running(unsafe { Pin::new_unchecked(future) })
        } else {
            current_runtime
        };

        shared.resume = payload;

        let poll_result = if let ThreadRuntime::Running(mut future) = current_runtime {
            let poll_result = future.as_mut().poll(
                // We don't have any Future implementations that utilize LocalWakers, making this sorta sound.
                // If Rust requires that references always point to valid memory, this becomes unsound.
                unsafe { NonNull::dangling().as_mut() }
            );
            *runtime = ThreadRuntime::Running(future);
            poll_result
        } else {
            unreachable!()
        };

        match poll_result {
            Poll::Pending => (),
            Poll::Ready(()) => {
                kctx_static.exit_thread();
            }
        }
    }

    fn read_ipc_data(&self, data: &mut kernel::IPCData) {
        *data = *self.ipc_data.borrow();
    }

    fn write_ipc_data(&self, data: &kernel::IPCData) {
        *&mut *self.ipc_data.borrow_mut() = *data;
    }
}

pub struct ThreadImpl {
    index: kernel::ThreadIndex,
    dead: RefCell<bool>,
    waker_queue: RefCell<Vec<Rc<kernel::Waker>>>,
}

impl ThreadImpl {
    pub fn new(index: kernel::ThreadIndex) -> ThreadImpl {
        ThreadImpl {
            index,
            dead: RefCell::new(false),
            waker_queue: RefCell::new(vec![]),
        }
    }

    pub fn on_death(&self, this: &kernel::KObjectRef, ct: &mut kernel::CommonThreadManager) {
        let mut dead = self.dead.borrow_mut();
        if !*dead {
            *dead = true;
            use std::mem::replace;
            let queue = replace(&mut *self.waker_queue.borrow_mut(), vec![]);
            queue.iter().for_each(|waker| {
                let _ = waker.wake(this, ct);
            });
        }
    }
}

impl fmt::Debug for ThreadImpl {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ThreadImpl")
    }
}

impl kernel::KSynchronizationObject for ThreadImpl {
    fn wait(&self, this: &kernel::KObjectRef, waker: Rc<kernel::Waker>, ct: &mut kernel::CommonThreadManager) {
        if *self.dead.borrow() {
            let _ = waker.wake(this, ct);
        } else {
            // Put into the queue
            self.waker_queue.borrow_mut().push(waker);
        }
    }

    fn remove_wakers_by_opaque_id(&self, id: u64) {
        self.waker_queue.borrow_mut().retain(|waker| !waker.check_opaque_id(id))
    }
}

impl kernel::KThread for ThreadImpl {
    fn exit(&self, this: &kernel::KObjectRef, ct: &mut kernel::CommonThreadManager) {
        self.on_death(this, ct);
    }
}

impl kernel::object::KObjectData for ThreadImpl {}
impl kernel::object::KObjectDebug for ThreadImpl {}
