use chrono::{Local, SecondsFormat, Utc};
use reqwest::blocking::Client;
use serde::Serialize;
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::Duration;

static LOG_BUFFER: Mutex<Vec<String>> = Mutex::new(Vec::new());
static REMOTE_LOGGER: OnceLock<Option<RemoteLogger>> = OnceLock::new();

const REMOTE_BATCH_SIZE: usize = 100;
const REMOTE_FLUSH_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct RemoteLogger {
    sender: mpsc::Sender<RemoteEvent>,
}

#[derive(Clone)]
struct RemoteEvent {
    timestamp: String,
    message: String,
}

#[derive(Serialize)]
struct HumioBatch<'a> {
    tags: HumioTags<'a>,
    events: &'a [HumioEvent],
}

#[derive(Serialize)]
struct HumioTags<'a> {
    system: &'a str,
    environment: &'a str,
}

#[derive(Serialize)]
struct HumioEvent {
    timestamp: String,
    attributes: HumioAttributes,
    rawstring: String,
}

#[derive(Serialize)]
struct HumioAttributes {
    app: &'static str,
    source: &'static str,
    message: String,
}

pub fn init_remote_logging() {
    let _ = REMOTE_LOGGER.get_or_init(build_remote_logger);
}

fn remote_logger() -> Option<&'static RemoteLogger> {
    REMOTE_LOGGER
        .get_or_init(build_remote_logger)
        .as_ref()
}

fn build_remote_logger() -> Option<RemoteLogger> {
    let ingest_url = option_env!("HUMIO_INGEST_URL")
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let ingest_token = option_env!("HUMIO_INGEST_TOKEN")
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let environment = option_env!("HUMIO_ENV")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("dev")
        .to_string();

    let (sender, receiver) = mpsc::channel::<RemoteEvent>();

    std::thread::Builder::new()
        .name("humio-log-shipper".into())
        .spawn(move || {
            let client = Client::new();
            let mut pending = Vec::new();

            loop {
                match receiver.recv_timeout(REMOTE_FLUSH_INTERVAL) {
                    Ok(event) => {
                        pending.push(event);
                        while pending.len() < REMOTE_BATCH_SIZE {
                            match receiver.try_recv() {
                                Ok(extra) => pending.push(extra),
                                Err(mpsc::TryRecvError::Empty) => break,
                                Err(mpsc::TryRecvError::Disconnected) => break,
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        if pending.is_empty() {
                            break;
                        }
                    }
                }

                if pending.is_empty() {
                    continue;
                }

                if let Err(error) = send_batch(&client, &ingest_url, &ingest_token, &environment, &pending) {
                    eprintln!("[humio] log upload failed: {}", error);
                }
                pending.clear();
            }
        })
        .ok()?;

    Some(RemoteLogger { sender })
}

fn send_batch(
    client: &Client,
    ingest_url: &str,
    ingest_token: &str,
    environment: &str,
    pending: &[RemoteEvent],
) -> Result<(), String> {
    let events = pending
        .iter()
        .map(|event| HumioEvent {
            timestamp: event.timestamp.clone(),
            rawstring: event.message.clone(),
            attributes: HumioAttributes {
                app: "anna",
                source: "desktop-app",
                message: event.message.clone(),
            },
        })
        .collect::<Vec<_>>();

    let payload = vec![HumioBatch {
        tags: HumioTags {
            system: "anna",
            environment,
        },
        events: &events,
    }];

    let response = client
        .post(ingest_url)
        .bearer_auth(ingest_token)
        .json(&payload)
        .send()
        .map_err(|error| error.to_string())?;

    if response.status().is_success() {
        return Ok(());
    }

    let status = response.status();
    let body = response.text().unwrap_or_default();
    Err(format!("HTTP {}: {}", status, body))
}

pub fn push_log(message: String) {
    let timestamp = Local::now().format("%H:%M:%S");
    let entry = format!("[{}] {}", timestamp, message);
    eprintln!("{}", entry);
    if let Ok(mut buf) = LOG_BUFFER.lock() {
        buf.push(entry.clone());
        // Keep last 500 entries
        if buf.len() > 500 {
            let excess = buf.len() - 500;
            buf.drain(..excess);
        }
    }

    if let Some(remote) = remote_logger() {
        let _ = remote.sender.send(RemoteEvent {
            timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            message: entry,
        });
    }
}

pub fn get_logs() -> Vec<String> {
    LOG_BUFFER.lock().map(|buf| buf.clone()).unwrap_or_default()
}

/// Use: `app_log!("Besked: {}", value);`
#[macro_export]
macro_rules! app_log {
    ($($arg:tt)*) => {
        $crate::logging::push_log(format!($($arg)*))
    };
}
