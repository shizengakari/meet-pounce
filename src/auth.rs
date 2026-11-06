use serde::Deserialize;
use std::collections::HashMap;
use std::error::Error;

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

/// リフレッシュトークンから新しいアクセストークンを取得する
pub async fn refresh_access_token(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let mut params = HashMap::new();
    params.insert("client_id", client_id);
    params.insert("client_secret", client_secret);
    params.insert("refresh_token", refresh_token);
    params.insert("grant_type", "refresh_token");

    let res = client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await?;

    let token: TokenResponse = res.json().await?;
    Ok(token.access_token)
}
