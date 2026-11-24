//! One live smoke test against the real control plane, run by hand and never by CI:
//!
//! ```text
//! cargo test -p versevision-bible --test live_smoke -- --ignored --nocapture
//! ```
//!
//! It enrolls a throwaway installation (a unique nonce, device name `versevision-s5b-test`), fetches John 3:16 in KJV, NKJV, NIV
//! and ESV through [`StagePilotProvider`], checks the text and that the ESV answer is `cached: false`, then revokes the
//! installation with its own credential. The credential is never printed; ESV text is not printed either.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Client;
use serde_json::{json, Value};
use versevision_bible::{InstallationAuth, Secret, StagePilotProvider};
use versevision_core::bibles::BibleProvider;
use versevision_core::references::{VersePoint, VerseSpan};

const ORIGIN: &str = "https://stagepilot.illuminary.studio";

struct Enrolled {
    id: String,
    credential: String,
}

impl InstallationAuth for Enrolled {
    fn origin(&self) -> String {
        ORIGIN.to_string()
    }

    fn installation_id(&self) -> Option<String> {
        Some(self.id.clone())
    }

    fn credential(&self) -> Option<Secret> {
        Some(Secret::new(self.credential.clone()))
    }
}

fn nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("vv-s5b-smoke-{nanos}-{}", std::process::id())
}

#[tokio::test]
#[ignore = "talks to the real control plane; run by hand"]
async fn enroll_fetch_john_3_16_and_revoke() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = Client::builder()
        .user_agent(format!(
            "VerseVision/{}",
            versevision_core::VERSEVISION_VERSION
        ))
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .unwrap();

    // 1. enroll a throwaway installation
    let response = client
        .post(format!("{ORIGIN}/v1/installations/enroll"))
        .header("content-type", "application/json")
        .body(json!({"nonce": nonce(), "deviceName": "versevision-s5b-test"}).to_string())
        .send()
        .await
        .expect("the control plane is reachable");
    let status = response.status().as_u16();
    let body: Value = response.json_text().await;
    assert!(
        status == 200 || status == 201,
        "enroll answered HTTP {status}"
    );
    let keys: Vec<&String> = body
        .as_object()
        .map(|o| o.keys().collect())
        .unwrap_or_default();
    let id = body["installationId"]
        .as_str()
        .unwrap_or_else(|| panic!("an installationId; the answer has the keys {keys:?}"))
        .to_string();
    let credential = body["installationCredential"]
        .as_str()
        .expect("a credential")
        .to_string();
    println!("enrolled installation {id} (HTTP {status}); credential not printed");
    let auth = Arc::new(Enrolled {
        id: id.clone(),
        credential: credential.clone(),
    });

    // 2. fetch John 3:16 in four versions (inside a block so the installation is revoked whatever happens)
    let outcome = async {
        let stored = StagePilotProvider::new("stagepilot", Some(auth.clone()));
        let esv = StagePilotProvider::for_esv("stagepilot-esv", Some(auth.clone()));
        let span = [VerseSpan {
            start: VersePoint {
                chapter: 3,
                verse: 16,
            },
            end: None,
        }];
        let mut report = Vec::new();
        for version in ["KJV", "NKJV", "NIV", "ESV"] {
            let provider = if version == "ESV" { &esv } else { &stored };
            let passage = provider
                .fetch_reference(version, "JHN 3:16")
                .await
                .map_err(|e| format!("{version}: {e}"))?;
            let verses = provider
                .fetch_passage(version, "JHN", &span)
                .await
                .map_err(|e| format!("{version}: {e}"))?;
            let text = &verses[0].text;
            // the raw answer, to see what the service sends before the cleanup
            let raw: Value = {
                let url = format!(
                    "{ORIGIN}/v1/installations/{id}/bible/passage?version={version}&ref=JHN%203:16"
                );
                let response = client
                    .get(url)
                    .header("authorization", format!("Bearer {credential}"))
                    .send()
                    .await
                    .map_err(|e| format!("{version} raw: {e}"))?;
                let status = response.status().as_u16();
                let value = response.json_text().await;
                if status != 200 {
                    return Err(format!("{version} raw: HTTP {status} {}", value["error"]));
                }
                value
            };
            let raw_text = raw["verses"][0]["text"].as_str().unwrap_or_default();
            let line = format!(
                "{version}: provider={} cached={} raw_starts_with_pilcrow={} raw_cached={} text_chars={} copyright={}",
                passage.provider,
                passage.cached,
                raw_text.starts_with('\u{b6}'),
                raw["cached"],
                text.chars().count(),
                verses[0].copyright.is_some(),
            );
            println!("{line}");
            if version == "ESV" {
                // a diagnostic that does not print the text: the first characters as code points
                let head: Vec<String> = text.chars().take(6).map(|c| format!("U+{:04X}", c as u32)).collect();
                println!("ESV: first characters {}", head.join(" "));
            }
            report.push(line);
            // ESV opens John 3:16 with a quotation mark (Jesus is speaking); Python's ESV provider shows it too
            if !text.trim_start_matches(['\u{201c}', '"']).starts_with("For God so loved the world") {
                return Err(format!("{version}: unexpected text start ({} chars)", text.chars().count()));
            }
            if text.starts_with('\u{b6}') || text.starts_with('[') {
                return Err(format!("{version}: the text still starts with a mark"));
            }
            if verses[0].book_id != "JHN" || verses[0].chapter != 3 || verses[0].verse != 16 {
                return Err(format!("{version}: wrong verse"));
            }
            if version == "ESV" {
                if passage.provider != "esv" || passage.cached {
                    return Err("ESV: expected provider esv and cached false".to_string());
                }
            } else if version == "KJV" || version == "NKJV" {
                if passage.provider != "api_bible" {
                    return Err(format!("{version}: expected provider api_bible"));
                }
            } else if passage.provider != "youversion" {
                return Err(format!("{version}: expected provider youversion"));
            }
        }
        Ok::<(), String>(())
    }
    .await;

    // 3. revoke the installation with its own credential
    let revoke = client
        .post(format!("{ORIGIN}/v1/installations/{id}/revoke"))
        .header("authorization", format!("Bearer {credential}"))
        .send()
        .await
        .expect("revoke is reachable");
    let revoke_status = revoke.status().as_u16();
    println!("revoked installation {id}: HTTP {revoke_status}");
    // after the revoke the credential is refused
    let after = StagePilotProvider::new("stagepilot", Some(auth))
        .fetch_reference("KJV", "JHN 3:16")
        .await
        .unwrap_err();
    println!("after the revoke: {after}");

    outcome.expect("the passages came back as expected");
    assert!(
        (200..300).contains(&revoke_status),
        "revoke answered HTTP {revoke_status}"
    );
    assert!(after.message().contains("credential was rejected"));
}

trait JsonText {
    async fn json_text(self) -> Value;
}

impl JsonText for reqwest::Response {
    async fn json_text(self) -> Value {
        let bytes = self.bytes().await.expect("a body");
        serde_json::from_slice(&bytes).expect("JSON")
    }
}
