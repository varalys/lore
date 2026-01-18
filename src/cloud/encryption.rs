//! End-to-end encryption for cloud sync.
//!
//! Provides passphrase-based key derivation using Argon2id and symmetric
//! encryption using AES-256-GCM. Session message content is encrypted
//! before upload and decrypted after download, ensuring that the cloud
//! service cannot read session contents.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rand::RngCore;

use super::CloudError;

/// Size of the encryption key in bytes (256 bits for AES-256).
pub const KEY_SIZE: usize = 32;

/// Size of the nonce in bytes (96 bits for AES-GCM).
pub const NONCE_SIZE: usize = 12;

/// Size of the salt for key derivation.
#[allow(dead_code)]
pub const SALT_SIZE: usize = 16;

/// Derives an encryption key from a passphrase and salt using Argon2id.
///
/// Uses Argon2id with secure default parameters suitable for key derivation.
/// The same passphrase and salt will always produce the same key, allowing
/// encryption and decryption across sessions.
///
/// # Arguments
///
/// * `passphrase` - The user's passphrase
/// * `salt` - A random salt (should be stored and reused for the same account)
///
/// # Returns
///
/// A 32-byte key suitable for AES-256-GCM encryption.
pub fn derive_key(passphrase: &str, salt: &[u8]) -> Result<Vec<u8>, CloudError> {
    // Convert salt bytes to the format expected by Argon2
    // SaltString::encode_b64 expects raw bytes and encodes them as base64
    let salt_string = SaltString::encode_b64(salt)
        .map_err(|e| CloudError::EncryptionError(format!("Invalid salt: {e}")))?;

    // Use Argon2id with default secure parameters
    let argon2 = Argon2::default();

    // Hash the password to derive the key
    let hash = argon2
        .hash_password(passphrase.as_bytes(), &salt_string)
        .map_err(|e| CloudError::EncryptionError(format!("Key derivation failed: {e}")))?;

    // Extract the hash bytes (the output is the derived key)
    let hash_output = hash
        .hash
        .ok_or_else(|| CloudError::EncryptionError("No hash output".to_string()))?;

    // Take the first KEY_SIZE bytes as the encryption key
    let key_bytes = hash_output.as_bytes();
    if key_bytes.len() < KEY_SIZE {
        return Err(CloudError::EncryptionError(
            "Derived key too short".to_string(),
        ));
    }

    Ok(key_bytes[..KEY_SIZE].to_vec())
}

/// Generates a random salt for key derivation.
///
/// The salt should be stored (in config) and reused for the same account
/// to ensure consistent key derivation.
#[allow(dead_code)]
pub fn generate_salt() -> Vec<u8> {
    let mut salt = vec![0u8; SALT_SIZE];
    rand::thread_rng().fill_bytes(&mut salt);
    salt
}

/// Encrypts data using AES-256-GCM.
///
/// The nonce is prepended to the ciphertext, so the output format is:
/// `nonce (12 bytes) || ciphertext || tag (16 bytes)`
///
/// # Arguments
///
/// * `data` - The plaintext data to encrypt
/// * `key` - The 32-byte encryption key
///
/// # Returns
///
/// The encrypted data with prepended nonce, suitable for base64 encoding.
pub fn encrypt_data(data: &[u8], key: &[u8]) -> Result<Vec<u8>, CloudError> {
    if key.len() != KEY_SIZE {
        return Err(CloudError::EncryptionError(format!(
            "Invalid key size: expected {KEY_SIZE}, got {}",
            key.len()
        )));
    }

    // Generate a random nonce
    let mut nonce_bytes = [0u8; NONCE_SIZE];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Create the cipher
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| CloudError::EncryptionError(format!("Cipher creation failed: {e}")))?;

    // Encrypt the data
    let ciphertext = cipher
        .encrypt(nonce, data)
        .map_err(|e| CloudError::EncryptionError(format!("Encryption failed: {e}")))?;

    // Prepend nonce to ciphertext
    let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);

    Ok(result)
}

/// Decrypts data that was encrypted with `encrypt_data`.
///
/// Expects the input format: `nonce (12 bytes) || ciphertext || tag (16 bytes)`
///
/// # Arguments
///
/// * `data` - The encrypted data with prepended nonce
/// * `key` - The 32-byte encryption key
///
/// # Returns
///
/// The decrypted plaintext data.
pub fn decrypt_data(data: &[u8], key: &[u8]) -> Result<Vec<u8>, CloudError> {
    if key.len() != KEY_SIZE {
        return Err(CloudError::EncryptionError(format!(
            "Invalid key size: expected {KEY_SIZE}, got {}",
            key.len()
        )));
    }

    if data.len() < NONCE_SIZE {
        return Err(CloudError::EncryptionError(
            "Encrypted data too short".to_string(),
        ));
    }

    // Extract nonce and ciphertext
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_SIZE);
    let nonce = Nonce::from_slice(nonce_bytes);

    // Create the cipher
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| CloudError::EncryptionError(format!("Cipher creation failed: {e}")))?;

    // Decrypt the data
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| CloudError::EncryptionError(format!("Decryption failed: {e}")))?;

    Ok(plaintext)
}

/// Encodes binary data as base64.
pub fn encode_base64(data: &[u8]) -> String {
    BASE64.encode(data)
}

