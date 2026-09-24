use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use canopee_sdk::CanopeeClient;

use super::common::short_peer_id;

pub(crate) async fn pair(qr: Option<String>, code: Option<String>) {
    let client = CanopeeClient::connect().await.unwrap();
    match qr {
        // New-device side: mint a code/session and print the QR payload
        // for the user to read off to the other machine.
        None => match client.pair_initiate().await {
            Ok(qr) => {
                let payload = bincode::serialize(&qr).expect("pairing QR serializes");
                println!("Pairing code: {}", qr.code);
                println!();
                println!("Scan this QR (or read this payload to the other device) and run:");
                println!(
                    "  canopee pair {} --code {}",
                    BASE64.encode(&payload),
                    qr.code
                );
                println!();
                println!(
                    "The source device will copy your identity to {} ({}).",
                    qr.device_name, qr.device_id
                );
                println!("Restart this node after pairing to take the identity over.");
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        },
        // Source-device side: verify the user-typed code, and deliver
        // the identity to the new device over the LAN.
        Some(payload_b64) => {
            let bytes = match BASE64.decode(payload_b64.as_bytes()) {
                Ok(bytes) => bytes,
                Err(e) => {
                    eprintln!("The QR payload is not valid base64: {e}");
                    std::process::exit(1);
                }
            };
            let qr: canopee_protocol::PairingQrData = match bincode::deserialize(&bytes) {
                Ok(qr) => qr,
                Err(e) => {
                    eprintln!("The QR payload is not valid pairing data: {e}");
                    std::process::exit(1);
                }
            };
            let code = match code {
                Some(code) => code,
                None => {
                    use std::io::BufRead;
                    print!("Type the pairing code shown on the device to approve: ");
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                    let mut line = String::new();
                    std::io::stdin().lock().read_line(&mut line).unwrap();
                    line.trim().to_string()
                }
            };
            println!("Pairing {} …", short_peer_id(&qr.device_id));
            match client.pair_complete(qr, code).await {
                Ok(message) => {
                    println!("{message}");
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}
