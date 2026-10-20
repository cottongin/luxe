use super::*;

fn sighting(path: &str, last_seen: u64) -> Sighting {
    Sighting { path: path.into(), last_seen }
}

/// A sighting refreshes a folder already known and adds a new one, and the oldest go past the cap.
///
/// The break this catches, one per assertion: a second entry for a folder seen again; a new folder
/// not kept; or the cap dropping the newest instead of the longest unseen.
#[test]
fn sightings_refresh_add_and_forget_the_oldest() {
    let mut folders: Vec<Sighting> =
        (0..REMEMBERED as u64).map(|age| sighting(&format!("/old/{age}"), 100 - age)).collect();
    remember(&mut folders, &["/old/3".into(), "/new".into()], 1_000);
    assert_eq!(folders.iter().filter(|sighting| sighting.path == "/old/3").count(), 1);
    assert!(folders.iter().any(|sighting| sighting.path == "/new"));
    assert_eq!(folders.len(), REMEMBERED);
    let oldest = format!("/old/{}", REMEMBERED - 1);
    assert!(!folders.iter().any(|sighting| sighting.path == oldest), "the longest unseen goes");
}

/// Running folders lead with their counts, then herdr's, then remembered ones, each once and only
/// while it is a folder.
///
/// The break this catches, one per folder: losing the count (app), listing a folder twice (app
/// again, from herdr), losing herdr's (tools), or listing one that has since gone (gone).
#[test]
fn the_listing_leads_with_running_folders_and_lists_each_once() {
    let root = tempfile::tempdir().unwrap();
    let folder = |name: &str| {
        let path = root.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        path.to_string_lossy().into_owned()
    };
    let (app, tools, notes) = (folder("app"), folder("tools"), folder("notes"));
    let gone = root.path().join("gone").to_string_lossy().into_owned();
    let book = FolderBook::at(root.path().join("folders.json"));
    book.saw(&[notes.clone(), gone]).unwrap();

    let listed = listing(&[app.clone(), app.clone()], &[app.clone(), tools.clone()], &book);
    let paths: Vec<&str> = listed.iter().map(|folder| folder.path.as_str()).collect();
    assert_eq!(paths, [app.as_str(), tools.as_str(), notes.as_str()]);
    assert_eq!(listed[0].sessions, 2);
    assert_eq!(listed[0].name, "app");
    assert_eq!(listed[1].sessions, 0);
}

/// A folder seen within the hour isn't written again; one seen longer ago is.
///
/// The break this catches: rewriting the file on every list, which a phone polling the session
/// list turns into a write every few seconds.
#[test]
fn a_recent_sighting_is_not_written_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("folders.json");
    let book = FolderBook::at(path.clone());
    let marked = |last_seen: u64| {
        let text = format!(r#"{{"folders":[{{"path":"/app","last_seen":{last_seen}}}],"mark":1}}"#);
        std::fs::write(&path, text).unwrap();
    };
    marked(now());
    book.saw(&["/app".into()]).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("mark"), "rewritten while fresh");
    marked(0);
    book.saw(&["/app".into()]).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("mark"), "not rewritten when stale");
}
