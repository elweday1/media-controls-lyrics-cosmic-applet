use cosmic::iced::{futures::SinkExt, futures::StreamExt, stream};
use futures::channel::mpsc::{Sender, unbounded};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
};

pub const BARS: usize = 8;

fn write_config() -> std::io::Result<PathBuf> {
    let path = std::env::temp_dir().join("cosmic-ext-music-lyrics-cava.conf");
    std::fs::write(
        &path,
        format!(
            "[general]\nbars = {BARS}\nframerate = 30\n\n\
             [input]\nmethod = pulse\nsource = auto\n\n\
             [output]\nmethod = raw\nraw_target = /dev/stdout\ndata_format = ascii\n\
             ascii_max_range = 100\nbar_delimiter = 59\n"
        ),
    )?;
    Ok(path)
}

/// Streams one frame of `BARS` levels (0-100) per cava update while subscribed.
/// Dropping the subscription stops the cava process.
pub fn spectrum() -> impl futures::Stream<Item = Vec<u8>> {
    stream::channel(8, |mut output: Sender<Vec<u8>>| async move {
        let Ok(config) = write_config() else {
            return;
        };
        let Ok(mut child) = Command::new("cava")
            .arg("-p")
            .arg(&config)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return;
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            return;
        };

        let (frames_tx, mut frames_rx) = unbounded::<Vec<u8>>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                let frame: Vec<u8> = line
                    .split(';')
                    .filter_map(|value| value.trim().parse().ok())
                    .collect();
                if frame.len() >= BARS && frames_tx.unbounded_send(frame).is_err() {
                    break;
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        });

        while let Some(frame) = frames_rx.next().await {
            if output.send(frame).await.is_err() {
                break;
            }
        }
    })
}
