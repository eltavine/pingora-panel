//! `resty.aes`: lua-resty-string's AES with OpenSSL's modes, padding and
//! `EVP_BytesToKey` key derivation, on RustCrypto's ciphers instead of
//! OpenSSL through an FFI.

use super::{failed, results};
use aes::{
    cipher::{
        array::{Array, ArraySize},
        block_padding::{NoPadding, Pkcs7},
        consts::{U12, U16, U24, U32, U8},
        BlockCipherDecrypt, BlockCipherEncrypt, BlockModeDecrypt, BlockModeEncrypt, BlockSizeUser,
        KeyInit, KeyIvInit, StreamCipher,
    },
    Aes128, Aes192, Aes256,
};
use aes_gcm::{AeadInOut, AesGcm};
use md5::Md5;
use mlua::{Function, Lua, LuaString, MultiValue, Table, UserData, UserDataMethods, Value};
use sha1::Sha1;
use sha2::{Digest, Sha224, Sha256, Sha384, Sha512};

const BLOCK: usize = 16;
const TAG: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Ecb,
    Cbc,
    Cfb1,
    Cfb8,
    Cfb128,
    Ofb,
    Ctr,
    Gcm,
}

impl Mode {
    fn named(name: &str) -> Option<Self> {
        Some(match name {
            "ecb" => Self::Ecb,
            "cbc" => Self::Cbc,
            "cfb1" => Self::Cfb1,
            "cfb8" => Self::Cfb8,
            "cfb128" => Self::Cfb128,
            "ofb" => Self::Ofb,
            "ctr" => Self::Ctr,
            "gcm" => Self::Gcm,
            _ => return None,
        })
    }

    /// The length of the IV OpenSSL gives the mode.
    const fn iv_len(self) -> usize {
        match self {
            Self::Ecb => 0,
            Self::Gcm => 12,
            _ => BLOCK,
        }
    }
}

const HASHES: [&str; 6] = ["md5", "sha1", "sha224", "sha256", "sha384", "sha512"];

fn digest(hash: &str, data: &[u8]) -> Vec<u8> {
    match hash {
        "sha1" => Sha1::digest(data).to_vec(),
        "sha224" => Sha224::digest(data).to_vec(),
        "sha256" => Sha256::digest(data).to_vec(),
        "sha384" => Sha384::digest(data).to_vec(),
        "sha512" => Sha512::digest(data).to_vec(),
        _ => Md5::digest(data).to_vec(),
    }
}

/// OpenSSL's `EVP_BytesToKey`: `length` bytes of digests, each of the one
/// before, the password and the salt, hashed `rounds` times.
fn bytes_to_key(
    hash: &str,
    password: &[u8],
    salt: Option<&[u8]>,
    rounds: usize,
    length: usize,
) -> Vec<u8> {
    let mut derived = Vec::with_capacity(length + 64);
    let mut previous = Vec::new();
    while derived.len() < length {
        let mut input = previous;
        input.extend_from_slice(password);
        input.extend_from_slice(salt.unwrap_or_default());
        let mut block = digest(hash, &input);
        for _ in 1..rounds {
            block = digest(hash, &block);
        }
        derived.extend_from_slice(&block);
        previous = block;
    }
    derived.truncate(length);
    derived
}

/// CFB with one bit of feedback, most significant bit first, as OpenSSL's
/// `aes-*-cfb1`.
fn cfb1<C: BlockCipherEncrypt + BlockSizeUser<BlockSize = U16>>(
    cipher: &C,
    iv: &[u8],
    data: &mut [u8],
    decrypting: bool,
) {
    let mut register = Array::<u8, U16>::try_from(iv).unwrap_or_default();
    for byte in data {
        let mut output = 0;
        for bit in (0..8).rev() {
            let mut stream = register;
            cipher.encrypt_block(&mut stream);
            let input = (*byte >> bit) & 1;
            let produced = input ^ (stream[0] >> 7);
            output |= produced << bit;
            let fed = if decrypting { input } else { produced };
            for index in 0..BLOCK - 1 {
                register[index] = (register[index] << 1) | (register[index + 1] >> 7);
            }
            register[BLOCK - 1] = (register[BLOCK - 1] << 1) | fed;
        }
        *byte = output;
    }
}

