#![feature(futures_api, async_await, await_macro)] // To use HLE threads

use ::pomelo_hle as hle;
use ::pomelo_kernel as kernel;
use ::pomelo_formats as formats;
use ::pomelo_guest as guest;
use ::pomelo_guest_dynarmic as guest_dynarmic;

#[macro_use]
extern crate pomelo_hle;

fn main() {
    let fcram = kernel::memory::PhysicalMemory::new(0x08000000 >> 12);
    let mut physmem = kernel::memory::VirtualMemory::new();

    physmem.map_memory(0x20000000, fcram.slice(), ());

    use kernel::memory::kmm;

    let mut kmm = kmm::KMM::new(&physmem);

    kmm.init_region(kmm::MemoryRegion::App, 0x20000000, 0x04000000 >> 12);
    kmm.init_region(kmm::MemoryRegion::System, 0x24000000, 0x02C00000 >> 12);
    kmm.init_region(kmm::MemoryRegion::Base, 0x26C00000, 0x01400000 >> 12);

    let mut kctx = kernel::Kernel::new(
        pomelo_hle::HLEHooks::new()
    );

    let (_srv_pid, srv_port) = hle::service::srv::start(&mut kctx);
    kctx.register_port(b"srv:", srv_port.clone()); // TODO: Register the port inside of srv

    use hle::service::helper::Service;

    AptU.start(&mut kctx);
    NdmU.start(&mut kctx);
    DSP.start(&mut kctx);
    CfgU.start(&mut kctx);
    FsUser.start(&mut kctx);

    let (pid, _) = kctx.new_process("hle_loader".into(), hle::ProcessHLE);

    let _ = kctx.new_thread(
        pid,
        hle::ThreadHLE::new(
            async move |mut context: hle::HLEContext| {
                let exheader_bytes = include_bytes!(r#"C:\Users\ds841\Documents\Homebrew\easygba\AOBT\exheader.bin"#);
                let code = include_bytes!(r#"C:\Users\ds841\Documents\Homebrew\easygba\AOBT\exefs\code.bin"#);

                use formats::exheader::*;
                let mut exheader = ExHeader::zero();
                exheader.copy_from_bytes(exheader_bytes);

                let text_size = (exheader.sci.text_csi.size_in_pages << 12) as usize;
                let rodata_size = (exheader.sci.rodata_csi.size_in_pages << 12) as usize;
                let data_size = (exheader.sci.data_csi.size_in_pages << 12) as usize;

                let codeset_info = hle::svc::CodeSetInfo {
                    name: "aobt",
                    text: hle::LLECodeSetSection {
                        start_addr: exheader.sci.text_csi.address,
                        data: Vec::from(&code[0..text_size]),
                    },
                    rodata: hle::LLECodeSetSection {
                        start_addr: exheader.sci.rodata_csi.address,
                        data: Vec::from(&code[text_size..(text_size + rodata_size)]),
                    },
                    data: hle::LLECodeSetSection {
                        start_addr: exheader.sci.data_csi.address,
                        data: Vec::from(&code[(text_size + rodata_size)..(text_size + rodata_size + data_size)]),
                    },
                    bss: exheader.sci.bss_size,
                    program_id: exheader.aci.arm11_local_caps.program_id,
                };

                // let codeset = hle::svc::create_codeset(context.kernel(), ).unwrap();

                // TODO: Use codeset and normal process spawning procedures

                let mut guest = guest_dynarmic::DynarmicGuest::new(guest::ProcessSvcHandler);

                let memory_map = guest.memory_map();

                {
                    use guest::{MemoryMap, Protection};

                    memory_map.map(
                        codeset_info.text.start_addr,
                        ((codeset_info.text.data.len() + 0xFFF) & !0xFFF) as u32,
                        ".text",
                        Some(&codeset_info.text.data),
                        Protection::ReadExecute,
                    );

                    memory_map.map(
                        codeset_info.rodata.start_addr,
                        ((codeset_info.rodata.data.len() + 0xFFF) & !0xFFF) as u32,
                        ".rodata",
                        Some(&codeset_info.rodata.data),
                        Protection::ReadOnly,
                    );

                    memory_map.map(
                        codeset_info.data.start_addr,
                        ((codeset_info.data.data.len() + 0xFFF) & !0xFFF) as u32,
                        ".data",
                        Some(&codeset_info.data.data),
                        Protection::ReadWrite,
                    );

                    memory_map.map(
                        codeset_info.data.start_addr + ((codeset_info.data.data.len() + 0xFFF) & !0xFFF) as u32,
                        (codeset_info.bss + 0xFFF) & !0xFFF,
                        ".bss",
                        None,
                        Protection::ReadWrite,
                    );

                    memory_map.map(
                        guest::USERLAND_STACK_BASE - exheader.sci.stack_size,
                        exheader.sci.stack_size,
                        "stack",
                        None,
                        Protection::ReadWrite,
                    );

                    // Misc

                    use byteorder::{LittleEndian, ByteOrder};

                    let mut cfg_mem = vec![0u8; 0x1000];

                    cfg_mem[0x2] = 0x34; // kernel_version_min
                    cfg_mem[0x3] = 0x2; // kernel_version_maj
                    LittleEndian::write_u64(&mut cfg_mem[0x8..], 0x0004013000008002); // ns_tid 
                    cfg_mem[0x10] = 0x2; // sys_core_ver
                    cfg_mem[0x14] = 0x1; // unit_info
                    cfg_mem[0x16] = 0x1; // prev_firm
                    LittleEndian::write_u32(&mut cfg_mem[0x18..], 0x0000F297); // ctr_sdk_ver
                    cfg_mem[0x62] = 0x34; // firm_version_min
                    cfg_mem[0x63] = 0x2; // firm_version_maj
                    cfg_mem[0x64] = 0x2; // firm_sys_core_ver
                    LittleEndian::write_u32(&mut cfg_mem[0x68..], 0x0000F297); // firm_ctr_sdk_ver

                    // APPMEMALLOC
                    LittleEndian::write_u32(&mut cfg_mem[0x40..], 64 * 1024 * 1024);

                    memory_map.map(0x1FF80000, 0x1000, "cfg_mem", Some(&cfg_mem), Protection::ReadOnly);

                    let mut shared_page = vec![0u8; 0x1000];

                    shared_page[0x4] = 0x1; // running_hw = product

                    memory_map.map(0x1FF81000, 0x1000, "shared_page", Some(&shared_page), Protection::ReadWrite);
                }

                let memory_map_rc = guest.memory_map_rc();

                use std::rc::Rc;
                use std::cell::RefCell;

                let guest = Rc::new(RefCell::new(guest));

                let (pid, process) = context.kernel().new_process(
                    "aobt".into(),
                    guest::ProcessContext::new(
                        &guest,
                        memory_map_rc,
                    ),
                );

                let _ = context.kernel().new_thread(
                    pid,
                    process.new_thread(
                        codeset_info.text.start_addr,
                        guest::USERLAND_STACK_BASE - 4,
                        0
                    )
                );
            }
        )
    );

    loop {
        kctx.run_next();
        kctx.tick();
    }
}

use kernel::ipc::*;

struct AptU;

impl AptU {
    async fn handle_ipc<'a>(&'a mut self, ipc: IPCContext<'a>, context: &'a mut hle::HLEContext) {
        let header = ipc.header();
        match header {
            IPCHeaderCode { command_id: 1, normal_parameter_count: 1, translate_parameter_size: 0 } => {
                let applet_attributes = ipc.request().read_normal();

                println!("APT:GetLockHandle({:X})", applet_attributes);

                // Dummy lock handle
                let lock = hle::svc::create_mutex(context.kernel(), false).unwrap();

                let mut res = ipc.response(0);
                res.write_normal(applet_attributes); // AppletAttr
                res.write_normal(0); // APT State
                // Lock Handle
                res.write_translate(&IPCTranslateParameter::Handle {
                    moved: true,
                    calling_process: false,
                    handles: IPCHandles::Single(Some(lock))
                });
            }
            IPCHeaderCode { command_id: 2, normal_parameter_count: 2, translate_parameter_size: 0 } => {
                println!("APT:Initialize({:X})", ipc.request().read_normal());

                let notification_event = hle::svc::create_event(context.kernel(), kernel::ResetType::OneShot).unwrap();
                let resume_event = hle::svc::create_event(context.kernel(), kernel::ResetType::Sticky).unwrap();
                
                // Resume the application
                hle::svc::signal_event(context.kernel(), Some(resume_event.clone())).unwrap();

                let mut res = ipc.response(0);
                // Notification Event Handle + Resume Event Handle
                res.write_translate(&IPCTranslateParameter::Handle {
                    moved: true,
                    calling_process: false,
                    handles: IPCHandles::Multiple(vec![
                        Some(notification_event),
                        Some(resume_event)
                    ])
                });
            }
            IPCHeaderCode { command_id: 3, normal_parameter_count: 1, translate_parameter_size: 0 } => {
                println!("APT:Enable({:X})", ipc.request().read_normal());

                ipc.response(0);
            }
            IPCHeaderCode { command_id: 13, normal_parameter_count: 2, translate_parameter_size: 0 } |
            IPCHeaderCode { command_id: 14, normal_parameter_count: 2, translate_parameter_size: 0 } => {
                let mut req = ipc.request();
                println!("APT:GlanceParameter({:X}, {:X})", req.read_normal(), req.read_normal());

                let mut res = ipc.response(0);

                res.write_normal(0); // Sender AppID
                res.write_normal(1); // Command (Wakeup)
                res.write_normal(0); // Actual Parameter Size
                // TODO: Move handle parameter, static buffer descriptor
            }
            IPCHeaderCode { command_id: 67, normal_parameter_count: 1, translate_parameter_size: 0 } => {
                println!("APT:NotifyToWait({:X})", ipc.request().read_normal());

                ipc.response(0);
            }
            IPCHeaderCode { command_id: 75, normal_parameter_count: 3, translate_parameter_size: 0 } => {
                println!("APT:AppletUtility({:X})", ipc.request().read_normal());

                ipc.response(0).write_normal(0);
            }
            _ => unimplemented!("APT:U: {:?}", header)
        }
    }
}

declare_service!(for AptU named ("APT:U") using handle_ipc);

struct NdmU;

impl NdmU {
    async fn handle_ipc<'a>(&'a mut self, ipc: IPCContext<'a>, context: &'a mut hle::HLEContext) {
        let header = ipc.header();
        match header {
            _ => {
                println!("ndm:u: {:?} (Stubbed)", header);
                ipc.response(0);
            }
        }
    }
}

