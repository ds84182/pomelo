use super::{Memory, PageSlice, Page};

pub struct PhysicalMemory(Box<[Page]>);

impl PhysicalMemory {
    pub fn new(pages: u32) -> PhysicalMemory {
        let mut vec = Vec::<Page>::with_capacity(pages as usize);
        unsafe { vec.set_len(pages as usize); }
        unsafe {
            std::ptr::write_bytes(vec.as_mut_ptr() as *mut u8, 0, (pages as usize) * 0x1000);
        }
        PhysicalMemory(vec.into_boxed_slice())
    }

    pub fn slice(&self) -> PageSlice {
        PageSlice::from(&self.0[..])
    }
}

impl Memory for PhysicalMemory {
    fn size_in_pages(&self) -> u32 {
        self.slice().size_in_pages()
    }

    fn read(&self, addr: u32, bytes: &mut [u8]) {
        self.slice().read(addr, bytes);
    }
    
    fn write(&self, addr: u32, bytes: &[u8]) {
        self.slice().write(addr, bytes);
    }
}
