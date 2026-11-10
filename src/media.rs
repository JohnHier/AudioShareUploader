//! Reading audio properties and converting to MP3 320 when a file does not
//! meet the programme's norms. The source file is never changed.

use crate::audioshare::api::Quality;
use crate::audioshare::queue::precheck;
use crate::export::{ExportFormat, ExportJob, export};
use crate::wav::WavReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

#[derive(Clone, Copy, Debug, Default)]
pub struct Info {
    pub duration_sec: f64,
    pub sample_rate: u32,
}

fn ext(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Duration and sample rate. WAV via the own reader (also RF64), the rest via symphonia.
pub fn probe(file: &Path) -> Result<Info, String> {
    if ext(file) == "wav" {
        let r = WavReader::open(file).map_err(|e| format!("WAV niet leesbaar: {e}"))?;
        let rate = r.fmt.sample_rate;
        return Ok(Info { duration_sec: r.frames as f64 / rate.max(1) as f64, sample_rate: rate });
    }
    let f = std::fs::File::open(file).map_err(|e| format!("Kan bestand niet openen: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(f), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(&ext(file));
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("Geen bruikbaar audiobestand: {e}"))?;
    let track = probed.format.default_track().ok_or("Geen audiospoor gevonden")?;
    let p = &track.codec_params;
    let rate = p.sample_rate.unwrap_or(0);
    let mut dur = match (p.n_frames, p.time_base) {
        (Some(n), Some(tb)) => {
            let t = tb.calc_time(n);
            t.seconds as f64 + t.frac
        }
        (Some(n), None) if rate > 0 => n as f64 / rate as f64,
        _ => 0.0,
    };
    if dur <= 0.0 {
        // No length in the header (e.g. some VBR MP3s): count the packets.
        let mut format = probed.format;
        let id = format.default_track().map(|t| t.id).unwrap_or(0);
        let tb = format.default_track().and_then(|t| t.codec_params.time_base);
        let mut ts = 0u64;
        while let Ok(pkt) = format.next_packet() {
            if pkt.track_id() == id {
                ts = pkt.ts() + pkt.dur();
            }
        }
        dur = match tb {
            Some(tb) => {
                let t = tb.calc_time(ts);
                t.seconds as f64 + t.frac
            }
            None if rate > 0 => ts as f64 / rate as f64,
            None => 0.0,
        };
    }
    Ok(Info { duration_sec: dur, sample_rate: rate })
}

pub struct Prepared {
    /// The file to upload: the source, or a converted copy in the work folder.
    pub file: PathBuf,
    pub converted: bool,
    pub info: Info,
}

/// Use the source as it is when it is an MP3 that meets the norms; otherwise
/// convert to MP3 320 in `work_dir`. Problems that converting can't solve
/// (duration) are returned as an error.
pub fn prepare(src: &Path, q: &Quality, work_dir: &Path, name: &str) -> Result<Prepared, String> {
    let info = probe(src)?;
    if ext(src) == "mp3" && precheck(src, info.duration_sec, info.sample_rate, q).is_empty() {
        return Ok(Prepared { file: src.to_path_buf(), converted: false, info });
    }
    std::fs::create_dir_all(work_dir).map_err(|e| format!("Werkmap niet aan te maken: {e}"))?;
    // 1. to WAV (MP3/FLAC/OGG are decoded; a WAV is read directly)
    let progress = AtomicU32::new(0);
    let wav = if ext(src) == "wav" { src.to_path_buf() } else { crate::decode::working_copy(src, &progress)? };
    let frames = WavReader::open(&wav).map_err(|e| format!("Omzetten mislukt: {e}"))?.frames;
    // 2. to MP3 320
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(60).collect();
    let dst = work_dir.join(format!("{safe}_{}.mp3", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let job = ExportJob { src: &wav, dst: &dst, format: ExportFormat::Mp3, segments: vec![(0, frames)], fade_ms: 0, crossfade_ms: 0, tags: Vec::new() };
    let cancel = AtomicBool::new(false);
    let res = export(&job, &progress, &cancel);
    if wav != src {
        let _ = std::fs::remove_file(&wav);
    }
    res.map_err(|e| format!("Omzetten naar MP3 mislukt: {e}"))?;
    let info2 = probe(&dst).unwrap_or(info);
    let problems = precheck(&dst, info2.duration_sec, info2.sample_rate, q);
    if !problems.is_empty() {
        let _ = std::fs::remove_file(&dst);
        return Err(format!("Voldoet ook na omzetten niet aan de normen: {}", problems.join("; ")));
    }
    Ok(Prepared { file: dst, converted: true, info: info2 })
}
