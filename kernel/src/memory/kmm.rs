use super::Memory;

use parking_lot::Mutex;

pub const MAIN_RAM_BASE: u32 = 0x20000000;

pub struct KMM<'mem, M: Memory + 'mem> {
    mem: &'mem M,
    region_app: Mutex<RegionDesc>,
    region_system: Mutex<RegionDesc>,
    region_base: Mutex<RegionDesc>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MemoryRegion {
    App, System, Base
}

macro_rules! select_region {
    ($kmm:expr, $region:expr) => {
        {
            let kmm = $kmm;
            match $region {
                MemoryRegion::App => &kmm.region_app,
                MemoryRegion::System => &kmm.region_system,
                MemoryRegion::Base => &kmm.region_base,
            }
        }
    };
}

macro_rules! select_region_mut {
    ($kmm:expr, $region:expr) => {
        {
            match $region {
                MemoryRegion::App => &mut $kmm.region_app,
                MemoryRegion::System => &mut $kmm.region_system,
                MemoryRegion::Base => &mut $kmm.region_base,
            }
        }
    };
}

impl<'a, M: Memory + 'a> KMM<'a, M> {
    pub fn new(mem: &'a M) -> Self {
        KMM {
            mem,
            region_app: Default::default(),
            region_system: Default::default(),
            region_base: Default::default(),
        }
    }

    pub fn init_region(&mut self, region: MemoryRegion, addr: u32, pages: u32) {
        let region = Mutex::get_mut(select_region_mut!(self, region));

        let ptr = MemoryBlockHeaderPtr::from_ptr(addr).expect("Invalid pointer passed into init_region");

        region.start = addr;
        region.size = pages;

        region.first = Some(ptr);
        region.last = Some(ptr);

        ptr.write(self.mem, &MemoryBlockHeader {
            size: pages,
            next: None.into_unsafe(),
            prev: None.into_unsafe(),
        });
    }

    pub fn allocate_linear(&self, region: MemoryRegion, pages: u32) -> Option<u32> {
        let mut region = select_region!(self, region).lock();

        let mut memory_block_ptr = region.first;
        loop {
            memory_block_ptr = if let Some(memory_block_ptr) = memory_block_ptr {
                let memory_block = memory_block_ptr.read(self.mem);
            
                if memory_block.size >= pages {
                    let rem = memory_block.size - pages;

                    let new_block_ptr = if rem > 0 {
                        let new_block_ptr = memory_block_ptr.offset(pages).expect("Bad free page length");
                        new_block_ptr.write(self.mem, &MemoryBlockHeader {
                            size: rem,
                            next: memory_block.next,
                            prev: memory_block.prev,
                        });
                        Some(new_block_ptr)
                    } else {
                        None
                    };

                    if region.first == Some(memory_block_ptr) {
                        region.first = new_block_ptr;
                    }

                    if region.last == Some(memory_block_ptr) {
                        region.last = new_block_ptr;
                    }

                    break Some(memory_block_ptr.0);
                } else {
                    memory_block.next.assert_safe()
                }
            } else {
                break None
            }
        }
    }

    fn find_insertion_point(region: &RegionDesc, mem: &M, addr: MemoryBlockHeaderPtr) -> BlockInsertionPoint {
        let mut memory_block_ptr = region.first;
        let mut prev_block_ptr = None;
        loop {
            match memory_block_ptr {
                None => {
                    break match prev_block_ptr {
                        Some(prev) => BlockInsertionPoint::Between(prev, None),
                        None => BlockInsertionPoint::First
                    }
                },
                Some(ptr) if ptr > addr => {
                    break match prev_block_ptr {
                        Some(prev) => BlockInsertionPoint::Between(prev, Some(ptr)),
                        None => BlockInsertionPoint::First
                    }
                },
                Some(ptr) => {
                    prev_block_ptr = Some(ptr);
                    memory_block_ptr = ptr.read(mem).next.assert_safe();
                }
            }
        }
    }

    fn merge_right(region: &mut RegionDesc, mem: &M, mut ptr: MemoryBlockHeaderPtr, cur: &mut MemoryBlockHeader) -> bool {
        // Merge right
        let end = ptr.offset(cur.size);
        let cur_next = cur.next.assert_safe();
        if let (Some(end), Some(cur_next)) = (end, cur_next) {
            if cur_next == end {
                let next = cur_next.read(mem);
                cur.size += next.size;
                cur.next = next.next;

                // Merged, update new next's prev pointer
                if let Some(next_next_ptr) = cur.next.assert_safe() {
                    let mut next_next = next_next_ptr.read(mem);
                    next_next.prev = Some(ptr).into_unsafe();
                    next_next_ptr.write(mem, &next_next);
                } else {
                    // Merged with last block
                    region.last = Some(ptr);
                }

                true
            } else {
                false
            }
        } else {
            false
        }
    }

    fn coalesce(region: &mut RegionDesc, mem: &M, mut ptr: MemoryBlockHeaderPtr) {
        let mut cur = ptr.read(mem);
        
        if Self::merge_right(region, mem, ptr, &mut cur) {
            ptr.write(mem, &cur);
        }

        if let Some(cur_prev) = cur.prev.assert_safe() {
            let mut prev = cur_prev.read(mem);
            if Self::merge_right(region, mem, cur_prev, &mut prev) {
                cur_prev.write(mem, &prev);
            }
        }
    }

