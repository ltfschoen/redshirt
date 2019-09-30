// Copyright(c) 2019 Pierre Krieger

//! Bindings for interfacing with the environment of the "kernel".

#![deny(intra_doc_link_resolution_failure)]

extern crate alloc;

use alloc::sync::Arc;
use core::{mem, task::{Context, Poll, Waker}};
use futures::prelude::*;
use parity_scale_codec::{DecodeAll, Encode};
use spin::Mutex;

pub use ffi::{Message, InterfaceMessage, ResponseMessage};

pub mod ffi;

#[cfg(target_arch = "wasm32")] // TODO: bad
pub fn next_message(to_poll: &mut [u64], block: bool) -> Option<Message> {
    unsafe {
        let mut out = Vec::with_capacity(32);
        loop {
            let ret = ffi::next_message(
                to_poll.as_mut_ptr(),
                to_poll.len() as u32,
                out.as_mut_ptr(),
                out.capacity() as u32,
                block,
            ) as usize;
            if ret == 0 {
                return None;
            }
            if ret > out.capacity() {
                out.reserve(ret);
                continue;
            }
            out.set_len(ret);
            let msg: Message = DecodeAll::decode_all(&out).unwrap();
            return Some(msg);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn next_message(to_poll: &mut [u64], block: bool) -> Option<Message> {
    unimplemented!()
}

pub fn emit_message(
    interface_hash: &[u8; 32],
    msg: &impl Encode,
    needs_answer: bool,
) -> Result<Option<u64>, ()> {
    unsafe {
        let buf = msg.encode();
        let mut event_id_out = 0;
        let ret = ffi::emit_message(
            interface_hash as *const [u8; 32] as *const _,
            buf.as_ptr(),
            buf.len() as u32,
            needs_answer,
            &mut event_id_out as *mut _,
        );
        if ret != 0 {
            return Err(());
        }

        if needs_answer {
            Ok(Some(event_id_out))
        } else {
            Ok(None)
        }
    }
}

pub fn emit_answer(message_id: u64, msg: &impl Encode) -> Result<(), ()> {
    unsafe {
        let buf = msg.encode();
        let ret = ffi::emit_answer(message_id, buf.as_ptr(), buf.len() as u32);
        if ret == 0 {
            Ok(())
        } else {
            Err(())
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub fn spawn_thread(function: impl FnOnce()) {
    let function_box: Box<Box<dyn FnOnce()>> = Box::new(Box::new(function));

    extern "C" fn caller(user_data: u32) {
        unsafe {
            let user_data = Box::from_raw(user_data as *mut Box<dyn FnOnce()>);
            user_data();
        }
    }

    unsafe {
        let thread_new = threads::ffi::ThreadsMessage::New(threads::ffi::ThreadNew {
            fn_ptr: mem::transmute(caller as extern "C" fn(u32)),
            user_data: Box::into_raw(function_box) as usize as u32,
        });

        emit_message(&threads::ffi::INTERFACE, &thread_new, false).unwrap();
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_thread(function: impl FnOnce()) {
    panic!()
}

#[cfg(target_arch = "wasm32")] // TODO: bad
// TODO: strongly-typed Future
pub fn message_response(msg_id: u64) -> impl Future<Output = ResponseMessage> {
    let message_sink = Arc::new(Mutex::new(Vec::new()));
    future::poll_fn(move |cx| {
        let mut message_sink_lock = message_sink.lock();
        if message_sink_lock.is_empty() {
            REACTOR.new_elems.lock().push((msg_id, message_sink.clone(), cx.waker().clone()));
            let futex_wake = threads::ffi::ThreadsMessage::FutexWake(threads::ffi::FutexWake {
                addr: &REACTOR.notify_futex as *const u32 as usize as u32,
                nwake: 1,
            });
            emit_message(&threads::ffi::INTERFACE, &futex_wake, false).unwrap();
            return Poll::Pending;
        }

        let outcome = mem::replace(&mut *message_sink_lock, Vec::new());
        let outcome: Message = DecodeAll::decode_all(&outcome).unwrap();
        match outcome {
            Message::Response(r) => Poll::Ready(r),
            _ => unreachable!()     // TODO: replace with std::hint::unreachable when we're mature
        }
    })
}

#[cfg(not(target_arch = "wasm32"))] // TODO: bad
// TODO: strongly-typed Future
pub fn message_response(msg_id: u64) -> impl Future<Output = ResponseMessage> {
    future::pending()
}

// TODO: add a variant of message_response but for multiple messages


lazy_static::lazy_static! {
    static ref REACTOR: Reactor = {
        // TODO: circular dependency with `threads`
        spawn_thread(|| background_thread());

        Reactor {
            notify_futex: 0,
            new_elems: Mutex::new(Vec::with_capacity(16))
        }
    };
}

struct Reactor {
    notify_futex: u32,
    new_elems: Mutex<Vec<(u64, Arc<Mutex<Vec<u8>>>, Waker)>>,
}

fn background_thread() {
    let mut message_ids = vec![0];
    let mut wakers = Vec::with_capacity(16);

    loop {
        let mut new_elems = REACTOR.new_elems.lock();

        // Basic cleanup in order to release memory acquired during peaks.
        if message_ids.capacity() - message_ids.len() >= 32 {
            message_ids.shrink_to_fit();
        }

        // We want to be notified whenever the non-background thread adds elements to the
        // `Reactor`.
        let wait_notify = {
            let msg = threads::ffi::ThreadsMessage::FutexWait(threads::ffi::FutexWait {
                addr: &REACTOR.notify_futex as *const u32 as usize as u32,
                val_cmp: 0,
            });
            emit_message(&threads::ffi::INTERFACE, &msg, true).unwrap().unwrap()
        };

        message_ids[0] = wait_notify;

        for (msg_id, sink, waker) in new_elems.drain(..) {
            // TODO: is it possible that we get a message id for a message that's already been responsed? figure this out
            if let Some(existing_pos) = message_ids.iter().position(|m| *m == msg_id) {
                wakers[existing_pos] = (sink, waker);
            } else {
                message_ids.push(msg_id);
                wakers.push((sink, waker));
            }
        }

        debug_assert!(new_elems.is_empty());
        // TODO: new_elems.shrink_to(16);

        loop {
            let msg = match next_message(&mut message_ids, true) {
                Some(Message::Response(msg)) => msg,
                Some(Message::Interface(_)) => unreachable!(),
                None => unreachable!(),
            };

            if msg.message_id == wait_notify {
                debug_assert_eq!(msg.index_in_list, 0);
                break;
            }

            debug_assert_ne!(msg.index_in_list, 0);
            message_ids.remove(msg.index_in_list as usize);

            let (sink, waker) = wakers.remove(msg.index_in_list as usize - 1);
            *sink.lock() = msg.actual_data;
            waker.wake();
        }
    }
}
