// Copyright 2021 Damir Jelić
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use prost::Message;
use x25519_dalek::PublicKey as Curve25591PublicKey;

use super::ratchet::{RatchetPublicKey, RemoteRatchetKey};

trait Encode {
    fn encode(self) -> Vec<u8>;
}

pub const MSB: u8 = 0b1000_0000;
const DROP_MSB: u8 = 0b0111_1111;

#[inline]
fn required_encoded_space_unsigned(mut v: u64) -> usize {
    if v == 0 {
        return 1;
    }

    let mut logcounter = 0;
    while v > 0 {
        logcounter += 1;
        v >>= 7;
    }
    logcounter
}

impl Encode for usize {
    fn encode(self) -> Vec<u8> {
        (self as u64).encode()
    }
}

impl Encode for u64 {
    #[inline]
    fn encode(self) -> Vec<u8> {
        let mut v = Vec::new();
        v.resize(required_encoded_space_unsigned(self), 0);

        let mut n = self;
        let mut i = 0;

        while n >= 0x80 {
            v[i] = MSB | (n as u8);
            i += 1;
            n >>= 7;
        }

        v[i] = n as u8;

        v
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OlmMessage {
    inner: Vec<u8>,
}

impl From<Vec<u8>> for OlmMessage {
    fn from(bytes: Vec<u8>) -> Self {
        Self { inner: bytes }
    }
}

impl OlmMessage {
    const VERSION: u8 = 3;

    const RATCHET_TAG: &'static [u8; 1] = b"\x0A";
    const INDEX_TAG: &'static [u8; 1] = b"\x10";
    const CIPHER_TAG: &'static [u8; 1] = b"\x22";

    pub fn as_bytes(&self) -> &[u8] {
        self.inner.as_ref()
    }

    pub fn as_payload_bytes(&self) -> &[u8] {
        let end = self.inner.len();
        &self.inner[..end - 8]
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.inner
    }

    pub fn append_mac(&mut self, mac: &[u8]) {
        let end = self.inner.len();
        self.inner[end - 8..].copy_from_slice(&mac[0..8]);
    }

    pub fn decode(self) -> Result<(RemoteRatchetKey, u64, Vec<u8>), ()> {
        let version = *self.inner.get(0).unwrap();

        if version != Self::VERSION {
            return Err(());
        }

        let inner = InnerMessage::decode(&self.inner[1..self.inner.len() - 8]).unwrap();

        let mut key = [0u8; 32];
        key.copy_from_slice(&inner.ratchet_key);

        let key = RemoteRatchetKey::from(key);
        let chain_index = inner.chain_index;
        let ciphertext = inner.ciphertext;

        Ok((key, chain_index, ciphertext))
    }

    fn from_parts_untyped(ratchet_key: Vec<u8>, index: u64, ciphertext: Vec<u8>) -> Self {
        // Prost optimizes away the chain index if it's 0, libolm can't decode
        // this, so encode our messages the pedestrian way instead.
        let index = index.encode();
        let ratchet_len = ratchet_key.len().encode();
        let ciphertext_len = ciphertext.len().encode();

        let message = [
            &[Self::VERSION].as_ref(),
            Self::RATCHET_TAG.as_ref(),
            &ratchet_len,
            &ratchet_key,
            Self::INDEX_TAG.as_ref(),
            &index,
            Self::CIPHER_TAG.as_ref(),
            &ciphertext_len,
            &ciphertext,
            &[0u8; 8],
        ]
        .concat();

        Self { inner: message }
    }

    pub(super) fn from_parts(
        ratchet_key: RatchetPublicKey,
        index: u64,
        ciphertext: Vec<u8>,
    ) -> Self {
        Self::from_parts_untyped(ratchet_key.to_vec(), index, ciphertext)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PrekeyMessage {
    pub(super) inner: Vec<u8>,
}

impl PrekeyMessage {
    const VERSION: u8 = 3;

    const ONE_TIME_KEY_TAG: &'static [u8; 1] = b"\x0A";
    const BASE_KEY_TAG: &'static [u8; 1] = b"\x12";
    const IDENTITY_KEY_TAG: &'static [u8; 1] = b"\x1A";
    const MESSAGE_TAG: &'static [u8; 1] = b"\x22";

    pub fn as_bytes(&self) -> &[u8] {
        self.inner.as_ref()
    }

    pub(super) fn from_parts_untyped(
        one_time_key: Vec<u8>,
        base_key: Vec<u8>,
        identity_key: Vec<u8>,
        message: Vec<u8>,
    ) -> Self {
        let one_time_key_len = one_time_key.len().encode();
        let base_key_len = base_key.len().encode();
        let identity_key_len = identity_key.len().encode();
        let message_len = message.len().encode();

        let message = [
            [Self::VERSION].as_ref(),
            Self::ONE_TIME_KEY_TAG,
            one_time_key_len.as_slice(),
            &one_time_key,
            Self::BASE_KEY_TAG,
            base_key_len.as_slice(),
            &base_key,
            Self::IDENTITY_KEY_TAG,
            identity_key_len.as_slice(),
            &identity_key,
            Self::MESSAGE_TAG,
            message_len.as_slice(),
            &message,
        ]
        .concat();

        Self { inner: message }
    }

    pub fn decode(
        self,
    ) -> Result<
        (
            Curve25591PublicKey,
            Curve25591PublicKey,
            Curve25591PublicKey,
            Vec<u8>,
        ),
        (),
    > {
        let version = *self.inner.get(0).unwrap();

        if version != Self::VERSION {
            return Err(());
        }

        let inner = InnerPreKeyMessage::decode(&self.inner[1..self.inner.len()]).unwrap();

        let mut one_time_key = [0u8; 32];
        let mut base_key = [0u8; 32];
        let mut identity_key = [0u8; 32];

        one_time_key.copy_from_slice(&inner.one_time_key);
        base_key.copy_from_slice(&inner.base_key);
        identity_key.copy_from_slice(&inner.identity_key);

        let one_time_key = Curve25591PublicKey::from(one_time_key);
        let base_key = Curve25591PublicKey::from(base_key);
        let identity_key = Curve25591PublicKey::from(identity_key);

        Ok((one_time_key, base_key, identity_key, inner.message))
    }

    pub(super) fn from_parts_untyped_prost(
        one_time_key: Vec<u8>,
        base_key: Vec<u8>,
        identity_key: Vec<u8>,
        message: Vec<u8>,
    ) -> Self {
        let message = InnerPreKeyMessage {
            one_time_key,
            base_key,
            identity_key,
            message,
        };

        let mut output: Vec<u8> = vec![0u8; message.encoded_len() + 1];
        output[0] = Self::VERSION;

        message.encode(&mut output[1..].as_mut()).unwrap();

        Self { inner: output }
    }
}

impl From<Vec<u8>> for PrekeyMessage {
    fn from(bytes: Vec<u8>) -> Self {
        Self {
            inner: bytes,
        }
    }
}

#[derive(Clone, Message, PartialEq)]
struct InnerMessage {
    #[prost(bytes, tag = "1")]
    pub ratchet_key: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub chain_index: u64,
    #[prost(bytes, tag = "4")]
    pub ciphertext: Vec<u8>,
}

#[derive(Clone, Message)]
struct InnerPreKeyMessage {
    #[prost(bytes, tag = "1")]
    pub one_time_key: Vec<u8>,
    #[prost(bytes, tag = "2")]
    pub base_key: Vec<u8>,
    #[prost(bytes, tag = "3")]
    pub identity_key: Vec<u8>,
    #[prost(bytes, tag = "4")]
    pub message: Vec<u8>,
}

#[cfg(test)]
mod test {
    use super::OlmMessage;

    #[test]
    fn encode() {
        let message = b"\x03\n\nratchetkey\x10\x01\"\nciphertext";
        let message_mac = b"\x03\n\nratchetkey\x10\x01\"\nciphertextMACHEREE";

        let ratchet_key = b"ratchetkey";
        let ciphertext = b"ciphertext";

        let mut encoded =
            OlmMessage::from_parts_untyped(ratchet_key.to_vec(), 1, ciphertext.to_vec());

        assert_eq!(encoded.as_payload_bytes(), message.as_ref());
        encoded.append_mac(b"MACHEREE");
        assert_eq!(encoded.as_payload_bytes(), message.as_ref());
        assert_eq!(encoded.as_bytes(), message_mac.as_ref());
    }
}
