use crate::kernel;
use crate as hle;
use hle::svc;
use kernel::KernelExt;
use kernel::object::*;
use kernel::ipc::*;

pub fn start(kctx: &mut kernel::Kernel) -> (kernel::ProcessId, kernel::KTypedObject<KClientPort>) {
    let (pid, _) = kctx.new_process(
        "srv".into(),
        hle::ProcessHLE,
    );

    // In a correct implementation, we would create a global port named srv:
    // For now, create a session pair and return the client side

    let (server, client) = hle::make_port_pair(kctx);

    kctx.new_thread(
        pid.clone(),
        hle::ThreadHLE::new(
            async move |mut context: hle::HLEContext| {
                println!("HLE srv: startup");

                let mut srv = SrvContext {
                    services: vec![],
                    seen_notifications: vec![],
                };

                let server_handle = context.kernel().new_handle(server.into_object());

                let mut handles = vec![];
                handles.push(Some(server_handle.clone()));

                let mut reply_target = None;
                let mut data = [0u8; 256];

                loop {
                    let (index, res) = await!(context.reply_and_receive(&handles, reply_target.take(), &mut data));
                    match (index, res) {
                        (None, Ok(())) => unreachable!(),
                        (None, Err(err)) => panic!("Failed somewhere: {:X}", err),
                        (Some(0), Ok(())) => {
                            // New client, accept session
                            let server_session = await!(context.accept_session(&server_handle)).unwrap();
                            handles.push(Some(server_session));
                        },
                        (Some(index), Ok(())) => {
                            await!(srv.handle_ipc(IPCContext::new(&mut data[..]), &mut context));

                            reply_target = handles[index].clone();
                        },
                        (Some(index), Err(err)) => {
                            panic!("Disconnect? {} {:X}", index, err)
                        }
                    }
                }
            }
        )
    );

    (pid, client)
}

pub struct SrvInterface(kernel::Handle);

impl SrvInterface {
    pub fn new(handle: kernel::Handle) -> SrvInterface {
        SrvInterface(handle)
    }

    pub async fn register_service<'a>(&'a self, context: &'a mut hle::HLEContext, name: &'a [u8], max_sessions: u32) -> kernel::KResult<kernel::Handle> {
        if name.len() > 8 {
            panic!("Invalid name given: {:X?}", name);
        }

        let (name_a, name_b) = {
            let mut raw_name = [0u8; 8];
            (&mut raw_name[0..name.len()]).copy_from_slice(name);

            use byteorder::{LittleEndian, ByteOrder};
            (LittleEndian::read_u32(&raw_name[0..4]), LittleEndian::read_u32(&raw_name[4..8]))
        };

        let mut ipc_data = [0u8; 256];
        {
            let mut ipc_writer = IPCWriter::new_request(3, &mut ipc_data[..]);
            ipc_writer.write_normal(name_a);
            ipc_writer.write_normal(name_b);
            ipc_writer.write_normal(name.len() as u32);
            ipc_writer.write_normal(max_sessions);
        }

        if await!(context.send_sync_request(&self.0, &mut ipc_data)).is_ok() {
            let mut ipc_reader = IPCReader::new_response(&mut ipc_data[..]);
            let res = ipc_reader.result();
            if res != 0 {
                return Err(res)
            }
            match ipc_reader.read_translate() {
                IPCTranslateParameter::Handle { handles: IPCHandles::Single(handle), .. } => return Ok(handle.unwrap()),
                _ => panic!("Got invalid response from SRV:GetServiceHandle")
            }
        } else {
            panic!("Other end disconnected")
        }
    }

    pub async fn get_service_handle<'a>(&'a self, context: &'a mut hle::HLEContext, name: &'a [u8], no_wait: bool) -> kernel::KResult<kernel::Handle> {
        if name.len() > 8 {
            panic!("Invalid name given: {:X?}", name);
        }

        let (name_a, name_b) = {
            let mut raw_name = [0u8; 8];
            (&mut raw_name[0..name.len()]).copy_from_slice(name);

            use byteorder::{LittleEndian, ByteOrder};
            (LittleEndian::read_u32(&raw_name[0..4]), LittleEndian::read_u32(&raw_name[4..8]))
        };

        let mut ipc_data = [0u8; 256];
        {
            let mut ipc_writer = IPCWriter::new_request(5, &mut ipc_data[..]);
            ipc_writer.write_normal(name_a);
            ipc_writer.write_normal(name_b);
            ipc_writer.write_normal(name.len() as u32);
            ipc_writer.write_normal(if no_wait { 1 } else { 0 });
        }

        if await!(context.send_sync_request(&self.0, &mut ipc_data)).is_ok() {
            let mut ipc_reader = IPCReader::new_response(&mut ipc_data[..]);
            let res = ipc_reader.result();
            if res != 0 {
                return Err(res)
            }
            match ipc_reader.read_translate() {
                IPCTranslateParameter::Handle { handles: IPCHandles::Single(handle), .. } => return Ok(handle.unwrap()),
                _ => panic!("Got invalid response from SRV:GetServiceHandle")
            }
        } else {
            panic!("Other end disconnected")
        }
    }
}

struct SrvContext {
    services: Vec<Service>,
    seen_notifications: Vec<u32>,
}

