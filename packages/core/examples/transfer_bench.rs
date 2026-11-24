//! Loopback benchmark of the transfer path: a TLS server writing received files
//! to disk (like the app's `FileUploadTarget::Path`) and a pinned TLS client
//! uploading files read from disk (like the app's `FileContent::Path`).
//!
//! Not covered: the per-file round trips of the app through its Dart isolates.
//!
//! Usage:
//!   cargo run --release --example transfer_bench --features full
//!
//! Environment variables:
//!   BENCH_TARGET=memory  discard received data instead of writing it to disk
//!   BENCH_RAW_DISK=1     first measure plain std::fs writes as a baseline

use localsend::http::client::LsHttpClientV2;
use localsend::http::dto_v2::{PrepareUploadRequestDtoV2, RegisterDtoV2};
use localsend::http::server::common::save::FileUploadTarget;
use localsend::http::server::v2::{PrepareUploadDecisionV2, ServerEventV2};
use localsend::http::server::web::WebConfig;
use localsend::http::server::{start_with_port, ServerConfigV2, TlsConfig};
use localsend::http::state::ClientInfo;
use localsend::model::discovery::ProtocolType;
use localsend::model::transfer::{FileContent, FileDto};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio_util::sync::CancellationToken;

struct Scenario {
    name: &'static str,
    count: usize,
    size: usize,
    concurrency: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let scenarios = [
        Scenario {
            name: "1 x 1 GiB",
            count: 1,
            size: 1024 << 20,
            concurrency: 1,
        },
        Scenario {
            name: "200 x 1 MiB",
            count: 200,
            size: 1 << 20,
            concurrency: 2,
        },
        Scenario {
            name: "200 x 1 MiB",
            count: 200,
            size: 1 << 20,
            concurrency: 4,
        },
        Scenario {
            name: "2000 x 16 KiB",
            count: 2000,
            size: 16 << 10,
            concurrency: 1,
        },
        Scenario {
            name: "2000 x 16 KiB",
            count: 2000,
            size: 16 << 10,
            concurrency: 2,
        },
        Scenario {
            name: "2000 x 16 KiB",
            count: 2000,
            size: 16 << 10,
            concurrency: 4,
        },
        Scenario {
            name: "2000 x 16 KiB",
            count: 2000,
            size: 16 << 10,
            concurrency: 8,
        },
    ];

    let root = std::env::temp_dir().join(format!("localsend-bench-{}", uuid::Uuid::new_v4()));
    let out_dir = root.join("out");
    std::fs::create_dir_all(&out_dir)?;

    if std::env::var("BENCH_RAW_DISK").is_ok() {
        // Baseline: plain std::fs writes from this process, no LocalSend code involved.
        use std::io::Write;
        let chunk = vec![7u8; 512 << 10];
        for run in 1..=3 {
            let started = Instant::now();
            let mut file = std::fs::File::create(root.join("raw.bin"))?;
            for _ in 0..2048 {
                file.write_all(&chunk)?;
            }
            drop(file);
            println!(
                "raw std::fs 1 GiB write, run {run}: {:.1} MiB/s",
                1024.0 / started.elapsed().as_secs_f64()
            );
        }
    }

    let receiver = localsend::crypto::cert::generate_self_signed()?;
    let sender = localsend::crypto::cert::generate_self_signed()?;
    let port = start_server(&receiver, out_dir.clone()).await?;

    let client = Arc::new(LsHttpClientV2::try_new(
        &sender.private_key_pem,
        &sender.certificate_pem,
        Some(receiver.fingerprint.clone()),
        None,
    )?);

