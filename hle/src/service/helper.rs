use crate::kernel;

pub trait Service {
    fn start(self, kctx: &mut kernel::Kernel) -> kernel::ProcessId;
}

#[macro_export]
macro_rules! declare_service {
    (for $item:ident named ($name:expr) using $handle_ipc:ident) => {
        impl crate::hle::service::helper::Service for $item {
            fn start(mut self, kctx: &mut kernel::Kernel) -> kernel::ProcessId {
                use crate::hle::service::srv;

                let (pid, _) = kctx.new_process(
                    $name.into(),
                    hle::ProcessHLE,
                );

                kctx.new_thread(
                    pid.clone(),
                    hle::ThreadHLE::new(
                        async move |mut context: hle::HLEContext| {
                            println!(concat!("[", $name, "] Started"));

                            let port = context.kernel().lookup_port(b"srv:").expect("Could not find srv: port");
                            let port = context.kernel().new_handle(port.into_object());
                            let srv_session = await!(context.create_session_to_port(&port)).unwrap();

                            let srv = srv::SrvInterface::new(srv_session);

                            let server_port = await!(srv.register_service(&mut context, $name.as_bytes(), 255)).unwrap();

                            let mut handles = vec![];
                            handles.push(Some(server_port.clone()));

                            let mut reply_target = None;
                            let mut data = [0u8; 256];

                            loop {
                                let (index, res) = await!(context.reply_and_receive(&handles, reply_target.take(), &mut data));
                                match (index, res) {
                                    (None, Ok(())) => unreachable!(),
                                    (None, Err(err)) => panic!("Failed somewhere: {:X}", err),
                                    (Some(0), Ok(())) => {
                                        // New client, accept session
                                        let server_session = await!(context.accept_session(&server_port)).unwrap();
                                        handles.push(Some(server_session));
                                    },
                                    (Some(index), Ok(())) => {
                                        // Got request
                                        await!(self.handle_ipc(IPCContext::new(&mut data[..]), &mut context));

                                        reply_target = handles[index].clone();
                                    },
                                    (Some(index), Err(err)) => {
                                        panic!("Disconnect? {:X}", err)
                                    }
                                }
                            }
                        }
                    )
                );

                return pid
            }
        }
    };
}
