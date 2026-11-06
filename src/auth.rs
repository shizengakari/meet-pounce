use serde::Deserialize;
use std::error::Error;

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

/// リフレッシュトークンから新しいアクセストークンを取得する
pub async fn refresh_access_token(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];

    let res = client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await?;

    let token: TokenResponse = res.json().await?;
    Ok(token.access_token)
}
