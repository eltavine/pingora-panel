use super::*;
use chrono::TimeZone;
use std::fs;

fn staged() -> tempfile::TempDir {
    let staging = tempfile::tempdir().unwrap();
    let root = staging.path();
    fs::create_dir_all(root.join("databases")).unwrap();
    fs::write(root.join("databases/config.db"), b"SQLite format 3\0config").unwrap();
    fs::create_dir_all(root.join("sites/shop/assets")).unwrap();
    fs::create_dir_all(root.join("sites/shop/empty")).unwrap();
    fs::write(root.join("sites/shop/index.html"), b"<h1>Shop</h1>\n").unwrap();
    let long = format!("sites/shop/assets/{}.css", "a".repeat(180));
    fs::write(root.join(long), b"body {}\n").unwrap();
    staging
}

fn taken() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2027, 1, 15, 8, 0, 0).unwrap()
}

#[test]
fn archives_list_what_they_hold_and_are_checked_whole() {
    let staging = staged();
    let out = tempfile::tempdir().unwrap();
    let archive = out.path().join("backup.tar.zst");
    let written = write(
        &archive,
        staging.path(),
        "1.2.3",
        &["configuration", "sites"],
        taken(),
    )
    .unwrap();
    assert_eq!(written.format, FORMAT);
    assert_eq!(written.contents, ["configuration", "sites"]);
    assert_eq!(
        written.directories,
        [
            "databases",
            "sites",
            "sites/shop",
            "sites/shop/assets",
            "sites/shop/empty"
        ]
    );
    assert_eq!(written.members.len(), 3);
    let index = written.member("sites/shop/index.html").unwrap();
    assert_eq!(index.size, 14);
    assert_eq!(
        index.sha256,
        hex::encode(Sha256::digest(b"<h1>Shop</h1>\n"))
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&archive).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "archives are read by their owner only");
    }

    assert_eq!(manifest(&archive).unwrap(), written);
    assert_eq!(verify(&archive).unwrap(), written);
    assert_eq!(
        read_member(&archive, "databases/config.db", 1024).unwrap(),
        b"SQLite format 3\0config"
    );
    let refused = read_member(&archive, "databases/config.db", 4).unwrap_err();
    assert_eq!(refused.code.as_str(), "RESOURCE_EXHAUSTED");

    let copy = out.path().join("config.db");
    assert_eq!(
        copy_member(&archive, "databases/config.db", &copy).unwrap(),
        22
    );
    assert_eq!(fs::read(&copy).unwrap(), b"SQLite format 3\0config");
}

#[test]
fn a_directory_comes_back_whole_with_its_empty_directories() {
    let staging = staged();
    let out = tempfile::tempdir().unwrap();
    let archive = out.path().join("backup.tar.zst");
    write(&archive, staging.path(), "1.2.3", &["sites"], taken()).unwrap();

    let restored = out.path().join("shop");
    let extraction = extract(&archive, "sites/shop", &restored).unwrap();
    assert_eq!(extraction.files, 2);
    assert_eq!(
        fs::read(restored.join("index.html")).unwrap(),
        b"<h1>Shop</h1>\n"
    );
    assert!(restored.join("empty").is_dir());
    assert!(!restored.join("config.db").exists());

    let missing = extract(&archive, "sites/blog", &out.path().join("blog")).unwrap_err();
    assert_eq!(missing.code.as_str(), "NOT_FOUND");
    assert!(!out.path().join("blog").exists());
    let existing = extract(&archive, "sites/shop", &restored).unwrap_err();
    assert_eq!(existing.code.as_str(), "STORAGE_UNAVAILABLE");
}

/// An archive whose manifest lists `listed` but which holds `held`.
fn forged(path: &Path, listed: &[u8], held: &[(&str, &[u8])]) {
    let manifest = Manifest {
        format: FORMAT.to_owned(),
        format_version: FORMAT_VERSION,
        product_version: "1.2.3".to_owned(),
        created_at: taken(),
        contents: vec!["sites".to_owned()],
        directories: vec!["sites".to_owned()],
        members: vec![Member {
            path: "sites/index.html".to_owned(),
            size: listed.len() as u64,
            sha256: hex::encode(Sha256::digest(listed)),
        }],
    };
    let encoder = zstd::Encoder::new(File::create(path).unwrap(), 3).unwrap();
    let mut builder = tar::Builder::new(encoder);
    let listed = serde_json::to_vec(&manifest).unwrap();
    let mut entries = vec![(MANIFEST.to_owned(), listed)];
    entries.extend(
        held.iter()
            .map(|(name, content)| (name.to_string(), content.to_vec())),
    );
    for (name, content) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        let bytes = name.as_bytes();
        header.as_old_mut().name[..bytes.len()].copy_from_slice(bytes);
        header.set_cksum();
        builder.append(&header, &content[..]).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap();
}

#[test]
fn damaged_archives_are_refused_and_leave_nothing_behind() {
    let out = tempfile::tempdir().unwrap();
    let archive = out.path().join("forged.tar.zst");

    forged(
        &archive,
        b"<h1>Shop</h1>",
        &[("sites/index.html", b"<h1>Shoq</h1>")],
    );
    assert!(manifest(&archive).is_ok(), "the manifest alone reads");
    let changed = verify(&archive).unwrap_err();
    assert_eq!(changed.code.as_str(), "VALIDATION_FAILED");
    assert!(
        changed.message.contains("not the content listed"),
        "{changed}"
    );
    let restored = out.path().join("restored");
    assert!(extract(&archive, "sites", &restored).is_err());
    assert!(!restored.exists(), "a damaged archive restores nothing");

    fs::remove_file(&archive).unwrap();
    forged(&archive, b"<h1>Shop</h1>", &[]);
    let missing = verify(&archive).unwrap_err();
    assert!(missing.message.contains("is missing"), "{missing}");

    fs::remove_file(&archive).unwrap();
    forged(
        &archive,
        b"<h1>Shop</h1>",
        &[("sites/index.html", b"<h1>Shop</h1>"), ("../escape", b"x")],
    );
    let escaping = verify(&archive).unwrap_err();
    assert!(escaping.message.contains("not listed"), "{escaping}");
    assert!(!out.path().parent().unwrap().join("escape").exists());

    fs::write(&archive, b"not an archive").unwrap();
    assert_eq!(
        manifest(&archive).unwrap_err().code.as_str(),
        "VALIDATION_FAILED"
    );
}

#[test]
fn only_files_and_directories_are_archived() {
    let staging = staged();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd", staging.path().join("sites/passwd")).unwrap();
        let out = tempfile::tempdir().unwrap();
        let archive = out.path().join("backup.tar.zst");
        let refused = write(&archive, staging.path(), "1.2.3", &["sites"], taken()).unwrap_err();
        assert_eq!(refused.code.as_str(), "INVALID_ARGUMENT");
        assert!(!archive.exists(), "a refused archive is not left behind");
    }
}
