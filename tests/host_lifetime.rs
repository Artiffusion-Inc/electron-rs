#![cfg(windows)]

use electron_rs::Electron;
use serde_json::Value;
use std::{
  fs,
  io::{BufRead, BufReader, Write},
  net::{TcpListener, TcpStream},
  os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
  path::PathBuf,
  process::{Child, Command, Stdio},
  thread,
  time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::Win32::{
  Foundation::{HANDLE, WAIT_OBJECT_0},
  System::Threading::{
    OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
  },
};

const WAIT: Duration = Duration::from_secs(10);
const WORKER: &str = "ELECTRON_RS_LIFETIME_WORKER";

struct Host(Child);
impl Drop for Host {
  fn drop(&mut self) {
    let _ = self.0.kill();
    let _ = self.0.wait();
  }
}

struct Process(OwnedHandle);
impl Drop for Process {
  fn drop(&mut self) {
    unsafe {
      let _ = TerminateProcess(HANDLE(self.0.as_raw_handle()), 1);
    }
  }
}

fn host_lifetime_worker(root: std::ffi::OsString) {
  let runtime = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .unwrap();
  let electron = runtime
    .block_on(Electron::with_args(
      PathBuf::from(root).join("fixture.exe"),
      0,
      &[],
      WAIT,
    ))
    .unwrap();
  let event = electron.events().unwrap().recv_timeout(WAIT).unwrap();
  assert_eq!(event["event"], "lifetime.prepared");
  let mut control =
    TcpStream::connect(std::env::var("ELECTRON_RS_LIFETIME_CONTROL").unwrap()).unwrap();
  writeln!(control, "{}", event["params"]).unwrap();
  thread::sleep(Duration::from_secs(30));
  electron.shutdown();
}

#[test]
fn forced_host_exit_reaps_electron_and_its_descendant() {
  if let Some(root) = std::env::var_os(WORKER) {
    return host_lifetime_worker(root);
  }
  let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("target")
    .join(format!(
      "host-lifetime-{}-{}",
      std::process::id(),
      SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
    ));
  fs::create_dir_all(&root).unwrap();
  fs::write(root.join("fixture.rs"), r#"
use std::{io::{BufRead, Write}, net::TcpStream, process::{Command, Stdio}, thread, time::Duration};
fn main() {
  let args: Vec<_> = std::env::args().collect();
  if args.iter().any(|arg| arg == "--descendant") {
    thread::sleep(Duration::from_secs(30));
    return;
  }
  let port = &args[args.iter().position(|arg| arg == "--port").unwrap() + 1];
  let mut token = String::new();
  std::io::stdin().lock().read_line(&mut token).unwrap();
  let mut socket = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
  writeln!(socket, "{{\"event\":\"ready\",\"params\":{{\"token\":\"{}\",\"electron\":\"lifetime-fixture\"}}}}", token.trim()).unwrap();
  let child = Command::new(std::env::current_exe().unwrap()).arg("--descendant").stdin(Stdio::null()).spawn().unwrap();
  writeln!(socket, "{{\"event\":\"lifetime.prepared\",\"params\":{{\"main\":{},\"descendant\":{}}}}}", std::process::id(), child.id()).unwrap();
  thread::sleep(Duration::from_secs(30));
}
"#).unwrap();
  assert!(Command::new("rustc")
    .arg(root.join("fixture.rs"))
    .arg("-o")
    .arg(root.join("fixture.exe"))
    .status()
    .unwrap()
    .success());
  fs::write(root.join("main.cjs"), "").unwrap();
  let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
  listener.set_nonblocking(true).unwrap();
  let mut host = Host(
    Command::new(std::env::current_exe().unwrap())
      .args([
        "--exact",
        "forced_host_exit_reaps_electron_and_its_descendant",
        "--nocapture",
      ])
      .env(WORKER, &root)
      .env(
        "ELECTRON_RS_LIFETIME_CONTROL",
        listener.local_addr().unwrap().to_string(),
      )
      .stdout(Stdio::null())
      .spawn()
      .unwrap(),
  );
  let deadline = Instant::now() + WAIT + WAIT;
  let control = loop {
    match listener.accept() {
      Ok((stream, _)) => break stream,
      Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
        thread::sleep(Duration::from_millis(10))
      }
      result => panic!("host did not prepare the process tree: {result:?}"),
    }
  };
  control.set_read_timeout(Some(WAIT)).unwrap();
  let mut line = String::new();
  BufReader::new(control).read_line(&mut line).unwrap();
  let pids: Value = serde_json::from_str(&line).unwrap();
  let processes = ["main", "descendant"].map(|name| {
    let pid = pids[name].as_u64().unwrap().try_into().unwrap();
    let handle =
      unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid) }.unwrap();
    let process = Process(unsafe { OwnedHandle::from_raw_handle(handle.0) });
    assert_ne!(
      unsafe { WaitForSingleObject(HANDLE(process.0.as_raw_handle()), 0) },
      WAIT_OBJECT_0,
      "{name} must be running before host termination"
    );
    (name, process)
  });
  host.0.kill().unwrap();
  host.0.wait().unwrap();
  for (name, process) in &processes {
    assert_eq!(
      unsafe { WaitForSingleObject(HANDLE(process.0.as_raw_handle()), 5000) },
      WAIT_OBJECT_0,
      "{name} survived forced host termination"
    );
  }
  drop(processes);
  drop(host);
  fs::remove_dir_all(root).unwrap();
}
