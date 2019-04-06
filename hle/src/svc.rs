use crate::kernel;

pub fn create_event(kctx: &mut kernel::Kernel, reset_type: kernel::ResetType) -> kernel::KResult<kernel::Handle> {
    println!("CreateEvent({:?})", reset_type);

    let (_, timer) = kctx.new_object_handle(crate::event::EventImpl::new(reset_type));

    Ok(timer)
}

// TODO: This call may reschedule?
pub fn signal_event(kctx: &mut kernel::Kernel, handle: Option<kernel::Handle>) -> kernel::KResult<()> {
    println!("SignalEvent({:?})", handle);

    let object = handle.as_ref().and_then(|h| kctx.lookup_handle(h));
    let event: Option<&kernel::KEvent> = object.as_ref().map(|rc| &**rc).and_then(|o| o.into());

    if let (Some(object), Some(event)) = (&object, event) {
        event.signal(object, &mut kctx.threads);

        Ok(())
    } else {
        Err(kernel::errors::HANDLE_TYPE_MISMATCH)
    }
}

pub fn create_timer(kctx: &mut kernel::Kernel, reset_type: kernel::ResetType) -> kernel::KResult<kernel::Handle> {
    println!("CreateTimer({:?})", reset_type);

    let (_, timer) = kctx.new_object_handle(crate::timer::TimerImpl::new(reset_type));

    Ok(timer)
}

pub fn set_timer(kctx: &mut kernel::Kernel, handle: Option<kernel::Handle>, initial: i64, interval: i64) -> kernel::KResult<()> {
    println!("SetTimer({:?}, {:X}, {:X})", handle, initial, interval);

    let object = handle.as_ref().and_then(|h| kctx.lookup_handle(h));
    let timer: Option<&kernel::KTimer> = object.as_ref().map(|rc| &**rc).and_then(|o| o.into());

    if let Some(timer) = timer {
        timer.set(initial, interval);

        Ok(())
    } else {
        Err(kernel::errors::HANDLE_TYPE_MISMATCH)
    }
}

pub fn create_address_arbiter(kctx: &mut kernel::Kernel) -> kernel::KResult<kernel::Handle> {
    println!("CreateAddressArbiter()");

    let (_, timer) = kctx.new_object_handle(crate::arbiter::ArbiterImpl::new());

    Ok(timer)
}

fn make_timeout(ns: i64) -> kernel::Timeout {
    if ns >= 0 {
        if ns == 0 {
            kernel::Timeout::Instant
        } else {
            kernel::Timeout::After(ns as u64)
        }
    } else {
        kernel::Timeout::Forever
    }
}

pub struct Reschedule;

pub fn wait_sync_1(kctx: &mut kernel::Kernel, handle: Option<kernel::Handle>, timeout: i64) -> kernel::KResult<Option<Reschedule>> {
    println!("WaitSynchronization1({:?}, {:X})", handle, timeout);

    let object = handle.as_ref().and_then(|h| kctx.lookup_handle(h));
    let sync: Option<&kernel::KSynchronizationObject> = object.as_ref().map(|rc| &**rc).and_then(|o| o.into());

    let time = make_timeout(timeout);

    // TODO: if ns == 0, don't wait & don't reschedule

    if let Some(sync) = sync {
        let rc = object.as_ref().unwrap(); // TODO: Elide this clone by changing how wakers are done
        let waker = kctx.suspend_thread([rc].iter().map(|rc| rc.clone()), time.absolute(kctx.time()), kernel::ThreadWaitType::WaitSync1);

        sync.wait(rc, waker, &mut kctx.threads);

        Ok(Some(Reschedule))
    } else {
        Err(kernel::errors::HANDLE_TYPE_MISMATCH)
    }
}

pub fn wait_sync_n(kctx: &mut kernel::Kernel, handles: &[Option<kernel::Handle>], wait_all: bool, timeout: i64) -> kernel::KResult<Option<Reschedule>> {
    println!("WaitSynchronizationN({:X?}, {}, {:X})", handles, wait_all, timeout);

    if wait_all {
        unimplemented!("wait_all unimplemented")
    }

    let time = make_timeout(timeout);

    // TODO: Use smallvec to avoid allocations

    // Now with the handles, we want to get the underlying kernel objects
    let objects: Vec<kernel::KObjectRef> = handles.iter()
        .map(|h| h.as_ref().and_then(|h| kctx.lookup_handle(h)).expect("null object in waitsync"))
        .collect();

    let absolute = time.absolute(kctx.time());

    let waker = kctx.suspend_thread(objects.iter(), absolute, kernel::ThreadWaitType::WaitSyncN);

    // And then translate the kernel objects into KSynchronizationObjects so we can wait on them
    // let mut ok = true;
    for object in objects.iter() {
        let sync: Option<&kernel::KSynchronizationObject> = (&**object).into();

        if let Some(sync) = sync {
            sync.wait(object, waker.clone(), &mut kctx.threads);
        } else {
            // This code path is bugged, disabled:
            unimplemented!("waitsync with invalid objects");

            // Result
            // regs.sr(0, kernel::errors::HANDLE_TYPE_MISMATCH.into());
            // ok = false;
            // break
        }
    }

    Ok(Some(Reschedule))
}

