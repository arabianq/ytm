use anyhow::Result;
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;
use ytmapi_rs::{YtMusic, auth::OAuthToken};

const TOKEN_FILE: &str = "token.json";

pub enum AuthEvent {
    Checking,
    CodeRequired {
        user_code: String,
        verification_url: String,
    },
    Authenticated(YtMusic<OAuthToken>),
    Error(String),
}

#[derive(Serialize, Deserialize)]
struct SavedToken {
    token: OAuthToken,
}

pub fn start_auth_flow(tx: Sender<AuthEvent>) {
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            if let Err(e) = auth_logic(&tx).await {
                let _ = tx.send(AuthEvent::Error(e.to_string()));
            }
        });
    });
}

async fn auth_logic(tx: &Sender<AuthEvent>) -> Result<()> {
    dotenv::dotenv().ok();
    let _ = tx.send(AuthEvent::Checking);

    let token_path = PathBuf::from(TOKEN_FILE);

    // 1. Проверяем сохраненный токен
    if token_path.exists() {
        if let Ok(file_content) = tokio::fs::read_to_string(&token_path).await {
            if let Ok(saved) = serde_json::from_str::<SavedToken>(&file_content) {
                let yt = YtMusic::from_auth_token(saved.token);
                let _ = tx.send(AuthEvent::Authenticated(yt));
                return Ok(());
            }
        }
    }

    let client = ytmapi_rs::Client::new()?;

    let client_id = env::var("YOUTUI_OAUTH_CLIENT_ID")
        .or_else(|_| env::var("CLIENT_ID"))
        .map(|s| s.trim().to_string())
        .map_err(|_| anyhow::anyhow!(t!("config.client_id_not_found")))?;

    let client_secret = env::var("YOUTUI_OAUTH_CLIENT_SECRET")
        .or_else(|_| env::var("CLIENT_SECRET"))
        .map(|s| s.trim().to_string())
        .map_err(|_| anyhow::anyhow!(t!("config.client_secret_not_found")))?;

    if client_id.starts_with("GOCSPX-") {
        return Err(anyhow::anyhow!(t!("config.client_id_is_secret")));
    }

    log::info!("Starting OAuth flow with Client ID: {}", client_id);

    let (code, url) = ytmapi_rs::generate_oauth_code_and_url(&client, &client_id)
        .await
        .map_err(|e| {
            log::error!("OAuth request failed: {:?}", e);
            anyhow::anyhow!(t!("auth.oauth_request_error", error = e.to_string()))
        })?;

    let user_code = url
        .split("user_code=")
        .nth(1)
        .unwrap_or("UNKNOWN")
        .to_string();

    let _ = tx.send(AuthEvent::CodeRequired {
        user_code,
        verification_url: url,
    });

    let token = loop {
        match ytmapi_rs::generate_oauth_token(
            &client,
            code.clone(),
            client_id.clone(),
            client_secret.clone(),
        )
        .await
        {
            Ok(t) => break t,
            Err(e) => {
                let err_msg = format!("{:?}", e);
                if err_msg.contains("authorization_pending") {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                } else {
                    return Err(anyhow::anyhow!(t!("auth.auth_error", error = err_msg)));
                }
            }
        }
    };

    let saved_token = SavedToken {
        token: token.clone(),
    };
    let json = serde_json::to_string_pretty(&saved_token)?;
    tokio::fs::write(&token_path, json).await?;

    let yt = YtMusic::from_auth_token(token);

    let _ = tx.send(AuthEvent::Authenticated(yt));

    Ok(())
}