fn gcm<C, N>(
    key: &[u8],
    nonce: &[u8],
    aad: &[u8],
    data: &mut [u8],
    tag: Option<&[u8]>,
) -> Option<Vec<u8>>
where
    C: BlockCipherEncrypt + BlockSizeUser<BlockSize = U16> + KeyInit,
    N: ArraySize,
{
    let cipher = AesGcm::<C, N>::new_from_slice(key).ok()?;
    let nonce = Array::<u8, N>::try_from(nonce).ok()?;
    match tag {
        None => cipher
            .encrypt_inout_detached(&nonce, aad, data.into())
            .ok()
            .map(|tag| tag.to_vec()),
        Some(tag) => {
            let tag = Array::<u8, U16>::try_from(tag).ok()?;
            cipher
                .decrypt_inout_detached(&nonce, aad, data.into(), &tag)
                .ok()
                .map(|()| Vec::new())
        }
    }
}

/// What a cipher object does with its key and IV.
struct Aes {
    mode: Mode,
    key: Vec<u8>,
    iv: Vec<u8>,
    padding: bool,
}

/// What `encrypt` and `decrypt` were asked, and why they could not.
enum Outcome {
    Done(Vec<u8>),
    Sealed(Vec<u8>, Vec<u8>),
    Failed(&'static str),
}

impl Aes {
    fn run<C>(&self, data: &[u8], decrypting: bool, tag: Option<&[u8]>, aad: &[u8]) -> Outcome
    where
        C: BlockCipherEncrypt + BlockCipherDecrypt + BlockSizeUser<BlockSize = U16> + KeyInit,
    {
        let (key, iv) = (&self.key[..], &self.iv[..]);
        let finished = if decrypting {
            "EVP_DecryptFinal_ex failed"
        } else {
            "EVP_EncryptFinal_ex failed"
        };
        let mut buffer = data.to_vec();
        match self.mode {
            Mode::Ecb | Mode::Cbc => {
                if !self.padding && !data.len().is_multiple_of(BLOCK) {
                    return Outcome::Failed(finished);
                }
                let done = match (self.mode, decrypting, self.padding) {
                    (Mode::Ecb, false, true) => ecb::Encryptor::<C>::new_from_slice(key)
                        .map(|mode| mode.encrypt_padded_vec::<Pkcs7>(data))
                        .ok(),
                    (Mode::Ecb, false, false) => ecb::Encryptor::<C>::new_from_slice(key)
                        .map(|mode| mode.encrypt_padded_vec::<NoPadding>(data))
                        .ok(),
                    (Mode::Ecb, true, true) => ecb::Decryptor::<C>::new_from_slice(key)
                        .ok()
                        .and_then(|mode| mode.decrypt_padded_vec::<Pkcs7>(data).ok()),
                    (Mode::Ecb, true, false) => ecb::Decryptor::<C>::new_from_slice(key)
                        .ok()
                        .and_then(|mode| mode.decrypt_padded_vec::<NoPadding>(data).ok()),
                    (_, false, true) => cbc::Encryptor::<C>::new_from_slices(key, iv)
                        .map(|mode| mode.encrypt_padded_vec::<Pkcs7>(data))
                        .ok(),
                    (_, false, false) => cbc::Encryptor::<C>::new_from_slices(key, iv)
                        .map(|mode| mode.encrypt_padded_vec::<NoPadding>(data))
                        .ok(),
                    (_, true, true) => cbc::Decryptor::<C>::new_from_slices(key, iv)
                        .ok()
                        .and_then(|mode| mode.decrypt_padded_vec::<Pkcs7>(data).ok()),
                    (_, true, false) => cbc::Decryptor::<C>::new_from_slices(key, iv)
                        .ok()
                        .and_then(|mode| mode.decrypt_padded_vec::<NoPadding>(data).ok()),
                };
                return done.map_or(Outcome::Failed(finished), Outcome::Done);
            }
            Mode::Cfb1 => match C::new_from_slice(key) {
                Ok(cipher) => cfb1(&cipher, iv, &mut buffer, decrypting),
                Err(_) => return Outcome::Failed(finished),
            },
            Mode::Cfb8 if decrypting => match cfb8::Decryptor::<C>::new_from_slices(key, iv) {
                Ok(mut mode) => mode.decrypt(&mut buffer),
                Err(_) => return Outcome::Failed(finished),
            },
            Mode::Cfb8 => match cfb8::Encryptor::<C>::new_from_slices(key, iv) {
                Ok(mut mode) => mode.encrypt(&mut buffer),
                Err(_) => return Outcome::Failed(finished),
            },
            Mode::Cfb128 if decrypting => {
                match cfb_mode::BufDecryptor::<C>::new_from_slices(key, iv) {
                    Ok(mut mode) => mode.decrypt(&mut buffer),
                    Err(_) => return Outcome::Failed(finished),
                }
            }
            Mode::Cfb128 => match cfb_mode::BufEncryptor::<C>::new_from_slices(key, iv) {
                Ok(mut mode) => mode.encrypt(&mut buffer),
                Err(_) => return Outcome::Failed(finished),
            },
            Mode::Ofb => match ofb::Ofb::<C>::new_from_slices(key, iv) {
                Ok(mut mode) => mode.apply_keystream(&mut buffer),
                Err(_) => return Outcome::Failed(finished),
            },
            Mode::Ctr => match ctr::Ctr128BE::<C>::new_from_slices(key, iv) {
                Ok(mut mode) => mode.apply_keystream(&mut buffer),
                Err(_) => return Outcome::Failed(finished),
            },
            Mode::Gcm => {
                if decrypting && tag.is_none_or(|tag| tag.len() != TAG) {
                    return Outcome::Failed(finished);
                }
                let tag = if decrypting { tag } else { None };
                let sealed = match iv.len() {
                    8 => gcm::<C, U8>(key, iv, aad, &mut buffer, tag),
                    12 => gcm::<C, U12>(key, iv, aad, &mut buffer, tag),
                    16 => gcm::<C, U16>(key, iv, aad, &mut buffer, tag),
                    24 => gcm::<C, U24>(key, iv, aad, &mut buffer, tag),
                    32 => gcm::<C, U32>(key, iv, aad, &mut buffer, tag),
                    _ => None,
                };
                return match sealed {
                    Some(_) if decrypting => Outcome::Done(buffer),
                    Some(tag) => Outcome::Sealed(buffer, tag),
                    None => Outcome::Failed(finished),
                };
            }
        }
        Outcome::Done(buffer)
    }

