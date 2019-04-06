// KObject custom trait VTable hack support features
#![feature(raw, specialization, const_transmute, const_raw_ptr_deref)]

pub mod ipc;
pub mod object;
mod object_manager;

use std::num::NonZeroU32;
use std::fmt;
use std::rc::{Rc, Weak};
use std::cell::{RefCell, Cell, Ref, RefMut};
use std::borrow::Cow;
use std::ops::Deref;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub use object::{KObject, KSynchronizationObject, KClientSession, KServerSession, KClientPort, KServerPort, KTimer, KAddressArbiter, KEvent, KThread, IPCData};

use object_manager::ObjectTable;

pub type KObjectRef = Rc<KObject>;
pub type KWeakObjectRef = Weak<KObject>;
pub type KObjectTable = ObjectTable<Handle, KObjectRef>;

use object::{KObjectData, KObjectDynCast};

pub struct KTypedObject<T: ?Sized>
where KObject: KObjectDynCast<T>
{
    // Needs to be a reference (and not the KObject itself) so we can safely take a reference for the lifetime of KTypedObject
    object: KObjectRef,
    ptr: *const T, // Reference borrowed from KObjectRef, alive as long at KObjectRef (which can't be represented)
}

impl<T: ?Sized> KTypedObject<T>
where KObject: KObjectDynCast<T>
{
    pub fn new(object: &KObjectRef) -> Option<Self> {
        let ptr: Option<*const T> = object.as_ref().dyn_cast();
        ptr.map(|ptr| {
            KTypedObject {
                object: object.clone(),
                ptr,
            }
        })
    }

    pub fn object(&self) -> &KObjectRef {
        &self.object
    }

    pub fn into_object(self) -> KObjectRef {
        self.object
    }

    pub fn cast<X: ?Sized>(this: Self) -> Option<KTypedObject<X>> where KObject: KObjectDynCast<X> {
        KTypedObject::new(&this.object)
    }

    pub fn downgrade(this: &Self) -> KWeakTypedObject<T> {
        KWeakTypedObject {
            object: Rc::downgrade(&this.object),
            ptr: this.ptr
        }
    }
}

impl<T: ?Sized> AsRef<T> for KTypedObject<T>
where KObject: KObjectDynCast<T>
{
    fn as_ref(&self) -> &T {
        unsafe { &*self.ptr }
    }
}

impl<T: ?Sized> Deref for KTypedObject<T>
where KObject: KObjectDynCast<T>
{
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.ptr }
    }
}

impl<T: ?Sized> Clone for KTypedObject<T>
where KObject: KObjectDynCast<T>
{
    fn clone(&self) -> Self {
        KTypedObject {
            object: self.object.clone(),
            ptr: self.ptr,
        }
    }
}

pub struct KWeakTypedObject<T: ?Sized>
where KObject: KObjectDynCast<T>
{
    // Needs to be a reference (and not the KObject itself) so we can safely take a reference for the lifetime of KTypedObject
    object: KWeakObjectRef,
    ptr: *const T, // Reference borrowed from KObjectRef, alive as long at KObjectRef (which can't be represented)
}

impl<T: ?Sized> KWeakTypedObject<T>
where KObject: KObjectDynCast<T>
{
    pub fn new(object: &KObjectRef) -> Option<Self> {
        let ptr: Option<*const T> = object.as_ref().dyn_cast();
        ptr.map(|ptr| {
            KWeakTypedObject {
                object: Rc::downgrade(object),
                ptr,
            }
        })
    }

    pub fn empty() -> Self {
        use std::mem;
        KWeakTypedObject {
            object: Weak::new(),
            ptr: unsafe { mem::zeroed() },
        }
    }

    pub fn object(&self) -> &KWeakObjectRef {
        &self.object
    }

    pub fn upgrade(&self) -> Option<KTypedObject<T>> {
        self.object.upgrade().map(|object| {
            KTypedObject {
                object,
                ptr: self.ptr,
            }
        })
    }
}

impl<T: ?Sized> Clone for KWeakTypedObject<T>
where KObject: KObjectDynCast<T>
{
    fn clone(&self) -> Self {
        KWeakTypedObject {
            object: self.object.clone(),
            ptr: self.ptr,
        }
    }
}

#[derive(Clone, Ord, PartialOrd, PartialEq, Eq)]
pub struct Handle(NonZeroU32);

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:X}", self.0.get())
    }
}

