//! VerseVision Bible providers (S5): API.Bible, YouVersion, the ESV API and local USFM files, behind the planner's
//! [`versevision_core::bibles::BibleProvider`] trait; the key store; the scripture cache.
//!
//! The planner reads Bible text through `ProviderRegistry<AnyProvider>` (routing by `bible.version_routes`, in `versevision-core`).
//! [`RegistryBuilder`] makes one from a `[bible]` config, a [`KeyStore`] and an optional [`ScriptureCache`]; the host app names the
//! keys file and the cache folder. The library starts no runtime and spawns nothing: its futures run on whatever executor drives
//! them (StagePilot's tokio runtime, or the standalone binary's), and they are `Send`.
//!
//! Behaviour is Python's (`python/src/versevision/bibles/`): same endpoints, headers, query parameters, error texts, timeouts and
//! (no) retry. See `oracle/providers/` for what is compared with Python.

mod api_bible;
mod cache;
mod entities;
mod esv;
pub mod html;
mod http;
mod keys;
mod provider;
pub mod pyjson;
mod remote;
mod secret;
mod usfm;
mod youversion;

pub use api_bible::{verse_text as api_bible_verse_text, ApiBibleProvider};
pub use cache::ScriptureCache;
pub use esv::{EsvProvider, COPYRIGHT as ESV_COPYRIGHT};
pub use keys::{FileKeyStore, KeyError, KeyStore, MemoryKeyStore};
pub use provider::{AnyProvider, BoxFuture, DynBibleProvider, RegistryBuilder};
pub use secret::Secret;
pub use usfm::LocalUsfmProvider;
pub use youversion::YouVersionProvider;
