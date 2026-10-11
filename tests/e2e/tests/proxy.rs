//! The harness's TCP proxy (S5, #189): relays, cuts, comes back, and
//! leaves its port free.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};

use rutis_e2e::{hang_guard, Scenario};

/// Echo each line back, on `connections` connections one after the other.
fn echo(connections: usize) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..connections {
            let (stream, _) = listener.accept().expect("the echo server's accept");
            let mut writer = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines() {
                // A cut connection may end with a reset.
                let Ok(line) = line else { break };
                if writeln!(writer, "{line}").is_err() {
                    break;
                }
            }
        }
    });
    (addr, server)
}

fn connect(addr: std::net::SocketAddr) -> (TcpStream, BufReader<TcpStream>) {
    let stream = TcpStream::connect(addr).expect("a connection through the proxy");
    stream.set_read_timeout(Some(hang_guard())).unwrap();
    let reader = BufReader::new(stream.try_clone().unwrap());
    (stream, reader)
}

fn round_trip(stream: &mut TcpStream, reader: &mut BufReader<TcpStream>, text: &str) {
    writeln!(stream, "{text}").unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).expect("the echo");
    assert_eq!(line.trim_end(), text);
}

#[test]
fn a_proxy_relays_cuts_and_restores() {
    let s = Scenario::new("proxy");
    let (target, server) = echo(2);
    let proxy = s.proxy(target);

    let (mut stream, mut reader) = connect(proxy.addr());
    round_trip(&mut stream, &mut reader, "before");

    proxy.cut();
    // The connection is closed: end of stream, or a reset.
    let mut rest = Vec::new();
    match reader.read_to_end(&mut rest) {
        Ok(_) => assert!(rest.is_empty(), "{rest:?}"),
        Err(error) => assert!(
            !matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
            "not closed within the hang guard"
        ),
    }
    // New connections are refused.
    let refused = TcpStream::connect(proxy.addr()).expect_err("the proxy is cut");
    assert_eq!(refused.kind(), ErrorKind::ConnectionRefused, "{refused}");

    proxy.restore();
    let (mut stream, mut reader) = connect(proxy.addr());
    round_trip(&mut stream, &mut reader, "after");
    drop((stream, reader));

    // The scenario's end closes the proxy and checks its port is free.
    s.finish();
    server.join().expect("the echo server");
    drop(proxy);
}
