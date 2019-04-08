use std::cell::Cell;
use super::{Page, Memory};

pub struct PageSlice<'a>(&'a [Page]);

impl<'a> PageSlice<'a> {
    pub fn size_in_bytes(&self) -> u32 {
        self.size_in_bytes() * 0x1000
    }

    pub fn bytes(&self) -> &'a [Cell<u8>] {
        unsafe {
            std::slice::from_raw_parts(self.0.as_ptr() as *const Cell<u8>, self.0.len() * 0x1000)
        }
    }

    pub fn pages(&self) -> &'a [Page] {
        self.0
    }
}

impl<'a> From<&'a [Page]> for PageSlice<'a> {
    fn from(pages: &'a [Page]) -> Self {
        Self(pages)
    }
}

impl<'a> From<&'a Page> for PageSlice<'a> {
    fn from(page: &'a Page) -> Self {
        Self(std::slice::from_ref(page))
    }
}

impl<'a> Memory for PageSlice<'a> {
    fn size_in_pages(&self) -> u32 {
        self.0.len() as u32
    }

    fn read(&self, addr: u32, bytes: &mut [u8]) {
        let addr = addr as usize;
        let backing = &self.bytes()[addr..(addr + bytes.len())];
        for (a, b) in bytes.iter_mut().zip(backing.iter()) {
            *a = b.get();
        }
    }
    
    fn write(&self, addr: u32, bytes: &[u8]) {
        let addr = addr as usize;
        let backing = &self.bytes()[addr..(addr + bytes.len())];
        for (a, b) in bytes.iter().zip(backing.iter()) {
            b.set(*a);
        }
    }
}
