//! A client for a local embedding server that speaks `POST /v1/embeddings` over plain HTTP/1.1.
//! The address comes from `LIBRARY_EMBED_URL` (for example `http://embedder:8080`); without it,
//! semantic search is not configured. The text never leaves the host's network.
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

type Result<T> = std::result::Result<T, String>;

/// The instruction the model card gives for a search query.
const QUERY: &str = "task: search result | query: ";
/// Connecting to a server on the same host takes milliseconds; a longer wait means it is down. A batch
/// request may wait as long as its own wait: a virtual machine's network can stall a connection for
/// seconds under load (seen after a few hundred requests in a row) while the server is healthy.
const CONNECT: Duration = Duration::from_secs(2);
/// A query took about 50 ms on two cores without a GPU; two seconds leaves room for a busy server.
pub(crate) const QUERY_WAIT: Duration = Duration::from_secs(2);
/// The words that mark a request cut because the wait ran out while the server was still working.
const CUT: &str = "no answer within";

/// The embedding server's address, when semantic search is configured.
pub(crate) fn url() -> Option<String> {
    std::env::var("LIBRARY_EMBED_URL").ok().filter(|u| !u.trim().is_empty())
}

/// The model the vectors come from. Vectors from another model are computed again.
pub(crate) fn model() -> String {
    std::env::var("LIBRARY_EMBED_MODEL")
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "embeddinggemma-2".into())
}

pub(crate) fn query_text(query: &str) -> String {
    format!("{QUERY}{query}")
}

pub(crate) fn document_text(title: &str, part: &str) -> String {
    format!("title: {title} | text: {part}")
}

/// One unit-length vector for each text, in order.
pub(crate) fn embed(url: &str, texts: &[String], wait: Duration) -> Result<Vec<Vec<f32>>> {
    let rest = url
        .strip_prefix("http://")
        .ok_or("LIBRARY_EMBED_URL must start with http://")?;
    let host = rest.split('/').next().unwrap_or(rest);
    let address = host
        .to_socket_addrs()
        .map_err(|e| format!("embedding server {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("embedding server {host}: no address"))?;
    let io = |e: std::io::Error| match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
            format!("embedding server {host}: {CUT} {:.1} s", wait.as_secs_f64())
        }
        _ => format!("embedding server {host}: {e}"),
    };
    let mut stream = TcpStream::connect_timeout(&address, wait.max(CONNECT)).map_err(io)?;
    stream.set_read_timeout(Some(wait)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(wait)).map_err(|e| e.to_string())?;
    let body = json!({"input": texts}).to_string();
    let request = format!(
        "POST /v1/embeddings HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).map_err(io)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(io)?;
    let split = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("embedding server: incomplete response")?;
    let head = String::from_utf8_lossy(&response[..split]).to_ascii_lowercase();
    let status = head.split_whitespace().nth(1).unwrap_or("").to_owned();
    let mut payload = response[split + 4..].to_vec();
    if head.contains("transfer-encoding: chunked") {
        payload = dechunk(&payload)?;
    }
    if status != "200" {
        let text: String = String::from_utf8_lossy(&payload).chars().take(200).collect();
        return Err(format!("embedding server {host}: HTTP {status}: {text}"));
    }
    let value: Value = serde_json::from_slice(&payload).map_err(|e| format!("embedding server: {e}"))?;
    let data = value["data"].as_array().ok_or("embedding server: no data")?;
    if data.len() != texts.len() {
        return Err(format!("embedding server: {} vectors for {} texts", data.len(), texts.len()));
    }
    let mut vectors = vec![Vec::new(); texts.len()];
    for (i, item) in data.iter().enumerate() {
        let at = item["index"].as_u64().map_or(i, |v| v as usize);
        let vector = item["embedding"]
            .as_array()
            .ok_or("embedding server: no embedding")?
            .iter()
            .map(|x| x.as_f64().map(|f| f as f32).ok_or("embedding server: bad number"))
            .collect::<std::result::Result<Vec<f32>, _>>()?;
        *vectors.get_mut(at).ok_or("embedding server: bad index")? = vector;
    }
    let dim = vectors.first().map_or(0, Vec::len);
    if dim == 0 || vectors.iter().any(|v| v.len() != dim) {
        return Err("embedding server: vectors differ in length".into());
    }
    Ok(vectors.into_iter().map(normalized).collect())
}

/// Whether an error from `embed` means the wait ran out, rather than a server that is down or wrong.
pub(crate) fn was_cut(error: &str) -> bool {
    error.contains(CUT)
}

fn dechunk(mut body: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let end = body
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("embedding server: bad chunk")?;
        let size = usize::from_str_radix(String::from_utf8_lossy(&body[..end]).trim(), 16)
            .map_err(|_| "embedding server: bad chunk")?;
        body = &body[end + 2..];
        if size == 0 {
            return Ok(out);
        }
        out.extend_from_slice(body.get(..size).ok_or("embedding server: short chunk")?);
        body = body.get(size + 2..).ok_or("embedding server: short chunk")?;
    }
}

/// Unit length, so a dot product is the cosine.
fn normalized(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

pub(crate) fn to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub(crate) fn from_bytes(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn serve(reply: String) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0; 4096];
            let _ = s.read(&mut buf);
            s.write_all(reply.as_bytes()).unwrap();
        });
        format!("http://{address}")
    }

    #[test]
    fn reads_plain_and_chunked_responses_and_normalizes() {
        let body = r#"{"data":[{"index":0,"embedding":[3,4]},{"index":1,"embedding":[0,2]}]}"#;
        let plain = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        let v = embed(&serve(plain), &["a".into(), "b".into()], QUERY_WAIT).unwrap();
        assert_eq!(v, vec![vec![0.6, 0.8], vec![0.0, 1.0]]);
        let chunked = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
            body.len()
        );
        assert_eq!(embed(&serve(chunked), &["a".into(), "b".into()], QUERY_WAIT).unwrap().len(), 2);
    }

    #[test]
    fn reports_a_server_error_and_a_missing_server() {
        let e = embed(&serve("HTTP/1.1 503 Busy\r\n\r\nloading".into()), &["a".into()], QUERY_WAIT).unwrap_err();
        assert!(e.contains("HTTP 503"), "{e}");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let gone = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        assert!(embed(&gone, &["a".into()], QUERY_WAIT).is_err());
    }

    #[test]
    fn a_request_past_its_wait_is_reported_as_cut() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0; 4096];
            let _ = s.read(&mut buf);
            std::thread::sleep(Duration::from_millis(500));
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\n\r\n{}");
        });
        let e = embed(&format!("http://{address}"), &["a".into()], Duration::from_millis(50)).unwrap_err();
        assert!(was_cut(&e), "{e}");
        let e = embed(&serve("HTTP/1.1 503 Busy\r\n\r\nloading".into()), &["a".into()], QUERY_WAIT).unwrap_err();
        assert!(!was_cut(&e), "{e}");
    }

    #[test]
    fn vectors_round_trip_through_bytes() {
        let v = vec![0.25f32, -1.5, 3.0];
        assert_eq!(from_bytes(&to_bytes(&v)), v);
    }
}
