#![feature(drain_filter)] // Used in `AddressArbiterImpl`
#![feature(unsized_locals)] // Used for `Box<dyn FnOnce()>` in `hle::thread::ThreadRuntime`
#![feature(futures_api, async_await, await_macro)] // Used to implement most of `hle::thread`
#![feature(existential_type)] // Used to implement `ServiceHelper` in `hle::service::helper`

use ::pomelo_kernel as kernel;

mod arbiter;
mod event;
mod port;
mod session;
mod thread;
mod timer;
pub mod svc;

pub mod service;

pub use self::port::*;
pub use self::session::*;
pub use self::thread::*;
