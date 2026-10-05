use super::*;
use panel_application::{IdempotencyKey, RequestDeadline, RequestId};

fn scope() -> RequestScope {
    RequestScope::new(RequestId::new("request-1").unwrap())
}

fn context() -> CommandContext {
    CommandContext::new(
        RequestId::new("request-1").unwrap(),
        RequestId::new("request-1").unwrap(),
        "ops",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new("key-1").unwrap(),
    )
    .unwrap()
}

fn path(value: &str) -> SitePath {
    SitePath::parse(value).unwrap()
}

/// shop/ with an index and assets/, and a file at the top.
fn sites() -> (tempfile::TempDir, LocalSiteFiles) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sites");
    std::fs::create_dir_all(root.join("shop/assets")).unwrap();
    std::fs::write(root.join("shop/index.html"), "<h1>Shop</h1>").unwrap();
    std::fs::write(root.join("shop/assets/app.js"), "alert(1)").unwrap();
    std::fs::write(root.join("robots.txt"), "User-agent: *").unwrap();
    let files = LocalSiteFiles::open(&root).unwrap();
    (directory, files)
}

#[tokio::test]
async fn directories_list_directories_first() {
    let (_directory, files) = sites();
    let top = files.directory(scope(), path("")).await.unwrap();
    let names: Vec<_> = top
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.kind, entry.size_bytes))
        .collect();
    assert_eq!(
        names,
        [
            ("shop", SiteEntryKind::Directory, 0),
            ("robots.txt", SiteEntryKind::File, 13)
        ]
    );
    assert!(top.entries[1].modified.is_some());
    let shop = files.directory(scope(), path("shop")).await.unwrap();
    assert_eq!(shop.entries[0].name, "assets");
    let missing = files.directory(scope(), path("blog")).await.unwrap_err();
    assert_eq!(missing.code.as_str(), "NOT_FOUND");
    let not_a_directory = files.directory(scope(), path("robots.txt")).await;
    assert!(not_a_directory.is_err());
}

#[tokio::test]
async fn files_are_read_whole_with_an_entity_tag() {
    let (_directory, files) = sites();
    let index = files.file(scope(), path("shop/index.html")).await.unwrap();
    assert_eq!(index.content, b"<h1>Shop</h1>");
    assert_eq!(index.tag, format!("\"{}\"", digest(b"<h1>Shop</h1>")));
    assert!(files.file(scope(), path("shop")).await.is_err());
    assert!(files.file(scope(), path("")).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn links_never_lead_out_of_the_directory() {
    let (directory, files) = sites();
    let outside = directory.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret"), "hunter2").unwrap();
    let root = directory.path().join("sites");
    std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
    std::os::unix::fs::symlink("../outside/secret", root.join("shop/peek")).unwrap();
    std::os::unix::fs::symlink("index.html", root.join("shop/home.html")).unwrap();

    for escaping in ["escape/secret", "shop/peek"] {
        let refused = files.file(scope(), path(escaping)).await;
        assert!(refused.is_err(), "{escaping}");
    }
    assert!(files.directory(scope(), path("escape")).await.is_err());
    let write = files
        .write_file(
            context(),
            path("escape/planted"),
            b"x".to_vec(),
            WriteCondition::Any,
        )
        .await;
    assert!(write.is_err());
    assert!(!outside.join("planted").exists());
    let inside = files.file(scope(), path("shop/home.html")).await.unwrap();
    assert_eq!(
        inside.content, b"<h1>Shop</h1>",
        "a link within the directory is followed"
    );
    let listed = files.directory(scope(), path("")).await.unwrap();
    let escape = listed
        .entries
        .iter()
        .find(|entry| entry.name == "escape")
        .unwrap();
    assert_eq!(escape.kind, SiteEntryKind::Link);
}

#[tokio::test]
async fn writes_replace_files_atomically_and_on_their_condition() {
    let (directory, files) = sites();
    let write = |name: &str, content: &str, condition: WriteCondition| {
        files.write_file(
            context(),
            path(name),
            content.as_bytes().to_vec(),
            condition,
        )
    };
    let created = write("blog/posts/first.html", "first", WriteCondition::Absent)
        .await
        .unwrap();
    assert!(created.created, "and its directories with it");
    assert_eq!(created.size_bytes, 5);
    assert_eq!(created.sha256, digest(b"first"));
    let again = write("blog/posts/first.html", "again", WriteCondition::Absent)
        .await
        .unwrap_err();
    assert_eq!(again.code.as_str(), "PRECONDITION_FAILED");

    let index = files.file(scope(), path("shop/index.html")).await.unwrap();
    let replaced = write(
        "shop/index.html",
        "<h1>New</h1>",
        WriteCondition::Tagged(index.tag.clone()),
    )
    .await
    .unwrap();
    assert!(!replaced.created);
    let stale = write(
        "shop/index.html",
        "<h1>Old</h1>",
        WriteCondition::Tagged(index.tag),
    )
    .await
    .unwrap_err();
    assert_eq!(stale.code.as_str(), "PRECONDITION_FAILED");
    assert_eq!(
        std::fs::read_to_string(directory.path().join("sites/shop/index.html")).unwrap(),
        "<h1>New</h1>"
    );
    let over_a_directory = write("shop/assets", "x", WriteCondition::Any)
        .await
        .unwrap_err();
    assert_eq!(over_a_directory.code.as_str(), "CONFLICT");
    let leftovers: Vec<_> = std::fs::read_dir(directory.path().join("sites/shop"))
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".writing"))
        .collect();
    assert!(leftovers.is_empty());
    assert!(write("", "x", WriteCondition::Any).await.is_err());
}

#[tokio::test]
async fn directories_are_created_and_entries_removed() {
    let (directory, files) = sites();
    files
        .create_directory(context(), path("blog/2027"))
        .await
        .unwrap();
    assert!(directory.path().join("sites/blog/2027").is_dir());
    assert!(files.create_directory(context(), path("")).await.is_err());

    let removed = files
        .remove(context(), path("robots.txt"), false)
        .await
        .unwrap();
    assert_eq!((removed.kind, removed.removed), (SiteEntryKind::File, 1));
    let full = files
        .remove(context(), path("shop"), false)
        .await
        .unwrap_err();
    assert_eq!(full.code.as_str(), "CONFLICT");
    let removed = files.remove(context(), path("shop"), true).await.unwrap();
    assert_eq!(
        (removed.kind, removed.removed),
        (SiteEntryKind::Directory, 4),
        "shop, its index, assets and app.js"
    );
    assert!(!directory.path().join("sites/shop").exists());
    assert!(files.remove(context(), path(""), true).await.is_err());
    let missing = files
        .remove(context(), path("shop"), true)
        .await
        .unwrap_err();
    assert_eq!(missing.code.as_str(), "NOT_FOUND");
}
