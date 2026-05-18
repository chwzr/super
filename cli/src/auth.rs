use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::Rng;
use sha2::{Digest, Sha256};
use shared::{AuthorizeRequest, RefreshRequest, TokenResponse, UserProfile};
use crate::config::{load_config, save_config};

pub struct AuthClient {
    base_url: String,
    http: reqwest::Client,
}

impl AuthClient {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            http: reqwest::Client::new(),
        }
    }

    pub async fn login_flow(&self) -> Result<shared::CliConfig, Box<dyn std::error::Error>> {
        let code_verifier = generate_code_verifier();
        let code_challenge = compute_s256_challenge(&code_verifier);

        // Bind listener first to get the actual port
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        println!("Listening on port {port} for callback...");

        // Build login URL with the real port so redirect works
        let login_url = format!(
            "{}/auth/login?code_challenge={}&redirect_uri=http://localhost:{}/callback",
            self.base_url, code_challenge, port
        );
        webbrowser::open(&login_url)?;

        let (mut stream, _) = listener.accept()?;
        use std::io::{BufRead, BufReader, Write};
        let mut reader = BufReader::new(&mut stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line)?;

        let code = extract_code_from_request(&request_line)
            .ok_or("no authorization code in callback")?;

        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body><h1>Logged in! You can close this window.</h1></body></html>";
        stream.write_all(response.as_bytes())?;

        let tokens = self.exchange_code(&code, &code_verifier).await?;
        let profile = self.get_profile(&tokens.access_token).await?;

        let mut config = load_config();
        config.access_token = Some(tokens.access_token);
        config.refresh_token = Some(tokens.refresh_token);
        config.openrouter_api_key = Some(profile.openrouter_api_key);
        save_config(&config);

        Ok(config)
    }

    async fn exchange_code(
        &self,
        code: &str,
        code_verifier: &str,
    ) -> Result<TokenResponse, Box<dyn std::error::Error>> {
        let resp = self
            .http
            .post(format!("{}/auth/authorize", self.base_url))
            .json(&AuthorizeRequest {
                code: code.to_string(),
                code_verifier: code_verifier.to_string(),
            })
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("authorize failed: {}", resp.text().await?).into());
        }
        Ok(resp.json().await?)
    }

    async fn get_profile(
        &self,
        token: &str,
    ) -> Result<UserProfile, Box<dyn std::error::Error>> {
        let resp = self
            .http
            .get(format!("{}/auth/me", self.base_url))
            .bearer_auth(token)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("profile fetch failed: {}", resp.text().await?).into());
        }
        Ok(resp.json().await?)
    }

    #[allow(dead_code)]
    pub async fn refresh_token(
        &self,
        refresh_token: &str,
    ) -> Result<TokenResponse, Box<dyn std::error::Error>> {
        let resp = self
            .http
            .post(format!("{}/auth/refresh", self.base_url))
            .json(&RefreshRequest {
                refresh_token: refresh_token.to_string(),
            })
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("refresh failed: {}", resp.text().await?).into());
        }
        Ok(resp.json().await?)
    }
}

fn generate_code_verifier() -> String {
    let bytes: [u8; 32] = rand::thread_rng().gen();
    URL_SAFE_NO_PAD.encode(bytes)
}

fn compute_s256_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn extract_code_from_request(request_line: &str) -> Option<String> {
    let path = request_line.split_whitespace().nth(1)?;
    let query = path.split('?').nth(1)?;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next()? == "code" {
            return parts.next().map(|s| s.to_string());
        }
    }
    None
}