    fn apply(&self, data: &[u8], decrypting: bool, tag: Option<&[u8]>, aad: &[u8]) -> Outcome {
        match self.key.len() {
            16 => self.run::<Aes128>(data, decrypting, tag, aad),
            24 => self.run::<Aes192>(data, decrypting, tag, aad),
            _ => self.run::<Aes256>(data, decrypting, tag, aad),
        }
    }
}

fn answer(lua: &Lua, outcome: Outcome) -> mlua::Result<MultiValue> {
    match outcome {
        Outcome::Done(data) => Ok(results([Value::String(lua.create_string(data)?)])),
        Outcome::Sealed(data, tag) => {
            let pair =
                lua.create_sequence_from([lua.create_string(data)?, lua.create_string(tag)?])?;
            Ok(results([Value::Table(pair)]))
        }
        Outcome::Failed(why) => failed(lua, 1, why),
    }
}

impl UserData for Aes {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method(
            "encrypt",
            |lua, this, (data, aad): (LuaString, Option<LuaString>)| {
                let aad = aad.as_ref().map(LuaString::as_bytes);
                let outcome = this.apply(
                    &data.as_bytes(),
                    false,
                    None,
                    aad.as_deref().unwrap_or_default(),
                );
                answer(lua, outcome)
            },
        );
        methods.add_method(
            "decrypt",
            |lua, this, (data, tag, aad): (LuaString, Option<LuaString>, Option<LuaString>)| {
                let tag = tag.as_ref().map(LuaString::as_bytes);
                let aad = aad.as_ref().map(LuaString::as_bytes);
                let outcome = this.apply(
                    &data.as_bytes(),
                    true,
                    tag.as_deref(),
                    aad.as_deref().unwrap_or_default(),
                );
                answer(lua, outcome)
            },
        );
    }
}

