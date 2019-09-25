// Copyright(c) 2019 Pierre Krieger

//! Implements the TCP interface.

use async_std::net::TcpStream;
use fnv::FnvHashMap;
use futures::{prelude::*, ready};
use std::{
    io,
    net::{Ipv6Addr, SocketAddr},
    pin::Pin,
    task::Context,
    task::Poll,
};

pub struct TcpState {
    next_socket_id: u32,
    sockets: FnvHashMap<u32, TcpConnec>,
}

#[derive(Debug)]
pub enum TcpResponse {
    Open(u64, tcp::ffi::TcpOpenResponse),
    Read(u64, tcp::ffi::TcpReadResponse),
    Write(u64, tcp::ffi::TcpWriteResponse),
}

impl TcpState {
    pub fn new() -> TcpState {
        TcpState {
            next_socket_id: 1,
            sockets: FnvHashMap::default(),
        }
    }

    pub fn handle_message(&mut self, event_id: Option<u64>, message: tcp::ffi::TcpMessage) {
        match message {
            tcp::ffi::TcpMessage::Open(open) => {
                let event_id = event_id.unwrap();
                let ip_addr = Ipv6Addr::from(open.ip);
                let socket_addr = if let Some(ip_addr) = ip_addr.to_ipv4() {
                    SocketAddr::new(ip_addr.into(), open.port)
                } else {
                    SocketAddr::new(ip_addr.into(), open.port)
                };
                let socket_id = self.next_socket_id;
                self.next_socket_id += 1;
                let socket = TcpStream::connect(socket_addr);
                self.sockets.insert(
                    socket_id,
                    TcpConnec::Connecting(socket_id, event_id, Box::pin(socket)),
                );
            }
            tcp::ffi::TcpMessage::Close(close) => {
                let _ = self.sockets.remove(&close.socket_id);
            }
            tcp::ffi::TcpMessage::Read(read) => {
                let event_id = event_id.unwrap();
                self.sockets
                    .get_mut(&read.socket_id)
                    .unwrap()
                    .start_read(event_id);
            }
            tcp::ffi::TcpMessage::Write(write) => {
                let event_id = event_id.unwrap();
                self.sockets
                    .get_mut(&write.socket_id)
                    .unwrap()
                    .start_write(event_id, write.data);
            }
        }
    }

    /// Returns the next message to respond to, and the response.
    pub async fn next_event(&mut self) -> TcpResponse {
        // `select_all` panics if the list passed to it is empty, so we have to account for that.
        while self.sockets.is_empty() {
            futures::pending!()
        }

        let (ev, _, _) =
            future::select_all(self.sockets.values_mut().map(|tcp| tcp.next_event())).await;
        println!("answering with {:?}", ev);
        ev
    }
}

enum TcpConnec {
    Connecting(
        u32,
        u64,
        Pin<Box<dyn Future<Output = Result<TcpStream, io::Error>> + Send>>,
    ),
    Socket {
        socket_id: u32,
        tcp_stream: TcpStream,
        pending_read: Option<u64>,
        pending_write: Option<(u64, Vec<u8>)>,
    },
    Poisoned,
}

impl TcpConnec {
    pub fn start_read(&mut self, event_id: u64) {
        let pending_read = match self {
            TcpConnec::Socket {
                ref mut pending_read,
                ..
            } => pending_read,
            _ => panic!(),
        };

        assert!(pending_read.is_none());
        *pending_read = Some(event_id);
    }

    pub fn start_write(&mut self, event_id: u64, data: Vec<u8>) {
        let pending_write = match self {
            TcpConnec::Socket {
                ref mut pending_write,
                ..
            } => pending_write,
            _ => panic!(),
        };

        assert!(pending_write.is_none());
        *pending_write = Some((event_id, data));
    }

    pub fn next_event<'a>(&'a mut self) -> impl Future<Output = TcpResponse> + 'a {
        future::poll_fn(move |cx| {
            let (new_self, event) = match self {
                TcpConnec::Connecting(id, event_id, ref mut fut) => {
                    match ready!(Future::poll(Pin::new(fut), cx)) {
                        Ok(socket) => {
                            let ev = TcpResponse::Open(
                                *event_id,
                                tcp::ffi::TcpOpenResponse { result: Ok(*id) },
                            );
                            (
                                TcpConnec::Socket {
                                    socket_id: *id,
                                    tcp_stream: socket,
                                    pending_write: None,
                                    pending_read: None,
                                },
                                ev,
                            )
                        }
                        Err(_) => {
                            let ev = TcpResponse::Open(
                                *event_id,
                                tcp::ffi::TcpOpenResponse { result: Err(()) },
                            );
                            (TcpConnec::Poisoned, ev)
                        }
                    }
                }

                TcpConnec::Socket {
                    socket_id,
                    tcp_stream,
                    pending_read,
                    pending_write,
                } => {
                    let write_finished = if let Some((msg_id, data_to_write)) = pending_write {
                        if !data_to_write.is_empty() {
                            let num_written = ready!(AsyncWrite::poll_write(
                                Pin::new(tcp_stream),
                                cx,
                                &data_to_write
                            ))
                            .unwrap();
                            for _ in 0..num_written {
                                data_to_write.remove(0);
                            }
                        }
                        if data_to_write.is_empty() {
                            ready!(AsyncWrite::poll_flush(Pin::new(tcp_stream), cx)).unwrap();
                            Some(*msg_id)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if let Some(msg_id) = write_finished {
                        *pending_write = None;
                        return Poll::Ready(TcpResponse::Write(
                            msg_id,
                            tcp::ffi::TcpWriteResponse { result: Ok(()) },
                        ));
                    }

                    if let Some(msg_id) = pending_read.clone() {
                        let mut buf = [0; 1024];
                        let num_read =
                            ready!(AsyncRead::poll_read(Pin::new(tcp_stream), cx, &mut buf))
                                .unwrap();
                        *pending_read = None;
                        return Poll::Ready(TcpResponse::Read(
                            msg_id,
                            tcp::ffi::TcpReadResponse {
                                result: Ok(buf[..num_read].to_vec()),
                            },
                        ));
                    }

                    return Poll::Pending;
                }

                TcpConnec::Poisoned => panic!(),
            };

            *self = new_self;
            Poll::Ready(event)
        })
    }
}
