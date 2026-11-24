//! `versevision keys`: store, remove and check the Bible API keys. Write-only: a key is read from a hidden prompt or from standard input,
//! never from an argument, and nothing here (or anywhere in this binary) prints one. Python has no such command; its keys file
//! (`keys.env`, lines `NAME=value`) and the environment variables it reads are what this edits.

use versevision_bible::Secret;
use versevision_bible::{FileKeyStore, KeyStore};

/// The providers a key can be stored for and the variable each one reads (`api_key_env` of the default provider configs).
pub const PROVIDERS: [(&str, &str); 3] = [
    ("api_bible", "API_BIBLE_API_KEY"),
    ("youversion", "YOUVERSION_API_KEY"),
    ("esv", "ESV_API_KEY"),
];

pub fn variable_for(provider: &str) -> Option<&'static str> {
    let wanted = provider.trim().to_lowercase().replace('-', "_");
    PROVIDERS
        .iter()
        .find(|(name, var)| *name == wanted || var.to_lowercase() == wanted)
        .map(|(_, var)| *var)
}

pub fn store(path: &std::path::Path) -> FileKeyStore {
    FileKeyStore::new(path)
}

pub fn is_set(store: &dyn KeyStore, variable: &str) -> bool {
    store.has_key(variable)
}

pub fn secret(text: &str) -> Secret {
    Secret::new(text)
}
