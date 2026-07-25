//! AES-256-CBC DM encryption, byte-compatible with server/src/utils/crypto.js:
//! key = SHA-256(secret), format `hex_iv:hex_ciphertext`, PKCS7 padding.

use aes::cipher::{block_padding::Pkcs7, BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use rand::Rng;
use sha2::{Digest, Sha256};

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

#[derive(Clone)]
pub struct DmCrypto {
    key: [u8; 32],
}

impl DmCrypto {
    pub fn new(secret: &str) -> Self {
        let key: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
        Self { key }
    }

    pub fn encrypt(&self, text: &str) -> String {
        let mut iv = [0u8; 16];
        rand::rng().fill_bytes(&mut iv);
        let ct = Aes256CbcEnc::new(&self.key.into(), &iv.into())
            .encrypt_padded_vec::<Pkcs7>(text.as_bytes());
        format!("{}:{}", hex::encode(iv), hex::encode(ct))
    }

    /// Falls back to returning the input unchanged if it is not in the
    /// `iv:ciphertext` format or fails to decrypt (old plaintext DMs).
    pub fn decrypt(&self, encrypted: &str) -> String {
        let Some((iv_hex, ct_hex)) = encrypted.split_once(':') else {
            return encrypted.to_string();
        };
        // Exactly two parts, like the Node version's split(':') length check.
        if ct_hex.contains(':') {
            return encrypted.to_string();
        }
        let (Ok(iv), Ok(ct)) = (hex::decode(iv_hex), hex::decode(ct_hex)) else {
            return encrypted.to_string();
        };
        if iv.len() != 16 {
            return encrypted.to_string();
        }
        let iv: [u8; 16] = iv.try_into().unwrap();
        match Aes256CbcDec::new(&self.key.into(), &iv.into())
            .decrypt_padded_vec::<Pkcs7>(&ct)
            .ok()
            .and_then(|pt| String::from_utf8(pt).ok())
        {
            Some(plain) => plain,
            None => encrypted.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "akami-wtf-dm-encryption-super-secret-key-98765";
    // Produced by Node crypto.js with the same secret and a fixed IV.
    const NODE_ENC: &str = "000102030405060708090a0b0c0d0e0f:df0ab383d0f1a05779b4821357fb05b2e53c77af357597f4dc9e1499ec171087c1019215390eb5ea2aebaa2db89e3157";

    #[test]
    fn decrypts_node_ciphertext() {
        let c = DmCrypto::new(SECRET);
        assert_eq!(c.decrypt(NODE_ENC), "Привет, это тест DM! 🎉");
    }

    #[test]
    fn roundtrip() {
        let c = DmCrypto::new(SECRET);
        let enc = c.encrypt("hello world");
        assert_ne!(enc, "hello world");
        assert_eq!(c.decrypt(&enc), "hello world");
    }

    #[test]
    fn falls_back_on_plaintext() {
        let c = DmCrypto::new(SECRET);
        assert_eq!(c.decrypt("просто старое сообщение"), "просто старое сообщение");
        assert_eq!(c.decrypt("a:b:c"), "a:b:c");
        assert_eq!(c.decrypt(""), "");
    }
}
