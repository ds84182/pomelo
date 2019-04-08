use std::fmt::Debug;
use std::{ptr, ptr::NonNull, raw, mem};
use std::rc::Rc;

use crate as kernel;

use crate::ipc::IPCContext;

#[derive(Debug)]
pub struct KObject {
    dispatch: &'static KObjectVTable,
    data: NonNull<()>,
}

#[derive(Debug)]
pub struct KObjectVTable {
    drop_the_ball_fn: fn(NonNull<()>) -> (),
    sync_vtable: Option<NonNull<()>>,
    client_session_vtable: Option<NonNull<()>>,
    server_session_vtable: Option<NonNull<()>>,
    client_port_vtable: Option<NonNull<()>>,
    server_port_vtable: Option<NonNull<()>>,
    timer_vtable: Option<NonNull<()>>,
    addr_arb_vtable: Option<NonNull<()>>,
    event_vtable: Option<NonNull<()>>,
    thread_vtable: Option<NonNull<()>>,
    codeset_vtable: Option<NonNull<()>>,
    mutex_vtable: Option<NonNull<()>>,

    debug_vtable: Option<NonNull<()>>,
}

fn drop_the_ball<T>(boom_box: NonNull<()>) {
    unsafe { Box::from_raw(boom_box.cast::<T>().as_ptr()) };
}

pub trait KObjectData: Sized + Debug {
    const VTABLE: &'static KObjectVTable = &KObjectVTable {
        drop_the_ball_fn: drop_the_ball::<Self>,
        sync_vtable: <(&dyn KSynchronizationObject, Self) as AutoVTableUniversal>::VTABLE,
        client_session_vtable: <(&dyn KClientSession, Self) as AutoVTableUniversal>::VTABLE,
        server_session_vtable: <(&dyn KServerSession, Self) as AutoVTableUniversal>::VTABLE,
        client_port_vtable: <(&dyn KClientPort, Self) as AutoVTableUniversal>::VTABLE,
        server_port_vtable: <(&dyn KServerPort, Self) as AutoVTableUniversal>::VTABLE,
        timer_vtable: <(&dyn KTimer, Self) as AutoVTableUniversal>::VTABLE,
        addr_arb_vtable: <(&dyn KAddressArbiter, Self) as AutoVTableUniversal>::VTABLE,
        event_vtable: <(&dyn KEvent, Self) as AutoVTableUniversal>::VTABLE,
        thread_vtable: <(&dyn KThread, Self) as AutoVTableUniversal>::VTABLE,
        codeset_vtable: <(&dyn KCodeSet, Self) as AutoVTableUniversal>::VTABLE,
        mutex_vtable: <(&dyn KMutex, Self) as AutoVTableUniversal>::VTABLE,

        debug_vtable: <(&dyn KObjectDebug, Self) as AutoVTableUniversal>::VTABLE,
    };
}

impl KObject {
    pub fn new<T: KObjectData>(data: T) -> KObject {
        KObject {
            dispatch: T::VTABLE,
            data: unsafe { NonNull::new_unchecked(Box::leak(Box::new(data))).cast() }
        }
    }

    pub fn try_downcast<'a, T: KObjectData>(&self) -> Option<&'a T> {
        if (self.dispatch as *const KObjectVTable) == (T::VTABLE as *const KObjectVTable) {
            // Do something unholy
            Some(unsafe { &*self.data.cast::<T>().as_ptr() })
        } else {
            None
        }
    }
}

pub trait KObjectDynCast<T: ?Sized> {
    fn dyn_cast(&self) -> Option<*const T>;
}

impl<T: KObjectData> KObjectDynCast<T> for KObject {
    fn dyn_cast(&self) -> Option<*const T> {
        self.try_downcast().map(|r| r as *const T)
    }
}

impl Drop for KObject {
    fn drop(&mut self) {
        (self.dispatch.drop_the_ball_fn)(self.data)
    }
}

macro_rules! get_trait_vtable {
    ($type:ty, $trait:ty) => {
        // Sorry rustc
        Some(unsafe { NonNull::new_unchecked(mem::transmute::<_, raw::TraitObject>((&*ptr::null::<$type>()) as &$trait).vtable) })
    };
}

trait AutoVTable {
    const VTABLE: Option<NonNull<()>>;
}

trait AutoVTableUniversal {
    const VTABLE: Option<NonNull<()>>;
}

impl<T> AutoVTableUniversal for T {
    default const VTABLE: Option<NonNull<()>> = None;
}

impl<T: AutoVTable> AutoVTableUniversal for T {
    const VTABLE: Option<NonNull<()>> = <T as AutoVTable>::VTABLE;
}

