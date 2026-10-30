//! **A file decoded from its top**, packet by packet: what a `DiskIn` streams
//! when the file is not read by arithmetic -- a compressed one, or any the
//! decoder reads. It reads through the files the platform has
//! (`dsp::pcm::Source`), so the same code serves a native reader thread and
//! an offline render in a page, whose thread reads the page's files on the
//! spot.

/// An open file, its decoder and the track it plays.
pub(crate) struct Decoded {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::audio::AudioDecoder>,
    track_id: u32,
    /// The file's channel count.
    pub(crate) channels: usize,
    path: String,
}

impl Decoded {
    /// Opens `path` to decode from its top.
    pub(crate) fn open(path: &str) -> Result<Decoded, String> {
        use symphonia::core::codecs::CodecParameters;
        use symphonia::core::codecs::audio::AudioDecoderOptions;
        use symphonia::core::formats::probe::Hint;
        use symphonia::core::formats::{FormatOptions, TrackType};
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;

        let err = |e: String| format!("{path}: {e}");
        let file = crate::dsp::pcm::Source::open(path).map_err(|e| err(e.to_string()))?;
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
        {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                stream,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| err(e.to_string()))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| err("no audio track".into()))?;
        let params = match track.codec_params.as_ref() {
            Some(CodecParameters::Audio(params)) => params,
            _ => return Err(err("default track is not audio".into())),
        };
        let channels = params.channels.as_ref().map_or(0, |c| c.count());
        if channels == 0 {
            return Err(err("could not determine channel count".into()));
        }
        let track_id = track.id;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|e| err(e.to_string()))?;
        Ok(Decoded {
            format,
            decoder,
            track_id,
            channels,
            path: path.to_string(),
        })
    }

    /// **The next packet's samples**, interleaved, in `out` (emptied first):
    /// `Ok(true)` while there is one, `Ok(false)` at the end of the file. A
    /// packet that does not decode is passed over, as a player does.
    pub(crate) fn next(&mut self, out: &mut Vec<f32>) -> Result<bool, String> {
        use symphonia::core::errors::Error as SymError;

        out.clear();
        loop {
            match self.format.next_packet() {
                Ok(Some(packet)) => {
                    if packet.track_id != self.track_id {
                        continue;
                    }
                    match self.decoder.decode(&packet) {
                        Ok(decoded) => {
                            decoded.copy_to_vec_interleaved(out);
                            return Ok(true);
                        }
                        Err(SymError::DecodeError(_)) => continue,
                        Err(e) => return Err(format!("{}: {e}", self.path)),
                    }
                }
                Ok(None) => return Ok(false),
                Err(e) => return Err(format!("{}: {e}", self.path)),
            }
        }
    }

    /// **From the top again**, exactly: the file opened anew rather than
    /// sought, since a compressed format has no exact frame seek.
    pub(crate) fn again(&mut self) -> Result<(), String> {
        *self = Decoded::open(&self.path)?;
        Ok(())
    }
}
