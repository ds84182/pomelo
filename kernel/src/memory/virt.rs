use std::collections::BTreeMap;
use std::marker::PhantomData;

use super::Memory;

pub enum PageSpanKind<'a, M: Memory + 'a> {
    Normal {
        backing: M,
        phantom: PhantomData<&'a ()>,
    },
    // MMIO {

    // }
}

pub struct PageSpan<'a, M: Memory + 'a, T: Clone> {
    size: u32,
    kind: PageSpanKind<'a, M>,
    tag: T,
}

pub struct VirtualMemory<'a, M: Memory + 'a, T: Clone> {
    pages: BTreeMap<u32, PageSpan<'a, M, T>>,
}

struct MemoryLookup<T> {
    item: T,
    offset: u32, // Offset from start of item, in pages
}

impl<'a, M: Memory + 'a, T: Clone> VirtualMemory<'a, M, T> {
    pub fn new() -> Self {
        Self {
            pages: Default::default()
        }
    }

    fn lookup(&self, page: u32) -> Option<MemoryLookup<&PageSpan<M, T>>> {
        use std::ops::Bound::Included;
        let (found_page, found_item) = self.pages.range((Included(&0), Included(&page))).rev().next()?;
        if (found_page + found_item.size) > page {
            Some(MemoryLookup {
                item: found_item,
                offset: page - found_page
            })
        } else {
            None
        }
    }

    pub fn map_memory(&mut self, addr: u32, mem: M, tag: T) {
        let page_span = PageSpan {
            size: mem.size_in_pages(),
            kind: PageSpanKind::Normal {
                backing: mem,
                phantom: PhantomData,
            },
            tag,
        };

        self.pages.insert(addr.wrapping_shr(12), page_span);
    }
}

impl<'a, M: Memory + 'a, T: Clone> Memory for VirtualMemory<'a, M, T> {
    fn size_in_pages(&self) -> u32 {
        0 // Unsized
    }

    fn read(&self, mut addr: u32, bytes: &mut [u8]) {
        let mut rem = bytes.len() as u32;
        let mut page_offset = addr & 0xFFF;
        let mut slice_offset = 0u32;

        while rem > 0 {
            let MemoryLookup { item, offset } = self.lookup(addr.wrapping_shr(12)).expect("Unmapped memory read");
            let item_pages = item.size - offset; // Number of pages it can fulfill
            let item_bytes = (item_pages << 12) - page_offset;

            let max = if item_bytes > rem { rem } else { item_bytes };

            match &item.kind {
                PageSpanKind::Normal { backing, .. } => {
                    backing.read(addr, &mut bytes[(slice_offset as usize)..((slice_offset + max) as usize)]);
                }
            }

            addr = addr + max;
            slice_offset = slice_offset + max;
            rem = rem - max;

            page_offset = 0;
        }
    }
    
    fn write(&self, mut addr: u32, bytes: &[u8]) {
        let mut rem = bytes.len() as u32;
        let mut page_offset = addr & 0xFFF;
        let mut slice_offset = 0u32;

        while rem > 0 {
            let MemoryLookup { item, offset } = self.lookup(addr.wrapping_shr(12)).expect("Unmapped memory read");
            let item_pages = item.size - offset; // Number of pages it can fulfill
            let item_bytes = (item_pages << 12) - page_offset;

            let max = if item_bytes > rem { rem } else { item_bytes };

            match &item.kind {
                PageSpanKind::Normal { backing, .. } => {
                    backing.write((offset << 12) | (addr & 0xFFF), &bytes[(slice_offset as usize)..((slice_offset + max) as usize)]);
                }
            }

            addr = addr + max;
            slice_offset = slice_offset + max;
            rem = rem - max;

            page_offset = 0;
        }
    }
}
