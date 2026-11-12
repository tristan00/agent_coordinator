use serde_json::{Value, json};
use std::{env, fs, io::{BufRead, BufReader}, path::Path, process::{Command, Stdio}};

fn u32le(out: &mut Vec<u8>, n: u32) { out.extend(n.to_le_bytes()); }
fn chunk(tag: &[u8; 4], data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut out = tag.to_vec();
    u32le(&mut out, data.len().try_into()?);
    out.extend(data);
    if data.len() % 2 != 0 { out.push(0); }
    Ok(out)
}
fn list(tag: &[u8; 4], data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    chunk(b"LIST", &[tag.as_slice(), data].concat())
}
fn decode(text: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let (mut out, mut bits, mut count) = (Vec::new(), 0u32, 0);
    for c in text.bytes().take_while(|c| *c != b'=') {
        let n = alphabet.iter().position(|x| *x == c).ok_or("Invalid base64")?;
        bits = (bits << 6) | n as u32;
        count += 6;
        if count >= 8 { count -= 8; out.push((bits >> count) as u8); }
    }
    Ok(out)
}
fn dimensions(jpeg: &[u8]) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    if !jpeg.starts_with(&[255, 216]) { return Err("Not JPEG".into()); }
    let mut i = 2;
    while i + 4 <= jpeg.len() {
        if jpeg[i] != 255 { return Err("Invalid JPEG marker".into()); }
        let marker = jpeg[i + 1];
        if marker == 255 { i += 1; continue; }
        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        if len < 2 || i + 2 + len > jpeg.len() { break; }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) && len >= 7 {
            return Ok((u16::from_be_bytes([jpeg[i + 7], jpeg[i + 8]]) as u32,
                u16::from_be_bytes([jpeg[i + 5], jpeg[i + 6]]) as u32));
        }
        i += 2 + len;
    }
    Err("JPEG dimensions missing".into())
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 5 { return Err("Usage: recording-adapter ws://host/devtools/page/id seconds fps output.avi (requires Node with WebSocket)".into()); }
    let seconds: f64 = args[2].parse()?;
    let fps: u32 = args[3].parse()?;
    if !seconds.is_finite() || seconds <= 0.0 || fps == 0 || fps > 1_000_000 {
        return Err("Positive duration and FPS representable in AVI microseconds required".into());
    }
    let destination = Path::new(&args[4]);
    let parent = destination.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent)?;
    if parent.starts_with(Path::new(env!("CARGO_MANIFEST_DIR"))) {
        return Err("Recording data must be outside the source directory".into());
    }
    let mut command = Command::new("node");
    command.args(["--input-type=module", "-e", include_str!("capture.mjs"), &args[1], &args[2]])
        .stdout(Stdio::piped()).stderr(Stdio::inherit());
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn()?;
    let captured = (|| -> Result<_, Box<dyn std::error::Error>> {
        let mut frames = Vec::new();
        let mut end = None;
        for line in BufReader::new(child.stdout.take().ok_or("Missing pipe")?).lines() {
            let value: Value = serde_json::from_str(&line?)?;
            if let Some(t) = value["end"].as_f64() { end = Some(t); continue; }
            let t = value["timestamp"].as_f64().filter(|x| x.is_finite()).ok_or("Missing CDP timestamp")?;
            let jpeg = decode(value["data"].as_str().ok_or("Missing JPEG")?)?;
            frames.push((t, jpeg, value["received"].clone()));
        }
        Ok((frames, end.ok_or("Incomplete recording")?))
    })();
    if captured.is_err() { let _ = child.kill(); }
    let status = child.wait()?;
    let (frames, end) = captured?;
    if !status.success() || frames.is_empty() { return Err("Capture failed".into()); }
    let (width, height) = dimensions(&frames[0].1)?;
    for pair in frames.windows(2) {
        if pair[1].0 < pair[0].0 { return Err("CDP clock moved backwards".into()); }
    }
    for frame in &frames { if dimensions(&frame.1)? != (width, height) { return Err("Viewport changed dimensions during capture".into()); } }
    let origin = frames[0].0;
    let duration = end.max(frames.last().unwrap().0 - origin + 1.0 / fps as f64);
    let count = (duration * fps as f64).ceil();
    if count > u32::MAX as f64 { return Err("AVI frame count overflow".into()); }
    let count = count as u32;
    let mut movi = Vec::new();
    let mut index = Vec::new();
    let mut mapping = Vec::new();
    let mut selected = 0;
    for i in 0..count {
        let t = i as f64 / fps as f64;
        while selected + 1 < frames.len() && frames[selected + 1].0 - origin <= t { selected += 1; }
        let bytes = &frames[selected].1;
        index.extend(b"00dc");
        u32le(&mut index, 16);
        u32le(&mut index, (movi.len() + 4).try_into()?);
        u32le(&mut index, bytes.len().try_into()?);
        movi.extend(chunk(b"00dc", bytes)?);
        mapping.push(selected);
    }
    let max = frames.iter().map(|x| x.1.len()).max().unwrap() as u32;
    let mut avih = Vec::new();
    for n in [1_000_000 / fps, max.saturating_mul(fps), 0, 16, count, 0, 1, max, width, height, 0, 0, 0, 0] { u32le(&mut avih, n); }
    let mut strh = b"vidsMJPG".to_vec();
    for n in [0, 0, 0, 1, fps, 0, count, max, u32::MAX, 0] { u32le(&mut strh, n); }
    strh.extend([0u8; 4]);
    strh.extend(u16::try_from(width)?.to_le_bytes());
    strh.extend(u16::try_from(height)?.to_le_bytes());
    let mut strf = Vec::new();
    for n in [40, width, height, 24 << 16 | 1, u32::from_le_bytes(*b"MJPG"), max, 0, 0, 0, 0] { u32le(&mut strf, n); }
    let stream = [chunk(b"strh", &strh)?, chunk(b"strf", &strf)?].concat();
    let header = [chunk(b"avih", &avih)?, list(b"strl", &stream)?].concat();
    let body = [b"AVI ".to_vec(), list(b"hdrl", &header)?, list(b"movi", &movi)?, chunk(b"idx1", &index)?].concat();
    let avi = chunk(b"RIFF", &body)?;
    let manifest = json!({"timingOrigin": "First Page.screencastFrame metadata.timestamp, seconds since Unix epoch; video t=0", "protocol": "https://chromedevtools.github.io/devtools-protocol/tot/Page/#type-ScreencastFrameMetadata", "originEpochSeconds": origin, "stopSecondsFromFirstReceive": end, "stopClock": "Node performance.now monotonic; independent of CDP epoch clock", "resampling": "Latest source timestamp at or before each constant-rate presentation time; hold last frame through stop", "fps": fps, "frames": count, "durationSeconds": count as f64 / fps as f64, "width": width, "height": height, "sourceFrames": frames.iter().map(|x| json!({"epochSeconds":x.0,"relativeSeconds":x.0-origin,"receivedMilliseconds":x.2})).collect::<Vec<_>>(), "sourceIndexByOutputFrame": mapping});
    fs::write(destination.with_extension("timing.json"), serde_json::to_vec_pretty(&manifest)?)?;
    fs::write(destination, avi)?;
    println!("{}", json!({"output": args[4], "frames": count, "seconds": count as f64 / fps as f64}));
    Ok(())
}
fn main() {
    if let Err(e) = run() { eprintln!("{e}"); std::process::exit(1); }
}
