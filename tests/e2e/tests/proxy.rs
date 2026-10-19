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

/// Retargeted, the proxy relays new connections to the new target (a far
/// end started again on a port of its own, #455) and leaves the
/// connections it relays alone.
#[test]
fn a_retargeted_proxy_relays_new_connections_to_the_new_target() {
    let s = Scenario::new("proxy-retarget");
    let (first, first_server) = echo(1);
    let (second, second_server) = echo(1);
    let proxy = s.proxy(first);
    let (mut old, mut old_reader) = connect(proxy.addr());
    round_trip(&mut old, &mut old_reader, "to the first");

    proxy.retarget(second);
    // Each server accepts one connection: a new connection relayed to the
    // first would get no echo.
    let (mut new, mut new_reader) = connect(proxy.addr());
    round_trip(&mut new, &mut new_reader, "to the second");
    round_trip(&mut old, &mut old_reader, "still to the first");
    drop((old, old_reader, new, new_reader));

    s.finish();
    first_server.join().expect("the first echo server");
    second_server.join().expect("the second echo server");
    drop(proxy);
}

/// A client that sends its request, half-closes, then reads the reply
/// gets the reply: an end of stream is passed on as a half-close, not as
/// the end of the connection.
#[test]
fn a_half_close_is_passed_on() {
    let s = Scenario::new("proxy-half-close");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = listener.local_addr().unwrap();
    // Reads the request to its end, then answers.
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the server's accept");
        stream.set_read_timeout(Some(hang_guard())).unwrap();
        let mut request = String::new();
        stream
            .read_to_string(&mut request)
            .expect("the request, to its end");
        write!(stream, "got {request}").expect("the reply");
    });
    let proxy = s.proxy(target);

    let mut stream = TcpStream::connect(proxy.addr()).unwrap();
    stream.set_read_timeout(Some(hang_guard())).unwrap();
    stream.write_all(b"request").unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).expect("the reply");
    assert_eq!(reply, "got request");

    server.join().expect("the server");
    drop(stream);
    s.finish();
}

/// A cut in the middle of a transfer ends it on both sides: the client
/// sees its connection end, the server's writes fail.
#[test]
fn a_cut_ends_a_transfer_in_progress() {
    let s = Scenario::new("proxy-cut-transfer");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = listener.local_addr().unwrap();
    // Writes until its connection is closed.
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the server's accept");
        let chunk = [b'x'; 4096];
        while stream.write_all(&chunk).is_ok() {}
    });
    let proxy = s.proxy(target);

    let mut stream = TcpStream::connect(proxy.addr()).unwrap();
    stream.set_read_timeout(Some(hang_guard())).unwrap();
    // Reads on its own thread, so the proxy is never blocked writing to it.
    let (receiving, received) = std::sync::mpsc::channel();
    let client = std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(n) => {
                    let _ = receiving.send(n);
                }
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    return Err("the connection did not end within the hang guard");
                }
                // A reset ends it too.
                Err(_) => return Ok(()),
            }
        }
    });
    received
        .recv_timeout(hang_guard())
        .expect("data through the proxy");

    proxy.cut();
    client
        .join()
        .expect("the client")
        .expect("the client's connection ends");
    server.join().expect("the server");
    s.finish();
}
