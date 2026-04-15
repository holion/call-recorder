use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

const CLIENT_ID: &str = env!("GOOGLE_CLIENT_ID");
const CLIENT_SECRET: &str = env!("GOOGLE_CLIENT_SECRET");

#[derive(serde::Serialize)]
pub struct GoogleTokens {
    pub id_token: String,
    pub access_token: String,
}

pub async fn authenticate() -> Result<GoogleTokens> {
    // Bind to a random available port
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{}", port);

    // Build Google OAuth URL
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?\
        client_id={}&\
        redirect_uri={}&\
        response_type=code&\
        scope=openid%20email%20profile&\
        access_type=offline&\
        prompt=select_account",
        CLIENT_ID,
        urlencoding(&redirect_uri),
    );

    // Open in system browser
    open::that(&auth_url)?;

    // Wait for the OAuth callback
    let code = tokio::task::spawn_blocking(move || -> Result<String> {
        let (mut stream, _) = listener.accept()?;
        let mut reader = BufReader::new(&stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line)?;

        // Parse the authorization code from "GET /?code=...&scope=... HTTP/1.1"
        let code = request_line
            .split_whitespace()
            .nth(1)
            .and_then(|path| {
                path.split('?')
                    .nth(1)
                    .and_then(|query| {
                        query.split('&').find_map(|param| {
                            let mut kv = param.splitn(2, '=');
                            match (kv.next(), kv.next()) {
                                (Some("code"), Some(val)) => Some(urldecode(val)),
                                _ => None,
                            }
                        })
                    })
            })
            .ok_or_else(|| anyhow::anyhow!("No auth code in callback"))?;

        // Send a nice response to the browser
        let body = "<html><body><h2>Login gennemført!</h2><p>Du kan lukke dette vindue og vende tilbage til Call Recorder.</p></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes())?;
        stream.flush()?;

        Ok(code)
    })
    .await??;

    // Exchange auth code for tokens
    let client = reqwest::Client::new();
    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", code.as_str()),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("redirect_uri", &redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await?;

    let body: serde_json::Value = resp.json().await?;

    let id_token = body["id_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No id_token in response: {}", body))?
        .to_string();

    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token in response"))?
        .to_string();

    Ok(GoogleTokens {
        id_token,
        access_token,
    })
}

fn urlencoding(s: &str) -> String {
    s.replace(':', "%3A").replace('/', "%2F")
}

fn urldecode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}