/// Decodes base64 data to binary.
pub fn decode_base64(data: &str) -> Result<Vec<u8>, CloudError> {
    BASE64
        .decode(data)
        .map_err(|e| CloudError::EncryptionError(format!("Base64 decode failed: {e}")))
}

/// Encodes a key as hexadecimal for storage.
pub fn encode_key_hex(key: &[u8]) -> String {
    hex::encode(key)
}

/// Decodes a hexadecimal key.
pub fn decode_key_hex(hex_str: &str) -> Result<Vec<u8>, CloudError> {
    hex::decode(hex_str).map_err(|e| CloudError::EncryptionError(format!("Hex decode failed: {e}")))
}

// We need hex encoding, add a simple implementation
mod hex {
    pub fn encode(data: &[u8]) -> String {
        data.iter().map(|b| format!("{:02x}", b)).collect()
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, String> {
        if !s.len().is_multiple_of(2) {
            return Err("Hex string has odd length".to_string());
        }

        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("Invalid hex: {e}")))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_salt_length() {
        let salt = generate_salt();
        assert_eq!(salt.len(), SALT_SIZE);
    }

    #[test]
    fn test_generate_salt_randomness() {
        let salt1 = generate_salt();
        let salt2 = generate_salt();
        assert_ne!(salt1, salt2);
    }

    #[test]
    fn test_derive_key_consistency() {
        let passphrase = "test passphrase";
        let salt = generate_salt();

        let key1 = derive_key(passphrase, &salt).unwrap();
        let key2 = derive_key(passphrase, &salt).unwrap();

        assert_eq!(key1, key2);
        assert_eq!(key1.len(), KEY_SIZE);
    }

    #[test]
    fn test_derive_key_different_passphrases() {
        let salt = generate_salt();

        let key1 = derive_key("passphrase1", &salt).unwrap();
        let key2 = derive_key("passphrase2", &salt).unwrap();

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_derive_key_different_salts() {
        let passphrase = "test passphrase";
        let salt1 = generate_salt();
        let salt2 = generate_salt();

        let key1 = derive_key(passphrase, &salt1).unwrap();
        let key2 = derive_key(passphrase, &salt2).unwrap();

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let passphrase = "test passphrase";
        let salt = generate_salt();
        let key = derive_key(passphrase, &salt).unwrap();

        let plaintext = b"Hello, World! This is a test message.";
        let encrypted = encrypt_data(plaintext, &key).unwrap();
        let decrypted = decrypt_data(&encrypted, &key).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_produces_different_ciphertext() {
        let salt = generate_salt();
        let key = derive_key("passphrase", &salt).unwrap();

        let plaintext = b"test data";
        let encrypted1 = encrypt_data(plaintext, &key).unwrap();
        let encrypted2 = encrypt_data(plaintext, &key).unwrap();

        // Different nonces should produce different ciphertexts
        assert_ne!(encrypted1, encrypted2);
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let salt = generate_salt();
        let key1 = derive_key("passphrase1", &salt).unwrap();
        let key2 = derive_key("passphrase2", &salt).unwrap();

        let plaintext = b"secret data";
        let encrypted = encrypt_data(plaintext, &key1).unwrap();

        let result = decrypt_data(&encrypted, &key2);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_with_corrupted_data_fails() {
        let salt = generate_salt();
        let key = derive_key("passphrase", &salt).unwrap();

        let plaintext = b"secret data";
        let mut encrypted = encrypt_data(plaintext, &key).unwrap();

        // Corrupt the ciphertext
        if let Some(byte) = encrypted.get_mut(NONCE_SIZE + 5) {
            *byte ^= 0xFF;
        }

        let result = decrypt_data(&encrypted, &key);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_data_invalid_key_size() {
        let short_key = vec![0u8; 16]; // Too short
        let result = encrypt_data(b"data", &short_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_data_too_short() {
        let salt = generate_salt();
        let key = derive_key("passphrase", &salt).unwrap();

        let short_data = vec![0u8; 5]; // Shorter than nonce
        let result = decrypt_data(&short_data, &key);
        assert!(result.is_err());
    }

    #[test]
    fn test_base64_roundtrip() {
        let data = b"test binary data \x00\x01\x02";
        let encoded = encode_base64(data);
        let decoded = decode_base64(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_hex_roundtrip() {
        let data = vec![0u8, 1, 2, 255, 128, 64];
        let encoded = encode_key_hex(&data);
        let decoded = decode_key_hex(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex::encode(&[0, 255, 128]), "00ff80");
    }

    #[test]
    fn test_hex_decode_invalid() {
        assert!(hex::decode("xyz").is_err());
        assert!(hex::decode("abc").is_err()); // Odd length
    }

    #[test]
    fn test_encrypt_empty_data() {
        let salt = generate_salt();
        let key = derive_key("passphrase", &salt).unwrap();

        let plaintext = b"";
        let encrypted = encrypt_data(plaintext, &key).unwrap();
        let decrypted = decrypt_data(&encrypted, &key).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_large_data() {
        let salt = generate_salt();
        let key = derive_key("passphrase", &salt).unwrap();

        // 1 MB of data
        let plaintext: Vec<u8> = (0..1_000_000).map(|i| (i % 256) as u8).collect();
        let encrypted = encrypt_data(&plaintext, &key).unwrap();
        let decrypted = decrypt_data(&encrypted, &key).unwrap();

        assert_eq!(decrypted, plaintext);
    }
}
