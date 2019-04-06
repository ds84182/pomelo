use byteorder::{ByteOrder, LittleEndian};

use std::{cell::RefCell, rc::Rc};

const USERLAND_STACK_BASE: u32 = 0x10000000;
const USERLAND_TLS_BASE: u32 = 0x1FF82000;

const STACK_SIZE: u32 = 0x00002000;

use ::pomelo_kernel as kernel;
use ::pomelo_hle as hle;

mod guest_abstraction;

pub use crate::guest_abstraction::*;

use kernel::{HandleImpl, ThreadWaitType};

use kernel::object::*;

struct ThreadContext<G: GuestContext, M: MemoryMap> {
    tls: u32,
    ctx: RefCell<G::SavedContext>,
    guest: Rc<RefCell<G>>,
    memory: Rc<M>,
}

impl<G: GuestContext<SvcHandler=impl SvcHandler<KernelContext=kernel::Kernel, KernelResume=kernel::ThreadResume>>, M: MemoryMap> kernel::Thread for ThreadContext<G, M> {
    fn resume(&self, payload: kernel::ThreadResume, kctx: &mut kernel::Kernel) {
        let mut context = self.guest.borrow_mut();

        {
            // Copy IPC data from kernel-readable thread area
            let mut ipc_data: IPCData = [0u8; 0x100];
            kctx.get_thread_ipc_data(&mut ipc_data);
            self.memory.write(self.tls + 0x80, &ipc_data);
        }

        self.store(&mut *context);

        context.run(payload, kctx).expect("Failed to run guest");

        self.load(&*context);
    }

    fn read_ipc_data(&self, data: &mut IPCData) {
        unimplemented!()
    }

    fn write_ipc_data(&self, data: &IPCData) {
        unimplemented!()
    }
}

impl<G: GuestContext, M: MemoryMap> ThreadContext<G, M> {
    pub fn new(guest: Rc<RefCell<G>>, memory: Rc<M>) -> ThreadContext<G, M> {
        ThreadContext {
            tls: 0,
            ctx: RefCell::new(G::SavedContext::zero()),
            guest,
            memory,
        }
    }

    pub fn set_arg(&mut self, v: u32) {
        self.ctx.borrow_mut().sr(0, v.into())
    }

    pub fn pc(&self) -> u32 {
        self.ctx.borrow().r(15).into()
    }

    pub fn set_pc(&mut self, v: u32) {
        self.ctx.borrow_mut().sr(15, v.into())
    }

    pub fn set_sp(&mut self, v: u32) {
        self.ctx.borrow_mut().sr(13, v.into())
    }

    pub fn set_cpsr(&mut self, v: u32) {
        self.ctx.borrow_mut().set_cpsr(v)
    }

    pub fn load(&self, guest: &G) {
        guest.save(&mut *self.ctx.borrow_mut());
    }

    pub fn store(&self, guest: &mut G) {
        guest.restore(&*self.ctx.borrow(), self.tls);
    }
}

struct ProcessTLS {
    free: Vec<u8>, // (offset >> 9)
    next_offset: u8, // in pages, (addr >> 12)
}

impl ProcessTLS {
    fn new() -> ProcessTLS {
        ProcessTLS {
            free: vec![],
            next_offset: 0,
        }
    }
}

pub struct ProcessContext<G: GuestContext + 'static, M: MemoryMap> {
    guest: Rc<RefCell<G>>,
    memory: Rc<M>,
    tls: RefCell<ProcessTLS>,
}

impl<G: GuestContext<SvcHandler=impl SvcHandler<KernelContext=kernel::Kernel, KernelResume=kernel::ThreadResume>> + 'static, M: MemoryMap + 'static> kernel::Process for ProcessContext<G, M> {
    fn init_thread(&self, init: kernel::ThreadInitializer) -> Box<kernel::Thread> {
        Box::new(self.new_thread(init.entrypoint, init.stack_top, init.arg))
    }
}