type Arguments = (
    Value,
    LuaString,
    Option<LuaString>,
    Option<Table>,
    Value,
    Option<usize>,
    Option<usize>,
    Option<bool>,
);

fn new(
    lua: &Lua,
    (_, key, salt, cipher, hash, rounds, iv_len, padding): Arguments,
) -> mlua::Result<MultiValue> {
    let (size, mode) = match &cipher {
        Some(cipher) => (
            cipher.get::<Option<usize>>("size")?.unwrap_or(128),
            cipher
                .get::<Option<String>>("cipher")?
                .unwrap_or_else(|| "cbc".into()),
        ),
        None => (128, "cbc".into()),
    };
    let (Some(mode), 128 | 192 | 256) = (Mode::named(&mode), size) else {
        return failed(lua, 1, "bad cipher");
    };
    let key_len = size / 8;
    let (key, iv) = match hash {
        Value::Table(raw) => {
            let Some(iv) = raw.get::<Option<LuaString>>("iv")? else {
                return failed(lua, 1, "iv is needed");
            };
            let iv = iv.as_bytes().to_vec();
            if iv.len() > key_len {
                return failed(lua, 1, "bad iv length");
            }
            let key = match raw.get::<Option<Function>>("method")? {
                Some(method) => method.call::<LuaString>(key)?.as_bytes().to_vec(),
                None => key.as_bytes().to_vec(),
            };
            if key.len() != key_len {
                return failed(lua, 1, "bad key length");
            }
            (key, iv)
        }
        hash => {
            let hash = match &hash {
                Value::Nil => "md5".to_owned(),
                Value::String(name) => name.to_str()?.to_owned(),
                _ => return failed(lua, 1, "bad hash"),
            };
            if !HASHES.contains(&hash.as_str()) {
                return failed(lua, 1, "bad hash");
            }
            let salt = salt.as_ref().map(LuaString::as_bytes);
            if salt.as_ref().is_some_and(|salt| salt.len() != 8) {
                return failed(lua, 1, "salt must be 8 characters or nil");
            }
            let derived = bytes_to_key(
                &hash,
                &key.as_bytes(),
                salt.as_deref(),
                rounds.unwrap_or(1).max(1),
                key_len + mode.iv_len(),
            );
            let (key, iv) = derived.split_at(key_len);
            let iv = if mode == Mode::Gcm {
                let wanted = iv_len.unwrap_or(key_len);
                if wanted > key_len {
                    return failed(lua, 1, "bad iv length");
                }
                let mut nonce = iv.to_vec();
                nonce.resize(wanted, 0);
                nonce
            } else {
                iv.to_vec()
            };
            (key.to_vec(), iv)
        }
    };
    let iv = match mode {
        Mode::Ecb => Vec::new(),
        Mode::Gcm => iv,
        _ => {
            let mut iv = iv;
            iv.resize(BLOCK, 0);
            iv
        }
    };
    let aes = Aes {
        mode,
        key,
        iv,
        padding: padding.unwrap_or(true),
    };
    Ok(results([Value::UserData(lua.create_userdata(aes)?)]))
}

