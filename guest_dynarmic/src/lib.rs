use pomelo_guest::*;

use byteorder::{ByteOrder, LittleEndian};

use std::cell::{Cell, RefCell, RefMut};
use std::rc::Rc;

use dynarmic::{Executor, JitContext, memory::MemoryImpl, coproc};

use ::pomelo_kernel as kernel;

pub struct ThreadRegs {
    cpsr: u32,
    regs: [u32; 16],
    ext_regs: [u32; 64],
}

impl Regs for ThreadRegs {
    fn r(&self, reg: u8) -> RegValue {
        self.regs[(reg & 0xF) as usize].into()
    }

    fn sr(&mut self, reg: u8, value: RegValue) {
        self.regs[(reg & 0xF) as usize] = value.into();
    }

    fn set_cpsr(&mut self, value: u32) {
        self.cpsr = value;
    }
}

impl SavedGuestContext for ThreadRegs {
    fn zero() -> Self {
        ThreadRegs {
            cpsr: 0,
            regs: [Default::default(); 16],
            ext_regs: [Default::default(); 64],
        }
    }
}

#[derive(Default)]
struct ServiceContext<S: SvcHandler> {
    kctx: Cell<Option<*mut kernel::Kernel>>,
    svc_handler: S,
    ran_svc: Cell<bool>,
    tls: Cell<u32>,
}

struct DynarmicHandlers<S: SvcHandler> {
    mem: Rc<DynarmicMemory>,
    ctx: Rc<ServiceContext<S>>,
}

impl<S: SvcHandler> dynarmic::Handlers for DynarmicHandlers<S> {
    type Memory = DynarmicMemory;

    fn memory(&self) -> &Self::Memory {
        &self.mem
    }

    fn handle_svc(&mut self, context: JitContext, swi: u32) {
        let ctx = &self.ctx;

        let mut regs = ThreadRegs::zero();

        regs.cpsr = context.cpsr();
        regs.regs = *context.regs();
        regs.ext_regs = *context.extregs();

        let tls = ctx.tls.get();
        let kctx = unsafe { &mut *ctx.kctx.get().unwrap() };
        let svc_handler = &ctx.svc_handler;
        let result = svc_handler.handle(&mut regs, tls, self.mem.as_ref(), kctx);
        ctx.ran_svc.set(true);

        match result {
            SvcResult::Continue => (),
            SvcResult::Reschedule => {
                println!("Reschedule");
                context.halt();
            }
        }

        context.set_cpsr(regs.cpsr);
        *context.regs_mut() = regs.regs;
        *context.extregs_mut() = regs.ext_regs;
    }

    fn make_coprocessors<'jit>(&'jit mut self) -> Option<[Option<coproc::CoprocessorCallbacks<'jit>>; 16]> {
        let mut cp: [Option<coproc::CoprocessorCallbacks<'jit>>; 16] = Default::default();

        cp[15] = Some(coproc::CoprocessorCallbacks::callbacks_from(Box::new(Cp15 {
            ctx: Rc::clone(&self.ctx)
        })));

        Some(cp)
    }
}

pub struct DynarmicGuest<S: SvcHandler> {
    mem: Rc<DynarmicMemory>,
    exec: RefCell<Executor<DynarmicHandlers<S>>>,
    ctx: Rc<ServiceContext<S>>,
}

impl<S: SvcHandler> DynarmicGuest<S> {
    fn executor(&self) -> RefMut<Executor<DynarmicHandlers<S>>> {
        self.exec.borrow_mut()
    }
}

impl<'a, S: SvcHandler + 'static> DynarmicGuest<S> {
    pub fn new(svc_handler: S) -> Self {
        let ctx: Rc<ServiceContext<S>> = Rc::new(ServiceContext {
            kctx: Default::default(),
            svc_handler,
            ran_svc: Default::default(),
            tls: Cell::new(0),
        });

        let mem = Rc::new(DynarmicMemory(RefCell::new(MemoryImpl::new())));

        let mut exec = Executor::new(DynarmicHandlers {
            mem: Rc::clone(&mem),
            ctx: Rc::clone(&ctx),
        });

        {
            let context = exec.context();

            context.set_cpsr(0b10000);
            context.set_fpscr(0x03C00010);
            // context.set_fpexc(0x40000000);
        }

        DynarmicGuest {
            mem,
            exec: RefCell::new(exec),
            ctx,
        }
    }

    fn inner(&self) -> &Self {
        &self
    }

    pub fn memory_map(&mut self) -> &impl MemoryMap {
        self.mem.as_ref()
    }

    pub fn memory_map_rc(&mut self) -> Rc<impl MemoryMap> {
        Rc::clone(&self.mem)
    }
}