impl<G: GuestContext + 'static, M: MemoryMap> ProcessContext<G, M> {
    pub fn new(guest: &Rc<RefCell<G>>, memory_map: M) -> Self {
        ProcessContext {
            guest: Rc::clone(&guest),
            memory: Rc::new(memory_map),
            tls: RefCell::new(ProcessTLS::new()),
        }
    }

    fn allocate_tls(&self) -> u32 {
        let mut tls = self.tls.borrow_mut();
        if tls.free.is_empty() {
            // Allocate a page for TLS
            self.memory.map(USERLAND_TLS_BASE + ((tls.next_offset as u32) << 12), 0x1000, "tls", None, Protection::ReadWrite);

            tls.free.reserve(8);
            for offset in &[7, 6, 5, 4, 3, 2, 1, 0] {
                let addr = (tls.next_offset << 3) | offset;
                tls.free.push(addr)
            }

            tls.next_offset += 1;
        }

        (USERLAND_TLS_BASE as u32) + ((tls.free.pop().expect("Failed to allocate TLS") as u32) << 9)
    }

    fn new_thread(&self, entrypoint: u32, stack_top: u32, arg: u32) -> ThreadContext<G, M> {
        let mut thread_context = ThreadContext::new(self.guest.clone(), self.memory.clone());

        let tls = self.allocate_tls();

        thread_context.set_sp(stack_top);
        thread_context.tls = tls;
        thread_context.set_pc(entrypoint);
        thread_context.set_arg(arg);
        thread_context.set_cpsr(
            0b10000 | // User
            if (entrypoint & 1) != 0 { 0b100000 } else { 0 } // Thumb
        );

        thread_context
    }
}

pub struct ProcessSvcHandler;

fn get_svc(regs: &impl Regs, mem: &impl MemoryMap) -> u32 {
    let pc = regs.r(15).u32();
    let mut svc_bytes = [0u8; 4];
    mem.read(pc - 4, &mut svc_bytes[..]);
    LittleEndian::read_u32(&svc_bytes[..]) & 0xFFFFFF
}

impl SvcHandler for ProcessSvcHandler {
    type KernelContext = kernel::Kernel;
    type KernelResume = kernel::ThreadResume;

    fn handle<R: Regs, M: MemoryMap>(&self, regs: &mut R, tls: u32, mem: M, kctx: &mut kernel::Kernel) -> SvcResult {
        handle_svc(regs, tls, mem, kctx)
    }

    fn handle_resume<R: Regs, M: MemoryMap>(&self, resume: Self::KernelResume, regs: &mut R, tls: u32, mem: M, kctx: &mut Self::KernelContext) {
        let svc = get_svc(regs, &mem);

        match svc {
            0x4F => {
                println!("ReplyRecv resume");
                // Handle ReplyAndRecieve resumption
                let index_out = regs.r(0).u32();
                let handles = regs.r(1).u32();
                let handle_count = regs.r(2).u32();
                println!("{} {} {}", index_out, handles, handle_count);

                let mut vec = vec![0u8; (handle_count as usize) * 4];
                let handles = {
                    mem.read(handles, &mut vec);
                    // Translate the handles from u8 to Option<kernel::Handle> using an unsafe mutable borrow and transmute
                    use std::mem::transmute;
                    &mut (unsafe { transmute::<_, &mut [Option<kernel::Handle>]>(&mut vec[..]) })[0..(handle_count as usize)]
                    // We won't be running on a big endian system, but if we do, this data needs to be endian swapped
                };

                match hle::svc::reply_and_receive_post(kctx, handles, &resume) {
                    (index, res) => {
                        let mut index_data = [0u8; 4];
                        LittleEndian::write_u32(&mut index_data[..], index as u32);
                        mem.write(index_out, &index_data);

                        if let Err(err) = res {
                            regs.sr(0, err.into());
                        } else {
                            regs.sr(0, 0.into());
                        }
                    }
                }
            },
            _ => ()
        }

        match resume {
            kernel::ThreadResume::Normal => {},
            kernel::ThreadResume::TimeoutReached => {
                regs.sr(0, kernel::errors::TIMEOUT_REACHED.into());
            },
            kernel::ThreadResume::WaitSyncN { index } => {
                regs.sr(1, (index as u32).into());
            }
        }
    }
}

