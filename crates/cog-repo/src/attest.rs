//! Detached signatures over release documents (release-manifest.json, provenance).
//!
//!   weft-cog-repo sign-file   <file.json> --key <priv.pem> [--out <file.sig>]
//!   weft-cog-repo verify-file <file.json> [--sig <file.sig>] [--pin <pubkey-hex>]...
//!
//! The signature is Ed25519 over the exact bytes of the file, written as 128 hex
//! characters and a newline. Only well-formed JSON is signed: a JSON document is
//! never an ELF binary, so a signature made here cannot stand in for a cog binary
//! signature made by `sign`.

use cog_repo::weavelogic_key;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::path::{Path, PathBuf};

type R<T> = Result<T, String>;

fn sig_path(file: &Path, explicit: Option<&str>) -> PathBuf {
    match explicit {
        Some(p) => PathBuf::from(p),
        None => {
            let mut s = file.as_os_str().to_owned();
            s.push(".sig");
            PathBuf::from(s)
        }
    }
}

fn require_json(bytes: &[u8], file: &Path) -> R<()> {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .map(|_| ())
        .map_err(|e| {
            format!(
                "{}: only JSON release documents are signed ({e})",
                file.display()
            )
        })
}

pub fn sign_bytes(key: &SigningKey, bytes: &[u8]) -> String {
    format!("{}\n", hex::encode(key.sign(bytes).to_bytes()))
}

pub fn verify_bytes(keys: &[VerifyingKey], bytes: &[u8], sig_hex: &str) -> R<()> {
    let raw: [u8; 64] = hex::decode(sig_hex.trim())
        .map_err(|e| format!("signature is not hex: {e}"))?
        .try_into()
        .map_err(|_| "signature is not 64 bytes".to_string())?;
    let sig = Signature::from_bytes(&raw);
    if keys.iter().any(|k| k.verify(bytes, &sig).is_ok()) {
        Ok(())
    } else {
        Err("Ed25519 signature rejected (not signed by a pinned key)".into())
    }
}

pub fn cmd_sign_file(args: &[String], key: &SigningKey) -> R<()> {
    let file = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .ok_or("sign-file needs <file>")?;
    if key.verifying_key().to_bytes() != weavelogic_key().to_bytes() {
        return Err("the signing key does not match the pinned WeaveLogic public key".into());
    }
    let file = Path::new(file);
    let bytes = std::fs::read(file).map_err(|e| format!("read {}: {e}", file.display()))?;
    require_json(&bytes, file)?;
    let out = sig_path(file, crate::arg(args, "--out"));
    std::fs::write(&out, sign_bytes(key, &bytes))
        .map_err(|e| format!("write {}: {e}", out.display()))?;
    eprintln!("signed {} -> {}", file.display(), out.display());
    Ok(())
}

pub fn cmd_verify_file(args: &[String], pins: Vec<VerifyingKey>) -> R<()> {
    let file = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .ok_or("verify-file needs <file>")?;
    let file = Path::new(file);
    let bytes = std::fs::read(file).map_err(|e| format!("read {}: {e}", file.display()))?;
    let sp = sig_path(file, crate::arg(args, "--sig"));
    let sig = std::fs::read_to_string(&sp).map_err(|e| format!("read {}: {e}", sp.display()))?;
    verify_bytes(&pins, &bytes, &sig).map_err(|e| format!("{}: {e}", file.display()))?;
    println!(
        "  OK    {} ({})",
        file.display(),
        &cog_repo::sha256_hex(&bytes)[..16]
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signed_document_verifies_and_one_changed_byte_is_rejected() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let pins = [key.verifying_key()];
        let doc = br#"{"a":1}"#;
        let sig = sign_bytes(&key, doc);
        verify_bytes(&pins, doc, &sig).unwrap();
        assert!(verify_bytes(&pins, br#"{"a":2}"#, &sig).is_err());
        let other = SigningKey::from_bytes(&[6; 32]).verifying_key();
        assert!(verify_bytes(&[other], doc, &sig).is_err());
        assert!(verify_bytes(&pins, doc, "zz").is_err());
    }

    #[test]
    fn only_json_is_signable() {
        assert!(require_json(b"\x7fELF probe", Path::new("x")).is_err());
        assert!(require_json(b"{}", Path::new("x")).is_ok());
    }
}
