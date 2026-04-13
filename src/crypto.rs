use aes_gcm::{AeadInOut, Aes256Gcm, KeyInit, Nonce, Tag, aes::cipher::consts::U12};
use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub fn encrypt(admin_secret: Option<&str>, plaintext: &str) -> Result<String> {
    let key = derive_key(admin_secret.unwrap_or_default());
    let cipher = Aes256Gcm::new_from_slice(&key).context("failed to initialize AES-256-GCM")?;

    let random = Uuid::new_v4().into_bytes();
    let mut iv = [0u8; 12];
    iv.copy_from_slice(&random[..12]);

    let mut buffer = plaintext.as_bytes().to_vec();
    let nonce = Nonce::<U12>::from(iv);
    let tag = cipher
        .encrypt_inout_detached(&nonce, b"", buffer.as_mut_slice().into())
        .context("failed to encrypt provider instance config")?;

    Ok(format!(
        "{}:{}:{}",
        STANDARD.encode(iv),
        STANDARD.encode(&tag[..]),
        STANDARD.encode(buffer)
    ))
}

pub fn decrypt(admin_secret: Option<&str>, ciphertext: &str) -> Result<String> {
    let key = derive_key(admin_secret.unwrap_or_default());
    let cipher = Aes256Gcm::new_from_slice(&key).context("failed to initialize AES-256-GCM")?;

    let mut parts = ciphertext.split(':');
    let Some(iv_b64) = parts.next() else {
        return Err(anyhow!("invalid encrypted payload: missing IV"));
    };
    let Some(tag_b64) = parts.next() else {
        return Err(anyhow!("invalid encrypted payload: missing auth tag"));
    };
    let Some(data_b64) = parts.next() else {
        return Err(anyhow!("invalid encrypted payload: missing ciphertext"));
    };
    if parts.next().is_some() {
        return Err(anyhow!("invalid encrypted payload: too many parts"));
    }

    let iv = STANDARD.decode(iv_b64).context("failed to decode IV")?;
    let tag = STANDARD
        .decode(tag_b64)
        .context("failed to decode auth tag")?;
    let mut data = STANDARD
        .decode(data_b64)
        .context("failed to decode ciphertext")?;

    let nonce = Nonce::<U12>::try_from(iv.as_slice())
        .map_err(|_| anyhow!("invalid encrypted payload: IV length mismatch"))?;
    let tag = Tag::try_from(tag.as_slice())
        .map_err(|_| anyhow!("invalid encrypted payload: auth tag length mismatch"))?;
    cipher
        .decrypt_inout_detached(&nonce, b"", data.as_mut_slice().into(), &tag)
        .context("failed to decrypt provider instance config")?;

    String::from_utf8(data).context("decrypted provider instance config is not valid UTF-8")
}

fn derive_key(secret: &str) -> [u8; 32] {
    let digest = Sha256::digest(secret.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    key
}
