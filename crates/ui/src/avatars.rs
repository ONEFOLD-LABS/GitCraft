//! Author images on the commit graph, from Gravatar, only when the user switched them on
//! (docs/decisions/ADR-005).
//!
//! What leaves the machine: one HTTPS request per author, `https://gravatar.com/avatar/<hash>`,
//! where `<hash>` is the SHA-256 of the trimmed, lower-cased e-mail address, with a generic
//! user agent and nothing else. Answers are cached on disk (images for 7 days, "no image" for
//! 1 day), so each author is asked for at most once a week. Until an image is there, and for
//! authors without one, the node shows the initials.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// Requested image size in pixels (drawn at about 19 px, sharp on HiDPI).
const SIZE: u32 = 64;
const MAX_BYTES: u64 = 256 * 1024;
const MAX_SIDE: u32 = 512;
const KEEP_IMAGE: Duration = Duration::from_secs(7 * 24 * 3600);
const KEEP_NONE: Duration = Duration::from_secs(24 * 3600);

/// The Gravatar key for an e-mail address.
pub(crate) fn hash(email: &str) -> String {
    let digest = Sha256::digest(email.trim().to_lowercase().as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// An answer of the loader thread: the Gravatar key and the image, if there is one.
type Loaded = (String, Option<egui::ColorImage>);

enum Entry {
    Pending,
    Missing,
    Ready(egui::TextureHandle),
}

/// Loaded and requested author images. Drawing asks for images through a shared reference;
/// requests go to one background thread.
pub(crate) struct Avatars {
    enabled: bool,
    cache: Option<PathBuf>,
    entries: RefCell<HashMap<String, Entry>>,
    requests: RefCell<Option<Sender<String>>>,
    results: RefCell<Option<Receiver<Loaded>>>,
}

impl Avatars {
    /// Avatars switched off: initials only, no requests.
    pub(crate) fn off() -> Avatars {
        Avatars {
            enabled: false,
            cache: None,
            entries: RefCell::new(HashMap::new()),
            requests: RefCell::new(None),
            results: RefCell::new(None),
        }
    }

    /// Avatars switched on, cached in `cache` (the platform cache directory).
    pub(crate) fn on(cache: Option<PathBuf>) -> Avatars {
        Avatars {
            enabled: true,
            cache,
            ..Avatars::off()
        }
    }

    /// The image for `email` if it is loaded; asks for it otherwise.
    pub(crate) fn get(&self, ctx: &egui::Context, email: &str) -> Option<egui::TextureId> {
        if !self.enabled {
            return None;
        }
        let key = hash(email);
        let mut entries = self.entries.borrow_mut();
        match entries.get(&key) {
            Some(Entry::Ready(t)) => return Some(t.id()),
            Some(Entry::Pending | Entry::Missing) => return None,
            None => {}
        }
        entries.insert(key.clone(), Entry::Pending);
        drop(entries);
        self.request(ctx, key);
        None
    }

    fn request(&self, ctx: &egui::Context, key: String) {
        let mut requests = self.requests.borrow_mut();
        if requests.is_none() {
            let (tx, rx) = channel::<String>();
            let (done_tx, done_rx) = channel();
            let cache = self.cache.clone();
            let ctx = ctx.clone();
            let started = std::thread::Builder::new()
                .name("gitcraft-avatars".into())
                .spawn(move || {
                    let agent = agent();
                    while let Ok(key) = rx.recv() {
                        let image = load(&agent, cache.as_deref(), &key);
                        if done_tx.send((key, image)).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                });
            if started.is_err() {
                return;
            }
            *requests = Some(tx);
            *self.results.borrow_mut() = Some(done_rx);
        }
        if let Some(tx) = requests.as_ref() {
            let _ = tx.send(key);
        }
    }

    /// Takes images that arrived and uploads them as textures.
    pub(crate) fn poll(&self, ctx: &egui::Context) {
        let arrived: Vec<_> = match self.results.borrow().as_ref() {
            Some(rx) => rx.try_iter().collect(),
            None => return,
        };
        let mut entries = self.entries.borrow_mut();
        for (key, image) in arrived {
            let entry = match image {
                Some(img) => Entry::Ready(ctx.load_texture(
                    format!("avatar-{key}"),
                    img,
                    egui::TextureOptions::LINEAR,
                )),
                None => Entry::Missing,
            };
            entries.insert(key, entry);
        }
    }
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent("GitCraft")
        .max_redirects(2)
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::Rustls)
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}

/// From the cache if fresh, else from Gravatar (then cached). `None`: no image.
fn load(agent: &ureq::Agent, cache: Option<&Path>, key: &str) -> Option<egui::ColorImage> {
    if let Some(dir) = cache {
        if fresh(&dir.join(format!("{key}.none")), KEEP_NONE) {
            return None;
        }
        let file = dir.join(format!("{key}.img"));
        if fresh(&file, KEEP_IMAGE)
            && let Ok(bytes) = std::fs::read(&file)
        {
            return decode(&bytes);
        }
    }
    let url = format!("https://gravatar.com/avatar/{key}?s={SIZE}&d=404");
    match agent.get(&url).call() {
        Ok(mut resp) => {
            let bytes = resp
                .body_mut()
                .with_config()
                .limit(MAX_BYTES)
                .read_to_vec()
                .ok()?;
            let image = decode(&bytes)?;
            if let Some(dir) = cache {
                store(dir, &format!("{key}.img"), &bytes);
            }
            Some(image)
        }
        Err(ureq::Error::StatusCode(404)) => {
            if let Some(dir) = cache {
                store(dir, &format!("{key}.none"), b"");
            }
            None
        }
        // Offline, blocked, timed out: try again in a later session.
        Err(_) => None,
    }
}

fn fresh(path: &Path, keep: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < keep)
}

fn store(dir: &Path, name: &str, bytes: &[u8]) {
    let _ = std::fs::create_dir_all(dir);
    let tmp = dir.join(format!("{name}.tmp"));
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(name));
    }
}

/// PNG or JPEG, at most [`MAX_SIDE`] pixels per side; anything else is no image.
pub(crate) fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(4 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_sha256_of_the_normalised_address() {
        // sha256("test@example.com")
        assert_eq!(
            hash("  Test@Example.com \n"),
            "973dfe463ec85785f5f95af5ba3906eedb2d931c24e69824a89ea65dba4e813b"
        );
    }

    #[test]
    fn switched_off_never_asks() {
        let a = Avatars::off();
        assert_eq!(a.get(&egui::Context::default(), "a@b"), None);
        assert!(a.requests.borrow().is_none(), "no thread, no request");
    }

    #[test]
    fn cached_images_and_misses_are_used_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 10, 10, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        store(dir.path(), "aaaa.img", &png);
        store(dir.path(), "bbbb.none", b"");
        // An agent that cannot reach anything: proves the cache answered.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .proxy(ureq::Proxy::new("http://127.0.0.1:9").ok())
            .build()
            .into();
        let img = load(&agent, Some(dir.path()), "aaaa").unwrap();
        assert_eq!(img.size, [8, 8]);
        assert!(load(&agent, Some(dir.path()), "bbbb").is_none());
    }

    #[test]
    fn hostile_images_are_refused() {
        assert!(decode(b"not an image").is_none());
        let mut big = Vec::new();
        image::RgbaImage::new(1024, 4)
            .write_to(&mut std::io::Cursor::new(&mut big), image::ImageFormat::Png)
            .unwrap();
        assert!(decode(&big).is_none(), "wider than the limit");
    }
}
