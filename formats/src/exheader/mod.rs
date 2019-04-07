#[repr(C)]
pub struct ExHeader {
    pub sci: SystemControlInfo,
    pub aci: AccessControlInfo,
    pub access_desc_sig: [u8; 0x100],
    pub ncch_hdr_public_key: [u8; 0x100],
    pub acli: AccessControlInfo,
}

unsafe impl plain::Plain for ExHeader {}

impl ExHeader {
    pub fn zero() -> ExHeader {
        unsafe { std::mem::zeroed() }
    }

    pub fn copy_from_bytes(&mut self, b: &[u8]) {
        (self as &mut plain::Plain).copy_from_bytes(b).expect("ExHeader truncated");
    }
}

static_assertions::const_assert!(exheader_size; std::mem::size_of::<ExHeader>() == 0x800);

#[repr(C)]
pub struct SystemControlInfo {
    pub title: [u8; 8],
    _0: [u8; 5],
    pub flag: u8,
    pub remaster_version: u16,
    pub text_csi: CodeSetInfo,
    pub stack_size: u32,
    pub rodata_csi: CodeSetInfo,
    _1: [u8; 4],
    pub data_csi: CodeSetInfo,
    pub bss_size: u32,
    pub dependency_list: [[u8; 8]; 48],
    pub system_info: SystemInfo,
}

#[repr(C)]
pub struct CodeSetInfo {
    pub address: u32,
    pub size_in_pages: u32,
    pub size_in_bytes: u32,
}

#[repr(C)]
pub struct SystemInfo {
    pub savedata_size: u64,
    pub jump_id: u64,
    _0: [u8; 0x30],
}

#[repr(C)]
pub struct AccessControlInfo {
    pub arm11_local_caps: Arm11LocalCaps,
    pub arm11_kernel_caps: Arm11KernelCaps,
    pub arm9_access_control: Arm9AccessControl,
}

#[repr(C)]
pub struct Arm11LocalCaps {
    pub program_id: u64,
    pub core_version: u32,
    pub flag1: u8,
    pub flag2: u8,
    pub flag0: u8,
    pub priority: u8,
    pub resource_limits: [[u8; 2]; 16],
    pub storage_info: StorageInfo,
    pub service_access_control: [[u8; 8]; 32],
    pub extended_service_access_control: [[u8; 8]; 2],
    _0: [u8; 0xF],
    pub resource_limit_category: u8,
}

#[repr(C)]
pub struct StorageInfo {
    pub extdata_id: u64,
    pub system_savedata_id: u64,
    pub storage_accessible_unique_id: u64,
    pub flags: u64,
}

#[repr(C)]
pub struct Arm11KernelCaps {
    pub descriptors: [u32; 28],
    _0: [u8; 0x10],
}

#[repr(C)]
pub struct Arm9AccessControl {
    pub descriptors: [u8; 15],
    pub descriptor_version: u8,
}
