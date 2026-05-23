use std::collections::HashMap;
use std::io::{self, BufRead, BufReader};
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::bounded;
use lsp_server::{Connection, Message, RequestId};
use lsp_types::{InitializeParams, InitializeResult, ServerCapabilities};
use tracing::{debug, error};


pub type CrashCallback = Arc<dyn Fn(String) + Send + Sync>;

/// One LSPClient per spawned language server process.
/// Uses blocking I/O via lsp-server. Callers wrap in spawn_blocking.
pub struct LspClient {
    pub name: String,
    connection: Option<Connection>,
    io_threads: Option<ChildIoThreads>,
    pub capabilities: Option<ServerCapabilities>,
    pub is_initialized: bool,
    is_stopping: Arc<AtomicBool>,
    start_failed: bool,
    start_error: Option<String>,
}

/// IoThreads equivalent wrapping our custom child-process reader/writer threads.
struct ChildIoThreads {
    reader: JoinHandle<io::Result<()>>,
    writer: JoinHandle<io::Result<()>>,
    dropper: JoinHandle<()>,
}

impl ChildIoThreads {
    fn join(self) -> io::Result<()> {
        // Reader
        match self.reader.join() {
            Ok(r) => r?,
            Err(err) => std::panic::panic_any(err),
        }
        // Dropper
        match self.dropper.join() {
            Ok(_) => (),
            Err(err) => std::panic::panic_any(err),
        }
        // Writer
        match self.writer.join() {
            Ok(r) => r,
            Err(err) => std::panic::panic_any(err),
        }
    }
}

