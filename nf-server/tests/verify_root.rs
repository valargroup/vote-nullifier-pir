//! Offline CLI acceptance tests replaying eight independently sourced mainnet blocks.

#[cfg(feature = "upstream")]
extern crate upstream_chain as zakura_chain;

use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Output},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use zakura_chain::{
    block::Block,
    serialization::{ZcashDeserialize, ZcashSerialize},
    transaction::Transaction,
};

const RAW: [&[u8]; 8] = [
    include_bytes!("fixtures/verify-root/mainnet-3428143.bin"),
    include_bytes!("fixtures/verify-root/mainnet-3428144.bin"),
    include_bytes!("fixtures/verify-root/mainnet-3428145.bin"),
    include_bytes!("fixtures/verify-root/mainnet-3428146.bin"),
    include_bytes!("fixtures/verify-root/mainnet-3428147.bin"),
    include_bytes!("fixtures/verify-root/mainnet-3428148.bin"),
    include_bytes!("fixtures/verify-root/mainnet-3428149.bin"),
    include_bytes!("fixtures/verify-root/mainnet-3428150.bin"),
];

fn snapshot() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/verify-root/snapshot.json")).unwrap()
}

#[derive(Clone, Copy)]
enum Mode {
    Valid,
    Missing,
    MissingMiddle,
    WrongBlock,
    MissingTransaction,
    MissingAction,
    Transient,
    Unavailable,
    Oversized,
    Truncated,
}

struct Server {
    url: String,
    stopped: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn start(mode: Mode) -> Self {
        let blocks: HashMap<_, _> = RAW
            .into_iter()
            .map(|raw| {
                let block = Block::zcash_deserialize(raw).unwrap();
                (block.hash().to_string(), raw.to_vec())
            })
            .collect();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = stopped.clone();
        let count = requests.clone();
        let handle = thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("accept: {error}"),
                };
                // Accepted sockets inherit O_NONBLOCK on macOS. The fixture's
                // read_exact calls require blocking I/O on every platform.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let Some(request) = read_request(&mut stream) else {
                    continue;
                };
                let number = count.fetch_add(1, Ordering::Relaxed);
                assert_eq!(request["method"], "getblock");
                assert_eq!(request["params"][1], 0);
                let hash = request["params"][0].as_str().unwrap();
                if (matches!(mode, Mode::Transient) && number < 2)
                    || matches!(mode, Mode::Unavailable)
                {
                    respond(&mut stream, "503 Service Unavailable", b"temporary");
                    continue;
                }
                if matches!(mode, Mode::Oversized) {
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\nConnection: close\r\n\r\n");
                    continue;
                }
                if matches!(mode, Mode::Missing)
                    || (matches!(mode, Mode::MissingMiddle) && number == 3)
                {
                    respond(
                        &mut stream,
                        "200 OK",
                        br#"{"id":1,"result":null,"error":{"code":-8}}"#,
                    );
                    continue;
                }
                let mut raw = blocks[hash].clone();
                match mode {
                    Mode::WrongBlock => raw = RAW[0].to_vec(),
                    Mode::Truncated => {
                        raw.pop();
                    }
                    Mode::MissingTransaction | Mode::MissingAction => {
                        let mut block = Block::zcash_deserialize(raw.as_slice()).unwrap();
                        if matches!(mode, Mode::MissingTransaction) {
                            let i = block
                                .transactions
                                .iter()
                                .position(|tx| tx.ironwood_actions().count() > 0)
                                .unwrap();
                            block.transactions.remove(i);
                        } else {
                            for tx in &mut block.transactions {
                                if let Transaction::V6 {
                                    ironwood_shielded_data: Some(bundle),
                                    ..
                                } = Arc::make_mut(tx)
                                {
                                    if bundle.actions.len() > 1 {
                                        bundle.actions = bundle
                                            .actions
                                            .iter()
                                            .skip(1)
                                            .cloned()
                                            .collect::<Vec<_>>()
                                            .try_into()
                                            .unwrap();
                                        bundle.proof.0.truncate(bundle.proof.0.len() - 2272);
                                        break;
                                    }
                                }
                            }
                        }
                        raw.clear();
                        block.zcash_serialize(&mut raw).unwrap();
                    }
                    _ => {}
                }
                let body = serde_json::to_vec(
                    &serde_json::json!({"id":1,"result":hex::encode(raw),"error":null}),
                )
                .unwrap();
                respond(&mut stream, "200 OK", &body);
            }
        });
        Self {
            url,
            stopped,
            requests,
            handle: Some(handle),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.handle.take().unwrap().join().unwrap();
    }
}

