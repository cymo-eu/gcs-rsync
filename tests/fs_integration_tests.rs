use std::path::{Path, PathBuf};

use futures::{StreamExt, TryStreamExt};
use gcs_sync::{
    oauth2::token::AuthorizedUserCredentials,
    sync::{DefaultSource, FsRSync, RMirrorStatus, RSync, RSyncStatus, RelativePath},
};
use tokio::io::AsyncWriteExt;

struct TestConfig {
    base_path: PathBuf,
}

const CONCURRENCY_LEVEL: usize = 12;

impl TestConfig {
    fn new() -> Self {
        let base_path = {
            let uuid = uuid::Uuid::new_v4().to_hyphenated().to_string();
            let p = PathBuf::from(format!("fs_integration_tests/{}/", uuid));
            p
        };
        Self { base_path }
    }

    fn file_path(&self, file_name: &str) -> PathBuf {
        let mut p = self.base_path.clone();
        let file_name = file_name.strip_prefix('/').unwrap_or(file_name);
        p.push(file_name);
        p
    }
}

impl Drop for TestConfig {
    fn drop(&mut self) {
        std::fs::remove_dir_all(self.base_path.as_path()).unwrap();
    }
}

async fn write_to_file(path: &Path, content: &str) {
    assert_create_file(path, content).await;
}

async fn assert_create_file(path: &Path, content: &str) {
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.unwrap();
    }
    let mut file = tokio::fs::File::create(path).await.unwrap();
    file.write_all(content.as_bytes()).await.unwrap();
}

async fn delete_file(path: &Path) {
    tokio::fs::remove_file(path).await.unwrap()
}

async fn setup_files(file_names: &[PathBuf], content: &str) {
    futures::stream::iter(file_names)
        .for_each_concurrent(CONCURRENCY_LEVEL, |x| {
            assert_create_file(x.as_path(), content)
        })
        .await;
}

async fn delete_files(file_names: &[PathBuf]) {
    futures::stream::iter(file_names)
        .for_each_concurrent(CONCURRENCY_LEVEL, |x| delete_file(x.as_path()))
        .await;
}

async fn sync(fs_client: &FsRSync) -> Vec<RSyncStatus> {
    let mut actual = fs_client
        .sync()
        .await
        .try_buffer_unordered(CONCURRENCY_LEVEL)
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    actual.sort();
    actual
}

async fn mirror(fs_client: &FsRSync) -> Vec<RMirrorStatus> {
    let mut actual = fs_client
        .mirror()
        .await
        .try_buffer_unordered(CONCURRENCY_LEVEL)
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    actual.sort();
    actual
}

fn created(path: &str) -> RSyncStatus {
    RSyncStatus::Created(RelativePath::new(path))
}

fn updated(path: &str) -> RSyncStatus {
    RSyncStatus::Updated(RelativePath::new(path))
}

fn already_sinced(path: &str) -> RSyncStatus {
    RSyncStatus::AlreadySynced(RelativePath::new(path))
}

fn deleted(path: &str) -> RMirrorStatus {
    RMirrorStatus::Deleted(RelativePath::new(path))
}

fn not_deleted(path: &str) -> RMirrorStatus {
    RMirrorStatus::NotDeleted(RelativePath::new(path))
}

fn synced(x: RSyncStatus) -> RMirrorStatus {
    RMirrorStatus::Synced(x)
}

#[tokio::test]
async fn test_fs_to_fs_sync() {
    let src_t = TestConfig::new();

    let file_names = vec![
        "/hello/world/test.txt",
        "test.json",
        "a/long/path/hello_world.toml",
    ]
    .into_iter()
    .map(|x| src_t.file_path(x))
    .collect::<Vec<_>>();

    setup_files(&file_names[..], "Hello World").await;

    let dst_t = TestConfig::new();

    let source = DefaultSource::fs(src_t.base_path.as_path());
    let dest = DefaultSource::fs(dst_t.base_path.as_path());
    let fs_client = RSync::new(source, dest);

    let actual = sync(&fs_client).await;

    assert_eq!(
        vec![
            created("a/long/path/hello_world.toml"),
            created("hello/world/test.txt"),
            created("test.json"),
        ],
        actual
    );

    let actual = sync(&fs_client).await;

    assert_eq!(
        vec![
            already_sinced("a/long/path/hello_world.toml"),
            already_sinced("hello/world/test.txt"),
            already_sinced("test.json"),
        ],
        actual
    );

    write_to_file(src_t.file_path("test.json").as_path(), "top new content").await;
    write_to_file(src_t.file_path("new.json").as_path(), "top new content").await;
    let actual = sync(&fs_client).await;

    assert_eq!(
        vec![
            created("new.json"),
            updated("test.json"),
            already_sinced("a/long/path/hello_world.toml"),
            already_sinced("hello/world/test.txt"),
        ],
        actual
    );

    delete_files(&file_names[..]).await;

    let actual = mirror(&fs_client).await;

    assert_eq!(
        vec![
            synced(already_sinced("new.json")),
            deleted("a/long/path/hello_world.toml"),
            deleted("hello/world/test.txt"),
            deleted("test.json"),
            not_deleted("new.json")
        ],
        actual
    );
}