impl<S: SvcHandler> GuestContext for DynarmicGuest<S> {
    type SavedContext = ThreadRegs;
    type GuestError = ();
    type SvcHandler = S;

    fn save(&self, regs: &mut ThreadRegs) {
        let mut exec = self.executor();
        let ctx = exec.context();

        regs.cpsr = ctx.cpsr();
        regs.regs = *ctx.regs();
        regs.ext_regs = *ctx.extregs();
    }

    fn restore(&mut self, regs: &ThreadRegs, tls: u32) {
        let mut exec = self.executor();
        let ctx = exec.context();

        ctx.set_cpsr(regs.cpsr);
        *ctx.regs_mut() = regs.regs;
        *ctx.extregs_mut() = regs.ext_regs;
        self.ctx.tls.set(tls);
    }

    fn breakpoint(&mut self, addr: u32, thumb: bool) {

    }

    fn run(&mut self, resume: kernel::ThreadResume, kctx: &mut kernel::Kernel) -> Result<(), Self::GuestError> {
        if self.ctx.ran_svc.replace(false) {
            // Handle SVC resume

            let mut regs = ThreadRegs::zero();

            self.save(&mut regs);

            let tls: u32 = self.ctx.tls.get();

            let mem = self.mem.as_ref();
            self.ctx.svc_handler.handle_resume(resume, &mut regs, tls, mem, kctx);

            self.restore(&regs, tls);
        }

        let kctx_static = unsafe { std::mem::transmute::<_, &mut (dyn kernel::Kernel + 'static)>(kctx) };

        self.ctx.kctx.set(Some(kctx_static));

        loop {
            self.executor().run();

            if self.ctx.ran_svc.get() {
                break
            }
        }

        self.ctx.kctx.set(None);
        
        Ok(())
    }
}

struct Cp15<S: SvcHandler> {
    ctx: Rc<ServiceContext<S>>,
}

impl<'jit, S: SvcHandler> coproc::Coprocessor<'jit> for Cp15<S> {
    fn compile_get_one_word(&'jit self, two: bool, opc1: u32, cr_n: coproc::CoprocReg, cr_m: coproc::CoprocReg, opc2: u32) -> coproc::CallbackOrAccessOneWord<'jit> {
        coproc::CallbackOrAccess::Access(&self.ctx.tls)
    }
}

struct DynarmicMemory(RefCell<MemoryImpl>);

impl dynarmic::memory::Memory for DynarmicMemory {
    fn read<T: dynarmic::memory::Primitive>(&self, addr: u32) -> T {
        self.0.borrow().read(addr)
    }

    fn write<T: dynarmic::memory::Primitive>(&self, addr: u32, value: T) {
        self.0.borrow().write(addr, value)
    }

    fn is_read_only(&self, addr: u32) -> bool {
        self.0.borrow().is_read_only(addr)
    }
}

impl MemoryMap for DynarmicMemory {
    fn map(&self, addr: u32, size: u32, name: &str, init: Option<&[u8]>, prot: Protection) {
        println!("Mapped {} at {:X} ({:X} bytes)", name, addr, size);

        self.0.borrow_mut().map_memory(addr, size >> 12, match prot {
            Protection::ReadOnly => true,
            Protection::ReadExecute => true,
            _ => false
        });

        if let Some(data) = init {
            assert!(data.len() <= (size as usize));
            self.write(addr, &data);
        }
    }
}

impl Memory for DynarmicMemory {
    fn read(&self, addr: u32, bytes: &mut [u8]) {
        let mem = self.0.borrow();

        use dynarmic::memory::Memory;

        if (addr & 3) == 0 && (bytes.len() & 3) == 0 {
            // u32 alignment, accelerate reads
            for i in 0..(bytes.len() >> 2) {
                let v = mem.read::<u32>(addr + (i as u32 * 4));
                LittleEndian::write_u32(&mut bytes[(i * 4)..], v);
            }
        } else {
            // slow path
            for (i, byte) in bytes.iter_mut().enumerate() {
                *byte = mem.read(addr + i as u32);
            }
        }
    }

    fn write(&self, addr: u32, bytes: &[u8]) {
        let mem = self.0.borrow();

        use dynarmic::memory::Memory;

        if (addr & 3) == 0 && (bytes.len() & 3) == 0 {
            // u32 alignment, accelerate reads
            for i in 0..(bytes.len() >> 2) {
                let v = LittleEndian::read_u32(&bytes[(i * 4)..]);
                mem.write(addr + (i as u32 * 4), v);
            }
        } else {
            // slow path
            for (i, byte) in bytes.iter().enumerate() {
                mem.write(addr + i as u32, *byte);
            }
        }
    }
}