/// Build a custom LSP transport that reads from a child process's stdout and
/// writes to its stdin, returning a Connection + ChildIoThreads pair.
fn child_transport(
    child_stdout: ChildStdout,
    child_stdin: ChildStdin,
) -> (Connection, ChildIoThreads) {
    let (drop_sender, drop_receiver) = bounded::<Message>(0);
    let (writer_sender, writer_receiver) = bounded::<Message>(0);

    let writer = thread::Builder::new()
        .name("LspClientWriter".to_owned())
        .spawn(move || {
            let mut stdin = child_stdin;
            writer_receiver.into_iter().try_for_each(|it: Message| {
                let result = it.write(&mut stdin);
                let _ = drop_sender.send(it);
                result
            })
        })
        .unwrap();

    let dropper = thread::Builder::new()
        .name("LspMessageDropper".to_owned())
        .spawn(move || {
            drop_receiver.into_iter().for_each(drop);
        })
        .unwrap();

    let (reader_sender, reader_receiver) = bounded::<Message>(0);

    let reader = thread::Builder::new()
        .name("LspClientReader".to_owned())
        .spawn(move || {
            let stdout = child_stdout;
            let mut buf_reader = BufReader::new(stdout);
            loop {
                match Message::read(&mut buf_reader) {
                    Ok(Some(msg)) => {
                        let is_exit =
                            matches!(&msg, Message::Notification(n) if n.method == "exit");
                        debug!("LSP client received: {:?}", &msg);
                        if let Err(e) = reader_sender.send(msg) {
                            return Err(io::Error::other(e));
                        }
                        if is_exit {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
            Ok(())
        })
        .unwrap();

    let io_threads = ChildIoThreads { reader, writer, dropper };
    let connection = Connection { sender: writer_sender, receiver: reader_receiver };
    (connection, io_threads)
}

impl LspClient {
    pub fn new(name: String) -> Self {
        Self {
            name,
            connection: None,
            io_threads: None,
            capabilities: None,
            is_initialized: false,
            is_stopping: Arc::new(AtomicBool::new(false)),
            start_failed: false,
            start_error: None,
        }
    }

    /// Spawn the language server process and set up JSON-RPC connection.
    pub fn start(
        &mut self,
        command: &str,
        args: &[String],
        env: Option<&HashMap<String, String>>,
        cwd: Option<&str>,
        on_crash: Option<CrashCallback>,
    ) -> Result<(), String> {
        let mut cmd = Command::new(command);
        cmd.args(args);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        if let Some(env_vars) = env {
            for (k, v) in env_vars {
                cmd.env(k, v);
            }
        }
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn '{}': {}", self.name, e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("'{}': stdin not available", self.name))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("'{}': stdout not available", self.name))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| format!("'{}': stderr not available", self.name))?;

        // Capture stderr in background thread
        let stderr_name = self.name.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(line) = line {
                    let trimmed = line.trim().to_string();
                    if !trimmed.is_empty() {
                        debug!("[LSP SERVER {}] {}", stderr_name, trimmed);
                    }
                }
            }
        });

        // Monitor process exit in background thread
        let exit_stopping = self.is_stopping.clone();
        let exit_name = self.name.clone();
        thread::spawn(move || {
            let status = child.wait();
            if !exit_stopping.load(Ordering::Relaxed) {
                match status {
                    Ok(s) if !s.success() => {
                        let msg = format!(
                            "LSP server '{}' crashed (exit code {:?})",
                            exit_name,
                            s.code()
                        );
                        error!("{}", msg);
                        if let Some(cb) = &on_crash {
                            cb(msg);
                        }
                    }
                    Err(e) => {
                        error!("LSP server '{}' process error: {}", exit_name, e);
                    }
                    _ => {}
                }
            }
        });

        let (connection, io_threads) = child_transport(stdout, stdin);

        self.connection = Some(connection);
        self.io_threads = Some(io_threads);
        self.is_initialized = false;
        self.start_failed = false;
        debug!("LSP client started for {}", self.name);

        Ok(())
    }

    /// Send initialize request and store capabilities.
    pub fn initialize(
        &mut self,
        params: InitializeParams,
    ) -> Result<InitializeResult, String> {
        let conn = self.connection()?;
        let id = RequestId::from(1i32);
        // Clone the sender so we can drop the immutable borrow on `self`
        // before writing to self.capabilities / self.is_initialized.
        let sender = conn.sender.clone();

        let init_params = serde_json::to_value(&params)
            .map_err(|e| format!("Serialize init params: {}", e))?;

        sender
            .send(Message::Request(lsp_server::Request {
                id: id.clone(),
                method: "initialize".to_string(),
                params: init_params,
            }))
            .map_err(|e| format!("Send initialize: {}", e))?;

        let result: InitializeResult = match conn.receiver.recv() {
            Ok(Message::Response(resp)) => {
                if let Some(err) = resp.error {
                    return Err(format!("Initialize failed: {}", err.message));
                }
                serde_json::from_value(resp.result.unwrap_or_default())
                    .map_err(|e| format!("Parse init result: {}", e))?
            }
            Ok(other) => {
                return Err(format!(
                    "Unexpected response during init: {:?}",
                    other
                ));
            }
            Err(e) => {
                return Err(format!("Receive init response: {}", e));
            }
        };

        self.capabilities = Some(result.capabilities.clone());

        // Send initialized notification
        sender
            .send(Message::Notification(lsp_server::Notification {
                method: "initialized".to_string(),
                params: serde_json::Value::Object(Default::default()),
            }))
            .map_err(|e| format!("Send initialized: {}", e))?;

        self.is_initialized = true;
        debug!("LSP server '{}' initialized", self.name);
        Ok(result)
    }

    /// Send a request and wait for the response.
    pub fn send_request<T: serde::de::DeserializeOwned>(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T, String> {
        let conn = self.connection()?;
        self.check_ready()?;

        let id = RequestId::from(rand::random::<i32>());

        conn.sender
            .send(Message::Request(lsp_server::Request {
                id: id.clone(),
                method: method.to_string(),
                params,
            }))
            .map_err(|e| format!("Send request '{}': {}", method, e))?;

        match conn.receiver.recv() {
            Ok(Message::Response(resp)) => {
                if let Some(err) = resp.error {
                    return Err(format!(
                        "LSP error {}: {}",
                        err.code, err.message
                    ));
                }
                let raw = resp.result.unwrap_or(serde_json::Value::Null);
                serde_json::from_value::<T>(raw).map_err(|e| {
                    format!("Deserialize response for '{}': {}", method, e)
                })
            }
            Ok(other) => Err(format!(
                "Unexpected response for '{}': {:?}",
                method, other
            )),
            Err(e) => Err(format!(
                "Failed to receive response for '{}': {}",
                method, e
            )),
        }
    }

    /// Send a notification (fire-and-forget).
    pub fn send_notification(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), String> {
        let conn = self.connection()?;
        self.check_ready()?;

        conn.sender
            .send(Message::Notification(lsp_server::Notification {
                method: method.to_string(),
                params,
            }))
            .map_err(|e| format!("Send notification '{}': {}", method, e))
    }

    /// Stop the server gracefully.
    pub fn stop(&mut self) -> Result<(), String> {
        self.is_stopping.store(true, Ordering::Relaxed);

        if let Some(ref conn) = self.connection {
            let id = RequestId::from(0i32);
            let _ = conn.sender.send(Message::Request(lsp_server::Request {
                id,
                method: "shutdown".to_string(),
                params: serde_json::Value::Null,
            }));
            let _ = conn.sender.send(Message::Notification(
                lsp_server::Notification {
                    method: "exit".to_string(),
                    params: serde_json::Value::Null,
                },
            ));
        }

        if let Some(io) = self.io_threads.take() {
            let _ = io.join();
        }

        self.connection = None;
        self.is_initialized = false;
        self.capabilities = None;
        self.start_failed = false;
        self.is_stopping.store(false, Ordering::Relaxed);

        debug!("LSP client stopped for {}", self.name);
        Ok(())
    }

    fn connection(&self) -> Result<&Connection, String> {
        self.connection
            .as_ref()
            .ok_or_else(|| format!("LSP client '{}' not started", self.name))
    }

    fn check_ready(&self) -> Result<(), String> {
        if self.start_failed {
            return Err(
                self.start_error
                    .clone()
                    .unwrap_or_else(|| format!("'{}' failed to start", self.name))
            );
        }
        if !self.is_initialized {
            return Err(format!("'{}' not initialized", self.name));
        }
        Ok(())
    }
}
