// This is based on https://github.com/iced-rs/iced/blob/master/examples/download_progress/src/download.rs
// with some modifications to store the downloaded bytes in `Progress::Finished` and `State::Downloading`
// and to keep track of any download errors.
use futures::{
    channel::{mpsc, oneshot},
    SinkExt, Stream, StreamExt,
};
use iced::stream::try_channel;

use std::{hash::Hash, io::Read};

/// The downloaded archive weighs tens of megabytes, leave room for a slow link.
const TIMEOUT_SECS: u64 = 30 * 60;

const CHUNK_SIZE: usize = 64 * 1024;

// Just a little utility function
pub fn file<I: 'static + Hash + Copy + Send + Sync, T: ToString>(
    id: I,
    url: T,
) -> iced::Subscription<(I, Result<Progress, DownloadError>)> {
    crate::utils::subscription::run_with_id(
        id,
        download(url.to_string()).map(move |progress| (id, progress)),
    )
}

fn download(url: String) -> impl Stream<Item = Result<Progress, DownloadError>> {
    try_channel(100, move |mut output: mpsc::Sender<Progress>| async move {
        let (tx, mut rx) = mpsc::unbounded();
        let (done, downloader) = oneshot::channel();
        std::thread::spawn(move || {
            let _ = done.send(download_blocking(&url, &tx));
        });

        // Dropping this future drops `rx`, which is what tells the blocking thread to stop.
        while let Some(progress) = rx.next().await {
            let _ = output.send(progress).await;
        }

        downloader
            .await
            .map_err(|e| DownloadError::RequestFailed(e.to_string()))?
    })
}

/// Downloads `url`, reporting progress over `tx`. Returns as soon as the receiver is gone.
fn download_blocking(url: &str, tx: &mpsc::UnboundedSender<Progress>) -> Result<(), DownloadError> {
    let mut response = minreq::get(url).with_timeout(TIMEOUT_SECS).send_lazy()?;
    let total = response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok());

    if tx.unbounded_send(Progress::Downloading(0.0)).is_err() {
        return Ok(());
    }

    let mut bytes = Vec::with_capacity(total.unwrap_or(0));
    let mut chunk = vec![0u8; CHUNK_SIZE];
    loop {
        // Checked every chunk, not just when progress is sent: without a content length there is
        // no progress to report and this is the only thing that notices the receiver is gone.
        if tx.is_closed() {
            return Ok(());
        }
        let read = response.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(total) = total {
            let percentage = 100.0 * bytes.len() as f32 / total as f32;
            if tx
                .unbounded_send(Progress::Downloading(percentage))
                .is_err()
            {
                return Ok(());
            }
        }
    }

    let _ = tx.unbounded_send(Progress::Finished(bytes));

    Ok(())
}

#[derive(Debug, Clone)]
pub enum Progress {
    Downloading(f32),
    Finished(Vec<u8>),
}

#[derive(Debug, Clone)]
pub enum DownloadError {
    RequestFailed(String),
    NoContentLength,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NoContentLength => {
                write!(f, "Response has unknown content length.")
            }
            Self::RequestFailed(e) => {
                write!(f, "Request error: '{e}'.")
            }
        }
    }
}

impl From<minreq::Error> for DownloadError {
    fn from(error: minreq::Error) -> Self {
        DownloadError::RequestFailed(error.to_string())
    }
}

impl From<std::io::Error> for DownloadError {
    fn from(error: std::io::Error) -> Self {
        DownloadError::RequestFailed(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::executor::block_on;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    const BODY_LEN: usize = 100 * 1024 * 1024;

    fn serve_endless_body() -> (String, Arc<AtomicUsize>) {
        serve_body(true)
    }

    /// Serves a body of `BODY_LEN` bytes, and reports how many of them made it into the socket.
    /// Without a content length the client reads until the connection closes, which is the case
    /// where no progress is ever reported.
    fn serve_body(with_content_length: bool) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/bitcoind", listener.local_addr().expect("addr"));
        let written = Arc::new(AtomicUsize::new(0));
        let served = written.clone();

        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            while reader.read_line(&mut line).expect("read line") > 2 {
                line.clear();
            }
            let mut stream = reader.into_inner();
            let headers = if with_content_length {
                format!("HTTP/1.1 200 OK\r\nContent-Length: {BODY_LEN}\r\n\r\n")
            } else {
                "HTTP/1.1 200 OK\r\n\r\n".to_string()
            };
            stream.write_all(headers.as_bytes()).expect("write headers");

            let chunk = vec![7u8; CHUNK_SIZE];
            while served.load(Ordering::Relaxed) < BODY_LEN {
                if stream.write_all(&chunk).is_err() {
                    break;
                }
                served.fetch_add(chunk.len(), Ordering::Relaxed);
            }
        });

        (url, written)
    }

    #[test]
    fn dropped_receiver_stops_the_download() {
        let (url, written) = serve_endless_body();
        let (tx, mut rx) = mpsc::unbounded();
        let downloader = std::thread::spawn(move || download_blocking(&url, &tx));

        let first = block_on(rx.next()).expect("first progress");
        assert!(matches!(first, Progress::Downloading(p) if p == 0.0));
        drop(rx);

        let res = downloader.join().expect("downloader thread");
        assert!(matches!(res, Ok(())));
        assert!(
            written.load(Ordering::Relaxed) < BODY_LEN,
            "the whole body was downloaded despite the receiver being dropped"
        );
    }

    #[test]
    fn dropped_receiver_stops_a_download_without_content_length() {
        let (url, written) = serve_body(false);
        let (tx, mut rx) = mpsc::unbounded();
        let downloader = std::thread::spawn(move || download_blocking(&url, &tx));

        // No content length means no progress is ever sent, so only the closed channel can stop it.
        block_on(rx.next()).expect("first progress");
        drop(rx);

        let res = downloader.join().expect("downloader thread");
        assert!(matches!(res, Ok(())));
        assert!(
            written.load(Ordering::Relaxed) < BODY_LEN,
            "the whole body was downloaded despite the receiver being dropped"
        );
    }
}
