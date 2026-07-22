use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::TryRngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

pub const PREFIX: &str = "exo-encrypted:v1:";
const MEMORY_KIB: u32 = 65_536;
const ITERATIONS: u32 = 3;
const LANES: u32 = 4;

#[derive(Debug, Error)]
pub enum EncryptionError {
    #[error("not an encrypted note body")]
    NotEncrypted,
    #[error("invalid encryption envelope")]
    InvalidEnvelope,
    #[error("unable to derive encryption key")]
    KeyDerivation,
    #[error("unable to encrypt note")]
    Encrypt,
    #[error("unable to decrypt note")]
    Decrypt,
    #[error("operating system random source failed")]
    Random,
}

#[derive(Debug, Deserialize, Serialize)]
struct Envelope {
    v: u8,
    kdf: String,
    m: u32,
    t: u32,
    p: u32,
    s: String,
    n: String,
    ct: String,
}

#[must_use]
pub fn is_encrypted(body: &str) -> bool {
    body.starts_with(PREFIX)
}

pub fn encrypt(
    note_id: &str,
    passphrase: &str,
    plaintext: &str,
) -> Result<String, EncryptionError> {
    let mut salt = [0_u8; 16];
    let mut nonce = [0_u8; 12];
    OsRng
        .try_fill_bytes(&mut salt)
        .map_err(|_| EncryptionError::Random)?;
    OsRng
        .try_fill_bytes(&mut nonce)
        .map_err(|_| EncryptionError::Random)?;
    encrypt_with_material(note_id, passphrase, plaintext, salt, nonce)
}

fn encrypt_with_material(
    note_id: &str,
    passphrase: &str,
    plaintext: &str,
    salt: [u8; 16],
    nonce: [u8; 12],
) -> Result<String, EncryptionError> {
    let key = derive_key(passphrase, &salt, MEMORY_KIB, ITERATIONS, LANES)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| EncryptionError::Encrypt)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            aes_gcm::aead::Payload {
                msg: plaintext.as_bytes(),
                aad: &aad(note_id),
            },
        )
        .map_err(|_| EncryptionError::Encrypt)?;
    let envelope = Envelope {
        v: 1,
        kdf: "argon2id".to_owned(),
        m: MEMORY_KIB,
        t: ITERATIONS,
        p: LANES,
        s: URL_SAFE_NO_PAD.encode(salt),
        n: URL_SAFE_NO_PAD.encode(nonce),
        ct: URL_SAFE_NO_PAD.encode(ciphertext),
    };
    let json = serde_json::to_vec(&envelope).map_err(|_| EncryptionError::InvalidEnvelope)?;
    Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(json)))
}

pub fn decrypt(note_id: &str, passphrase: &str, body: &str) -> Result<String, EncryptionError> {
    let encoded = body
        .strip_prefix(PREFIX)
        .ok_or(EncryptionError::NotEncrypted)?;
    let json = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| EncryptionError::InvalidEnvelope)?;
    let envelope: Envelope =
        serde_json::from_slice(&json).map_err(|_| EncryptionError::InvalidEnvelope)?;
    if envelope.v != 1
        || envelope.kdf != "argon2id"
        || envelope.m == 0
        || envelope.t == 0
        || envelope.p == 0
    {
        return Err(EncryptionError::InvalidEnvelope);
    }
    let salt = URL_SAFE_NO_PAD
        .decode(envelope.s)
        .map_err(|_| EncryptionError::InvalidEnvelope)?;
    let nonce = URL_SAFE_NO_PAD
        .decode(envelope.n)
        .map_err(|_| EncryptionError::InvalidEnvelope)?;
    let ciphertext = URL_SAFE_NO_PAD
        .decode(envelope.ct)
        .map_err(|_| EncryptionError::InvalidEnvelope)?;
    if salt.len() != 16 || nonce.len() != 12 {
        return Err(EncryptionError::InvalidEnvelope);
    }
    let key = derive_key(passphrase, &salt, envelope.m, envelope.t, envelope.p)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| EncryptionError::Decrypt)?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            aes_gcm::aead::Payload {
                msg: &ciphertext,
                aad: &aad(note_id),
            },
        )
        .map_err(|_| EncryptionError::Decrypt)?;
    String::from_utf8(plaintext).map_err(|_| EncryptionError::Decrypt)
}

fn derive_key(
    passphrase: &str,
    salt: &[u8],
    memory: u32,
    iterations: u32,
    lanes: u32,
) -> Result<Zeroizing<[u8; 32]>, EncryptionError> {
    let params = Params::new(memory, iterations, lanes, Some(32))
        .map_err(|_| EncryptionError::KeyDerivation)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0_u8; 32]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, key.as_mut())
        .map_err(|_| EncryptionError::KeyDerivation)?;
    Ok(key)
}

fn aad(note_id: &str) -> Vec<u8> {
    format!("exo-encrypted:v1\0{note_id}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_binds_ciphertext_to_note_id() {
        let envelope =
            encrypt_with_material("note001", "hunter2", "secret", [7; 16], [9; 12]).unwrap();
        assert_eq!(decrypt("note001", "hunter2", &envelope).unwrap(), "secret");
        assert!(matches!(
            decrypt("note002", "hunter2", &envelope),
            Err(EncryptionError::Decrypt)
        ));
        assert!(matches!(
            decrypt("note001", "wrong", &envelope),
            Err(EncryptionError::Decrypt)
        ));
    }
}
