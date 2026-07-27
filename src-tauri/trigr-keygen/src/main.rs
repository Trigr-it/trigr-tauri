// Keyfire licence key generator.
//
// Emits KEYFIRE. keys by default: 10-byte binary payload + Ed25519 signature,
// prefix "KEYFIRE.", ~110 chars total. See src-tauri/src/licence.rs top-of-file
// comment for the exact byte layout.
//
// Subcommands:
//   init                                          Generate keypair, save private key,
//                                                 print public key for licence.rs.
//   sign --email <e> [--days N] [--tier T]        Sign a new licence key. Each
//                                                 issued key is appended to a
//                                                 local CSV log (use --no-log
//                                                 to skip). --email is required
//                                                 for the CSV log only; it is
//                                                 NOT embedded in the key.
//
// Private key path defaults to %USERPROFILE%\.trigr\private-signing-key.bin.
// Log path defaults to %USERPROFILE%\.trigr\issued-keys.csv.
// Override either with TRIGR_SIGNING_KEY or TRIGR_KEY_LOG env vars.

use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use chrono::{Duration, Utc};
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use rand::RngCore;

// Payload v2 (KEYFIRE.) layout — see licence.rs for the verifier side.
const PAYLOAD_V2_VERSION: u8 = 0x01;
const PAYLOAD_V2_TIER_PRO: u8 = 0x01;
const KEY_PREFIX: &str = "KEYFIRE.";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return ExitCode::FAILURE;
    }

    match args[0].as_str() {
        "init" => cmd_init(),
        "sign" => cmd_sign(&args[1..]),
        "help" | "--help" | "-h" => {
            print_usage();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("Unknown subcommand: {}", other);
            print_usage();
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    eprintln!(
        r#"Keyfire licence key generator

Run from inside src-tauri/trigr-keygen/, or invoke the built binary at
src-tauri/trigr-keygen/target/release/trigr-keygen.exe directly.

Usage:
  cargo run --release -- init
      Generate a new Ed25519 keypair, save the private key to disk,
      and print the public key to paste into licence.rs.

  cargo run --release -- sign --email <e> [--days N] [--tier T] [--no-log] [--key-only]
      Sign a new licence key. Defaults: --days 30, --tier pro.
      A row is appended to issued-keys.csv unless --no-log is passed.
      --key-only prints ONLY the key string to stdout with no trailing
      newline (and no header, fields, or log-path line). Designed to be
      piped straight to a clipboard tool, e.g. PowerShell:
        trigr-keygen sign --email <e> --key-only | Set-Clipboard
      The CSV log is still written unless --no-log is also passed.

Environment:
  TRIGR_SIGNING_KEY    Path to the private key file. Defaults to
                       %USERPROFILE%\.trigr\private-signing-key.bin
  TRIGR_KEY_LOG        Path to the CSV log of issued keys. Defaults to
                       %USERPROFILE%\.trigr\issued-keys.csv
"#
    );
}

fn default_key_path() -> PathBuf {
    let home = env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    PathBuf::from(home)
        .join(".trigr")
        .join("private-signing-key.bin")
}

fn key_path() -> PathBuf {
    env::var_os("TRIGR_SIGNING_KEY")
        .map(PathBuf::from)
        .unwrap_or_else(default_key_path)
}

fn cmd_init() -> ExitCode {
    let path = key_path();
    if path.exists() {
        eprintln!("Refusing to overwrite existing key at:");
        eprintln!("  {}", path.display());
        eprintln!();
        eprintln!("If you really want to regenerate, delete that file first.");
        eprintln!("WARNING: regenerating means every issued key stops working.");
        return ExitCode::FAILURE;
    }

    let mut csprng = OsRng;
    let signing_key = SigningKey::generate(&mut csprng);
    let verifying_key = signing_key.verifying_key();

    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("Failed to create directory {}: {}", parent.display(), e);
            return ExitCode::FAILURE;
        }
    }
    if let Err(e) = fs::write(&path, signing_key.to_bytes()) {
        eprintln!("Failed to write private key: {}", e);
        return ExitCode::FAILURE;
    }

    let pub_b64 = B64.encode(verifying_key.to_bytes());

    println!();
    println!("=== Ed25519 keypair generated ===");
    println!();
    println!("Private key saved to:");
    println!("  {}", path.display());
    println!();
    println!("IMPORTANT - back up this file to 1Password (or equivalent) now.");
    println!("If you lose it, you cannot issue more keys without shipping a new");
    println!("app version that embeds a new public key.");
    println!();
    println!("Public key (paste into licence.rs PUBLIC_KEY_B64):");
    println!();
    println!("  {}", pub_b64);
    println!();

    ExitCode::SUCCESS
}

