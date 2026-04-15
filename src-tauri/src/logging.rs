use std::sync::Mutex;
use chrono::Local;

static LOG_BUFFER: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn push_log(message: String) {
    let timestamp = Local::now().format("%H:%M:%S");
    let entry = format!("[{}] {}", timestamp, message);
    eprintln!("{}", entry);
    if let Ok(mut buf) = LOG_BUFFER.lock() {
        buf.push(entry);
        // Keep last 500 entries
        if buf.len() > 500 {
            let excess = buf.len() - 500;
            buf.drain(..excess);
        }
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