macro_rules! try_into_trait {
    ($vtable:ident, $($trait:tt)*) => {
        impl KObjectDynCast<dyn $($trait)*> for KObject {
            fn dyn_cast(&self) -> Option<*const $($trait)*> {
                if let Some(vtable) = self.dispatch.$vtable {
                    // God is dead
                    Some(unsafe { mem::transmute(raw::TraitObject { data: self.data.as_ptr(), vtable: vtable.as_ptr() }) })
                } else {
                    None
                }
            }
        }

        impl<'a> Into<Option<&'a $($trait)*>> for &'a KObject {
            fn into(self) -> Option<&'a $($trait)*> {
                if let Some(vtable) = self.dispatch.$vtable {
                    // God is dead
                    Some(unsafe { mem::transmute(raw::TraitObject { data: self.data.as_ptr(), vtable: vtable.as_ptr() }) })
                } else {
                    None
                }
            }
        }

        impl<T: $($trait)*> AutoVTable for (&dyn $($trait)*, T) {
            const VTABLE: Option<NonNull<()>> = get_trait_vtable!(T, $($trait)*);
        }
    };
}

pub trait KObjectDebug: Debug {}
try_into_trait!(debug_vtable, KObjectDebug);

pub trait KSynchronizationObject {
    fn wait(&self, this: &kernel::KObjectRef, waker: Rc<kernel::Waker>, ct: &mut kernel::CommonThreadManager);
    fn remove_wakers_by_opaque_id(&self, id: u64);
}
try_into_trait!(sync_vtable, KSynchronizationObject);

// REMEMBER: There can only be one outstanding request from a client at time

pub const IPC_REQUEST_SIZE: usize = 0x100;

pub type IPCData = [u8; IPC_REQUEST_SIZE];

pub trait KClientSession: KSynchronizationObject {
    // TODO: Remove this, move to HLE Threads
    // The fact that this takes the entire kernel means that this is a hack!
    fn handle_sync_request<'a>(&self, ipc: IPCContext<'a>, kctx: &mut kernel::Kernel) {}
    // Sends the IPC request to the server, must be waited on with wait (from KSynchronizationObject)
    // Returns false if the remote end has been disconnected
    fn sync_request(&self, waker: Rc<kernel::Waker>, ct: &mut kernel::CommonThreadManager) -> bool { false }
}
try_into_trait!(client_session_vtable, KClientSession);

pub trait KServerSession: KSynchronizationObject {
    // Takes a handle to the thread calling recieve, translates IPC data from the client
    // into the IPC data slot of this thread.
    // Takes a kernel context because we need to lookup objects in the object manager,
    // modify handle tables of the source and destination process,
    // and access IPC data for both the client and server threads.
    // Returns false if the client must wait
    fn receive(&self, this_thread: kernel::ThreadIndex, kctx: &mut kernel::Kernel) -> bool;
    // Replies to an IPC request
    fn reply(&self, this_thread: kernel::ThreadIndex, kctx: &mut kernel::Kernel);
    // Called when the KClientSession goes away
    fn on_endpoint_closed(&self);
}
try_into_trait!(server_session_vtable, KServerSession);

pub trait KClientPort: KSynchronizationObject {
    // Caller must wait after this
    // Takes the given server end and sends it to the KServerPort
    fn create_session_to_port(&self, server_session: kernel::KTypedObject<KServerSession>, ct: &mut kernel::CommonThreadManager);
}
try_into_trait!(client_port_vtable, KClientPort);

pub trait KServerPort: KSynchronizationObject {
    fn accept_session(&self, ct: &mut kernel::CommonThreadManager) -> Option<kernel::KTypedObject<KServerSession>>; // Caller must sleep if None
}
try_into_trait!(server_port_vtable, KServerPort);

pub trait KTimer {
    fn set(&self, initial_ns: i64, interval: i64);
    fn cancel(&self);
    fn tick(&self, this: &kernel::KObjectRef, time_ns: u64, ct: &mut kernel::CommonThreadManager);
}
try_into_trait!(timer_vtable, KTimer);

pub trait KAddressArbiter {
    fn wait(&self, waker: Rc<kernel::Waker>, addr: u32);
    fn signal(&self, this: &kernel::KObjectRef, amount: i32, addr: u32, ct: &mut kernel::CommonThreadManager) -> bool; // true on reschedule, false otherwise
    fn remove_wakers_by_opaque_id(&self, id: u64);
}
try_into_trait!(addr_arb_vtable, KAddressArbiter);

pub trait KEvent {
    fn signal(&self, this: &kernel::KObjectRef, ct: &mut kernel::CommonThreadManager);
    fn clear(&self);
}
try_into_trait!(event_vtable, KEvent);

pub trait KThread: KSynchronizationObject {
    fn exit(&self, this: &kernel::KObjectRef, ct: &mut kernel::CommonThreadManager);
}
try_into_trait!(thread_vtable, KThread);

pub trait KCodeSet {
    fn create_process(&self, kctx: &mut kernel::Kernel); // -> kernel::KResult<kernel::Process>;
}
try_into_trait!(codeset_vtable, KCodeSet);

pub trait KMutex {
    fn release(&self, this: &kernel::KObjectRef, ct: &mut kernel::CommonThreadManager);
}
try_into_trait!(mutex_vtable, KMutex);