declare_service!(for NdmU named ("ndm:u") using handle_ipc);

struct DSP;

impl DSP {
    async fn handle_ipc<'a>(&'a mut self, ipc: IPCContext<'a>, context: &'a mut hle::HLEContext) {
        let header = ipc.header();
        match header {
            _ => {
                println!("dsp::DSP: {:?} (Stubbed)", header);
                ipc.response(0);
            }
        }
    }
}

declare_service!(for DSP named ("dsp::DSP") using handle_ipc);

struct CfgU;

impl CfgU {
    async fn handle_ipc<'a>(&'a mut self, ipc: IPCContext<'a>, context: &'a mut hle::HLEContext) {
        let header = ipc.header();
        match header {
            IPCHeaderCode { command_id: 1, normal_parameter_count: 2, translate_parameter_size: 0 } => {
                let mut req = ipc.request();
                println!("cfg:u:GetConfigInfoBlk2({:X}, {:X}) (Stubbed)", req.read_normal(), req.read_normal());

                ipc.response(0);
            }
            _ => {
                panic!("cfg:u: {:?} (Stubbed)", header);
            }
        }
    }
}

declare_service!(for CfgU named ("cfg:u") using handle_ipc);

struct FsUser;

impl FsUser {
    async fn handle_ipc<'a>(&'a mut self, ipc: IPCContext<'a>, context: &'a mut hle::HLEContext) {
        let header = ipc.header();
        match header {
            IPCHeaderCode { command_id: 2145, normal_parameter_count: 1, translate_parameter_size: 2 } => {
                let mut req = ipc.request();
                println!("fs:USER:InitializeWithSDKVersion({:X})", req.read_normal());

                ipc.response(0);
            }
            IPCHeaderCode { command_id: 2146, normal_parameter_count: 1, translate_parameter_size: 0 } => {
                let mut req = ipc.request();
                println!("fs:USER:SetPriority({:X})", req.read_normal());

                ipc.response(0);
            }
            IPCHeaderCode { command_id: 2051, normal_parameter_count: 8, translate_parameter_size: 0 } => {
                let mut req = ipc.request();
                let (
                    transaction,
                    archive_id,
                    archive_path_type,
                    archive_path_size,
                    file_path_type,
                    file_path_size,
                    open_flags,
                    attributes
                ) = (
                    req.read_normal(),
                    req.read_normal(),
                    req.read_normal(),
                    req.read_normal(),
                    req.read_normal(),
                    req.read_normal(),
                    req.read_normal(),
                    req.read_normal(),
                );
                println!("fs:USER:OpenFileDirectly({:X}, {:X}, {:X}, {:X}, {:X}, {:X}, {:X}, {:X})",
                    transaction,
                    archive_id,
                    archive_path_type,
                    archive_path_size,
                    file_path_type,
                    file_path_size,
                    open_flags,
                    attributes
                );

                ipc.response(0);
            }
            _ => {
                panic!("fs:USER: {:?} (Stubbed)", header);
            }
        }
    }
}

declare_service!(for FsUser named ("fs:USER") using handle_ipc);