fn read_request(stream: &mut TcpStream) -> Option<serde_json::Value> {
    let mut bytes = Vec::new();
    let mut one = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut one).ok()?;
        bytes.push(one[0]);
        assert!(bytes.len() < 16_384);
    }
    let headers = String::from_utf8(bytes).unwrap();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    let mut body = vec![0; length];
    stream.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn respond(stream: &mut TcpStream, status: &str, body: &[u8]) {
    let header = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn run(server: &Server, root: &str) -> Output {
    let dir = tempfile::tempdir().unwrap();
    let meta = snapshot();
    let output = Command::new(env!("CARGO_BIN_EXE_nf-server"))
        .current_dir(dir.path())
        // These existing sync controls must have no effect on this command.
        .env("LWD_URLS", "http://127.0.0.1:1")
        .env("SVOTE_PIR_SYNC_RESET", "1")
        .env("SVOTE_PIR_VOTING_CONFIG_URL", "http://127.0.0.1:1")
        .args([
            "verify-root",
            "--zcash-network",
            "main",
            "--height",
            "3428150",
            "--trusted-block-hash",
            meta["hash"].as_str().unwrap(),
            "--expected-circuit-root",
            root,
            "--block-rpc-url",
            &server.url,
        ])
        .output()
        .unwrap();
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "verification must not create artifacts"
    );
    output
}

#[test]
fn verifies_mainnet_snapshot_and_prints_only_json() {
    let server = Server::start(Mode::Valid);
    let meta = snapshot();
    let output = run(&server, meta["circuit_root"].as_str().unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["matches"], true);
    assert_eq!(result["verified_blocks"], 8);
    assert_eq!(result["ironwood_actions"], 43);
    assert_eq!(result["computed_circuit_root"], meta["circuit_root"]);
    assert_eq!(result["computed_pir_root"], meta["pir_root"]);
    assert_eq!(server.requests.load(Ordering::Relaxed), 8);
}

#[test]
fn wrong_root_is_a_nonzero_result_with_both_roots() {
    let server = Server::start(Mode::Valid);
    let output = run(&server, &"00".repeat(32));
    assert!(!output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["matches"], false);
    assert_eq!(result["computed_circuit_root"], snapshot()["circuit_root"]);
}

#[test]
fn provider_omissions_and_malformed_blocks_never_produce_a_result() {
    for mode in [
        Mode::Missing,
        Mode::WrongBlock,
        Mode::MissingTransaction,
        Mode::MissingAction,
        Mode::Oversized,
        Mode::Truncated,
    ] {
        let server = Server::start(mode);
        let output = run(&server, snapshot()["circuit_root"].as_str().unwrap());
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            server.requests.load(Ordering::Relaxed),
            1,
            "invalid data must not retry"
        );
    }
}

#[test]
fn transient_failures_retry_without_skipping_blocks() {
    let server = Server::start(Mode::Transient);
    let output = run(&server, snapshot()["circuit_root"].as_str().unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(server.requests.load(Ordering::Relaxed), 10);
}

#[test]
fn incomplete_history_and_exhausted_retries_fail_closed() {
    for (mode, attempts) in [(Mode::MissingMiddle, 4), (Mode::Unavailable, 3)] {
        let server = Server::start(mode);
        let output = run(&server, snapshot()["circuit_root"].as_str().unwrap());
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            server.requests.load(Ordering::Relaxed),
            attempts,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn invalid_snapshot_arguments_fail_before_network_access() {
    let server = Server::start(Mode::Valid);
    let meta = snapshot();
    for (network, height, hash, root) in [
        (
            "main",
            "3428140",
            meta["hash"].as_str().unwrap(),
            meta["circuit_root"].as_str().unwrap(),
        ),
        (
            "main",
            "3428151",
            meta["hash"].as_str().unwrap(),
            meta["circuit_root"].as_str().unwrap(),
        ),
        (
            "test",
            "3428150",
            meta["hash"].as_str().unwrap(),
            meta["circuit_root"].as_str().unwrap(),
        ),
        (
            "main",
            "3428150",
            "invalid",
            meta["circuit_root"].as_str().unwrap(),
        ),
        ("main", "3428150", meta["hash"].as_str().unwrap(), "invalid"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_nf-server"))
            .args([
                "verify-root",
                "--zcash-network",
                network,
                "--height",
                height,
                "--trusted-block-hash",
                hash,
                "--expected-circuit-root",
                root,
                "--block-rpc-url",
                &server.url,
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    assert_eq!(server.requests.load(Ordering::Relaxed), 0);
}

#[test]
fn raw_extraction_agrees_with_independently_downloaded_compact_nullifiers() {
    let mut actual: Vec<_> = RAW
        .into_iter()
        .flat_map(|raw| {
            let block = Block::zcash_deserialize(raw).unwrap();
            block
                .ironwood_nullifiers()
                .map(|nf| hex::encode(<[u8; 32]>::from(*nf)))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut expected: Vec<_> = snapshot()["nullifiers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
}
