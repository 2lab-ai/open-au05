//! TEA (Tiny Encryption Algorithm) and 64-byte HID frame codec for the
//! Ulanzi Vibe Key AU05 vendor channel (usage page 0xFFFC, report ID 0x55).

use crate::protocol::types::{MessageType, Operation, REPORT_ID, REPORT_LENGTH};
use std::fmt;

/// Standard 32-round TEA in ECB mode with little-endian 32-bit halves.
pub struct Tea;

impl Tea {
    /// Constant key recovered from Ulanzi Studio (`gaui_custom_encrypt_keys`).
    pub const KEY: [u32; 4] = [0xCAA5_BACA, 0xBC2A_8A6D, 0xCA5A_9EBA, 0x9BB8_8BCA];
    const DELTA: u32 = 0x9E37_79B9;
    const ROUNDS: u32 = 32;

    pub fn encrypt_block(block: [u8; 8]) -> [u8; 8] {
        Self::transform_block(block, false)
    }

    pub fn decrypt_block(block: [u8; 8]) -> [u8; 8] {
        Self::transform_block(block, true)
    }

    fn transform_block(block: [u8; 8], decrypt: bool) -> [u8; 8] {
        let mut v0 = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
        let mut v1 = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        let k = Self::KEY;
        let mix = |v: u32, sum: u32, k0: u32, k1: u32| -> u32 {
            (v << 4).wrapping_add(k0) ^ v.wrapping_add(sum) ^ (v >> 5).wrapping_add(k1)
        };
        let mut sum: u32 = if decrypt {
            Self::DELTA.wrapping_mul(Self::ROUNDS)
        } else {
            0
        };
        for _ in 0..Self::ROUNDS {
            if decrypt {
                v1 = v1.wrapping_sub(mix(v0, sum, k[2], k[3]));
                v0 = v0.wrapping_sub(mix(v1, sum, k[0], k[1]));
                sum = sum.wrapping_sub(Self::DELTA);
            } else {
                sum = sum.wrapping_add(Self::DELTA);
                v0 = v0.wrapping_add(mix(v1, sum, k[0], k[1]));
                v1 = v1.wrapping_add(mix(v0, sum, k[2], k[3]));
            }
        }
        let mut out = [0u8; 8];
        out[..4].copy_from_slice(&v0.to_le_bytes());
        out[4..].copy_from_slice(&v1.to_le_bytes());
        out
    }

    /// Encrypts every complete 8-byte block; leaves any trailing partial block untouched.
    pub fn encrypt(bytes: &[u8]) -> Vec<u8> {
        let mut out = bytes.to_vec();
        for chunk in out.as_chunks_mut::<8>().0 {
            *chunk = Self::encrypt_block(*chunk);
        }
        out
    }

    /// Decrypts every complete 8-byte block; leaves any trailing partial block untouched.
    pub fn decrypt(bytes: &[u8]) -> Vec<u8> {
        let mut out = bytes.to_vec();
        for chunk in out.as_chunks_mut::<8>().0 {
            *chunk = Self::decrypt_block(*chunk);
        }
        out
    }
}

/// Encodes and decodes 64-byte vendor HID reports (`0x55` + 63 encrypted bytes).
pub struct FrameCodec;

impl FrameCodec {
    /// Pads `message` to 64 bytes, TEA-encrypts all 8 blocks, and returns
    /// `[0x55, cipher[0..63]]` (reproducing the vendor library byte for byte).
    pub fn encode_output_report(message: &[u8]) -> [u8; REPORT_LENGTH] {
        let mut plain = [0u8; REPORT_LENGTH];
        let n = message.len().min(REPORT_LENGTH);
        plain[..n].copy_from_slice(&message[..n]);
        let cipher = Tea::encrypt(&plain);
        let mut report = [0u8; REPORT_LENGTH];
        report[0] = REPORT_ID;
        report[1..].copy_from_slice(&cipher[..REPORT_LENGTH - 1]);
        report
    }

    /// Strips report ID `0x55` (if present) and decrypts all complete 8-byte blocks
    /// (first 7 blocks = 56 plaintext bytes for a standard 64-byte HID input report).
    pub fn decode_input_report(report: &[u8]) -> Result<Frame, &'static str> {
        let payload = if report.first() == Some(&REPORT_ID) {
            &report[1..]
        } else if report.len() == REPORT_LENGTH - 1 {
            report
        } else {
            return Err("invalid report ID");
        };
        if payload.len() < 8 {
            return Err("report too short");
        }
        let full_len = (payload.len() / 8) * 8;
        let decrypted = Tea::decrypt(&payload[..full_len]);
        Ok(Frame::new(decrypted))
    }
}

/// A decoded plaintext AU05 protocol frame (up to 56 or 64 bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub bytes: Vec<u8>,
}

impl Frame {
    pub fn new(mut bytes: Vec<u8>) -> Self {
        if bytes.len() < 8 {
            bytes.resize(8, 0);
        }
        Self { bytes }
    }

    pub fn byte(&self, idx: usize) -> u8 {
        self.bytes.get(idx).copied().unwrap_or(0)
    }

    pub fn raw_type(&self) -> u8 {
        self.byte(0)
    }

    pub fn msg_type(&self) -> Option<MessageType> {
        MessageType::from_u8(self.byte(0) & 0x1F)
    }

    /// Bit 7 of byte 0 is set on replies relayed from the microphone.
    pub fn from_mic(&self) -> bool {
        self.byte(0) & 0x80 != 0
    }

    pub fn group(&self) -> u8 {
        self.byte(1) & 0x0F
    }

    pub fn command(&self) -> u8 {
        self.byte(2)
    }

    pub fn operation(&self) -> u8 {
        self.byte(3)
    }

    pub fn is_reply(&self) -> bool {
        self.operation() & Operation::REPLY_FLAG != 0
    }

    pub fn is_read_reply(&self) -> bool {
        self.operation() & Operation::SUCCESS_FLAG != 0
    }

    pub fn u16_le(&self, idx: usize) -> u16 {
        u16::from_le_bytes([self.byte(idx), self.byte(idx + 1)])
    }

    pub fn u32_le(&self, idx: usize) -> u32 {
        u32::from_le_bytes([
            self.byte(idx),
            self.byte(idx + 1),
            self.byte(idx + 2),
            self.byte(idx + 3),
        ])
    }

    pub fn slice(&self, start: usize, len: usize) -> Vec<u8> {
        if start >= self.bytes.len() {
            return Vec::new();
        }
        let end = (start + len).min(self.bytes.len());
        self.bytes[start..end].to_vec()
    }

    pub fn short_hex(&self) -> String {
        let mut end = self.bytes.len();
        while end > 4 && self.bytes[end - 1] == 0 {
            end -= 1;
        }
        self.bytes[..end]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl fmt::Display for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short_hex())
    }
}