/// The `resty.aes` module.
pub(super) fn module(lua: &Lua) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    module.raw_set("_VERSION", "0.16")?;
    let hashes = lua.create_table()?;
    for name in HASHES {
        hashes.raw_set(name, name)?;
    }
    module.raw_set("hash", hashes)?;
    module.raw_set(
        "cipher",
        lua.create_function(|lua, (size, mode): (Option<usize>, Option<String>)| {
            let size = size.unwrap_or(128);
            let mode = mode.unwrap_or_else(|| "cbc".into());
            if !matches!(size, 128 | 192 | 256) || Mode::named(&mode).is_none() {
                return Ok(Value::Nil);
            }
            let cipher = lua.create_table()?;
            cipher.raw_set("size", size)?;
            cipher.raw_set("method", format!("aes-{size}-{mode}"))?;
            cipher.raw_set("cipher", mode)?;
            Ok(Value::Table(cipher))
        })?,
    )?;
    module.raw_set("new", lua.create_function(new)?)?;
    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        hex::decode(text).unwrap()
    }

    fn sealed(aes: &Aes, data: &[u8]) -> Vec<u8> {
        match aes.apply(data, false, None, b"") {
            Outcome::Done(data) => data,
            _ => panic!("not encrypted"),
        }
    }

    #[test]
    fn modes_give_nist_sp_800_38a_ciphertexts() {
        let key = hex("2b7e151628aed2a6abf7158809cf4f3c");
        let iv = hex("000102030405060708090a0b0c0d0e0f");
        let plain = hex("6bc1bee22e409f96e93d7e117393172a");
        for (mode, iv, plain, expected) in [
            (
                Mode::Ecb,
                vec![],
                plain.clone(),
                "3ad77bb40d7a3660a89ecaf32466ef97",
            ),
            (
                Mode::Cbc,
                iv.clone(),
                plain.clone(),
                "7649abac8119b246cee98e9b12e9197d",
            ),
            (
                Mode::Cfb128,
                iv.clone(),
                plain.clone(),
                "3b3fd92eb72dad20333449f8e83cfb4a",
            ),
            (
                Mode::Ofb,
                iv.clone(),
                plain.clone(),
                "3b3fd92eb72dad20333449f8e83cfb4a",
            ),
            (
                Mode::Ctr,
                hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff"),
                plain.clone(),
                "874d6191b620e3261bef6864990db6ce",
            ),
            (
                Mode::Cfb8,
                iv.clone(),
                hex("6bc1bee22e409f96e93d7e117393172aae2d"),
                "3b79424c9c0dd436bace9e0ed4586a4f32b9",
            ),
            (
                Mode::Cfb1,
                iv.clone(),
                plain.clone(),
                "68b3a264f838f5f8c3101070d1ab4c2e",
            ),
        ] {
            let aes = Aes {
                mode,
                key: key.clone(),
                iv,
                padding: false,
            };
            let encrypted = sealed(&aes, &plain);
            assert_eq!(hex::encode(&encrypted), expected, "{mode:?}");
            let Outcome::Done(decrypted) = aes.apply(&encrypted, true, None, b"") else {
                panic!("{mode:?} not decrypted");
            };
            assert_eq!(decrypted, plain, "{mode:?}");
        }
    }

    #[test]
    fn bytes_to_key_derives_what_openssl_enc_does() {
        let derived = bytes_to_key("md5", b"AKeyForAES", None, 1, 32);
        assert_eq!(
            hex::encode(derived),
            "fcc41dd2eaf1f07e166c6f82b2c39153ab5b6c1fa5b460748fc4260feb0335ac"
        );
        let derived = bytes_to_key("sha512", b"AKeyForAES-256-CBC", Some(b"MySalt!!"), 1, 48);
        assert_eq!(
            hex::encode(derived),
            "f9e55bf7ee606bd10f45c5881696ba00a9169c3d14e7254f1b23361abe4766404ef5b982b94825d9f6d8af6c5a5bef5f"
        );
    }

    #[test]
    fn gcm_seals_and_checks_as_its_specification_has_it() {
        let aes = Aes {
            mode: Mode::Gcm,
            key: vec![0; 16],
            iv: vec![0; 12],
            padding: true,
        };
        let Outcome::Sealed(data, tag) = aes.apply(&[0; 16], false, None, b"") else {
            panic!("not sealed");
        };
        assert_eq!(hex::encode(&data), "0388dace60b6a392f328c2b971b2fe78");
        assert_eq!(hex::encode(&tag), "ab6e47d42cec13bdf53a67b21257bddf");
        assert!(
            matches!(aes.apply(&data, true, Some(&tag), b""), Outcome::Done(plain) if plain == [0; 16])
        );
        let mut forged = tag.clone();
        forged[0] ^= 1;
        assert!(matches!(
            aes.apply(&data, true, Some(&forged), b""),
            Outcome::Failed(_)
        ));
        assert!(matches!(
            aes.apply(&data, true, Some(&tag), b"aad"),
            Outcome::Failed(_)
        ));
        assert!(matches!(
            aes.apply(&data, true, None, b""),
            Outcome::Failed(_)
        ));
    }
}