impl Handle {
    pub fn from_raw(value: u32) -> Option<Handle> {
        NonZeroU32::new(value).map(Handle)
    }

    pub fn raw(&self) -> u32 {
        self.0.get()
    }
}

pub trait HandleImpl {
    fn raw(&self) -> u32;
}

impl HandleImpl for Option<Handle> {
    fn raw(&self) -> u32 {
        self.as_ref().map(|handle| handle.raw()).unwrap_or(0)
    }
}

impl crate::object_manager::ObjectHandle for Handle {
    fn from(value: u32) -> Option<Self> {
        Handle::from_raw(value)
    }

    fn raw(&self) -> u32 {
        self.0.get()
    }
}

pub type KResult<T> = std::result::Result<T, u32>;

pub mod errors {
    pub const TIMEOUT_REACHED: u32 = 0x09401BFE;
    pub const HANDLE_TYPE_MISMATCH: u32 = 0xD8E007F7;
    pub const HANDLE_TYPE_MISMATCH_2: u32 = 0xD9001BF7;
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ResetType {
    OneShot,
    Sticky,
    Pulse
}

impl ResetType {
    pub fn from_raw(id: u32) -> ResetType {
        match id {
            0 => ResetType::OneShot,
            1 => ResetType::Sticky,
            2 => ResetType::Pulse,
            _ => unreachable!()
        }
    }
}

pub trait Waker {
    // Returns Ok(()) if wake up successful, Err(()) if the underlying thread has already been woken up
    // Panics if the underlying thread has been woken up and has already resumed (e.g this waker is invalid)
    fn wake(&self, object: &KObjectRef, ct: &mut CommonThreadManager) -> Result<(), ()>;

    // Checks this waker's internal opaque ID
    // Returns true if the given id matches, false otherwise.
    fn check_opaque_id(&self, id: u64) -> bool;

    // Gets the thread ID from this waker
    fn thread_id(&self) -> ThreadIndex;
}

pub struct Signal {
    reset_type: ResetType,
    signaled: bool,
}

impl Signal {
    pub fn new(reset_type: ResetType) -> Signal {
        Signal {
            reset_type,
            signaled: false,
        }
    }

    pub fn is_pulse(&self) -> bool {
        self.reset_type == ResetType::Pulse
    }

    pub fn is_sticky(&self) -> bool {
        self.reset_type == ResetType::Sticky
    }

    pub fn signal(&mut self) {
        self.signaled = true;
    }

    pub fn consume(&mut self) -> bool {
        if self.signaled {
            match self.reset_type {
                ResetType::OneShot | ResetType::Pulse => self.signaled = false,
                ResetType::Sticky => {}
            }
            true
        } else {
            false
        }
    }

    pub fn unconsume(&mut self) {
        // Was signaled, turned out we couldn't wake up the thread
        self.signaled = true;
    }

