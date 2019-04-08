use byteorder::{LE, ByteOrder};

mod phys;
mod page_slice;
mod virt;

pub mod kmm;

pub use {
    phys::PhysicalMemory,
    page_slice::PageSlice,
    virt::VirtualMemory,
    kmm::KMM,
};

#[repr(transparent)]
pub struct Page(pub [u8; 0x1000]);

pub trait Memory {
    fn size_in_pages(&self) -> u32;
    fn read(&self, addr: u32, bytes: &mut [u8]);
    fn write(&self, addr: u32, bytes: &[u8]);

    fn read_u8(&self, addr: u32) -> u8 {
        let mut data = 0;
        self.read(addr, std::slice::from_mut(&mut data));
        data
    }

    fn read_u16(&self, addr: u32) -> u16 {
        let mut data = [0u8; 2];
        self.read(addr, &mut data);
        LE::read_u16(&data)
    }

    fn read_u32(&self, addr: u32) -> u32 {
        let mut data = [0u8; 4];
        self.read(addr, &mut data);
        LE::read_u32(&data)
    }

    fn read_u64(&self, addr: u32) -> u64 {
        let mut data = [0u8; 8];
        self.read(addr, &mut data);
        LE::read_u64(&data)
    }

    fn write_u8(&self, addr: u32, value: u8) {
        self.write(addr, &[value]);
    }

    fn write_u16(&self, addr: u32, value: u16) {
        let mut data = [0u8; 2];
        LE::write_u16(&mut data, value);
        self.write(addr, &data);
    }

    fn write_u32(&self, addr: u32, value: u32) {
        let mut data = [0u8; 4];
        LE::write_u32(&mut data, value);
        self.write(addr, &data);
    }

    fn write_u64(&self, addr: u32, value: u64) {
        let mut data = [0u8; 8];
        LE::write_u64(&mut data, value);
        self.write(addr, &data);
    }
}