    pub fn free(&self, region: MemoryRegion, addr: u32, pages: u32) {
        let ptr = MemoryBlockHeaderPtr::from_ptr(addr).expect("Invalid pointer given to free");

        let mut region = select_region!(self, region).lock();

        let mut memory_block = MemoryBlockHeader {
            size: pages,
            next: None.into_unsafe(),
            prev: None.into_unsafe(),
        };

        match Self::find_insertion_point(&*region, self.mem, ptr) {
            BlockInsertionPoint::First => {
                memory_block.next = region.first.into_unsafe();
                region.first = Some(ptr);

                if region.last.is_none() {
                    region.last = Some(ptr);
                }
            },
            BlockInsertionPoint::Between(prev, next) => {
                memory_block.prev = Some(prev).into_unsafe();
                let mut prev_block = prev.read(self.mem);
                prev_block.next = Some(ptr).into_unsafe();
                prev.write(self.mem, &prev_block);

                if let Some(next) = next {
                    memory_block.next = Some(next).into_unsafe();
                    let mut next_block = next.read(self.mem);
                    next_block.prev = Some(ptr).into_unsafe();
                    next.write(self.mem, &next_block);
                } else {
                    region.last = Some(ptr);
                }
            },
        }

        ptr.write(self.mem, &memory_block);

        Self::coalesce(&mut *region, self.mem, ptr);
    }
}

enum BlockInsertionPoint {
    First,
    Between(MemoryBlockHeaderPtr, Option<MemoryBlockHeaderPtr>)
}

pub trait FromUnsafeData: Sized {
    type Raw;

    // Some: Data is valid and now reinterpreted
    // None: Data is invalid
    fn from_unsafe_data(data: Self::Raw) -> Option<Self>;

    fn into_unsafe(self) -> UnsafeData<Self>;
}

#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct UnsafeData<Safe: FromUnsafeData>(Safe::Raw);

impl<Safe: FromUnsafeData> UnsafeData<Safe> {
    pub fn new(value: Safe::Raw) -> Self {
        Self(value)
    }

    pub fn try_safe(self) -> Option<Safe> {
        Safe::from_unsafe_data(self.0)
    }

    pub fn assert_safe(self) -> Safe where Safe::Raw: std::fmt::Debug {
        self.try_safe().expect("Unsafe data detected")
    }
}

#[derive(Default)]
pub struct RegionDesc {
    first: Option<MemoryBlockHeaderPtr>,
    last: Option<MemoryBlockHeaderPtr>,
    start: u32,
    size: u32,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[rustc_layout_scalar_valid_range_start(1)]
#[rustc_layout_scalar_valid_range_end(0x000FFFFF)]
pub struct MemoryBlockHeaderPtr(u32);

impl MemoryBlockHeaderPtr {
    fn from_ptr(ptr: u32) -> Option<Self> {
        // Check page alignment
        if (ptr & 0xFFF) != 0 {
            return None
        }

        // Check null page
        if (ptr & !0xFFF) == 0 {
            return None
        }

        unsafe {
            Some(Self(ptr >> 12))
        }
    }

    fn offset(self, pages: u32) -> Option<MemoryBlockHeaderPtr> {
        let new_ptr = self.0.checked_add(pages)?;

        if (new_ptr & 0x000FFFFF) != new_ptr {
            // Overflow
            return None
        }

        unsafe {
            Some(Self(new_ptr))
        }
    }
}

impl FromUnsafeData for Option<MemoryBlockHeaderPtr> {
    type Raw = u32;

    fn from_unsafe_data(ptr: Self::Raw) -> Option<Self> {
        // Check page alignment
        if (ptr & 0xFFF) != 0 {
            return None // Fatal
        }

        // Check null page
        if (ptr & !0xFFF) == 0 {
            return Some(None)
        }

        unsafe {
            Some(Some(MemoryBlockHeaderPtr(ptr >> 12)))
        }
    }

    fn into_unsafe(self) -> UnsafeData<Self> {
        UnsafeData(self.map(|ptr| ptr.0 << 12).unwrap_or(0))
    }
}

pub struct MemoryBlockHeader {
    size: u32, // In pages
    next: UnsafeData<Option<MemoryBlockHeaderPtr>>,
    prev: UnsafeData<Option<MemoryBlockHeaderPtr>>,
}

impl MemoryBlockHeaderPtr {
    pub fn read(&self, mem: &impl Memory) -> MemoryBlockHeader {
        let mut data = [0u8; 3 * 4];
        mem.read(self.0 << 12, &mut data);

        use byteorder::{LE, ByteOrder};

        MemoryBlockHeader {
            size: LE::read_u32(&data[0..4]),
            next: UnsafeData::new(LE::read_u32(&data[4..8])),
            prev: UnsafeData::new(LE::read_u32(&data[8..12])),
        }
    }

    pub fn write(&self, mem: &impl Memory, header: &MemoryBlockHeader) {
        let mut data = [0u8; 3 * 4];

        use byteorder::{LE, ByteOrder};

        LE::write_u32_into(&[
            header.size,
            header.next.0,
            header.prev.0,
        ], &mut data);

        mem.write(self.0 << 12, &data);
    }
}