// Just wait_sync_n with extra machinery post resume
pub fn reply_and_receive_pre(kctx: &mut kernel::Kernel, handles: &[Option<kernel::Handle>], reply_target: Option<kernel::Handle>) -> kernel::KResult<Option<Reschedule>> {
    if let Some(reply_target) = reply_target {
        let (_, tid) = kctx.current_pid_tid();
        let object = kctx.lookup_handle(&reply_target);
        let session = object.as_ref()
            .and_then(kernel::KTypedObject::<kernel::KServerSession>::new)
            .ok_or(kernel::errors::HANDLE_TYPE_MISMATCH)?;
        session.reply(tid, kctx);
    }

    wait_sync_n(kctx, handles, false, -1)
}

pub fn reply_and_receive_post(kctx: &mut kernel::Kernel, handles: &[Option<kernel::Handle>], resume: &kernel::ThreadResume) -> (usize, kernel::KResult<()>) {
    let index = match *resume {
        kernel::ThreadResume::WaitSyncN { index } => index,
        _ => unreachable!("{:?}", resume),
    };

    // Lets check what type of handle was the one that woke us up
    // If we were woken up by a KServerSession, we try to receive (otherwise we have to return an error)
    // Otherwise, we just return the index
    let object = handles[index].as_ref().and_then(|handle| kctx.lookup_handle(handle));
    let session = object.as_ref()
        .and_then(kernel::KTypedObject::<kernel::KServerSession>::new);
    
    if let Some(session) = session {
        let (_, tid) = kctx.current_pid_tid();
        if !session.receive(tid, kctx) {
            // Failed to get, return error
            return (index, Err(0xC920181A))
        }
    }

    (index, Ok(()))
}

pub fn close_handle(kctx: &mut kernel::Kernel, handle: Option<kernel::Handle>) -> kernel::KResult<()> {
    println!("CloseHandle({:?})", handle);

    let ok = handle.map(|handle| kctx.current_process().close_handle(&handle)).unwrap_or(false);

    if ok {
        Ok(())
    } else {
        Err(kernel::errors::HANDLE_TYPE_MISMATCH)
    }
}

pub fn send_sync_request(kctx: &mut kernel::Kernel, handle: Option<kernel::Handle>) -> kernel::KResult<Option<Reschedule>> {
    println!("SendSyncRequest({:?})", handle);

    let object = handle.as_ref().and_then(|h| kctx.lookup_handle(h));
    let sync: Option<&kernel::KClientSession> = object.as_ref().map(|rc| &**rc).and_then(|o| o.into());

    if let Some(sync) = sync {
        let rc = object.as_ref().unwrap();
        let waker = kctx.suspend_thread([rc].iter().map(|rc| rc.clone()), None, kernel::ThreadWaitType::WaitSync1);

        if !sync.sync_request(waker, &mut kctx.threads) {
            panic!("TODO: Handle remote end disconnected")
        }

        Ok(Some(Reschedule))
    } else {
        Err(kernel::errors::HANDLE_TYPE_MISMATCH)
    }
}

pub fn create_session_to_port(kctx: &mut kernel::Kernel, handle: Option<kernel::Handle>) -> kernel::KResult<(Option<Reschedule>, kernel::Handle)> {
    println!("CreateSessionToPort({:?})", handle);

    let object = handle.as_ref().and_then(|h| kctx.lookup_handle(h));
    let port: Option<&kernel::KClientPort> = object.as_ref().map(|rc| &**rc).and_then(|o| o.into());

    if let Some(port) = port {
        // Create session pair
        let (server, client) = crate::make_session_pair(kctx);

        port.create_session_to_port(server, &mut kctx.threads);

        let rc = object.as_ref().unwrap(); // TODO: Elide this clone by changing how wakers are done
        let waker = kctx.suspend_thread([rc].iter().map(|rc| rc.clone()), None, kernel::ThreadWaitType::WaitSync1);

        port.wait(rc, waker, &mut kctx.threads);

        Ok((Some(Reschedule), kctx.new_handle(client.into_object())))
    } else {
        Err(kernel::errors::HANDLE_TYPE_MISMATCH)
    }
}

pub fn accept_session(kctx: &mut kernel::Kernel, handle: Option<kernel::Handle>) -> kernel::KResult<Result<kernel::Handle, Reschedule>> {
    println!("AcceptSession({:?})", handle);

    let object = handle.as_ref().and_then(|h| kctx.lookup_handle(h));
    let port: Option<&kernel::KServerPort> = object.as_ref().map(|rc| &**rc).and_then(|o| o.into());

    if let Some(port) = port {
        match port.accept_session(&mut kctx.threads) {
            Some(session) => {
                Ok(Ok(kctx.new_handle(session.into_object())))
            },
            None => {
                let rc = object.as_ref().unwrap(); // TODO: Elide this clone by changing how wakers are done
                let waker = kctx.suspend_thread([rc].iter().map(|rc| rc.clone()), None, kernel::ThreadWaitType::WaitSync1);

                port.wait(rc, waker, &mut kctx.threads);

                Ok(Err(Reschedule))
            }
        }
    } else {
        Err(kernel::errors::HANDLE_TYPE_MISMATCH)
    }
}