struct Service {
    name: [u8; 8],
    name_len: u8,
    max_sessions: u32,
    client_port: kernel::Handle,
}

impl SrvContext {
    fn find_service(&self, name: [u8; 8], name_len: u8) -> Option<&Service> {
        self.services.iter().find(|service| {
            service.name_len == name_len && (&service.name[0..service.name_len as usize]) == (&name[0..name_len as usize])
        })
    }

    fn register_service(&mut self, service: Service) {
        self.services.push(service)
    }

    async fn handle_ipc<'a>(&'a mut self, ipc: IPCContext<'a>, context: &'a mut hle::HLEContext) {
        let header = ipc.header();
        match header {
            IPCHeaderCode { command_id: 1, normal_parameter_count: 0, translate_parameter_size: 2 } => {
                // Don't care about reading parameters, just write them
                ipc.response(0);
            },
            IPCHeaderCode { command_id: 2, normal_parameter_count: 0, translate_parameter_size: 0 } => {
                let mut response = ipc.response(0);
                // Incorrect handle type (should be Semaphore), but w/e
                let event = svc::create_event(context.kernel(), kernel::ResetType::OneShot).unwrap();
                // svc::signal_event(context.kernel(), Some(event.clone())).unwrap();
                response.write_translate(&IPCTranslateParameter::Handle {
                    moved: true,
                    calling_process: false,
                    handles: IPCHandles::Single(Some(event))
                });
            },
            IPCHeaderCode { command_id: 3, normal_parameter_count: 4, translate_parameter_size: 0 } => {
                use byteorder::{LittleEndian, ByteOrder};

                let mut name = [0u8; 8];
                let name_length;
                let max_sessions;

                {
                    let mut request = ipc.request();
                    LittleEndian::write_u32(&mut name[0..4], request.read_normal());
                    LittleEndian::write_u32(&mut name[4..8], request.read_normal());
                    name_length = request.read_normal();
                    max_sessions = request.read_normal();
                }

                println!("srv::RegisterService({:X?}, {}, {})", name, name_length, max_sessions);

                let (server_port, client_port) = hle::make_port_pair(context.kernel());
                let (server_port, client_port) = (
                    context.kernel().new_handle(server_port.into_object()),
                    context.kernel().new_handle(client_port.into_object())
                );

                // Register the server with the client_port and return the server_port
                self.register_service(Service {
                    name,
                    name_len: name_length as u8,
                    max_sessions,
                    client_port
                });

                ipc.response(0).write_translate(&IPCTranslateParameter::Handle {
                    moved: true,
                    calling_process: false,
                    handles: IPCHandles::Single(Some(server_port))
                })
            },
            IPCHeaderCode { command_id: 5, normal_parameter_count: 4, translate_parameter_size: 0 } => {
                use byteorder::{LittleEndian, ByteOrder};
                
                let mut name = [0u8; 8];
                let name_length;
                let flags;

                {
                    let mut request = ipc.request();
                    LittleEndian::write_u32(&mut name[0..4], request.read_normal());
                    LittleEndian::write_u32(&mut name[4..8], request.read_normal());
                    name_length = request.read_normal();
                    flags = request.read_normal();
                }

                println!("srv::GetServiceHandle({:X?}, {}, {})", name, name_length, flags);

                let service = self.find_service(name, name_length as u8).expect("Service not found");

                let session = await!(context.create_session_to_port(&service.client_port)).unwrap();

                ipc.response(0).write_translate(&IPCTranslateParameter::Handle {
                    moved: true,
                    calling_process: false,
                    handles: IPCHandles::Single(Some(session))
                })
            },
            IPCHeaderCode { command_id: 9, normal_parameter_count: 1, translate_parameter_size: 0 } => {
                // 260, 259, 261, 263, 518
                // 104, 103, 105, 107, 206
                let notification = ipc.request().read_normal();
                println!("srv::Subscribe({:X})", notification);
                self.seen_notifications.push(notification);
                ipc.response(0);
            },
            IPCHeaderCode { command_id: 11, normal_parameter_count: 0, translate_parameter_size: 0 } => {
                println!("srv::ReceiveNotification()");
                let mut response = ipc.response(0);
                response.write_normal(0x206); // next try 0x105
            },
            IPCHeaderCode { command_id: 12, normal_parameter_count: 2, translate_parameter_size: 0 } => {
                let (notification, flags) = {
                    let mut req = ipc.request();
                    (req.read_normal(), req.read_normal())
                };
                println!("srv::PublishToSubscriber({:X}, {:X})", notification, flags);
                if self.seen_notifications.contains(&notification) {
                    panic!("Not implemented, (seen: {:X?})", self.seen_notifications);
                }
                ipc.response(0);
            },
            IPCHeaderCode { command_id: 13, normal_parameter_count: 1, translate_parameter_size: 0 } => {
                let notification = ipc.request().read_normal();
                println!("srv::PublishAndGetSubscriber({:X})", notification);
                let mut response = ipc.response(0);
                if self.seen_notifications.contains(&notification) {
                    panic!("Not implemented, (seen: {:X?})", self.seen_notifications);
                }
                response.write_normal(1);
                response.write_normal(1);
            },
            _ => unimplemented!("srv: {:?}", header)
        }
    }
}
