//! Readers for bookmark formats that are plain files: Chromium's `Bookmarks` JSON, the Netscape
//! HTML export every browser can produce, and a folder of `.url` shortcuts.

use crate::userdata::Favourite;
use std::path::Path;

fn web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

fn push(out: &mut Vec<Favourite>, url: &str, title: &str, folder: &str) {
    let url = url.trim();
    if web(url) && !out.iter().any(|f| f.url == url) {
        out.push(Favourite {
            url: url.to_string(),
            title: title.trim().to_string(),
            folder: folder.to_string(),
        });
    }
}

fn join(parent: &str, name: &str) -> String {
    // A "/" inside an imported folder name stays part of the name (it would start a new level).
    let name = crate::userdata::clean_folder(&name.replace('/', "-"));
    match (parent.is_empty(), name.is_empty()) {
        (_, true) => parent.to_string(),
        (true, false) => name,
        (false, false) => format!("{parent} / {name}"),
    }
}

/// Chromium `Bookmarks`: the bookmark bar first (top level), then Other bookmarks and the
/// rest, each in a folder of its own. Sub-folders keep their names.
pub fn from_chromium_json(json: &str) -> Vec<Favourite> {
    fn walk(node: &serde_json::Value, folder: &str, out: &mut Vec<Favourite>) {
        match node["type"].as_str() {
            Some("url") => push(
                out,
                node["url"].as_str().unwrap_or(""),
                node["name"].as_str().unwrap_or(""),
                folder,
            ),
            _ => {
                if let Some(kids) = node["children"].as_array() {
                    let inner = join(folder, node["name"].as_str().unwrap_or(""));
                    kids.iter().for_each(|k| walk(k, &inner, out));
                }
            }
        }
    }
    let Ok(root) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, top) in [
        ("bookmark_bar", ""),
        ("other", "Other bookmarks"),
        ("synced", "Mobile bookmarks"),
    ] {
        if let Some(kids) = root["roots"][key]["children"].as_array() {
            kids.iter().for_each(|k| walk(k, top, &mut out));
        }
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&amp;", "&")
}

