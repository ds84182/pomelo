#![feature(futures_api, async_await, await_macro)] // To use HLE threads

use ::pomelo_hle as hle;
use ::pomelo_kernel as kernel;
use ::pomelo_formats as formats;
use ::pomelo_guest as guest;
use ::pomelo_guest_dynarmic as guest_dynarmic;

fn main() {
    let mut kctx = kernel::Kernel::new(
        pomelo_hle::HLEHooks::new()
    );

    let (_srv_pid, srv_port) = hle::service::srv::start(&mut kctx);

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

                let text_size = exheader.sci.text_csi.size_in_bytes as usize;
                let rodata_size = exheader.sci.rodata_csi.size_in_bytes as usize;
                let data_size = exheader.sci.data_csi.size_in_bytes as usize;

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