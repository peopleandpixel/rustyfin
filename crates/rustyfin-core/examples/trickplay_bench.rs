//! Times the trickplay task on real videos.
//!
//! ```text
//! cargo run --release -p rustyfin-core --example trickplay_bench -- <each|all> <data dir> <video>...
//! ```
//!
//! `each` runs the task once per video, each in a library of its own, and prints the time of each
//! run; `all` runs it once over a library holding every video, as the scheduled task does. Both
//! print each video's tile set info. The tiles are left under `<data dir>/<mode>/data`.

use std::path::Path;
use std::time::Instant;

use rustyfin_core::trickplay::{Cancel, generate_for_libraries};
use rustyfin_model::library::{Library, LibraryContentType, LibraryOptions};
use sqlx::SqlitePool;
use uuid::Uuid;

async fn library(pool: &SqlitePool, dir: &Path, videos: &[(Uuid, &str)]) -> String {
    let lib = Library {
        id: Uuid::new_v4(),
        name: "Bench".to_string(),
        content_type: LibraryContentType::Movies,
        paths: vec![dir.to_string_lossy().into_owned()],
    };
    rustyfin_db::repo_library::create_library(pool, &lib)
        .await
        .unwrap();
    let options = LibraryOptions {
        enable_trickplay: true,
        ..LibraryOptions::default()
    };
    rustyfin_db::repo_library::set_library_options(pool, lib.id, &options)
        .await
        .unwrap();
    for (id, path) in videos {
        sqlx::query(
            "INSERT INTO media_items (id, name, media_type, path, library_id, runtime_ticks)
             VALUES (?1, ?2, 'Movie', ?2, ?3, 1)",
        )
        .bind(id.to_string())
        .bind(path)
        .bind(lib.id.to_string())
        .execute(pool)
        .await
        .unwrap();
    }
    lib.id.to_string()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mode, root, paths @ ..] = args.as_slice() else {
        eprintln!("usage: trickplay_bench <each|all> <data dir> <video>...");
        std::process::exit(2);
    };
    let dir = Path::new(root).join(mode);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data");
    let pool = rustyfin_db::init_db(dir.join("bench.db").to_str().unwrap())
        .await
        .unwrap();
    let videos: Vec<(Uuid, &str)> = paths.iter().map(|p| (Uuid::new_v4(), p.as_str())).collect();

    let groups: Vec<&[(Uuid, &str)]> = match mode.as_str() {
        "each" => videos.chunks(1).collect(),
        "all" => vec![&videos[..]],
        _ => {
            eprintln!("mode must be each or all");
            std::process::exit(2);
        }
    };
    println!(
        "jobs\t{}\tcpus\t{}",
        rustyfin_core::trickplay::image_jobs(),
        std::thread::available_parallelism().map_or(0, |n| n.get())
    );
    let started = Instant::now();
    for group in groups {
        let lib = library(&pool, &dir, group).await;
        let run = Instant::now();
        let made = generate_for_libraries(&pool, &data, &[lib], &Cancel::new())
            .await
            .unwrap();
        if mode == "each" {
            println!(
                "time\t{:.1}\t{}\t{made}",
                run.elapsed().as_secs_f64(),
                group[0].1
            );
        }
    }
    println!("total\t{:.1}", started.elapsed().as_secs_f64());
    for (id, path) in &videos {
        for set in rustyfin_db::repo_trickplay::get_trickplay(&pool, *id)
            .await
            .unwrap()
        {
            println!(
                "set\t{path}\t{id}\t{}x{}\t{}x{}\t{}\t{}\t{}",
                set.width,
                set.height,
                set.tile_width,
                set.tile_height,
                set.thumbnail_count,
                set.interval,
                set.bandwidth
            );
        }
    }
}
