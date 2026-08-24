//! `RustNet.Security` — hashing, message authentication and symmetric
//! encryption for a device.
//!
//! Thin over [`rustclr_crypto`], which writes the algorithms out rather than
//! depending on them. What is worth saying at *this* layer is what the API
//! shape encourages:
//!
//! - **`VerifyHmac` exists** so that a program never has to write
//!   `computed == expected` on a tag. That comparison exits at the first
//!   differing byte, which leaks where it is and lets a tag be forged one byte
//!   at a time. The binding compares in constant time.
//! - **Encrypt and decrypt are one method.** Counter mode is its own inverse,
//!   and two names would imply a distinction that does not exist and a safety
//!   that is not there.
//! - **There is no `Random`.** Entropy belongs to the hardware; a board
//!   without a generator says so through the HAL rather than being handed a
//!   counter dressed up as randomness.

use crate::support::*;
use rustclr_core::{ExecutionError, Interpreter, Value};

#[allow(unused_imports)]
use crate::prelude::*;

fn argument_error(what: &str, message: &str) -> ExecutionError {
    ExecutionError::exception(
        rustclr_core::ClrExceptionKind::Argument,
        &format!("{what}: {message}"),
    )
}

pub fn register(interp: &mut Interpreter) {
    interp.register_native("RustNet.Security.Hash::Sha256(byte[])", |i, a| {
        let data = arg_bytes(i, a, 0)?;
        let digest = rustclr_crypto::sha256(&data);
        let array = byte_array(i, &digest);
        Ok(Some(Value::Obj(array)))
    });
    interp.register_native("RustNet.Security.Hash::Sha256Hex(byte[])", |i, a| {
        let data = arg_bytes(i, a, 0)?;
        let hex = rustclr_crypto::sha256::hex(&rustclr_crypto::sha256(&data));
        Ok(Some(string_value(i, &hex)))
    });
    interp.register_native("RustNet.Security.Hash::Crc32(byte[])", |i, a| {
        let data = arg_bytes(i, a, 0)?;
        // As `u32` bits in an `i32`: a CRC is a bit pattern, not a magnitude,
        // and reinterpreting is what `unchecked((int)crc)` does in C#.
        Ok(Some(Value::I32(rustclr_crypto::crc32(&data) as i32)))
    });

    interp.register_native("RustNet.Security.Hmac::Compute(byte[],byte[])", |i, a| {
        let key = arg_bytes(i, a, 0)?;
        let message = arg_bytes(i, a, 1)?;
        let tag = rustclr_crypto::hmac_sha256(&key, &message);
        let array = byte_array(i, &tag);
        Ok(Some(Value::Obj(array)))
    });
    // The reason this is a binding rather than something a program writes: a
    // `SequenceEqual` on a tag exits at the first differing byte.
    interp.register_native("RustNet.Security.Hmac::Verify(byte[],byte[],byte[])", |i, a| {
        let key = arg_bytes(i, a, 0)?;
        let message = arg_bytes(i, a, 1)?;
        let expected = arg_bytes(i, a, 2)?;
        let tag = rustclr_crypto::hmac_sha256(&key, &message);
        let ok = rustclr_crypto::constant_time_eq(&tag, &expected);
        Ok(Some(Value::I32(i32::from(ok))))
    });

    interp.register_native(
        "RustNet.Security.Cipher::Transform(byte[],byte[],byte[])",
        |i, a| {
            let key = arg_bytes(i, a, 0)?;
            let nonce = arg_bytes(i, a, 1)?;
            let mut data = arg_bytes(i, a, 2)?;
            if nonce.len() != 16 {
                return Err(argument_error(
                    "Cipher.Transform",
                    "the counter block is 16 bytes; a shorter one would be padded with \
                     something nobody chose",
                ));
            }
            let mut block = [0u8; 16];
            block.copy_from_slice(&nonce);
            if !rustclr_crypto::aes_ctr(&key, &block, &mut data) {
                return Err(argument_error(
                    "Cipher.Transform",
                    "the key must be 16 or 32 bytes (AES-128 or AES-256)",
                ));
            }
            let array = byte_array(i, &data);
            Ok(Some(Value::Obj(array)))
        },
    );
}
