//! Browser extensions: listing and installing.

use super::*;

impl BrowserApp {
    pub(super) fn request_extension_list(&self) {
        let view = self
            .persistent_view()
            .filter(|_| EXTENSIONS_ON.load(std::sync::atomic::Ordering::Relaxed));
        match view {
            Some(view) => webview::list_extensions(view, self.proxy.clone()),
            None => {
                let _ = self
                    .proxy
                    .send_event(BrowserEvent::ExtensionsListed("[]".into()));
            }
        }
    }

    /// Download an extension from the Chrome Web Store (by link or id) and load it.
    pub(super) fn install_extension_from_store(&self, source: &str) {
        let say = |app: &Self, title: &str, text: &str| {
            app.toast(&serde_json::json!({ "title": title, "text": text, "pose": "unfold" }));
        };
        if !EXTENSIONS_ON.load(std::sync::atomic::Ordering::Relaxed) {
            say(
                self,
                "Turn extensions on first.",
                "Switch them on in Settings and restart Shoji.",
            );
            return;
        }
        let Some(id) = extstore::parse_id(source) else {
            say(
                self,
                "That isn't an extension link.",
                "Paste the page's address from the Chrome Web Store, or the 32-letter id.",
            );
            return;
        };
        say(
            self,
            "Downloading the extension…",
            "It is added in a moment.",
        );
        let proxy = self.proxy.clone();
        let dir = self.data_dir.join("Extensions");
        let _ = std::thread::Builder::new()
            .name("shoji-extension".into())
            .spawn(move || {
                let _ =
                    proxy.send_event(BrowserEvent::ExtensionFetched(extstore::fetch(&id, &dir)));
            });
    }

    /// Pick an unpacked extension, copy it into the data folder (so moving the original can't
    /// break it) and hand it to the engine.
    pub(super) fn add_extension_from_folder(&self) {
        let say = |app: &Self, title: &str, text: &str| {
            app.toast(&serde_json::json!({ "title": title, "text": text, "pose": "unfold" }));
        };
        if !EXTENSIONS_ON.load(std::sync::atomic::Ordering::Relaxed) {
            say(
                self,
                "Turn extensions on first.",
                "Switch them on in Settings and restart Shoji.",
            );
            return;
        }
        let Some(view) = self.persistent_view() else {
            say(
                self,
                "Open a website first.",
                "Extensions are added to the browsing profile.",
            );
            return;
        };
        let start = webview::documents_dir().unwrap_or_else(|| self.data_dir.clone());
        let Some(picked) = webview::pick_folder(&start) else {
            return;
        };
        if !picked.join("manifest.json").is_file() {
            say(
                self,
                "Not an extension folder.",
                "It needs a manifest.json at the top. Unzip the extension first.",
            );
            return;
        }
        let extensions_dir = self.data_dir.join("Extensions");
        // A folder already inside our own copies is loaded in place; copying it onto itself
        // would delete it first.
        let dest = if picked
            .canonicalize()
            .ok()
            .zip(extensions_dir.canonicalize().ok())
            .is_some_and(|(p, e)| p.starts_with(e))
        {
            picked
        } else {
            // Named after the folder plus a hash of where it came from, so two extensions that
            // are both unzipped to "dist" don't overwrite each other.
            let stem: String = picked
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "extension".into())
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let hash = picked
                .to_string_lossy()
                .to_lowercase()
                .bytes()
                .fold(0xcbf29ce484222325u64, |h, b| {
                    (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
                });
            let dest = extensions_dir.join(format!("{stem}-{:08x}", hash as u32));
            if copy_dir_all(&picked, &dest, 0).is_err() {
                say(
                    self,
                    "Couldn't copy the extension.",
                    "Check that the folder can be read.",
                );
                return;
            }
            dest
        };
        webview::add_extension(view, &dest, self.proxy.clone());
    }
}