    println!(
        "{:<16} {:>5} {:>10} {:>12} {:>10}",
        "scenario", "par", "time", "MiB/s", "files/s"
    );
    for scenario in &scenarios {
        let src_dir = root.join(format!("src-{}-{}", scenario.count, scenario.size));
        let paths = create_files(&src_dir, scenario.count, scenario.size)?;

        let elapsed = run(
            &client,
            &sender.fingerprint,
            port,
            &paths,
            scenario.size,
            scenario.concurrency,
        )
        .await?;
        let secs = elapsed.as_secs_f64();
        let mib = (scenario.count * scenario.size) as f64 / (1 << 20) as f64;
        println!(
            "{:<16} {:>5} {:>9.2}s {:>12.1} {:>10.0}",
            scenario.name,
            scenario.concurrency,
            secs,
            mib / secs,
            scenario.count as f64 / secs
        );
        let _ = std::fs::remove_dir_all(&out_dir);
        std::fs::create_dir_all(&out_dir)?;
    }

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

fn create_files(dir: &Path, count: usize, size: usize) -> anyhow::Result<Vec<PathBuf>> {
    if dir.exists() {
        return Ok((0..count).map(|i| dir.join(format!("f{i}.bin"))).collect());
    }
    std::fs::create_dir_all(dir)?;
    let mut content = vec![0u8; size];
    let mut paths = Vec::with_capacity(count);
    for i in 0..count {
        // Varying, incompressible-looking content.
        for (j, byte) in content.iter_mut().enumerate() {
            *byte = (j.wrapping_mul(31) ^ i.wrapping_mul(7)) as u8;
        }
        let path = dir.join(format!("f{i}.bin"));
        std::fs::write(&path, &content)?;
        paths.push(path);
    }
    Ok(paths)
}

async fn run(
    client: &Arc<LsHttpClientV2>,
    sender_fingerprint: &str,
    port: u16,
    paths: &[PathBuf],
    size: usize,
    concurrency: usize,
) -> anyhow::Result<std::time::Duration> {
    let files: HashMap<String, FileDto> = (0..paths.len())
        .map(|i| {
            let id = format!("file-{i}");
            let dto = FileDto {
                id: id.clone(),
                file_name: format!("f{i}.bin"),
                size: size as u64,
                file_type: "application/octet-stream".to_string(),
                sha256: None,
                preview: None,
                metadata: None,
            };
            (id, dto)
        })
        .collect();

    let payload = PrepareUploadRequestDtoV2 {
        info: RegisterDtoV2 {
            alias: "Bench Sender".to_string(),
            version: "2.2".to_string(),
            device_model: None,
            device_type: None,
            fingerprint: sender_fingerprint.to_string(),
            port: 53317,
            protocol: ProtocolType::Https,
            download: false,
        },
        files,
    };

    let started = Instant::now();
    let prepared = client
        .prepare_upload(
            ProtocolType::Https,
            "127.0.0.1",
            port,
            None,
            payload,
            None,
            CancellationToken::new(),
        )
        .await?;
    let response = prepared
        .response
        .ok_or_else(|| anyhow::anyhow!("not accepted"))?;

    let semaphore = Arc::new(Semaphore::new(concurrency));
    let mut tasks = tokio::task::JoinSet::new();
    for (i, path) in paths.iter().enumerate() {
        let id = format!("file-{i}");
        let token = response.files[&id].clone();
        let session_id = response.session_id.clone();
        let client = client.clone();
        let path = path.clone();
        let permit = semaphore.clone().acquire_owned().await?;
        tasks.spawn(async move {
            let _permit = permit;
            let body = localsend::reqwest::Body::wrap_stream(FileContent::Path(path).into_stream());
            client
                .upload(
                    ProtocolType::Https,
                    "127.0.0.1",
                    port,
                    None,
                    &session_id,
                    &id,
                    &token,
                    body,
                    CancellationToken::new(),
                )
                .await
        });
    }
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(started.elapsed())
}

async fn start_server(
    identity: &localsend::crypto::cert::SelfSignedCert,
    out_dir: PathBuf,
) -> anyhow::Result<u16> {
    let (event_tx, mut event_rx) = mpsc::channel::<ServerEventV2>(16);
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                ServerEventV2::PrepareUpload {
                    files, decision_tx, ..
                } => {
                    let _ = decision_tx.send(PrepareUploadDecisionV2::Accept(
                        files.keys().cloned().collect(),
                    ));
                }
                ServerEventV2::FileUpload {
                    file, target_tx, ..
                } if std::env::var("BENCH_TARGET").as_deref() == Ok("memory") => {
                    // Discards the content: isolates the network and HTTP path from the disk.
                    let _ = file;
                    let (binary_tx, mut binary_rx) = mpsc::channel(16);
                    let (result_tx, result_rx) = oneshot::channel();
                    let _ = target_tx.send(FileUploadTarget::Stream {
                        binary_tx,
                        result_rx,
                    });
                    tokio::spawn(async move {
                        while binary_rx.recv().await.is_some() {}
                        let _ = result_tx.send(Ok(()));
                    });
                }
                ServerEventV2::FileUpload {
                    file, target_tx, ..
                } => {
                    let (result_tx, _result_rx) = oneshot::channel();
                    let (progress_tx, mut progress_rx) = mpsc::channel::<u64>(16);
                    // Drain progress like the app does.
                    tokio::spawn(async move { while progress_rx.recv().await.is_some() {} });
                    let _ = target_tx.send(FileUploadTarget::Path {
                        path: out_dir.join(&file.file_name),
                        result_tx,
                        progress_tx: Some(progress_tx),
                    });
                }
                _ => {}
            }
        }
    });

    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    std::mem::forget(stop_tx); // keep the server running until the process exits
    let handle = start_with_port(
        0,
        Some(TlsConfig {
            cert: identity.certificate_pem.clone(),
            private_key: identity.private_key_pem.clone(),
        }),
        ClientInfo {
            alias: "Bench Receiver".to_string(),
            version: "2.2".to_string(),
            device_model: None,
            device_type: None,
            token: identity.fingerprint.clone(),
        },
        None,
        Some(ServerConfigV2 {
            pin: None,
            verify_checksums: true,
            event_tx,
        }),
        WebConfig::default(),
        stop_rx,
    )
    .await?;
    Ok(handle.port())
}