    pub fn reset(&mut self) {
        self.signaled = false;
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ArbitrationType {
    Signal,
    WaitIfLessThan,
    DecrementAndWaitIfLessThan,
    WaitIfLessThanTimeout,
    DecrementAndWaitIfLessThanTimeout,
}

#[derive(Debug)]
pub enum ArbitrationAction {
    Signal,
    Decrement
}

#[derive(Debug)]
pub enum ArbitrationFail {
    Wait,
    WaitTimeout
}

impl ArbitrationType {
    pub fn from_raw(x: u32) -> ArbitrationType {
        match x {
            0 => ArbitrationType::Signal,
            1 => ArbitrationType::WaitIfLessThan,
            2 => ArbitrationType::DecrementAndWaitIfLessThan,
            3 => ArbitrationType::WaitIfLessThanTimeout,
            4 => ArbitrationType::DecrementAndWaitIfLessThanTimeout,
            _ => unreachable!()
        }
    }

    pub fn arbitrate(self, memory_value: impl FnOnce() -> i32, value: i32) -> (Option<ArbitrationAction>, Result<(), ArbitrationFail>) {
        match self {
            ArbitrationType::Signal => (Some(ArbitrationAction::Signal), Ok(())),
            ArbitrationType::WaitIfLessThan => (None, if memory_value() < value { Err(ArbitrationFail::Wait) } else { Ok(()) }),
            ArbitrationType::DecrementAndWaitIfLessThan =>
                if memory_value() < value {
                    (Some(ArbitrationAction::Decrement), Err(ArbitrationFail::Wait))
                } else {
                    (None, Ok(()))
                },
            ArbitrationType::WaitIfLessThanTimeout => (None, if memory_value() < value { Err(ArbitrationFail::WaitTimeout) } else { Ok(()) }),
            ArbitrationType::DecrementAndWaitIfLessThanTimeout => 
                if memory_value() < value {
                    (Some(ArbitrationAction::Decrement), Err(ArbitrationFail::WaitTimeout))
                } else {
                    (None, Ok(()))
                },
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Timeout {
    Instant, // ns == 0
    After(u64),
    Forever
}

impl Timeout {
    pub fn absolute(&self, base: u64) -> Option<u64> {
        match self {
            Timeout::Instant => Some(0u64),
            Timeout::After(ns) => Some(*ns),
            Timeout::Forever => None,
        }.map(|ns| ns + base)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
// TODO: Rename to ThreadId, use u32
pub struct ThreadIndex(usize);

impl ThreadIndex {
    pub fn new(x: usize) -> ThreadIndex {
        ThreadIndex(x)
    }

    pub fn zero() -> ThreadIndex {
        ThreadIndex(0)
    }

    pub fn get(&self) -> usize {
        self.0
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum ThreadState {
    Running,
    Ready,
    Paused,
    Waiting,
    Done
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum ThreadWaitType {
    WaitSync1,
    WaitSyncN,
    Arbiter,
    Sleep,
}

#[derive(Debug)]
pub enum ThreadWakeReason<'a> {
    Timeout,
    Object(&'a KObjectRef),
}

#[derive(Debug)]
pub enum ThreadResume {
    Normal,
    TimeoutReached,
    WaitSyncN {
        index: usize,
    }
}

pub trait Thread {
    fn resume(&self, payload: ThreadResume, kctx: &mut Kernel);
    fn read_ipc_data(&self, data: &mut IPCData);
    fn write_ipc_data(&self, data: &IPCData);
}

pub struct CommonThread {
    pub index: ThreadIndex,
    pid: ProcessId,
    state: ThreadState,
    wait_objects: Vec<KObjectRef>,
    wake_key: u64,
    wait_timeout: Option<u64>,
    wait_type: Option<ThreadWaitType>,
    resume: ThreadResume,
    kobj: KTypedObject<KThread>,
    ipc_data: IPCData,
}

impl CommonThread {
    pub fn new(pid: ProcessId, tid: ThreadIndex, kobj: KTypedObject<KThread>) -> CommonThread {
        CommonThread {
            index: tid,
            pid,
            state: ThreadState::Ready,
            wait_objects: vec![],
            wake_key: 0,
            wait_timeout: None,
            wait_type: None,
            resume: ThreadResume::Normal,
            kobj,
            ipc_data: [0u8; 256],
        }
    }

    fn wait<'a>(&mut self, objects: impl Iterator<Item=&'a KObjectRef>) {
        assert!(self.state == ThreadState::Running);
        self.wait_objects.clear();
        self.wait_objects.extend(objects.map(|r| r.clone()));
        self.state = ThreadState::Waiting;
    }

    fn wake_up(&mut self, reason: ThreadWakeReason) {
        self.state = ThreadState::Ready;
        self.wake_key = self.wake_key.wrapping_add(1);

        self.resume = match (reason, self.wait_type.expect("Timeout occurred on thread with invalid wait_type")) {
            (ThreadWakeReason::Timeout, ThreadWaitType::Sleep) => {
                // Do nothing, this was supposed to happen
                ThreadResume::Normal
            },

            (ThreadWakeReason::Timeout, ThreadWaitType::Arbiter) |
            (ThreadWakeReason::Timeout, ThreadWaitType::WaitSync1) |
            (ThreadWakeReason::Timeout, ThreadWaitType::WaitSyncN) => {
                // Hit timeout, return Result
                ThreadResume::TimeoutReached
            },

            (ThreadWakeReason::Object(..), ThreadWaitType::Sleep) => unreachable!(),

            (ThreadWakeReason::Object(..), ThreadWaitType::Arbiter) |
            (ThreadWakeReason::Object(..), ThreadWaitType::WaitSync1) => {
                // Nothing to do, normal behavior is to resume
                ThreadResume::Normal
            },

            (ThreadWakeReason::Object(object), ThreadWaitType::WaitSyncN) => {
                // Find object index in wait_objects
                let index = self.wait_objects.iter()
                    .position(|other| (&**other as *const KObject) == (&**object as *const KObject))
                    .expect("Unknown handle woke up thread during WaitSyncN");
                ThreadResume::WaitSyncN { index }
            },
        };

        self.wait_objects.iter().for_each(|object| {
            let object: &KObject = &*object;
            let sync: Option<&KSynchronizationObject> = object.into();
            let arb: Option<&KAddressArbiter> = object.into();
            if let Some(sync) = sync {
                sync.remove_wakers_by_opaque_id(self.index.get() as u64);
            }
            if let Some(arb) = arb {
                arb.remove_wakers_by_opaque_id(self.index.get() as u64);
            }
        });
        self.wait_objects.clear();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProcessId(u32);

impl ProcessId {
    pub fn raw(&self) -> u32 {
        self.0
    }
}

pub struct CommonProcess {
    pid: ProcessId,
    name: Cow<'static, str>,
    objects: RefCell<KObjectTable>,
}

impl CommonProcess {
    pub fn new_handle(&self, obj: KObjectRef) -> Handle {
        self.objects.borrow_mut().insert(obj)
    }

    pub fn close_handle(&self, handle: &Handle) -> bool {
        self.objects.borrow_mut().remove(handle).is_some()
    }

    pub fn lookup_close_handle(&self, handle: &Handle) -> Option<KObjectRef> {
        self.objects.borrow_mut().remove(handle)
    }

    pub fn lookup_handle(&self, handle: &Handle) -> Option<KObjectRef> {
        self.objects.borrow().lookup(handle).map(|rc| rc.clone())
    }
}

pub struct ThreadInitializer {
    pub entrypoint: u32,
    pub stack_top: u32,
    pub arg: u32,
}

pub trait Process {
    fn init_thread(&self, init: ThreadInitializer) -> Box<Thread> {
        unimplemented!("Native threads not implemented for this process")
    }
}

pub type ProcessPair = (CommonProcess, Rc<Process>);

struct ExecutionInfo {
    current: Option<(ProcessId, ThreadIndex)>,
    next: Option<ThreadIndex>,
}

pub struct ObjectManager {
    objects: Vec<KWeakObjectRef>,
}

impl ObjectManager {
    fn new() -> ObjectManager {
        ObjectManager {
            objects: vec![],
        }
    }

    pub fn new_object<T: KObjectData>(&mut self, data: T) -> KObjectRef {
        let obj = Rc::new(KObject::new(data));
        self.objects.push(Rc::downgrade(&obj));
        obj
    }

    pub fn new_object_typed<T: KObjectData>(&mut self, data: T) -> KTypedObject<T> {
        KTypedObject::new(&self.new_object(data)).unwrap()
    }

    pub fn for_each_object(&self, mut func: impl FnMut(KObjectRef)) {
        self.objects.iter().for_each(|weak| {
            if let Some(obj) = weak.upgrade() {
                func(obj)
            }
        });
    }
}

pub struct CommonThreadManager {
    threads: Vec<CommonThread>,
}

impl CommonThreadManager {
    fn new() -> CommonThreadManager {
        CommonThreadManager {
            threads: vec![]
        }
    }

    fn add(&mut self, thread: CommonThread) {
        self.threads.push(thread)
    }

    fn next_thread(&self) -> Option<ThreadIndex> {
        let ready_threads: Vec<usize> = self.threads.iter()
            .filter(|thread| thread.state == ThreadState::Ready)
            .map(|thread| thread.index.0)
            .collect();
        // println!("Ready threads: {:?}", ready_threads);

        self.threads.iter()
            .filter(|thread| thread.state == ThreadState::Ready)
            .map(|thread| thread.index.clone())
            // .reduce(|a, b| a.priority < b.priority)
            .next()
    }

    fn tick_at(&mut self, time: u64) {
        for thread in self.threads.iter_mut() {
            if let (ThreadState::Waiting, Some(timeout)) = (thread.state, thread.wait_timeout) {
                if timeout <= time {
                    thread.wake_up(ThreadWakeReason::Timeout);
                }
            }
        }
    }

    fn find_thread(&self, tid: ThreadIndex) -> &CommonThread {
        self.threads.iter()
            .find(|thread| thread.index == tid)
            .expect("Couldn't find CommonThread")
    }

    fn find_thread_mut(&mut self, tid: ThreadIndex) -> &mut CommonThread {
        self.threads.iter_mut()
            .find(|thread| thread.index == tid)
            .expect("Couldn't find CommonThread")
    }

    fn find_thread_pair_mut(&mut self, (a, b): (ThreadIndex, ThreadIndex)) -> (&mut CommonThread, &mut CommonThread) {
        let mut iter = self.threads.iter_mut().filter(|thread| thread.index == a || thread.index == b);
        let mut thread_a = iter.next().expect("Couldn't find a thread");
        let mut thread_b = iter.next().expect("Couldn't find a thread");
        if thread_a.index == b {
            use std::mem::swap;
            swap(&mut thread_a, &mut thread_b);
        }
        (thread_a, thread_b)
    }

    fn wake_up_thread(&mut self, index: ThreadIndex, key: u64, object: &KObjectRef) -> bool {
        let  thread = self.find_thread_mut(index);

        if thread.wake_key == key {
            // Successful wakeup
            thread.wake_up(ThreadWakeReason::Object(object));

            true
        } else {
            // Can't wake up
            false
        }
    }
}

pub struct CommonProcessManager {
    processes: Vec<ProcessPair>
}

impl CommonProcessManager {
    pub fn new() -> CommonProcessManager {
        CommonProcessManager {
            processes: vec![],
        }
    }

    pub fn find_process(&self, pid: ProcessId) -> &Rc<Process> {
        self.processes.iter()
            .find(|(process, _)| process.pid == pid)
            .map(|(_, b)| b)
            .expect("Couldn't find Box<Process>")
    }

    fn find_common_process(&self, pid: ProcessId) -> &CommonProcess {
        self.processes.iter()
            .map(|(process, _)| process)
            .find(|process| process.pid == pid)
            .expect("Couldn't find Process")
    }

    fn push(&mut self, pair: ProcessPair) {
        self.processes.push(pair)
    }
}

pub struct ActiveInterrupts {
    active: Mutex<Vec<u32>>,
}

impl ActiveInterrupts {
    fn new() -> Arc<ActiveInterrupts> {
        Arc::new(ActiveInterrupts {
            active: Mutex::new(vec![]),
        })
    }

    pub fn activate(&self, interrupt: u32) {
        self.active.lock().unwrap().push(interrupt)
    }

    pub fn drain(&self, f: impl FnMut(u32)) {
        self.active.lock().unwrap().drain(..).for_each(f);
    }
}

pub trait HLEHooks {
    fn make_hle_thread_object(&mut self, tid: ThreadIndex, obj_man: &mut ObjectManager) -> KTypedObject<KThread>;
}

pub struct Kernel {
    clk: clocksource::Clocksource,
    pub start_time_ns: u64,
    hle_hooks: Box<HLEHooks>,
    objects: ObjectManager,
    processes: CommonProcessManager,
    pub threads: CommonThreadManager,
    thread_boxes: RefCell<Vec<(ThreadIndex, Option<Box<Thread>>)>>,
    exec_info: RefCell<ExecutionInfo>,
    next_pid: Cell<u32>,
    next_tid: Cell<u32>,
    ports: HashMap<[u8; 8], KTypedObject<KClientPort>>,
    interrupts: HashMap<u32, KTypedObject<KEvent>>,
    active_interrupts: Arc<ActiveInterrupts>,
}

impl Kernel {
    pub fn new(hle: Box<HLEHooks>) -> Kernel {
        let clk = clocksource::Clocksource::new();
        let start_time_ns = clk.time();
        Kernel {
            clk,
            start_time_ns,
            hle_hooks: hle,
            objects: ObjectManager::new(),
            processes: CommonProcessManager::new(),
            threads: CommonThreadManager::new(),
            thread_boxes: RefCell::new(vec![]),
            exec_info: RefCell::new(ExecutionInfo {
                current: None,
                next: None,
            }),
            next_pid: Cell::new(0),
            next_tid: Cell::new(0),
            ports: Default::default(),
            interrupts: Default::default(),
            active_interrupts: ActiveInterrupts::new(),
        }
    }

    pub fn new_object<T: KObjectData>(&mut self, data: T) -> KObjectRef {
        self.objects.new_object(data)
    }

    pub fn new_object_typed<T: KObjectData>(&mut self, data: T) -> KTypedObject<T> {
        self.objects.new_object_typed(data)
    }

    pub fn new_handle(&self, obj: KObjectRef) -> Handle {
        self.current_process().new_handle(obj)
    }

    pub fn new_object_handle<T: KObjectData>(&mut self, data: T) -> (KObjectRef, Handle) {
        let obj = self.new_object(data);
        (obj.clone(), self.current_process().new_handle(obj))
    }

    pub fn lookup_handle(&self, handle: &Handle) -> Option<KObjectRef> {
        self.current_process().lookup_handle(handle)
    }

    pub fn current_time(&self) -> u64 {
        self.clk.time()
    }

    pub fn new_process<T: Process + 'static>(&mut self, name: Cow<'static, str>, process: T) -> (ProcessId, Rc<T>) {
        let pid = self.next_pid.get();
        self.next_pid.set(pid + 1);

        let process = Rc::new(process);

        self.processes.push(
            (
                CommonProcess {
                    pid: ProcessId(pid),
                    name,
                    objects: RefCell::new(ObjectTable::new()),
                },
                process.clone()
            )
        );

        (ProcessId(pid), process)
    }

    pub fn new_thread(&mut self, pid: ProcessId, thread: impl Thread + 'static) -> (ThreadIndex, KObjectRef) {
        self.new_thread_boxed(pid, Box::new(thread))
    }

    fn new_thread_boxed(&mut self, pid: ProcessId, thread: Box<Thread>) -> (ThreadIndex, KObjectRef) {
        let tid = self.next_tid.get();
        self.next_tid.set(tid + 1);

        let thread_kobj = self.hle_hooks.make_hle_thread_object(ThreadIndex(tid as usize), &mut self.objects);
        let obj = thread_kobj.object().clone();

        self.threads.add(
            CommonThread::new(pid, ThreadIndex(tid as usize), thread_kobj)
        );

        self.thread_boxes.borrow_mut().push(
            (ThreadIndex(tid as usize), Some(thread))
        );

        (ThreadIndex(tid as usize), obj)
    }

    pub fn create_thread(&mut self, init: ThreadInitializer) -> (ThreadIndex, Handle) {
        let (pid, _) = self.current_pid_tid();

        let thread = self.find_process(pid.clone()).init_thread(init);

        let (index, object) = self.new_thread_boxed(pid, thread);

        (index, self.new_handle(object))
    }

    pub fn next_thread(&self) -> Option<ThreadIndex> {
        // Finds another thread to execute
        self.threads.next_thread()
    }

    pub fn schedule_next(&self, thread: ThreadIndex) {
        self.exec_info.borrow_mut().next = Some(thread);
    }

    // TODO: Only provide this via the "outer" interface for the main loop, want to prevent reentrant calls
    pub fn tick_at(&mut self, time: u64) {
        self.threads.tick_at(time);

        let threads = &mut self.threads;
        self.objects.for_each_object(|object| {
            let timer: Option<&KTimer> = (&*object).into();
            if let Some(timer) = timer {
                timer.tick(&object, time, threads);
            }
        });
    }

    // TODO: Only provide this via the "outer" interface for the main loop, want to prevent reentrant calls
    pub fn tick(&mut self) {
        self.tick_at(self.time());
        let interrupts = &self.interrupts;
        let threads = &mut self.threads;
        self.active_interrupts.drain(|interrupt| {
            if let Some(event) = interrupts.get(&interrupt) {
                event.signal(event.object(), threads);
            }
        });
    }

    // TODO: Only provide this via the "outer" interface for the main loop, want to prevent reentrant calls
    pub fn run_next(&mut self) {
        let index = {
            let mut ei = self.exec_info.borrow_mut();
            if let Some(next) = ei.next.take() {
                Some(next)
            } else {
                self.next_thread()
            }
        };

        match index {
            Some(index) => self.run_thread(index),
            None => {},
        }
    }

    fn take_thread_box(&self, tid: ThreadIndex) -> Box<Thread> {
        let mut r = RefMut::map(self.thread_boxes.borrow_mut(), |threads| threads.iter_mut().find(|(index, _)| *index == tid).map(|(_, thread)| thread).expect("Couldn't find Box<Thread>"));
        r.take().expect("Thread is locked outside, unavailable to kernel. This means we have reentrant thread execution")
    }

    fn replace_thread_box(&self, tid: ThreadIndex, thread: Box<Thread>) {
        let mut r = RefMut::map(self.thread_boxes.borrow_mut(), |threads| threads.iter_mut().find(|(index, _)| *index == tid).map(|(_, thread)| thread).expect("Couldn't find Box<Thread>"));
        if !r.is_none() {
            panic!("Replacing thread box when thread is already available to kernel")
        }
        *r = Some(thread);
    }

    pub fn find_process(&self, pid: ProcessId) -> &Rc<Process> {
        self.processes.find_process(pid)
    }

    fn run_thread(&mut self, mut index: ThreadIndex) {
        let resume = {
            let mut thread = self.threads.find_thread_mut(index.clone());
            assert_eq!(thread.state, ThreadState::Ready);
            thread.state = ThreadState::Running;
            use std::mem;
            mem::replace(&mut thread.resume, ThreadResume::Normal)
        };

        println!("> Thread {:X} from {}", index.get(), self.processes.find_common_process(self.threads.find_thread(index.clone()).pid.clone()).name);
        self.exec_info.borrow_mut().current = Some((self.threads.find_thread(index.clone()).pid.clone(), index.clone()));

        let thread = self.take_thread_box(index.clone());
        
        thread.resume(resume, self);

        self.replace_thread_box(index, thread);

        index = self.exec_info.borrow_mut().current.take().expect("Current execution thread is gone").1;

        let mut thread = self.threads.find_thread_mut(index.clone());
        if thread.state == ThreadState::Running {
            thread.state = ThreadState::Ready;
        }

        println!("< Thread {:X} {:?} from {}", index.get(), thread.state, self.processes.find_common_process(thread.pid.clone()).name);
    }

    pub fn current_pid_tid(&self) -> (ProcessId, ThreadIndex) {
        self.exec_info.borrow().current.clone().expect("No current thread")
    }

    pub fn current_thread(&self) -> CurrentThread {
        let thread_id = Ref::map(self.exec_info.borrow(), |info| info.current.as_ref().expect("No current thread"));
        let thread = self.threads.find_thread(thread_id.1.clone());
        // let thread_impl = self.find_thread_box(thread_id.1.clone());
        CurrentThread {
            thread_id,
            thread,
            // thread_impl,
        }
    }

    pub fn current_process(&self) -> CurrentProcess {
        let pid = Ref::map(self.exec_info.borrow(), |info| &info.current.as_ref().expect("No current process").0);
        let process = self.processes.find_common_process(pid.clone());
        CurrentProcess {
            pid,
            process,
        }
    }

    pub fn suspend_thread<'a>(&mut self, objects: impl Iterator<Item=&'a KObjectRef>, timeout: Option<u64>, wait_type: ThreadWaitType) -> Rc<Waker> {
        let (_, tid) = self.current_pid_tid();
        let mut thread = self.threads.find_thread_mut(tid);

        thread.wait(objects);
        thread.wait_timeout = timeout;
        thread.wait_type = Some(wait_type);

        Rc::new(WakerImpl {
            thread: thread.index.clone(),
            key: thread.wake_key,
        })
    }

    pub fn exit_thread(&mut self) {
        let kobj = {
            let (_, tid) = self.current_pid_tid();
            let mut thread = self.threads.find_thread_mut(tid);

            thread.state = ThreadState::Done;
            thread.kobj.clone()
        };
        kobj.exit(kobj.object(), &mut self.threads);
    }

    pub fn set_thread_ipc_data(&mut self, data: &IPCData) {
        let (_, tid) = self.current_pid_tid();
        let mut thread = self.threads.find_thread_mut(tid);

        thread.ipc_data = *data;
    }

    pub fn get_thread_ipc_data(&self, data: &mut IPCData) {
        let (_, tid) = self.current_pid_tid();
        let thread = self.threads.find_thread(tid);

        *data = thread.ipc_data;
    }

    // Translate IPC data from the current thread to the destination thread's IPC data slot
    pub fn translate_ipc(&mut self, src: ThreadIndex, dest: ThreadIndex, reply: bool) {
        println!("Translate from {:?} to {:?} (reply? {})", src, dest, reply);

        let (src_thread, thread) = self.threads.find_thread_pair_mut((src, dest));
        let pid = src_thread.pid.clone();
        let current_process = self.processes.find_common_process(pid);
        let process = self.processes.find_common_process(thread.pid.clone());

        use crate::ipc::{IPCReader, IPCWriter, IPCTranslateParameter, IPCHandles};

        let mut reader = if reply {
            IPCReader::new_response(&src_thread.ipc_data)
        } else {
            IPCReader::new_request(&src_thread.ipc_data)
        };

        println!("IPC Command Header: {:?}", reader.header());

        let mut writer = if reply {
            IPCWriter::new_response(reader.header().command_id, reader.result(), &mut thread.ipc_data)
        } else {
            IPCWriter::new_request(reader.header().command_id, &mut thread.ipc_data)
        };

        fn translate_handle(handle: Option<Handle>, moved: bool, src: &CommonProcess, dest: &CommonProcess) -> Option<Handle> {
            let handle = handle?;
            let kobj = if moved {
                src.lookup_close_handle(&handle)
            } else {
                src.lookup_handle(&handle)
            }?;
            Some(dest.new_handle(kobj))
        }

        // 1:1 translate normal parameters
        for _ in 0..reader.header().normal_parameter_count {
            writer.write_normal(reader.read_normal());
        }

        // Translate only handles for now
        while reader.has_more_translate_params() {
            let translated = match reader.read_translate() {
                IPCTranslateParameter::Handle { moved, calling_process, handles } => {
                    let handles = match handles {
                        IPCHandles::Single(handle) => IPCHandles::Single(translate_handle(handle, moved, current_process, process)),
                        IPCHandles::Multiple(mut vec) => {
                            for handle in vec.iter_mut() {
                                *handle = translate_handle(handle.take(), moved, current_process, process);
                            }
                            IPCHandles::Multiple(vec)
                        }
                    };

                    IPCTranslateParameter::Handle { moved, calling_process, handles }
                },
                param => unimplemented!("Translate parameter {:?} not implemented for translation", param)
            };

            writer.write_translate(&translated);
        }
    }

    pub fn time(&self) -> u64 { self.clk.time() - self.start_time_ns }

    pub fn register_port(&mut self, name: &[u8], port: KTypedObject<KClientPort>) {
        let mut short_name = [0u8; 8];
        short_name[0..(name.len())].copy_from_slice(name);
        self.ports.insert(short_name, port);
    }

    pub fn lookup_port(&self, name: &[u8]) -> Option<KTypedObject<KClientPort>> {
        if name.len() > 8 { return None }
        let mut short_name = [0u8; 8];
        short_name[0..(name.len())].copy_from_slice(name);
        self.ports.get(&short_name).map(|handle| handle.clone())
    }

    pub fn bind_interrupt(&mut self, name: u32, event: KTypedObject<KEvent>) {
        self.interrupts.insert(name, event);
    }

    pub fn unbind_interrupt(&mut self, name: u32) {
        self.interrupts.remove(&name);
    }

    pub fn active_interrupts(&self) -> Arc<ActiveInterrupts> {
        Arc::clone(&self.active_interrupts)
    }
}

pub struct CurrentThread<'a> {
    thread_id: Ref<'a, (ProcessId, ThreadIndex)>,
    thread: &'a CommonThread,
    // thread_impl: Ref<'a, Box<Thread>>,
}

impl<'a> Deref for CurrentThread<'a> {
    type Target = CommonThread;
    fn deref(&self) -> &CommonThread {
        &*self.thread
    }
}

impl<'a> CurrentThread<'a> {
    /*pub fn read_ipc_data(&self, data: &mut IPCData) {
        self.thread_impl.read_ipc_data(data);
    }

    pub fn write_ipc_data(&self, data: &IPCData) {
        self.thread_impl.write_ipc_data(data);
    }*/
}

pub struct CurrentProcess<'a> {
    pid: Ref<'a, ProcessId>,
    process: &'a CommonProcess,
}

impl<'a> Deref for CurrentProcess<'a> {
    type Target = CommonProcess;
    fn deref(&self) -> &CommonProcess {
        &*self.process
    }
}

struct WakerImpl {
    thread: ThreadIndex,
    key: u64,
}

impl Waker for WakerImpl {
    fn wake(&self, object: &KObjectRef, ct: &mut CommonThreadManager) -> Result<(), ()> {
        if ct.wake_up_thread(self.thread.clone(), self.key, object) {
            Ok(())
        } else {
            Err(())
        }
    }
    
    fn check_opaque_id(&self, id: u64) -> bool {
        self.thread.get() == (id as usize)
    }

    fn thread_id(&self) -> ThreadIndex {
        self.thread.clone()
    }
}

