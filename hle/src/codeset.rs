use std::fmt;
use std::borrow::Cow;
use std::cell::RefCell;

use crate::kernel;
use kernel::object::*;

// CodeSet used to spawn HLE threads
pub struct HLECodeSetImpl {
    main_thread: RefCell<Option<crate::ThreadHLE>>,
    process_name: Cow<'static, str>,
}

impl HLECodeSetImpl {
    pub fn new(main_thread: crate::ThreadHLE, process_name: Cow<'static, str>) -> Self {
        Self {
            main_thread: RefCell::new(Some(main_thread)),
            process_name
        }
    }
}

impl fmt::Debug for HLECodeSetImpl {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "CodeSetImpl({:?})", self.process_name)
    }
}

impl KCodeSet for HLECodeSetImpl {
    fn create_process(&self, kctx: &mut kernel::Kernel) {
        let (pid, _) = kctx.new_process(self.process_name.clone(), crate::ProcessHLE);

        let _ = kctx.new_thread(
            pid,
            self.main_thread.borrow_mut().take().expect("CodeSet already used")
        );
    }
}

impl KObjectData for HLECodeSetImpl {}
impl KObjectDebug for HLECodeSetImpl {}

// CodeSet used to spawn LLE threads

pub struct LLECodeSetSection {
    pub start_addr: u32,
    pub data: Vec<u8>,
}

impl fmt::Debug for LLECodeSetSection {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Section({} bytes at 0x{:X})", self.data.len(), self.start_addr)
    }
}

#[derive(Debug)]
pub struct LLECodeSetImpl {
    text: LLECodeSetSection,
    rodata: LLECodeSetSection,
    data: LLECodeSetSection,
    bss: u32, // bss comes directly after data
    process_name: Cow<'static, str>,
    program_id: u64,
}

impl LLECodeSetImpl {
    pub fn new(text: LLECodeSetSection, rodata: LLECodeSetSection, data: LLECodeSetSection, bss: u32, process_name: Cow<'static, str>, program_id: u64) -> Self {
        Self { text, rodata, data, bss, process_name, program_id }
    }
}

impl KObjectData for LLECodeSetImpl {}
impl KObjectDebug for LLECodeSetImpl {}