/// The Netscape bookmark format: `<A HREF="...">Title</A>` entries, nested in `<DL>` folders
/// named by the `<H3>` before them. The browser's own bookmarks bar folder is the top level.
pub fn from_netscape_html(html: &str) -> Vec<Favourite> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    // One entry per open <DL>: the folder path inside it.
    let mut stack: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;
    loop {
        let next = ["<a ", "<h3", "<dl", "</dl"]
            .iter()
            .filter_map(|t| lower[at..].find(t).map(|i| (at + i, *t)))
            .min_by_key(|(i, _)| *i);
        let Some((start, tag)) = next else { break };
        let Some(tag_end) = lower[start..].find('>') else {
            break;
        };
        let body_start = start + tag_end + 1;
        match tag {
            "<dl" => {
                let parent = stack.last().cloned().unwrap_or_default();
                let path = match pending.take() {
                    Some(name) => join(&parent, &name),
                    None => parent,
                };
                stack.push(path);
                at = body_start;
            }
            "</dl" => {
                stack.pop();
                at = body_start;
            }
            "<h3" => {
                let body_end = lower[body_start..]
                    .find("</h3>")
                    .map_or(html.len(), |e| body_start + e);
                let name = unescape(&strip_tags(&html[body_start..body_end]));
                let bar = lower[start..start + tag_end].contains("personal_toolbar_folder");
                pending = Some(if bar { String::new() } else { name });
                at = body_end;
            }
            _ => {
                let tag_text = &html[start..start + tag_end];
                let tag_lower = &lower[start..start + tag_end];
                let body_end = lower[body_start..]
                    .find("</a>")
                    .map_or(html.len(), |e| body_start + e);
                if let Some(h) = tag_lower.find("href=") {
                    let rest = &tag_text[h + 5..];
                    let url = match rest.chars().next() {
                        Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
                        _ => rest.split_whitespace().next().unwrap_or(""),
                    };
                    let title = unescape(&strip_tags(&html[body_start..body_end]));
                    let folder = stack.last().map(String::as_str).unwrap_or("");
                    push(&mut out, &unescape(url), &title, folder);
                }
                at = body_end;
            }
        }
    }
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Internet Explorer's Favorites folder: `Name.url` files holding `URL=…`.
pub fn from_url_files(dir: &Path) -> Vec<Favourite> {
    fn walk(dir: &Path, out: &mut Vec<Favourite>, depth: usize, folder: &str) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            if path.is_dir() && depth < 6 {
                let name = e.file_name().to_string_lossy().into_owned();
                walk(&path, out, depth + 1, &join(folder, &name));
            } else if path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("url"))
            {
                let text = std::fs::read(&path)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                let url = text
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("URL="))
                    .unwrap_or("");
                let title = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                push(out, url, &title, folder);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out, 0, "");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromium_tree_is_flattened_in_bar_order() {
        let json = r#"{"roots":{
          "bookmark_bar":{"type":"folder","children":[
            {"type":"url","name":"A","url":"https://a.com/"},
            {"type":"folder","children":[{"type":"url","name":"B","url":"https://b.com/"}]}]},
          "other":{"type":"folder","children":[
            {"type":"url","name":"A again","url":"https://a.com/"},
            {"type":"url","name":"Script","url":"javascript:alert(1)"},
            {"type":"url","name":"C","url":"http://c.com"}]},
          "synced":{"type":"folder","children":[]}}}"#;
        let got: Vec<_> = from_chromium_json(json)
            .into_iter()
            .map(|f| f.title)
            .collect();
        assert_eq!(got, ["A", "B", "C"]);
        assert!(from_chromium_json("not json").is_empty());
    }

    #[test]
    fn netscape_html_is_parsed() {
        let html = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1><DL><p>
          <DT><H3>Folder</H3><DL><p>
          <DT><A HREF="https://example.com/?a=1&amp;b=2" ADD_DATE="1">Ex &amp; <b>co</b></A>
          <DT><a href='http://plain.org'>Plain</a>
          <DT><A HREF="place:sort=8">Skipped</A></DL><p></DL>"#;
        let got = from_netscape_html(html);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].url, "https://example.com/?a=1&b=2");
        assert_eq!(got[0].title, "Ex & co");
        assert_eq!(got[1].url, "http://plain.org");
    }

    #[test]
    fn url_shortcuts_are_read_recursively() {
        let dir = std::env::temp_dir().join(format!("shoji_fav_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("News")).unwrap();
        std::fs::write(
            dir.join("Home.url"),
            "[InternetShortcut]\r\nURL=https://home.example/\r\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("News").join("Paper.url"),
            "[InternetShortcut]\nURL=https://paper.example/\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "ignore").unwrap();
        let got: Vec<_> = from_url_files(&dir).into_iter().map(|f| f.title).collect();
        assert_eq!(got, ["Home", "Paper"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folders_survive_import() {
        let json = r#"{"roots":{
          "bookmark_bar":{"type":"folder","children":[
            {"type":"url","name":"Top","url":"https://top.com/"},
            {"type":"folder","name":"Dev","children":[
              {"type":"url","name":"Rust","url":"https://rust.dev/"},
              {"type":"folder","name":"Docs","children":[
                {"type":"url","name":"Std","url":"https://std.dev/"}]}]}]},
          "other":{"type":"folder","children":[
            {"type":"url","name":"Misc","url":"https://misc.com/"}]}}}"#;
        let got = from_chromium_json(json);
        let folder = |t: &str| got.iter().find(|f| f.title == t).unwrap().folder.clone();
        assert_eq!(folder("Top"), "");
        assert_eq!(folder("Rust"), "Dev");
        assert_eq!(folder("Std"), "Dev / Docs");
        assert_eq!(folder("Misc"), "Other bookmarks");

        let html = r#"<DL><p><DT><H3 PERSONAL_TOOLBAR_FOLDER="true">Bookmarks bar</H3><DL><p>
          <DT><A HREF="https://bar.com/">Bar</A>
          <DT><H3>Read</H3><DL><p><DT><A HREF="https://r.com/">R</A></DL><p>
          <DT><A HREF="https://after.com/">After</A></DL><p></DL>"#;
        let got = from_netscape_html(html);
        let folder = |t: &str| got.iter().find(|f| f.title == t).unwrap().folder.clone();
        assert_eq!(folder("Bar"), "");
        assert_eq!(folder("R"), "Read");
        assert_eq!(folder("After"), "");
    }
}
