use std::fmt;
use std::fmt::Debug;

#[derive(Copy, Clone)]
#[repr(C)]
pub union RegValue {
    unsigned: u32,
    signed: i32,
}

impl RegValue {
    pub fn u32(self) -> u32 {
        unsafe { self.unsigned }
    }

    pub fn i32(self) -> i32 {
        unsafe { self.signed }
    }
}

impl Into<u32> for RegValue {
    fn into(self) -> u32 {
        unsafe { self.unsigned }
    }
}

impl From<u32> for RegValue {
    fn from(value: u32) -> RegValue {
        RegValue { unsigned: value }
    }
}

impl Into<i32> for RegValue {
    fn into(self) -> i32 {
        unsafe { self.signed }
    }
}

impl From<i32> for RegValue {
    fn from(value: i32) -> RegValue {
        RegValue { signed: value }
    }
}

impl Debug for RegValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:X}", self.u32())
    }
}

pub trait Regs {
    fn r(&self, reg: u8) -> RegValue;
    fn sr(&mut self, reg: u8, value: RegValue);
    fn set_cpsr(&mut self, value: u32);
}

pub enum Protection {
    ReadOnly,
    ReadWrite,
    ReadExecute,
}

pub trait MemoryMap: Memory {
    fn map(&self, addr: u32, size: u32, name: &str, init: Option<&[u8]>, prot: Protection);
}

pub trait Memory {
    fn read(&self, addr: u32, bytes: &mut [u8]);
    fn write(&self, addr: u32, bytes: &[u8]);
}

pub trait GuestContext {
    type SavedContext: SavedGuestContext;
    type GuestError: Debug;
    type SvcHandler: SvcHandler;

    fn save(&self, out: &mut Self::SavedContext);
    fn restore(&mut self, saved: &Self::SavedContext, tls: u32);

    fn breakpoint(&mut self, addr: u32, thumb: bool);

    fn run(&mut self, resume: <Self::SvcHandler as SvcHandler>::KernelResume, kctx: &mut <Self::SvcHandler as SvcHandler>::KernelContext) -> Result<(), Self::GuestError>;
}

pub trait SavedGuestContext: Regs + 'static {
    fn zero() -> Self;
}

pub trait Guest<'a, T> where &'a T: GuestContext + MemoryMap + 'a {
    fn create<S: SvcHandler + 'static>(svc_handler: S) -> Self;
    fn inner(&'a self) -> &'a T;
}

pub enum SvcResult {
    Continue,
    Reschedule
}

pub trait SvcHandler {
    type KernelContext;
    type KernelResume;
    fn handle<R: Regs, M: MemoryMap>(&self, regs: &mut R, tls: u32, mem: M, kctx: &mut Self::KernelContext) -> SvcResult;
    fn handle_resume<R: Regs, M: MemoryMap>(&self, resume: Self::KernelResume, regs: &mut R, tls: u32, mem: M, kctx: &mut Self::KernelContext);
}