fn cmd_sign(args: &[String]) -> ExitCode {
    let mut email: Option<String> = None;
    let mut days: i64 = 30;
    let mut tier: String = "pro".to_string();
    let mut no_log = false;
    let mut key_only = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--email" => {
                i += 1;
                email = args.get(i).cloned();
            }
            "--days" => {
                i += 1;
                days = args
                    .get(i)
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(30);
            }
            "--tier" => {
                i += 1;
                tier = args.get(i).cloned().unwrap_or_else(|| "pro".to_string());
            }
            "--no-log" => {
                no_log = true;
            }
            "--key-only" => {
                key_only = true;
            }
            other => {
                eprintln!("Unknown arg: {}", other);
                return ExitCode::FAILURE;
            }
        }
        i += 1;
    }

    let email = match email {
        Some(e) if !e.is_empty() => e,
        _ => {
            eprintln!("--email is required");
            return ExitCode::FAILURE;
        }
    };

    let path = key_path();
    let key_bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to read private key from {}: {}", path.display(), e);
            eprintln!("Run `cargo run --release -- init` from src-tauri/trigr-keygen/ first.");
            return ExitCode::FAILURE;
        }
    };
    if key_bytes.len() != 32 {
        eprintln!("Private key file is not 32 bytes (got {})", key_bytes.len());
        return ExitCode::FAILURE;
    }
    let mut key_array = [0u8; 32];
    key_array.copy_from_slice(&key_bytes);
    let signing_key = SigningKey::from_bytes(&key_array);

    let exp = Utc::now() + Duration::days(days);

    // Tier byte — extend the match when new tiers ship. Keep this in sync
    // with the PAYLOAD_V2_TIER_* constants in licence.rs.
    let tier_byte = match tier.as_str() {
        "pro" => PAYLOAD_V2_TIER_PRO,
        other => {
            eprintln!("Unknown tier: {} (only 'pro' is supported today)", other);
            return ExitCode::FAILURE;
        }
    };

    // u32 unix seconds — good until year 2106. Well beyond any 30-day beta key.
    let exp_unix: u32 = exp.timestamp().try_into().unwrap_or_else(|_| {
        eprintln!("Refusing to sign a key with an exp outside the u32 unix-second range.");
        std::process::exit(1);
    });

    // Random 4-byte id. Rendered as 8-char lowercase hex for the CSV log
    // and for the "Key ID: xxxxxxxx" display on the user's Licence card.
    let id_bytes: [u8; 4] = {
        let mut buf = [0u8; 4];
        OsRng.fill_bytes(&mut buf);
        buf
    };
    let id = hex_encode(&id_bytes);

    // Build the 10-byte binary payload.
    let mut payload = [0u8; 10];
    payload[0] = PAYLOAD_V2_VERSION;
    payload[1] = tier_byte;
    payload[2..6].copy_from_slice(&exp_unix.to_be_bytes());
    payload[6..10].copy_from_slice(&id_bytes);

    let signature = signing_key.sign(&payload);

    let payload_b64 = B64.encode(payload);
    let sig_b64 = B64.encode(signature.to_bytes());
    // `.` is not in the base64url alphabet, so it's an unambiguous separator
    // even when the payload or signature happens to contain `-` or `_`.
    let licence_key = format!("{}{}.{}", KEY_PREFIX, payload_b64, sig_b64);

    if key_only {
        // Clipboard-safe path: ONLY the key string, no newline, no extras.
        // Designed to be piped straight to Set-Clipboard / pbcopy / xclip.
        // The CSV log still runs (silently) unless --no-log was also passed.
        print!("{}", licence_key);
        let _ = std::io::stdout().flush();
        if !no_log {
            let _ = append_to_log(&email, &tier, days, &exp.to_rfc3339(), &id);
        }
        return ExitCode::SUCCESS;
    }

    println!();
    println!("=== Licence key ===");
    println!();
    println!("  Email:   {}", email);
    println!("  Tier:    {}", tier);
    println!("  Expires: {} ({} days)", exp.format("%Y-%m-%d"), days);
    println!("  ID:      {}", id);
    println!();
    println!("Copy the key below and send it to the user:");
    println!();
    println!("{}", licence_key);
    println!();

    if !no_log {
        match append_to_log(&email, &tier, days, &exp.to_rfc3339(), &id) {
            Ok(path) => println!("Logged to: {}", path.display()),
            Err(e) => eprintln!("(warning: failed to write log entry: {})", e),
        }
    }
    println!();

    ExitCode::SUCCESS
}

fn log_path() -> PathBuf {
    env::var_os("TRIGR_KEY_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".trigr").join("issued-keys.csv")
        })
}

fn csv_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn append_to_log(
    email: &str,
    tier: &str,
    days: i64,
    expires_at: &str,
    id: &str,
) -> Result<PathBuf, String> {
    let path = log_path();
    let needs_header = !path.exists();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open {}: {}", path.display(), e))?;
    if needs_header {
        writeln!(file, "issued_at,email,tier,days,expires_at,id")
            .map_err(|e| format!("write header: {}", e))?;
    }
    let issued_at = Utc::now().to_rfc3339();
    writeln!(
        file,
        "{},{},{},{},{},{}",
        issued_at,
        csv_quote(email),
        csv_quote(tier),
        days,
        csv_quote(expires_at),
        csv_quote(id)
    )
    .map_err(|e| format!("write row: {}", e))?;
    Ok(path)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}
