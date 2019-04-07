#[repr(C)]
pub struct Header {
    sig: [u8; 0x100],
    magic: [u8; 4],
    content_size: u32,
    partition_id: u64,
    maker_code: u16,
    version: u16,
    verification_word: u32,
    program_id: u64,
    _0: [u8; 0x10],
    logo_region_hash: [u8; 0x20],
    product_code: [u8; 0x10],
    exheader_hash: [u8; 0x20],
    exheader_size: u32,
    _1: [u8; 4],
    flags: [u8; 8],
    plain_region_offset: u32,
    plain_region_size: u32,
    logo_region_offset: u32,
    logo_region_size: u32,
    exefs_offset: u32,
    exefs_size: u32,
    exefs_hash_region_size: u32,
    _2: [u8; 4],
    romfs_offset: u32,
    romfs_size: u32,
    romfs_hash_region_size: u32,
    _3: [u8; 4],
    exefs_superblock_hash: [u8; 0x20],
    romfs_superblock_hash: [u8; 0x20],
}

unsafe impl plain::Plain for Header {}

static_assertions::const_assert!(ncch_header_size; std::mem::size_of::<Header>() == 0x200);