fn make_timeout(timeout_low: u32, timeout_high: u32) -> kernel::Timeout {
    let ns = (timeout_high as u64) << 32 | (timeout_low as u64);

    if ns & 0x8000000000000000 == 0 {
        if ns == 0 {
            kernel::Timeout::Instant
        } else {
            kernel::Timeout::After(ns)
        }
    } else {
        kernel::Timeout::Forever
    }
}

fn handle_svc(regs: &mut impl Regs, tls: u32, mem: impl MemoryMap, kctx: &mut kernel::Kernel) -> SvcResult {
    let svc = get_svc(regs, &mem);

    let mut result = SvcResult::Continue;

    match svc {
        0x01 => {
            let operation = regs.r(0).u32();
            let addr0 = regs.r(1).u32();
            let addr1 = regs.r(2).u32();
            let size = regs.r(3).u32();
            let permissions = regs.r(4).u32();

            println!("ControlMemory({:X}, {:X}, {:X}, {:X}, {:X})", operation, addr0, addr1, size, permissions);

            // Oblige, sorta kinda
            mem.map(addr0, size, "ControlMemory", None, Protection::ReadWrite);
            
            // Result
            regs.sr(0, 0.into());
        },
        0x02 => {
            let addr = regs.r(2).u32();
            println!("QueryMemory({:X})", addr);

            // TODO: Correct implementation of QueryMemory

            if addr >= USERLAND_STACK_BASE-STACK_SIZE && addr < USERLAND_STACK_BASE {
                println!("ASKING ABOUT THE STACK");
                // Result
                regs.sr(0, (0).into());
                // base_process_virtual_address
                regs.sr(1, (USERLAND_STACK_BASE-STACK_SIZE).into());
                // size
                regs.sr(2, (STACK_SIZE).into());
                // MemoryPermission
                regs.sr(3, (3).into());
                // MemoryState (DONT KNOW IF 3 IS CORRECT)
                regs.sr(4, (3).into());
                // PageFlags (DONT KNOW IF 0 IS CORRECT)
                regs.sr(5, (0).into());
            } else if addr > 0x08000000 && addr < USERLAND_STACK_BASE-STACK_SIZE {
                println!("ASKING ABOUT UNMAPPED PAGES BELOW THE STACK");
                // Result
                regs.sr(0, (0).into());
                // base_process_virtual_address (DONT KNOW IF 0 IS CORRECT)
                regs.sr(1, (0x08000000).into());
                // size
                regs.sr(2, (0x08000000-STACK_SIZE).into());
                // MemoryPermission
                regs.sr(3, (0).into());
                // MemoryState (DONT KNOW IF 0 IS CORRECT)
                regs.sr(4, (0).into());
                // PageFlags (DONT KNOW IF 0 IS CORRECT)
                regs.sr(5, (0).into());
            } else {
                unimplemented!()
            }
        },
        0x08 => {
            let thread_priority = regs.r(0).u32();
            let entrypoint = regs.r(1).u32();
            let arg = regs.r(2).u32();
            let stack_top = regs.r(3).u32();
            let processor_id = regs.r(4).u32();
            println!("CreateThread({:X}, {:X}, {:X}, {:X}, {:X})", thread_priority, entrypoint, arg, stack_top, processor_id);

            let (index, handle) = kctx.create_thread(kernel::ThreadInitializer {
                entrypoint: entrypoint as u32,
                stack_top: stack_top as u32,
                arg: arg as u32,
            });

            // Result
            regs.sr(0, (0).into());
            // Handle<KThread>
            regs.sr(1, handle.raw().into());

            kctx.schedule_next(index);
            
            result = SvcResult::Reschedule;
        },
        0x09 => {
            println!("ExitThread()");

            kctx.exit_thread();

            result = SvcResult::Reschedule;
        },
        0x0A => {
            let ns_low = regs.r(0).u32();
            let ns_high = regs.r(1).u32();
            println!("SleepThread({:X}, {:X})", ns_low, ns_high);

            let time = make_timeout(ns_low, ns_high);

            kctx.suspend_thread([].iter(), time.absolute(kctx.time()), ThreadWaitType::Sleep); // Put the thread to sleep
            result = SvcResult::Reschedule;
        },
        0x17 => {
            let event_type = kernel::ResetType::from_raw(regs.r(1).u32() as u32);
            println!("CreateEvent({:?})", event_type);

            let (res, event) = match hle::svc::create_event(kctx, event_type) {
                Ok(handle) => (0, handle.raw()),
                Err(res) => (res, 0),
            };

            // Result
            regs.sr(0, (0).into());
            // Handle<KEvent>
            regs.sr(1, event.into());
        },
        0x18 => {
            let handle = regs.r(0).u32();
            let handle = kernel::Handle::from_raw(handle);
            println!("SignalEvent({:?})", handle);

            let object = handle.as_ref().and_then(|h| kctx.lookup_handle(h));
            let event: Option<&KEvent> = object.as_ref().map(|rc| &**rc).and_then(|o| o.into());

            if let Some(event) = event {
                event.signal(object.as_ref().unwrap(), &mut kctx.threads);

                // Result
                regs.sr(0, (0).into());

                // TODO: Only reschedule if needed
                result = SvcResult::Reschedule;
            } else {
                // Result
                regs.sr(0, kernel::errors::HANDLE_TYPE_MISMATCH.into());
            }
        },
        0x19 => {
            let handle = regs.r(0).u32();
            let handle = kernel::Handle::from_raw(handle);
            println!("ClearEvent({:?})", handle);

            let mut stack_data = [0u8; 12];
            mem.read(regs.r(13).u32(), &mut stack_data[..]);
            println!("{:X?}", stack_data);

            let object = handle.as_ref().and_then(|h| kctx.lookup_handle(h));
            let event: Option<&KEvent> = object.as_ref().map(|rc| &**rc).and_then(|o| o.into());

            if let Some(event) = event {
                event.clear();

                // Result
                regs.sr(0, (0).into());
            } else {
                // Result
                regs.sr(0, kernel::errors::HANDLE_TYPE_MISMATCH.into());
            }
        },
        0x1A => {
            let event_type = kernel::ResetType::from_raw(regs.r(1).u32() as u32);
            println!("CreateTimer({:?})", event_type);

            let (res, timer) = match hle::svc::create_timer(kctx, event_type) {
                Ok(handle) => (0, handle.raw()),
                Err(res) => (res, 0),
            };

            // Result
            regs.sr(0, (0).into());
            // Handle<KTimer>
            regs.sr(1, timer.into());
        },
        0x1B => {
            let handle = regs.r(0).u32();
            let handle = kernel::Handle::from_raw(handle);
            let interval_low = regs.r(1).u32() as u64;
            let initial_low = regs.r(2).u32() as u64;
            let initial_high = regs.r(3).u32() as u64;
            let interval_high = regs.r(4).u32() as u64;
            println!("SetTimer({:?}, {:X}, {:X}, {:X}, {:X})", handle, initial_low, interval_low, initial_high, interval_high);

            let object = handle.as_ref().and_then(|h| kctx.lookup_handle(h));
            let timer: Option<&KTimer> = object.as_ref().map(|rc| &**rc).and_then(|o| o.into());

            if let Some(timer) = timer {
                timer.set((initial_low | (initial_high << 32)) as i64, (interval_low | (interval_high << 32)) as i64);

                // Result
                regs.sr(0, (0).into());
            } else {
                // Result
                regs.sr(0, kernel::errors::HANDLE_TYPE_MISMATCH.into());
            }
        },
        0x1C => {
            let handle = regs.r(0).u32();
            let handle = kernel::Handle::from_raw(handle);
            println!("CancelTimer({:?})", handle);

            let mut stack_data = [0u8; 12];
            mem.read(regs.r(13).u32(), &mut stack_data[..]);
            println!("{:X?}", stack_data);

            let object = handle.as_ref().and_then(|h| kctx.lookup_handle(h));
            let timer: Option<&KTimer> = object.as_ref().map(|rc| &**rc).and_then(|o| o.into());

            if let Some(timer) = timer {
                timer.cancel();

                // Result
                regs.sr(0, (0).into());
            } else {
                // Result
                regs.sr(0, kernel::errors::HANDLE_TYPE_MISMATCH.into());
            }
        },
        0x1E => {
            let addr = regs.r(1).u32();
            let size = regs.r(2).u32();
            let creator_permission = regs.r(3).u32();
            let other_permission = regs.r(0).u32();
            println!("CreateMemoryBlock({:X}, {:X}, {:X}, {:X})", addr, size, creator_permission, other_permission);

            // TODO: Impl correctly
            
            // Result
            regs.sr(0, (0).into());
            // Handle<KMemoryBlock>
            regs.sr(1, (0xA000).into());
        },
        0x1F => {
            let handle = regs.r(0).u32();
            let addr = regs.r(1).u32();
            let creator_permission = regs.r(2).u32();
            let other_permission = regs.r(3).u32();
            println!("MapMemoryBlock({:X}, {:X}, {:X}, {:X})", handle, addr, creator_permission, other_permission);
            
            // TODO: Impl correctly
            mem.map(addr, 0x22000, "MapMemoryBlock", None, Protection::ReadWrite);

            // Result
            regs.sr(0, (0).into());
        },
        0x21 => {
            println!("CreateAddressArbiter()");

            let (res, arbiter) = match hle::svc::create_address_arbiter(kctx) {
                Ok(handle) => (0, handle.raw()),
                Err(res) => (res, 0),
            };

            // Result
            regs.sr(0, res.into());
            // Handle<KAddressArbiter>
            regs.sr(1, arbiter.into());
        },
        0x22 => {
            let handle = regs.r(0).u32();
            let addr = regs.r(1).u32();
            let typ = kernel::ArbitrationType::from_raw(regs.r(2).u32());
            let value = regs.r(3).i32();
            let ns_low = regs.r(4).u32();
            let ns_high = regs.r(5).u32();

            let handle = kernel::Handle::from_raw(handle);
            println!("ArbitrateAddress({:?}, {:X}, {:?}, {:X}, {:X}, {:X})", handle, addr, typ, value, ns_low, ns_high);

            let object = handle.as_ref().and_then(|h| kctx.lookup_handle(h));
            let arb: Option<&KAddressArbiter> = object.as_ref().map(|rc| &**rc).and_then(|o| o.into());

            if let Some(arb) = arb {
                let mut data = [0u8; 4];
                mem.read(addr, &mut data[..]);
                let memory_value = LittleEndian::read_i32(&data);

                let (action, arb_result) = typ.arbitrate(|| memory_value, value);

                println!("{:?} {:?} {:X}", action, arb_result, memory_value);
                // let mut stack_data = [0u8; 64];
                // mem.read(regs.r(13).u32(), &mut stack_data[..]);
                // println!("{:X?}", &stack_data[..]);

                let mut request_reschedule = false; // TODO: Reschdule requests should be at kernel level, and should be granted at the end of interrupts

                match action {
                    Some(kernel::ArbitrationAction::Decrement) => {
                        LittleEndian::write_i32(&mut data, memory_value.wrapping_sub(1));
                        mem.write(addr, &data[..]);
                    },
                    Some(kernel::ArbitrationAction::Signal) => {
                        arb.signal(object.as_ref().unwrap(), value, addr, &mut kctx.threads);
                    },
                    None => ()
                }

                match arb_result {
                    Ok(()) => {},
                    Err(kernel::ArbitrationFail::Wait) => {
                        request_reschedule = true;

                        let rc = object.as_ref().unwrap(); // TODO: Elide this clone by changing how wakers are done
                        let waker = kctx.suspend_thread([rc].iter().map(|rc| rc.clone()), None, ThreadWaitType::Arbiter);

                        arb.wait(waker, addr as u32);
                    },
                    Err(kernel::ArbitrationFail::WaitTimeout) => unimplemented!("Timeouts NYI")
                }

                // Result
                regs.sr(0, (0).into());

                if request_reschedule {
                    result = SvcResult::Reschedule;
                }
            } else {
                // Result
                regs.sr(0, kernel::errors::HANDLE_TYPE_MISMATCH.into());
            }
        },
        0x23 => {
            let handle = regs.r(0).u32();
            let handle = kernel::Handle::from_raw(handle);
            println!("CloseHandle({:?})", handle);

            let ok = handle.map(|handle| kctx.current_process().close_handle(&handle)).unwrap_or(false);

            if ok {
                // Result
                regs.sr(0, (0).into());
            } else {
                // Result
                regs.sr(0, kernel::errors::HANDLE_TYPE_MISMATCH.into());
            }
        },
        0x24 => {
            let handle = regs.r(0).u32();
            let timeout_low = regs.r(2).u32();
            let timeout_high = regs.r(3).u32();

            let handle = kernel::Handle::from_raw(handle);
            println!("WaitSynchronization1({:?}, {:X}, {:X})", handle, timeout_low, timeout_high);

            let mut stack_data = [0u8; 32];
            mem.read(regs.r(13).u32(), &mut stack_data[..]);
            println!("{:X?}", stack_data);

            let object = handle.as_ref().and_then(|h| kctx.lookup_handle(h));
            let sync: Option<&KSynchronizationObject> = object.as_ref().map(|rc| &**rc).and_then(|o| o.into());

            let time = make_timeout(timeout_low, timeout_high);

            // TODO: if ns == 0, don't wait & don't reschedule

            if let Some(sync) = sync {
                let rc = object.as_ref().unwrap(); // TODO: Elide this clone by changing how wakers are done
                let waker = kctx.suspend_thread([rc].iter().map(|rc| rc.clone()), time.absolute(kctx.time()), ThreadWaitType::WaitSync1);

                sync.wait(rc, waker, &mut kctx.threads);

                // Result
                regs.sr(0, (0).into());

                result = SvcResult::Reschedule;
            } else {
                // Result
                regs.sr(0, kernel::errors::HANDLE_TYPE_MISMATCH.into());
            }
        },
        0x25 => {
            let handles = regs.r(1).u32();
            let handle_count = regs.r(2).u32();
            let wait_all = regs.r(3).u32();
            let timeout_low = regs.r(4).u32();
            let timeout_high = regs.r(0).u32();
            println!("WaitSynchronizationN({:X}, {:X}, {:X}, {:X}, {:X})", handles, handle_count, wait_all, timeout_low, timeout_high);

            let time = make_timeout(timeout_low, timeout_high);

            // TODO: Use smallvec to avoid allocations

            let mut vec = vec![0u8; (handle_count as usize) * 4];
            let handles = {
                mem.read(handles, &mut vec);
                // Translate the handles from u8 to Option<kernel::Handle> using an unsafe mutable borrow and transmute
                use std::mem::transmute;
                &mut (unsafe { transmute::<_, &mut [Option<kernel::Handle>]>(&mut vec[..]) })[0..(handle_count as usize)]
                // We won't be running on a big endian system, but if we do, this data needs to be endian swapped
            };
            println!("{:X?}", handles);

            // Now with the handles, we want to get the underlying kernel objects
            let objects: Vec<kernel::KObjectRef> = handles.iter()
                .map(|h| h.as_ref().and_then(|h| kctx.lookup_handle(h)).expect("null object in waitsync"))
                .collect();

            let absolute = time.absolute(kctx.time());

            let waker = kctx.suspend_thread(objects.iter(), absolute, ThreadWaitType::WaitSyncN);

            // And then translate the kernel objects into KSynchronizationObjects so we can wait on them
            // let mut ok = true;
            for object in objects.iter() {
                let sync: Option<&KSynchronizationObject> = (&**object).into();

                if let Some(sync) = sync {
                    sync.wait(object, waker.clone(), &mut kctx.threads);
                } else {
                    // This code path is bugged, disabled:
                    unimplemented!("waitsync with invalid objects");

                    // Result
                    // regs.sr(0, kernel::errors::HANDLE_TYPE_MISMATCH.into());
                    // ok = false;
                    // break
                }
            }

            let mut wifi_data = [0u8; 8];
            mem.read(0x1FF81060, &mut wifi_data);
            println!("Wifi Data: {:?}", wifi_data);

            // Result
            regs.sr(0, (0).into());

            // Handle Index
            regs.sr(1, (-1).into());

            result = SvcResult::Reschedule;
        },
        0x27 => {
            let handle = regs.r(1).u32();
            println!("DuplicateHandle({:X})", handle);

            // Result
            regs.sr(0, (0).into());
            // Handle
            regs.sr(1, (handle).into());
        },
        0x28 => {
            println!("GetSystemTick()");

            let ticks_float = kctx.time() as f64 * 0.268111856f64;
            let ticks = ticks_float as u64;

            println!("ticks: {:X} lr: {:X}", ticks, regs.r(14).u32());
            let mut stack_data = [0u8; 12];
            mem.read(regs.r(13).u32(), &mut stack_data[..]);
            println!("{:X?}", stack_data);

            // Low
            regs.sr(0, ((ticks & 0xFFFFFFFF) as u32).into());
            // High
            regs.sr(1, (((ticks & 0xFFFFFFFF00000000) >> 32) as u32).into());
        },
        0x2B => {
            let handle = regs.r(1).u32();
            let typ = regs.r(2).u32();
            println!("GetProcessInfo({:X}, {})", handle, typ);

            if handle == 0xFFFF8001 && typ == 20 {
                // Phys mem NYI
                // Result
                regs.sr(0, (0).into());
                // Output Value Low
                regs.sr(1, (0xA0000000u32).into());
                // Output Value High
                regs.sr(2, (0).into());
            } else {
                unimplemented!()
            }
        },
        0x2D => {
            let name_address = regs.r(1).u32();
            println!("ConnectToPort({:X})", name_address);
            let mut name = [0u8; 11];
            mem.read(name_address, &mut name[..]);
            let name = name.split(|b| *b == 0).next().unwrap();
            println!("{:?}", name);
            use std::{io, io::Write};
            io::stdout().write(name).unwrap();
            println!("");

            // Whatever lmao

            if name != b"srv:" {
                panic!("Looking up invalid port");
            }

            let port_object = kctx.lookup_port(name);
            let port = port_object.map(|obj| kctx.new_handle(obj.into_object()));

            let (res, handle) = match hle::svc::create_session_to_port(kctx, port) {
                Ok((reschedule, handle)) => {
                    if reschedule.is_some() {
                        result = SvcResult::Reschedule;
                    }

                    (0, Some(handle))
                },
                Err(err) => {
                    (err, None)
                }
            };

            // Result
            regs.sr(0, res.into());
            // Handle<KClientSession>
            regs.sr(1, handle.raw().into());
        },
        0x32 => {
            let handle = regs.r(0).u32();
            let handle = kernel::Handle::from_raw(handle);
            println!("SendSyncRequest({:?})", handle);

            // Copy command from process into kernel-readable thread area
            let mut ipc_command: IPCData = [0u8; 0x100];
            mem.read(tls + 0x80, &mut ipc_command);
            kctx.set_thread_ipc_data(&ipc_command);

            // Send sync request
            let res = match hle::svc::send_sync_request(kctx, handle) {
                Ok(reschedule) => {
                    if reschedule.is_some() {
                        result = SvcResult::Reschedule;
                    }
                    0
                },
                Err(err) => err,
            };

            // Result
            regs.sr(0, res.into());
        },
        0x4A => {
            let handle = regs.r(1).u32();
            let handle = kernel::Handle::from_raw(handle);
            println!("AcceptSession({:?})", handle);

            match hle::svc::accept_session(kctx, handle) {
                Ok(Ok(server_session)) => {
                    regs.sr(0, 0.into());
                    regs.sr(1, server_session.raw().into())
                },
                Ok(Err(..)) => panic!("Unimplemented"),
                Err(err) => {
                    regs.sr(0, err.into());
                }
            }
        },
        0x4F => {
            let index_out = regs.r(0).u32();
            let handles = regs.r(1).u32();
            let handle_count = regs.r(2).u32();
            let reply_target = regs.r(3).u32();
            println!("ReplyAndReceive({:X}, {:X}, {:X}, {:X})", index_out, handles, handle_count, reply_target);

            // Copy command from process into kernel-readable thread area
            let mut ipc_command: IPCData = [0u8; 0x100];
            mem.read(tls + 0x80, &mut ipc_command);
            kctx.set_thread_ipc_data(&ipc_command);

            let mut vec = vec![0u8; (handle_count as usize) * 4];
            let handles = {
                mem.read(handles, &mut vec);
                // Translate the handles from u8 to Option<kernel::Handle> using an unsafe mutable borrow and transmute
                use std::mem::transmute;
                &mut (unsafe { transmute::<_, &mut [Option<kernel::Handle>]>(&mut vec[..]) })[0..(handle_count as usize)]
                // We won't be running on a big endian system, but if we do, this data needs to be endian swapped
            };
            println!("{:X?}", handles);

            let reply_target = kernel::Handle::from_raw(reply_target);

            match hle::svc::reply_and_receive_pre(kctx, handles, reply_target) {
                Ok(Some(..)) => {
                    result = SvcResult::Reschedule;
                },
                Ok(None) => unreachable!(),
                Err(err) => {
                    regs.sr(0, err.into())
                }
            }
        },
        0x50 => {
            let name = regs.r(0).u32();
            let handle = kernel::Handle::from_raw(regs.r(1).u32());
            let priority = regs.r(2).u32();
            let high_active = regs.r(3).u32();
            println!("BindInterrupt({:X}, {:X?}, {:X}, {:X})", name, handle, priority, high_active);

            use self::kernel::KTypedObject;

            let object = handle.as_ref().and_then(|h| kctx.lookup_handle(h));
            let event: Option<KTypedObject<KEvent>> = object.as_ref().and_then(KTypedObject::new);

            let result = if let Some(event) = event {
                kctx.bind_interrupt(name, event);

                let kinterrupts = kctx.active_interrupts();

                use std::thread;
                thread::spawn(move || {
                    use std::{io::{Read, Write}, fs};
                    let mut uio0 = fs::OpenOptions::new().read(true).write(true).open("/dev/uio0").expect("Failed to open file");
                    let mut data = [0u8; 4];
                    loop {
                        uio0.write(&[1, 0, 0, 0]).expect("Failed to enable interrupts");
                        uio0.read(&mut data).expect("Failed to read interrupt");
                        println!("Got interrupt");
                        kinterrupts.activate(name);
                    }
                });

                0
            } else {
                kernel::errors::HANDLE_TYPE_MISMATCH
            };

            // use std::io;
            // let mut s = String::new();
            // io::stdin().read_line(&mut s).unwrap();

            // Result
            regs.sr(0, result.into());
        },
        0x51 => {
            let name = regs.r(0).u32();
            let event = regs.r(1).u32();
            println!("UnbindInterrupt({:X}, {:X})", name, event);

            // use std::io;
            // let mut s = String::new();
            // io::stdin().read_line(&mut s).unwrap();

            // Result
            regs.sr(0, (0).into());
        },
        0x53 | 0x54 => {
            let process = regs.r(0).u32();
            let addr = regs.r(1).u32();
            let size = regs.r(2).u32();
            println!("Store/FlushProcessDataCache({:X}, {:X}, {:X})", process, addr, size);
        },
        svc @ _ => {
            panic!("Unknown {:X}", svc);
        }
    }

    result
}
